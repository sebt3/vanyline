//! `vnl-ruff-lsp` — serveur LSP stdio minimal : diagnostics ruff seuls.
//!
//! Rôle `diagnostics-merge` du multiplexeur (feature python-support) : reçoit le
//! doc-sync (fan-out de la session), lance `ruff check --output-format json
//! --force-exclude -` avec le contenu du buffer sur STDIN (jamais l'URI/le
//! chemin en argv, jamais via un shell — contrainte de sécurité du design),
//! convertit la sortie en notifications `textDocument/publishDiagnostics`. Ne
//! répond à AUCUNE requête sauf `initialize`/`shutdown` (`-32601` partout
//! ailleurs, réflexe défensif — le multiplexeur n'envoie de requêtes qu'au
//! primaire). Déclencheurs : `didOpen`/`didSave` immédiats, `didChange`
//! debouncé 500 ms ; résultat périmé abandonné (jamais un diagnostic plus vieux
//! que le buffer).
//!
//! Diagnostics de fonctionnement sur stderr (`eprintln!`, comme `bin/maint.rs`) :
//! stdout est réservé aux trames LSP. Le cwd est celui hérité du multiplexeur
//! (`spawn_aux_startup` : `current_dir(sandbox_root)`) — ruff y trouve le
//! `pyproject.toml`/`ruff.toml`/`.ruff.toml` du projet, zéro interpolation.

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
/// `lsp_diagnostics` — qui ne modifie jamais le fichier — voie ruff).
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

