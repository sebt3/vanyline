//! `vnl-hadolint-lsp` — serveur LSP stdio minimal : diagnostics hadolint seuls.
//!
//! Rôle `diagnostics-merge` du multiplexeur (feature docker-lsp) : reçoit le
//! doc-sync (fan-out de la session), lance `hadolint --format json --no-color -`
//! avec le contenu du buffer sur STDIN (jamais l'URI/le chemin en argv, jamais
//! via un shell — contrainte de sécurité du design), convertit la sortie en
//! notifications `textDocument/publishDiagnostics`. Ne répond à AUCUNE requête
//! (`-32601` partout, réflexe défensif — le multiplexeur n'envoie de requêtes
//! qu'au primaire, tâche 02). Déclencheurs : `didOpen`/`didSave` immédiats,
//! `didChange` debouncé 500 ms ; résultat périmé abandonné (jamais un diagnostic
//! plus vieux que le buffer).
//!
//! Diagnostics de fonctionnement sur stderr (`eprintln!`, comme `bin/maint.rs`) :
//! stdout est réservé aux trames LSP. Le cwd est celui hérité du multiplexeur
//! (`spawn_aux_startup` : `current_dir(sandbox_root)`) — hadolint y trouve le
//! `.hadolint.yaml` du projet, zéro interpolation (question ouverte tranchée).

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;

use vanyline_sandbox::lsp::{FrameReader, encode_message};

/// Fenêtre de debounce des `didChange` (design docker-lsp : ~500 ms ; la
/// question ouverte « déclencheurs » est tranchée « didSave + didChange
/// debouncé », et le `didOpen` immédiat est requis pour que le tool MCP
/// `lsp_diagnostics` — qui ne modifie jamais le fichier — voie hadolint).
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Document ouvert : contenu courant + version client (LSP).
#[derive(Clone)]
struct Doc {
    text: String,
    version: i64,
}

/// État partagé des tâches de lint : docs par URI + génération de debounce
/// par URI. Chaque mutation (didChange/didSave/re-déclenchement) incremente la
/// génération ; une tâche debouncée ou un lint en vol ne publie que si SA
/// génération est encore la courante à son réveil (pas de sleep annulable,
/// simple re-check — le pattern Notify du multiplexeur n'est pas requis).
struct Inner {
    docs: HashMap<String, Doc>,
    gens: HashMap<String, u64>,
}

/// État du serveur. std Mutex sur l'état (jamais tenu à travers un await),
/// Mutex asynchrone sur stdout pour l'atomicité de trame : une trame = un seul
/// `write_all` encodé, jamais entremêlée avec une autre publication.
struct State {
    inner: Mutex<Inner>,
    stdout: AsyncMutex<tokio::io::Stdout>,
}

impl State {
    fn new() -> Self {
        State {
            inner: Mutex::new(Inner {
                docs: HashMap::new(),
                gens: HashMap::new(),
            }),
            stdout: AsyncMutex::new(tokio::io::stdout()),
        }
    }

    /// Garde d'accès à l'état interne : une lock empoisonnée (panic dans une
    /// section qui ne fait qu'insérer/retirer) garde une map intacte — on
    /// récupère l'intérieur plutôt que de propager (motif de la lib).
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Incremente la génération de l'URI (disqualifie d'eux-mêmes les lints
    /// en vol et les debounce en sommeil) et la rend.
    fn bump_gen(&self, uri: &str) -> u64 {
        let mut inner = self.inner();
        let next = inner.gens.get(uri).copied().unwrap_or(0) + 1;
        inner.gens.insert(uri.to_string(), next);
        next
    }

    /// Instantané `(texte, version, génération)` du document à l'instant T —
    /// capturé au démarrage de chaque lint pour la détection de péremption.
    fn snapshot(&self, uri: &str) -> Option<(String, i64, u64)> {
        let inner = self.inner();
        let doc = inner.docs.get(uri)?;
        let generation = inner.gens.get(uri).copied()?;
        Some((doc.text.clone(), doc.version, generation))
    }

    fn current_gen(&self, uri: &str) -> Option<u64> {
        self.inner().gens.get(uri).copied()
    }

    fn doc_exists(&self, uri: &str) -> bool {
        self.inner().docs.contains_key(uri)
    }

    /// Le couple `(version, génération)` capturé au début du lint est-il
    /// encore celui du doc ? Non ⟹ résultat périmé (le buffer a bougé).
    fn is_current(&self, uri: &str, version: i64, generation: u64) -> bool {
        let inner = self.inner();
        inner.docs.get(uri).is_some_and(|d| d.version == version)
            && inner.gens.get(uri).copied() == Some(generation)
    }

    /// Lint immédiat en tâche détachée (didOpen/didSave). Deux URIs peuvent
    /// linter concurrentiellement.
    fn lint_immediate(self: &Arc<Self>, uri: String) {
        // Instantané SYNCHRONE au déclenchement : le lint capture
        // (uri, version, génération) à son DÉMARRAGE, pas au démarrage de la
        // tâche — une tâche détachée peut ne s'exécuter qu'après le traitement
        // d'un didChange suivant (ordonnancement du runtime) et hériterait
        // alors à tort de la génération de ce changement (double publication
        // du même contenu, prise en intégration).
        let Some((text, version, generation)) = self.snapshot(&uri) else {
            return;
        };
        let state = Arc::clone(self);
        tokio::spawn(async move {
            run_lint(state, uri, text, version, generation).await;
        });
    }

    /// Lint debouncé : la génération courante est capturée APRÈS l'increment —
    /// chaque bump a un seul porteur (invariant : jamais deux tâches vivantes
    /// à la même génération, donc jamais deux publications d'un même état).
    /// Au réveil du sleep, si la génération a bougé (nouveau didChange,
    /// didSave, re-déclenchement), cette tâche se disqualifie elle-même.
    fn lint_debounced(self: &Arc<Self>, uri: String) {
        let generation = self.bump_gen(&uri);
        let state = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(DEBOUNCE).await;
            if state.current_gen(&uri) != Some(generation) {
                return;
            }
            // Génération intouchée depuis l'armement ⟹ l'instantané courant
            // est bien celui de la fenêtre de debounce.
            let Some((text, version, _)) = state.snapshot(&uri) else {
                return;
            };
            run_lint(state, uri, text, version, generation).await;
        });
    }

    /// Notification `textDocument/publishDiagnostics` : la version du doc au
    /// moment du rideau est portée (optionnelle côté client, mais connue ici).
    async fn publish(&self, uri: &str, version: i64, diagnostics: Vec<Value>) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "version": version, "diagnostics": diagnostics },
        });
        self.write_frame(&msg).await;
    }

    async fn respond(&self, id: &Value, result: Value) {
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
        self.write_frame(&msg).await;
    }

    /// Erreur JSON-RPC `-32601 Method not found` — défensif : le multiplexeur
    /// (rôle `diagnostics-merge`) n'envoie JAMAIS de requête à cet aux, mais un
    /// serveur LSP correct répond plutôt que de pendre.
    async fn respond_method_not_found(&self, id: &Value) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "Method not found" },
        });
        self.write_frame(&msg).await;
    }

    /// Une trame = un `write_all` encodé sous verrou stdout + flush.
    async fn write_frame(&self, msg: &Value) {
        let frame = encode_message(msg.to_string().as_bytes());
        let mut stdout = self.stdout.lock().await;
        if stdout.write_all(&frame).await.is_ok() {
            let _ = stdout.flush().await;
        }
    }
}

/// Lance `hadolint --format json --no-color -`, écrit le buffer sur le stdin
/// du fils et rend sa stdout. Le code de sortie ≠ 0 (violations trouvées) est
/// NORMAL : stdout est rendue dans tous les cas, c'est `convert_hadolint_output`
/// qui décide. Spawn en échec / erreur d'E/S ⟹ `Err` (message à journaliser sur
/// stderr par l'appelant, contribution vide).
async fn run_hadolint(buffer: &str) -> Result<String, String> {
    // `hadolint --format json --no-color -` : le contenu du buffer est écrit
    // sur le STDIN du fils ; l'URL/l'URI/le chemin ne sont JAMAIS un argv ;
    // AUCUN shell (`Command::new` + argv littéraux uniquement, jamais de
    // `sh -c`). PAS de current_dir : le cwd du wrapper (posé par le
    // multiplexeur = sandbox_root, racine du projet git — `spawn_aux_startup`
    // dans lsp.rs) est hérité — c'est CE cwd que hadolint scanne pour son
    // `.hadolint.yaml` (design docker-lsp, question ouverte : respect gratuit,
    // zéro interpolation).
    let mut child = tokio::process::Command::new("hadolint")
        .args(["--format", "json", "--no-color", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("impossible de lancer hadolint: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "stdin du fils hadolint indisponible".to_string())?;
    let payload = buffer.as_bytes().to_vec();
    // Écriture du buffer en tâche séparée : au-delà de la capacité du pipe,
    // un write_all séquentiel avant l'attente bloquerait les deux bouts.
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&payload).await;
        let _ = stdin.shutdown().await;
    });
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("hadolint: attente de la sortie impossible: {e}"))?;
    let _ = writer.await;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Lint complet d'un URI : le couple `(texte, version, génération)` a été