/// Lance `ruff check --output-format json --force-exclude -`, écrit le buffer
/// sur le stdin du fils et rend sa stdout. Le code de sortie ≠ 0 (violations
/// trouvées) est NORMAL : stdout est rendue dans tous les cas, c'est
/// `convert_ruff_output` qui décide. Spawn en échec / erreur d'E/S ⟹ `Err`
/// (message à journaliser sur stderr par l'appelant, contribution vide).
async fn run_ruff(buffer: &str) -> Result<String, String> {
    // `ruff check --output-format json --force-exclude -` : le contenu du
    // buffer est écrit sur le STDIN du fils ; l'URL/l'URI/le chemin ne sont
    // JAMAIS un argv ; AUCUN shell (`Command::new` + argv littéraux uniquement,
    // jamais de `sh -c`). Writer détaché comme le hadolint — au-delà de la
    // capacité du pipe, un write_all séquentiel avant l'attente bloquerait les
    // deux bouts. PAS de current_dir : cwd hérité du multiplexeur
    // (sandbox_root, `spawn_aux_startup` dans lsp.rs) — c'est CE cwd que ruff
    // scanne pour son `pyproject.toml`/`ruff.toml`/`.ruff.toml`, zéro
    // interpolation.
    let mut child = tokio::process::Command::new("ruff")
        .args(["check", "--output-format", "json", "--force-exclude", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("impossible de lancer ruff: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "stdin du fils ruff indisponible".to_string())?;
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
        .map_err(|e| format!("ruff: attente de la sortie impossible: {e}"))?;
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
    let diagnostics = match run_ruff(&text).await {
        Ok(stdout) => {
            if serde_json::from_str::<Value>(&stdout).is_err() {
                eprintln!(
                    "vnl-ruff-lsp: VNL-SBX-LSP-013: sortie ruff non-JSON — contribution vide pour {uri}"
                );
            }
            convert_ruff_output(&stdout, &text)
        }
        Err(err) => {
            eprintln!("vnl-ruff-lsp: VNL-SBX-LSP-012: {err} — contribution vide pour {uri}");
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
                                "name": "vnl-ruff-lsp",
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
            // attente sur un fichier jamais modifié — sans ici, ruff ne serait
            // JAMAIS visible côté tools (note d'interprétation du fichier de
            // tâche).
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

/// Convertit la sortie `ruff check --output-format json` (tableau d'objets
/// `{code, message, location:{row,column}, end_location:{row,column}, url,
/// filename, cell, fix, name, noqa_row, severity}` — les champs inutiles sont
/// tolérés sans être lus) en diagnostics LSP. Positions ruff 1-based ⟹ ranges
/// LSP 0-based : start = `(row-1, column-1)` clampé à la longueur UTF-16 de la
/// ligne ; end depuis `end_location` (même conversion, même clamp ligne +
/// colonne), absent/`null`/malformé ⟹ FIN DE LA LIGNE de start (repli
/// à-la-hadolint), et un end écrasé avant le start par les clamps ⟹ fin de la
/// ligne de start aussi. Garde-fou `filename` : un item portant un `filename`
/// PRÉSENT et différent de `"-"` est ignoré (on passe toujours `-` en argv —
/// protection contre un changement de comportement futur ou un
/// `--stdin-filename` ajouté par un tiers). `severity` ruff RELAYÉE via
/// `ruff_severity` quand c'est une chaîne connue (ruff 0.16.6 pose `"error"` y
/// compris sur F401/I001) ; absente/`null`/inconnue ⟹ 2 (Warning) par défaut
/// (décision 2026-09-08). `code` verbatim (absente/`null` ⟹ clé
/// OMISE) ; `source`: `ruff` ; `message` verbatim ; `codeDescription.href`
/// seulement si `url` est une chaîne (`href` doit être une chaîne en LSP,
/// jamais `null` — le `url: null` de `invalid-syntax` ⟹ clé OMISE). Entrée
/// non-tableau / objet sans `location.row` exploitable ⟹ contribution vide
/// (`vec![]`). Limite documentée (design risque 2) : les colonnes ruff comptent
/// des scalaires Unicode, LSP parle UTF-16 ⟹ décalage possible de +1 par
/// caractère hors-BMP situé AVANT la violation sur la même ligne ; conversion
/// volontairement triviale (`-1`), pas de remap.
fn convert_ruff_output(ruff_json: &str, doc_text: &str) -> Vec<Value> {
    let Ok(parsed) = serde_json::from_str::<Value>(ruff_json) else {
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
            // Garde-fou `filename` : on passe toujours `-` en argv — un item
            // portant un autre nom de fichier ne vient pas de notre stdin.
            if let Some(filename) = obj.get("filename")
                && filename.as_str() != Some("-")
            {
                return None;
            }
            let location = obj.get("location")?.as_object()?;
            let row = location.get("row")?.as_i64()?;
            let column = location.get("column").and_then(Value::as_i64).unwrap_or(1);
            // 1-based → 0-based, ligne clampée au document (ruff peut
            // signaler au-delà d'un buffer édité entre-temps).
            let line_idx = usize::try_from(row - 1).unwrap_or(0).min(lines.len() - 1);
            let line_units = lines[line_idx].encode_utf16().count();
            let start_char = usize::try_from(column - 1).unwrap_or(0).min(line_units);
            // End depuis `end_location` (même conversion, même clamp) ; absent/
            // `null`/malformé ⟹ fin de la ligne de start (repli à-la-hadolint).
            let mut end = obj
                .get("end_location")
                .filter(|end_location| !end_location.is_null())
                .and_then(|end_location| {
                    let end_row = end_location.get("row")?.as_i64()?;
                    let end_column = end_location
                        .get("column")
                        .and_then(Value::as_i64)
                        .unwrap_or(1);
                    let end_idx = usize::try_from(end_row - 1)
                        .unwrap_or(0)
                        .min(lines.len() - 1);
                    let end_units = lines[end_idx].encode_utf16().count();
                    let end_char = usize::try_from(end_column - 1).unwrap_or(0).min(end_units);
                    Some((end_idx, end_char))
                })
                .unwrap_or((line_idx, line_units));
            // Un clamp qui écrase ne doit jamais produire de range inversée :
            // end antérieur au start ⟹ fin de la ligne de start.
            if end < (line_idx, start_char) {
                end = (line_idx, line_units);
            }
            let message = obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut diagnostic = serde_json::Map::new();
            diagnostic.insert(
                "range".to_string(),
                serde_json::json!({
                    "start": {"line": line_idx, "character": start_char},
                    "end": {"line": end.0, "character": end.1},
                }),
            );
            // Severity : la valeur ruff est EXPOSÉE quand c'est une chaîne
            // connue, Warning(2) n'est que le repli (décision 2026-09-08).
            diagnostic.insert(
                "severity".to_string(),
                serde_json::json!(ruff_severity(obj)),
            );
            if let Some(code) = obj.get("code").filter(|code| !code.is_null()) {
                diagnostic.insert("code".to_string(), code.clone());
            }
            diagnostic.insert("source".to_string(), serde_json::json!("ruff"));
            diagnostic.insert("message".to_string(), serde_json::json!(message));
            // `url` chaîne seulement (`invalid-syntax` rend `url: null` ⟹ clé
            // omise, `href` doit être une chaîne en LSP).
            if let Some(url) = obj.get("url").and_then(Value::as_str) {
                diagnostic.insert(
                    "codeDescription".to_string(),
                    serde_json::json!({"href": url}),
                );
            }
            Some(Value::Object(diagnostic))
        })
        .collect()
}

/// Sévérité LSP (`1` Error, `2` Warning, `3` Information, `4` Hint) depuis le
/// champ `severity` de ruff quand c'est une chaîne connue — `"error"`/`"fatal"`
/// → 1, `"warning"` → 2, `"info"`/`"information"`/`"notice"` → 3, `"hint"` → 4.
/// Absente / `null` / valeur inconnue ⟹ `2` (Warning) par défaut. Décision
/// 2026-09-08 : on relaie la sévérité ruff quand elle existe (ruff 0.16.6 pose
/// `"error"` y compris sur les lints de style comme `I001`), Warning ne reste
/// que le repli quand ruff n'en fournit aucune.
fn ruff_severity(obj: &serde_json::Map<String, Value>) -> i64 {
    match obj.get("severity").and_then(Value::as_str) {
        Some("error" | "fatal") => 1,
        Some("warning") => 2,
        Some("info" | "information" | "notice") => 3,
        Some("hint") => 4,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    // ── convert_ruff_output ─────────────────────────────────────────────────

    /// Test 1 : convert_ruff_output_example — TRACE-A verbatim (sortie réelle
    /// `ruff 0.16.6 check --output-format json --force-exclude -`), equality
    /// structurelle exacte (ranges, sévérités, code, source, ordre). Les `\n`
    /// internes des valeurs `fix.edits.content` sont des newlines littéraux
    /// échappés dans la JSON (préservés tels quels par la raw string) : les
    /// champs `cell`/`fix`/`name`/`noqa_row` de l'entrée n'apparaissent PAS
    /// dans la sortie ; `severity` de l'entrée (`"error"` sur les deux items)
    /// est relayée en `1` (Error) — cf. `ruff_severity`.
    #[test]
    fn convert_ruff_output_example() {
        let input = r#"[{"cell":null,"code":"I001","end_location":{"column":12,"row":2},"filename":"-","fix":{"applicability":"safe","edits":[{"content":"import json\nimport os\n\n","end_location":{"column":1,"row":4},"location":{"column":1,"row":1}}],"message":"Organize imports"},"location":{"column":1,"row":1},"message":"Import block is un-sorted or un-formatted","name":"unsorted-imports","noqa_row":1,"severity":"error","url":"https://docs.astral.sh/ruff/rules/unsorted-imports"},{"cell":null,"code":"F401","end_location":{"column":10,"row":1},"filename":"-","fix":{"applicability":"safe","edits":[{"content":"","end_location":{"column":1,"row":2},"location":{"column":1,"row":1}}],"message":"Remove unused import: `os`"},"location":{"column":8,"row":1},"message":"`os` imported but unused","name":"unused-import","noqa_row":1,"severity":"error","url":"https://docs.astral.sh/ruff/rules/unused-import"}]"#;
        let doc_text = "import os\nimport json\n\nprint(json.dumps({\"a\": 1}))\n";
        let out = convert_ruff_output(input, doc_text);
        let expected = json!([
            {"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":11}},
             "severity":1,"code":"I001","source":"ruff",
             "message":"Import block is un-sorted or un-formatted",
             "codeDescription":{"href":"https://docs.astral.sh/ruff/rules/unsorted-imports"}},
            {"range":{"start":{"line":0,"character":7},"end":{"line":0,"character":9}},
             "severity":1,"code":"F401","source":"ruff",
             "message":"`os` imported but unused",
             "codeDescription":{"href":"https://docs.astral.sh/ruff/rules/unused-import"}}
        ]);
        assert_eq!(Value::Array(out), expected);
    }

    /// Test 2 : convert_ruff_output_severity_relayed — la sévérité ruff est
    /// RELAYÉE quand c'est une chaîne connue (`"error"` → 1, `"warning"` → 2,
    /// `"info"` → 3, `"hint"` → 4) ; absente ou valeur inconnue ⟹ 2 (Warning)
    /// par défaut (décision 2026-09-08).
    #[test]
    fn convert_ruff_output_severity_relayed() {
        let input = r#"[
            {"cell":null,"code":"A","end_location":{"column":1,"row":1},"filename":"-","fix":null,"location":{"column":1,"row":1},"message":"a","name":"x","noqa_row":1,"severity":"error","url":null},
            {"cell":null,"code":"B","end_location":{"column":1,"row":2},"filename":"-","fix":null,"location":{"column":1,"row":2},"message":"b","name":"x","noqa_row":2,"severity":"warning","url":null},
            {"cell":null,"code":"C","end_location":{"column":1,"row":3},"filename":"-","fix":null,"location":{"column":1,"row":3},"message":"c","name":"x","noqa_row":3,"url":null},
            {"cell":null,"code":"D","end_location":{"column":1,"row":4},"filename":"-","fix":null,"location":{"column":1,"row":4},"message":"d","name":"x","noqa_row":4,"severity":"info","url":null},
            {"cell":null,"code":"E","end_location":{"column":1,"row":5},"filename":"-","fix":null,"location":{"column":1,"row":5},"message":"e","name":"x","noqa_row":5,"severity":"hint","url":null},
            {"cell":null,"code":"F","end_location":{"column":1,"row":6},"filename":"-","fix":null,"location":{"column":1,"row":6},"message":"f","name":"x","noqa_row":6,"severity":"bogus","url":null}
        ]"#;
        let out = convert_ruff_output(input, "a\nb\nc\nd\ne\nf");
        assert_eq!(out.len(), 6);
        let severities: Vec<i64> = out
            .iter()
            .map(|d| d["severity"].as_i64().unwrap())
            .collect();
        assert_eq!(severities, vec![1, 2, 2, 3, 4, 2]);
    }

    /// Test 3 : convert_ruff_output_url_null_omits_code_description — TRACE-B
    /// verbatim (2 items `invalid-syntax`, `url: null`) : le `codeDescription`
    /// est OMIS (`href` doit être une chaîne en LSP, jamais `null`), mais
    /// `code` reste présent.
    #[test]
    fn convert_ruff_output_url_null_omits_code_description() {
        let input = r#"[{"cell":null,"code":"invalid-syntax","end_location":{"column":13,"row":1},"filename":"-","fix":null,"location":{"column":12,"row":1},"message":"Expected a parameter or the end of the parameter list","name":"invalid-syntax","noqa_row":null,"severity":"error","url":null},{"cell":null,"code":"invalid-syntax","end_location":{"column":1,"row":2},"filename":"-","fix":null,"location":{"column":1,"row":2},"message":"unexpected EOF while parsing","name":"invalid-syntax","noqa_row":null,"severity":"error","url":null}]"#;
        let out = convert_ruff_output(input, "def broken(:\n");
        assert_eq!(out.len(), 2);
        // Item 1 : « def broken(: » = 12 unités UTF-16.
        assert_eq!(out[0]["code"], json!("invalid-syntax"));
        assert!(out[0].get("codeDescription").is_none());
        // `invalid-syntax` porte `severity: "error"` ⟹ relayé en 1 (Error).
        assert_eq!(out[0]["severity"], json!(1));
        assert_eq!(out[0]["range"]["start"], json!({"line":0,"character":11}));
        assert_eq!(out[0]["range"]["end"], json!({"line":0,"character":12}));
        // Item 2 : dernière ligne vide (doc fini par \n), start = end.
        assert!(out[1].get("codeDescription").is_none());
        assert_eq!(out[1]["range"]["start"], json!({"line":1,"character":0}));
        assert_eq!(out[1]["range"]["end"], json!({"line":1,"character":0}));
    }

    /// Test 4 : convert_ruff_output_filename_guard — item avec `filename`
    /// `"-"` conservé, `filename` absent conservé, `filename` étranger ignoré
    /// (on passe toujours `-` en argv).
    #[test]
    fn convert_ruff_output_filename_guard() {
        let input = r#"[
            {"cell":null,"code":"A","end_location":{"column":1,"row":1},"filename":"-","fix":null,"location":{"column":1,"row":1},"message":"filename = -","name":"g","noqa_row":1,"severity":"error","url":null},
            {"cell":null,"code":"B","end_location":{"column":1,"row":1},"fix":null,"location":{"column":1,"row":1},"message":"filename absent","name":"g","noqa_row":1,"severity":"error","url":null},
            {"cell":null,"code":"C","end_location":{"column":1,"row":1},"filename":"/chemin/autre.py","fix":null,"location":{"column":1,"row":1},"message":"filename étranger","name":"g","noqa_row":1,"severity":"error","url":null}
        ]"#;
        let out = convert_ruff_output(input, "x\n");
        // Le contrat du garde-fou : « absent ou "-" ⟹ conservé », seul un
        // filename ÉTRANGER est ignoré ⟹ 2 diagnostics (A et B), pas 1.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["code"], json!("A"));
        assert_eq!(out[1]["code"], json!("B"));
    }

    /// Test 5 : convert_ruff_output_clamps — ligne hors bornes (row 99 dans un
    /// doc de 2 lignes ⟹ clamp ligne 1), colonne hors borne ⟹ clamp fin de
    /// ligne (début comme fin) ; end_location antérieur au start après clamp
    /// (fabrication défensive) ⟹ end remplacé par la fin de la ligne de start.
    #[test]
    fn convert_ruff_output_clamps() {
        // Doc de 2 lignes sans newline final : ligne 99 → clamp ligne 1.
        let input = r#"[{"cell":null,"code":"L1","end_location":{"column":99,"row":99},"filename":"-","fix":null,"location":{"column":1,"row":99},"message":"ligne hors bornes","name":"l","noqa_row":99,"severity":"error","url":null}]"#;
        let out = convert_ruff_output(input, "a = 1\nb = 2");
        assert_eq!(out[0]["range"]["start"], json!({"line":1,"character":0}));
        assert_eq!(out[0]["range"]["end"], json!({"line":1,"character":5})); // « b = 2 »

        // Colonne hors borne → clamp fin de ligne (début comme fin).
        let input = r#"[{"cell":null,"code":"L2","end_location":{"column":99,"row":1},"filename":"-","fix":null,"location":{"column":99,"row":1},"message":"colonne hors borne","name":"l","noqa_row":1,"severity":"error","url":null}]"#;
        let out = convert_ruff_output(input, "a = 1\nb = 2");
        assert_eq!(out[0]["range"]["start"], json!({"line":0,"character":5}));
        assert_eq!(out[0]["range"]["end"], json!({"line":0,"character":5}));

        // end_location antérieur au start après clamp (start col 5, end col 2,
        // même ligne) ⟹ end = fin de la ligne de start.
        let input = r#"[{"cell":null,"code":"L3","end_location":{"column":2,"row":1},"filename":"-","fix":null,"location":{"column":5,"row":1},"message":"end antérieur après clamp","name":"l","noqa_row":1,"severity":"error","url":null}]"#;
        let out = convert_ruff_output(input, "a = 1\nb = 2");
        assert_eq!(out[0]["range"]["start"], json!({"line":0,"character":4}));
        assert_eq!(out[0]["range"]["end"], json!({"line":0,"character":5}));
    }

    /// Test 6 : convert_ruff_output_end_location_missing_falls_back_to_line_end
    /// — item avec `location` présent, SANS la clé `end_location` (objet d'un
    /// autre émetteur / version passée) ⟹ end = fin de la ligne de start en
    /// unités UTF-16. Doc `x = "🐛"\n` : la ligne fait 8 unités UTF-16 (`x`,
    /// espace, `=`, espace, `"`, 🐛 = 2 unités, `"`) pour 7 scalaires Unicode /
    /// 11 octets — `end.character` doit valoir 8 : c'est précisément le test
    /// qui casse une implémentation qui compterait en scalaires ou en octets au
    /// lieu d'UTF-16.
    #[test]
    fn convert_ruff_output_end_location_missing_falls_back_to_line_end() {
        let input = r#"[{"cell":null,"code":"N1","filename":"-","fix":null,"location":{"column":1,"row":1},"message":"sans end_location","name":"n","noqa_row":1,"severity":"error","url":null}]"#;
        let doc = "x = \"\u{1F41B}\"\n";
        let out = convert_ruff_output(input, doc);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["range"]["start"], json!({"line":0,"character":0}));
        // 8 unités UTF-16 (ni 7 scalaires, ni les octets).
        assert_eq!(out[0]["range"]["end"], json!({"line":0,"character":8}));
    }

    /// Test 7 : convert_ruff_output_utf16_shift_documented — TRACE-C verbatim
    /// (doc `x = print("🐛") ; import os\n`) : le F401 (colonne 25 en scalaires
    /// Unicode ruff) atterrit `character: 24` en sortie (24 = 25−1, conversion
    /// triviale), là où l'UTF-16 strict serait 25 — décalage de 1 unité ASSUMÉ
    /// par le design (risque 2, même classe que le « CRLF cosmétique »
    /// docker-lsp) : les colonnes ruff comptent des scalaires Unicode, LSP
    /// parle UTF-16, et un 🐛 avant la violation sur la même ligne décale
    /// d'une unité. Ce test FIGE 24 : c'est la limite documentée, pas un bug
    /// (pas de remap scalaires→UTF-16 en v1).
    #[test]
    fn convert_ruff_output_utf16_shift_documented() {
        let input = r#"[{"cell":null,"code":"I001","end_location":{"column":27,"row":1},"filename":"-","fix":{"applicability":"safe","edits":[{"content":"import os\n","end_location":{"column":27,"row":1},"location":{"column":18,"row":1}}],"message":"Organize imports"},"location":{"column":18,"row":1},"message":"Import block is un-sorted or un-formatted","name":"unsorted-imports","noqa_row":1,"severity":"error","url":"https://docs.astral.sh/ruff/rules/unsorted-imports"},{"cell":null,"code":"F401","end_location":{"column":27,"row":1},"filename":"-","fix":{"applicability":"safe","edits":[{"content":"","end_location":{"column":27,"row":1},"location":{"column":18,"row":1}}],"message":"Remove unused import: `os`"},"location":{"column":25,"row":1},"message":"`os` imported but unused","name":"unused-import","noqa_row":1,"severity":"error","url":"https://docs.astral.sh/ruff/rules/unused-import"}]"#;
        let doc = "x = print(\"\u{1F41B}\") ; import os\n";
        let out = convert_ruff_output(input, doc);
        assert_eq!(out.len(), 2);
        // I001 : 18 − 1 = 17 (conversion triviale).
        assert_eq!(out[0]["range"]["start"], json!({"line":0,"character":17}));
        // F401 : 25 − 1 = 24, non 25 — écart ASSUMÉ (limite documentée du
        // design risque 2, pas un bug : pas de remap scalaires→UTF-16 en v1).
        assert_eq!(out[1]["range"]["start"], json!({"line":0,"character":24}));
    }

    /// Test 8 : convert_ruff_output_garbage — `"{}"`, `"not json"`, tableau
    /// d'un objet sans `location`, tableau d'un objet avec `location.row`
    /// non-numérique ⟹ `vec![]` / items ignorés.
    #[test]
    fn convert_ruff_output_garbage() {
        assert!(convert_ruff_output("{}", "x\n").is_empty());
        assert!(convert_ruff_output("not json", "x\n").is_empty());
        assert!(
            convert_ruff_output(
                r#"[{"cell":null,"code":"X","fix":null,"message":"pas de location","name":"x","noqa_row":1,"severity":"error","url":null}]"#,
                "x\n"
            )
            .is_empty()
        );
        assert!(
            convert_ruff_output(
                r#"[{"cell":null,"code":"Y","fix":null,"location":{"column":1,"row":"première"},"message":"row non-numérique","name":"y","noqa_row":1,"severity":"error","url":null}]"#,
                "x\n"
            )
            .is_empty()
        );
    }

    // ── apply_content_changes ───────────────────────────────────────────────

    /// Test 9 : apply_content_changes_full_replace — changement sans `range`
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

    /// Test 10 : apply_content_changes_incremental — remplacement intra-ligne
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

    /// Test 11 : apply_content_changes_bounded — range au-delà de la fin du
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
}