/// capturé au DÉMARRAGE du lint (par `lint_immediate`/`lint_debounced`) ; à la
/// complétion, si le doc a bougé, le résultat est ABANDONNÉ (jamais un
/// diagnostic plus vieux que le buffer) et un lint debouncé du contenu courant
/// est re-déclenché. Spawn en échec / stdout non-JSON ⟹ contribution vide +
/// message sur stderr : le wrapper ne meurt jamais (le primaire sert encore —
/// dégradation silencieuse du design).
async fn run_lint(state: Arc<State>, uri: String, text: String, version: i64, generation: u64) {
    let diagnostics = match run_hadolint(&text).await {
        Ok(stdout) => {
            if serde_json::from_str::<Value>(&stdout).is_err() {
                eprintln!(
                    "vnl-hadolint-lsp: sortie hadolint non-JSON — contribution vide pour {uri}"
                );
            }
            convert_hadolint_output(&stdout, &text)
        }
        Err(err) => {
            eprintln!("vnl-hadolint-lsp: {err} — contribution vide pour {uri}");
            Vec::new()
        }
    };
    if !state.is_current(&uri, version, generation) {
        if state.doc_exists(&uri) {
            state.lint_debounced(uri);
        }
        return;
    }
    state.publish(&uri, version, diagnostics).await;
}

/// Traite une trame décodée. `true` = la boucle principale doit rendre la main
/// (notification `exit`).
async fn handle_frame(state: &Arc<State>, frame: &[u8]) -> bool {
    let Ok(msg) = serde_json::from_slice::<Value>(frame) else {
        return false; // trame non-JSON : ignorée (le flux n'est pas corrompu).
    };
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return false; // réponse d'un tiers / trame sans méthode : rien à faire.
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let is_request = msg.get("id").is_some_and(|id| !id.is_null());

    // Les REQUÊTES : initialize/shutdown répondent, tout le reste -32601.
    if is_request {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                state
                    .respond(
                        &id,
                        serde_json::json!({
                            "capabilities": {
                                "textDocumentSync": {"openClose": true, "change": 2}
                            },
                            "serverInfo": {
                                "name": "vnl-hadolint-lsp",
                                "version": env!("CARGO_PKG_VERSION"),
                            },
                        }),
                    )
                    .await;
            }
            "shutdown" => state.respond(&id, Value::Null).await,
            _ => state.respond_method_not_found(&id).await,
        }
        return false;
    }

    // Les NOTIFICATIONS.
    match method {
        "exit" => true,
        "textDocument/didOpen" => {
            let Some(td) = params.get("textDocument") else {
                return false;
            };
            let Some(uri) = td.get("uri").and_then(Value::as_str) else {
                return false;
            };
            let text = td.get("text").and_then(Value::as_str).unwrap_or_default();
            let version = td.get("version").and_then(Value::as_i64).unwrap_or(1);
            {
                let mut inner = state.inner();
                inner.docs.insert(
                    uri.to_string(),
                    Doc {
                        text: text.to_string(),
                        version,
                    },
                );
            }
            state.bump_gen(uri);
            // Lint immédiat : le tool MCP `lsp_diagnostics` fait didOpen +
            // attente sur un fichier jamais modifié — sans ici, hadolint ne
            // serait JAMAIS visible côté tools (note d'interprétation du
            // fichier de tâche).
            state.lint_immediate(uri.to_string());
            false
        }
        "textDocument/didChange" => {
            let Some(td) = params.get("textDocument") else {
                return false;
            };
            let Some(uri) = td.get("uri").and_then(Value::as_str) else {
                return false;
            };
            let empty_changes: Vec<Value> = Vec::new();
            let changes = params
                .get("contentChanges")
                .and_then(Value::as_array)
                .unwrap_or(&empty_changes);
            {
                let mut inner = state.inner();
                // URI jamais ouvert : notification orpheline, ignorée (aucun
                // texte sur lequel appliquer les edits).
                let Some(doc) = inner.docs.get_mut(uri) else {
                    return false;
                };
                apply_content_changes(&mut doc.text, changes);
                if let Some(version) = td.get("version").and_then(Value::as_i64) {
                    doc.version = version;
                }
            }
            state.lint_debounced(uri.to_string());
            false
        }
        "textDocument/didSave" => {
            let Some(uri) = params
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(Value::as_str)
            else {
                return false;
            };
            if !state.doc_exists(uri) {
                return false;
            }
            // Les debounce en sommeil se disqualifient d'eux-mêmes au re-check
            // de génération, puis lint immédiat du contenu courant.
            state.bump_gen(uri);
            state.lint_immediate(uri.to_string());
            false
        }
        "textDocument/didClose" => {
            let Some(uri) = params
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(Value::as_str)
            else {
                return false;
            };
            let removed = {
                let mut inner = state.inner();
                inner.gens.remove(uri);
                inner.docs.remove(uri)
            };
            if let Some(doc) = removed {
                // Publier [] remplace la part du wrapper dans la fusion de la
                // session — sinon ses derniers diagnostics resteraient publiés
                // pour un fichier fermé.
                state.publish(uri, doc.version, Vec::new()).await;
            }
            false
        }
        // `initialized` et toute autre notification : ignorées silencieusement.
        _ => false,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let state = Arc::new(State::new());
    let mut stdin = tokio::io::stdin();
    let mut reader = FrameReader::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stdin.read(&mut chunk).await {
            // EOF ou erreur de lecture du multiplexeur : fin de session.
            Ok(0) | Err(_) => break,
            Ok(n) => {
                reader.push(&chunk[..n]);
                while let Some(frame) = reader.next_frame() {
                    if handle_frame(&state, &frame).await {
                        return ExitCode::SUCCESS;
                    }
                }
            }
        }
    }
    ExitCode::SUCCESS
}

/// Applique les `contentChanges` LSP d'une notification `didChange` dans `text`,
/// dans l'ordre. Un changement SANS `range` = remplacement TOTAL (text
/// complet) ; avec `range` = édit incrémental borné (colonnes en unités UTF-16
/// comme le spec LSP, lignes/colonnes clamppées aux bornes du texte — un range
/// malformé ne doit jamais planter ni corrompre au-delà de lui-même).
fn apply_content_changes(text: &mut String, changes: &[Value]) {
    for change in changes {
        let Some(new_text) = change.get("text").and_then(Value::as_str) else {
            continue;
        };
        let range = match change.get("range") {
            Some(range) if !range.is_null() => range,
            // Absent ou `null` : remplacement TOTAL (le champ est optionnel en
            // LSP — le client MCP full-sync de lsp_client.rs l'omet).
            _ => {
                *text = new_text.to_string();
                continue;
            }
        };
        let Some(start) = range_position(range.get("start")) else {
            continue; // range malformé : changement ignoré, texte intact.
        };
        let Some(end) = range_position(range.get("end")) else {
            continue;
        };
        let (start, end) = (
            utf16_offset(text, start.0, start.1),
            utf16_offset(text, end.0, end.1),
        );
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        text.replace_range(start..end, new_text);
    }
}

/// `(line, character)` d'un `Position` LSP (0-based, colonnes en unités
/// UTF-16) ; `None` si la structure est malformée.
fn range_position(value: Option<&Value>) -> Option<(i64, i64)> {
    let pos = value?.as_object()?;
    let line = pos.get("line")?.as_i64()?;
    let character = pos.get("character")?.as_i64()?;
    Some((line, character))
}

/// Offset OCTET dans `text` de la position LSP `(line, character)` : lignes et
/// colonnes clamppées aux bornes du texte, `character` compté en unités
/// UTF-16. Une cible au milieu d'une paire de surrogate-clés reste avant le
/// caractère (indivisible) — jamais de panique, offset borné.
fn utf16_offset(text: &str, line: i64, character: i64) -> usize {
    // Offset de début de chaque ligne (ligne 0 = offset 0 ; après un newline
    // final, une ligne vide existe encore — sémantique LSP).
    let mut starts = vec![0_usize];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push(idx + 1);
        }
    }
    let line_idx = usize::try_from(line).unwrap_or(0).min(starts.len() - 1);
    let line_str = &text[starts[line_idx]..];
    let line_len = line_str.find('\n').unwrap_or(line_str.len());
    let target = usize::try_from(character).unwrap_or(0);
    let mut units = 0_usize;
    let mut bytes = 0_usize;
    for ch in line_str[..line_len].chars() {
        if units + ch.len_utf16() > target {
            break;
        }
        units += ch.len_utf16();
        bytes += ch.len_utf8();
    }
    starts[line_idx] + bytes
}

/// Convertit la sortie `hadolint --format json` (tableau d'objets
/// `{file,line,column,level,code,message}`) en diagnostics LSP. Positions
/// hadolint 1-based ⟹ ranges LSP 0-based : start = `(line-1, column-1)` clampé
/// à la longueur UTF-16 de la ligne ; end = FIN DE LIGNE (design risque 2 —
/// hadolint ne donne qu'un point). `file` de hadolint (`<stdin>`) IGNORE :
/// l'URI est celui du document linté, pas un champ externe. Sévérités :
/// error⟹1, warning⟹2, info⟹3, style⟹4, inconnu⟹3. `code` verbatim (chaîne),
/// `source`: `hadolint`. Entrée non-tableau / objet sans `line` exploitable ⟹
/// contribution vide (`vec![]`).
fn convert_hadolint_output(hadolint_json: &str, doc_text: &str) -> Vec<Value> {
    let Ok(parsed) = serde_json::from_str::<Value>(hadolint_json) else {
        return Vec::new();
    };
    let Value::Array(items) = parsed else {
        return Vec::new();
    };
    let lines: Vec<&str> = doc_text.split('\n').collect();
    items
        .iter()
        .filter_map(|item| {
            let obj = item.as_object()?;
            let line = obj.get("line")?.as_i64()?;
            let column = obj.get("column").and_then(Value::as_i64).unwrap_or(1);
            let level = obj.get("level").and_then(Value::as_str).unwrap_or_default();
            let severity = match level {
                "error" => 1,
                "warning" => 2,
                "info" => 3,
                "style" => 4,
                _ => 3,
            };
            // 1-based → 0-based, ligne clampée au document (hadolint peut
            // signaler au-delà d'un buffer édité entre-temps).
            let line_idx = usize::try_from(line - 1).unwrap_or(0).min(lines.len() - 1);
            let content = lines[line_idx];
            // Fin de range = fin de ligne, en unités UTF-16 comme le spec LSP.
            let line_units = content.encode_utf16().count();
            let start_char = usize::try_from(column - 1).unwrap_or(0).min(line_units);
            let message = obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(serde_json::json!({
                "range": {
                    "start": {"line": line_idx, "character": start_char},
                    // Design risque 2 : hadolint ne donne qu'un point — la
                    // range court jusqu'à la fin de la ligne.
                    "end": {"line": line_idx, "character": line_units},
                },
                "severity": severity,
                "code": obj.get("code").cloned().unwrap_or(Value::Null),
                "source": "hadolint",
                "message": message,
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    // ── convert_hadolint_output ─────────────────────────────────────────────

    /// Test 1 : convert_hadolint_output_example — l'exemple du fichier de tâche,
    /// equality structurelle exacte (ranges, sévérités, code, source, ordre).
    /// Note : la ligne 3 du doc (`ADD --link x /y`) fait 15 unités UTF-16 — le
    /// `end.character` est la fin de ligne CONTRACTUELLE (règle « fin de range =
    /// fin de ligne », répétée dans l'en-tête, le docstring et le message de
    /// commit), soit 15 et non le 18 de l'exemple (coquille arithmeticale : 18 =
    /// longueur de la LIGNE 1 dupliquée dans le second diagnostic).
    #[test]
    fn convert_hadolint_output_example() {
        let input = r#"[{"file":"<stdin>","line":1,"column":1,"level":"warning","code":"DL3007","message":"Using latest tags ..."},{"file":"<stdin>","line":3,"column":5,"level":"error","code":"DL3020","message":"COPY cannot be used with ADD --link"}]"#;
        let doc_text = "FROM ubuntu:latest\nRUN echo x\nADD --link x /y\n";
        let out = convert_hadolint_output(input, doc_text);
        let expected = json!([
            {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":18}},
             "severity":2,"code":"DL3007","source":"hadolint","message":"Using latest tags ..."},
            {"range":{"start":{"line":2,"character":4},"end":{"line":2,"character":15}},
             "severity":1,"code":"DL3020","source":"hadolint","message":"COPY cannot be used with ADD --link"}
        ]);
        assert_eq!(Value::Array(out), expected);
    }

    /// Test 2 : convert_hadolint_output_levels — les 4 niveaux + un inconnu
    /// (⟹ severity 3).
    #[test]
    fn convert_hadolint_output_levels() {
        let input = r#"[
            {"file":"<stdin>","line":1,"column":1,"level":"error","code":"A","message":"a"},
            {"file":"<stdin>","line":2,"column":1,"level":"warning","code":"B","message":"b"},
            {"file":"<stdin>","line":3,"column":1,"level":"info","code":"C","message":"c"},
            {"file":"<stdin>","line":4,"column":1,"level":"style","code":"D","message":"d"},
            {"file":"<stdin>","line":5,"column":1,"level":"mystere","code":"E","message":"e"}
        ]"#;
        let out = convert_hadolint_output(input, "a\nb\nc\nd\ne");
        let severities: Vec<i64> = out
            .iter()
            .map(|d| d["severity"].as_i64().unwrap())
            .collect();
        assert_eq!(severities, vec![1, 2, 3, 4, 3]);
    }

    /// Test 3 : convert_hadolint_output_clamps — ligne hors bornes (line 99
    /// dans un doc de 2 lignes ⟹ clamp ligne 1), colonne hors borne ⟹ clamp fin
    /// de ligne ; ligne vide ⟹ range 0..0.
    #[test]
    fn convert_hadolint_output_clamps() {
        // Doc de 2 lignes sans newline final : ligne 99 → clamp ligne 1.
        let input = r#"[{"file":"<stdin>","line":99,"column":1,"level":"info","code":"L1","message":"ligne hors bornes"}]"#;
        let out = convert_hadolint_output(input, "FROM x\nRUN y");
        assert_eq!(out[0]["range"]["start"], json!({"line":1,"character":0}));
        assert_eq!(out[0]["range"]["end"], json!({"line":1,"character":5})); // « RUN y »

        // Colonne hors borne → clamp fin de ligne (début comme fin).
        let input = r#"[{"file":"<stdin>","line":1,"column":99,"level":"info","code":"L2","message":"colonne hors borne"}]"#;
        let out = convert_hadolint_output(input, "FROM x\nRUN y");
        assert_eq!(out[0]["range"]["start"], json!({"line":0,"character":6}));
        assert_eq!(out[0]["range"]["end"], json!({"line":0,"character":6}));

        // Ligne vide → range 0..0.
        let input = r#"[{"file":"<stdin>","line":2,"column":1,"level":"info","code":"L3","message":"ligne vide"}]"#;
        let out = convert_hadolint_output(input, "x\n\ny");
        assert_eq!(out[0]["range"]["start"], json!({"line":1,"character":0}));
        assert_eq!(out[0]["range"]["end"], json!({"line":1,"character":0}));
    }

    /// Test 4 : convert_hadolint_output_utf16 — ligne contenant un emoji
    /// (2 unités UTF-16) : clamp et fin de ligne en unités UTF-16, pas en
    /// chars/bytes.
    #[test]
    fn convert_hadolint_output_utf16() {
        // « 🐛 » = 2 unités UTF-16 (1 char, 4 octets). Ligne : 9 + 2 + 1 = 12
        // unités UTF-16, 11 chars, 14 octets — le clamp suit les unités UTF-16.
        let doc = "RUN echo \u{1F41B}X";
        let input = r#"[{"file":"<stdin>","line":1,"column":12,"level":"info","code":"U1","message":"sur X"},{"file":"<stdin>","line":1,"column":99,"level":"info","code":"U2","message":"clamp"}]"#;
        let out = convert_hadolint_output(input, doc);
        assert_eq!(out[0]["range"]["start"], json!({"line":0,"character":11}));
        assert_eq!(out[0]["range"]["end"], json!({"line":0,"character":12}));
        assert_eq!(out[1]["range"]["start"], json!({"line":0,"character":12}));
        assert_eq!(out[1]["range"]["end"], json!({"line":0,"character":12}));
    }

    /// Test 5 : convert_hadolint_output_garbage — `"{}"`, `"not json"`,
    /// tableau d'un objet sans `line` ⟹ `vec![]`.
    #[test]
    fn convert_hadolint_output_garbage() {
        assert!(convert_hadolint_output("{}", "x\n").is_empty());
        assert!(convert_hadolint_output("not json", "x\n").is_empty());
        assert!(
            convert_hadolint_output(
                r#"[{"column":1,"level":"warning","code":"X","message":"pas de line"}]"#,
                "x\n"
            )
            .is_empty()
        );
    }

    // ── apply_content_changes ───────────────────────────────────────────────

    /// Test 6 : apply_content_changes_full_replace — changement sans `range`
    /// (ni `range: null`).
    #[test]
    fn apply_content_changes_full_replace() {
        let mut text = "abc\ndef".to_string();
        apply_content_changes(&mut text, &[json!({"text": "xyz"})]);
        assert_eq!(text, "xyz");
        // `range: null` = remplacement total aussi (le champ est optionnel).
        apply_content_changes(&mut text, &[json!({"range": null, "text": "nul"})]);
        assert_eq!(text, "nul");
    }

    /// Test 7 : apply_content_changes_incremental — remplacement intra-ligne
    /// (`latest` → `22.04`), insertion en fin de fichier, suppression
    /// multi-lignes.
    #[test]
    fn apply_content_changes_incremental() {
        // « FROM ubuntu:latest\n » : « latest » occupe les caractères 12..18.
        let mut text = "FROM ubuntu:latest\n".to_string();
        apply_content_changes(
            &mut text,
            &[json!({
                "range": {"start": {"line": 0, "character": 12}, "end": {"line": 0, "character": 18}},
                "text": "22.04"
            })],
        );
        assert_eq!(text, "FROM ubuntu:22.04\n");

        // Insertion en fin de fichier : range vide en (1,1) = fin du doc.
        let mut text = "a\nb".to_string();
        apply_content_changes(
            &mut text,
            &[json!({
                "range": {"start": {"line": 1, "character": 1}, "end": {"line": 1, "character": 1}},
                "text": "c"
            })],
        );
        assert_eq!(text, "a\nbc");

        // Suppression multi-lignes : [0,0]..[2,0] efface les lignes 0 et 1.
        let mut text = "a\nb\nc\n".to_string();
        apply_content_changes(
            &mut text,
            &[json!({
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 2, "character": 0}},
                "text": ""
            })],
        );
        assert_eq!(text, "c\n");
    }

    /// Test 8 : apply_content_changes_bounded — range au-delà de la fin du
    /// document ⟹ clamp, texte résultat déterministe, pas de panique.
    #[test]
    fn apply_content_changes_bounded() {
        let mut text = "abc".to_string();
        apply_content_changes(
            &mut text,
            &[json!({
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 99, "character": 99}},
                "text": "Z"
            })],
        );
        assert_eq!(text, "Z");

        // Position entièrement hors bornes → clamp en fin de document.
        let mut text = "a".to_string();
        apply_content_changes(
            &mut text,
            &[json!({
                "range": {"start": {"line": 5, "character": 5}, "end": {"line": 7, "character": 2}},
                "text": "Q"
            })],
        );
        assert_eq!(text, "aQ");

        // Range malformé (`start` absent) : changement ignoré, texte intact —
        // un range malformé ne corrompt pas au-delà de lui-même.
        let mut text = "intact".to_string();
        apply_content_changes(
            &mut text,
            &[json!({"range": {"end": {"line": 0, "character": 1}}, "text": "X"})],
        );
        assert_eq!(text, "intact");
    }

    /// Test 9 : apply_content_changes_sequence — plusieurs changements
    /// séquentiels appliqués dans l'ordre (chacun sur le texte muté par le
    /// précédent).
    #[test]
    fn apply_content_changes_sequence() {
        let mut text = "one\ntwo\nthree".to_string();
        apply_content_changes(
            &mut text,
            &[
                json!({
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}},
                    "text": "ONE"
                }),
                json!({
                    "range": {"start": {"line": 2, "character": 5}, "end": {"line": 2, "character": 5}},
                    "text": "!"
                }),
            ],
        );
        assert_eq!(text, "ONE\ntwo\nthree!");
    }
}
