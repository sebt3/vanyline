//! Client de protocole LSP par-dessus une session partagée (`crate::lsp::LspSession`).
//! Le process LSP ne s'initialise qu'une fois (flag partagé) ; chaque `LspClient`
//! s'abonne à la session, envoie requêtes/notifications et attend ses réponses.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::lsp::{ClientId, LspSession};

/// Durée d'attente des diagnostics push après un `didOpen`.
pub const DIAGNOSTICS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Client de protocole LSP : s'abonne à la session à la construction, se désabonne
/// au drop.
pub struct LspClient {
    session: Arc<LspSession>,
    client_id: ClientId,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    next_id: AtomicU64,
    root_uri: String,
}

impl LspClient {
    /// Abonne un client à `session` et prépare `root_uri` (URI `file://` de la racine
    /// du workspace, utilisée dans les params `initialize`).
    pub fn new(session: Arc<LspSession>, root_uri: String) -> Self {
        let (client_id, rx) = session.subscribe();
        Self {
            session,
            client_id,
            rx,
            next_id: AtomicU64::new(1),
            root_uri,
        }
    }

    /// Assure que le process est initialisé. Si CE client gagne `try_mark_initialized` :
    /// envoie `initialize` (params : `processId: null`, `rootUri: root_uri`,
    /// `capabilities: {}`, `workspaceFolders: [{"uri": root_uri, "name": ""}]`), attend la
    /// réponse et rend son `result` ; puis envoie la notification `initialized`.
    /// Si déjà initialisé : rend `Value::Null` sans envoyer quoi que ce soit.
    pub async fn initialize(&mut self) -> anyhow::Result<Value> {
        if !self.session.try_mark_initialized() {
            return Ok(Value::Null);
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst) as i64;
        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": self.root_uri,
                "capabilities": {},
                "workspaceFolders": [{
                    "uri": self.root_uri,
                    "name": ""
                }]
            }
        });

        // Send the initialize request (session rewrites ID, stores pending mapping).
        self.session
            .send(self.client_id, initialize.to_string().into_bytes())
            .await?;

        // Wait for the response: read messages from rx until we find one
        // matching our `id`.
        loop {
            let raw = self.rx.recv().await.ok_or_else(|| {
                anyhow::anyhow!(
                    "VNL-SBX-LSP-004: LSP process closed while waiting for initialize response"
                )
            })?;
            let msg: Value = match serde_json::from_slice(&raw) {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!("LSP client: failed to parse init response: {e}");
                    continue;
                }
            };
            if let Some(msg_id) = msg.get("id").and_then(|v| v.as_i64())
                && msg_id == id
            {
                let result = if let Some(result) = msg.get("result") {
                    result.clone()
                } else {
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-005: LSP response has neither 'result' nor 'error'"
                    ));
                };

                // Send `initialized` notification after getting the response.
                let initialized_notif = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "initialized",
                    "params": {}
                });
                let _ = self
                    .session
                    .send(self.client_id, initialized_notif.to_string().into_bytes())
                    .await;

                return Ok(result);
            }
            // Notification without id — ignore
        }
    }

    /// Envoie une notification JSON-RPC (`method` + `params`) au process. Erreur si le
    /// process est fermé (`VNL-SBX-LSP-003` depuis `LspSession::send`).
    pub async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.session
            .send(self.client_id, notif.to_string().into_bytes())
            .await
    }

    /// Envoie la notification `textDocument/didOpen` avec
    /// `{uri, languageId, version: 1, text}`.
    pub async fn did_open(&self, uri: &str, language_id: &str, text: &str) -> anyhow::Result<()> {
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text
                }
            }),
        )
        .await
    }

    /// Envoie `didOpen` seulement si CET appelant est le premier, tous clients de la
    /// session confondus (éditeur navigateur compris), à demander l'ouverture de
    /// `uri` (`session.try_mark_uri_open`) — sinon no-op. Un second `didOpen` sur une
    /// URI déjà ouverte est une violation de protocole LSP ; observé en usage réel
    /// comme cause probable de `lsp_diagnostics` "one-shot" (chaque appel de tool MCP
    /// envoyait son propre `didOpen` sur le même fichier).
    pub async fn ensure_open(
        &self,
        uri: &str,
        language_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        if !self.session.try_mark_uri_open(uri) {
            return Ok(());
        }
        self.did_open(uri, language_id, text).await
    }

    /// Envoie une requête `method` avec `params` et attend la réponse dont l'id correspond.
    /// Saute les notifications reçues avant la réponse. Rend le `result` de la réponse.
    /// Erreurs : process fermé en attendant (`VNL-SBX-LSP-004`), réponse `error` du serveur
    /// (`VNL-SBX-LSP-005`), réponse sans `result` ni `error`.
    pub async fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) as i64;
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.session
            .send(self.client_id, req.to_string().into_bytes())
            .await?;

        loop {
            let raw = self.rx.recv().await.ok_or_else(|| {
                anyhow::anyhow!("VNL-SBX-LSP-004: LSP process closed while waiting for response")
            })?;

            let msg: Value = match serde_json::from_slice(&raw) {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!("LSP client: failed to parse response: {e}");
                    continue;
                }
            };

            match msg.get("id").and_then(|v| v.as_i64()) {
                Some(msg_id) if msg_id == id => {
                    if let Some(error) = msg.get("error") {
                        return Err(anyhow::anyhow!(
                            "VNL-SBX-LSP-005: LSP server error: {}",
                            error
                        ));
                    }
                    if let Some(result) = msg.get("result") {
                        return Ok(result.clone());
                    }
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-005: LSP response has neither 'result' nor 'error'"
                    ));
                }
                _ => continue,
            }
        }
    }

    /// `initialize` + `ensure_open(uri, language_id, text)`, puis rend les
    /// diagnostics en cache pour `uri` (`LspSession::wait_for_diagnostics` — alimenté
    /// par TOUT abonné qui a jamais reçu un `publishDiagnostics` pour cette URI, pas
    /// seulement cet appel-ci : sans ça, un push déjà arrivé avant l'abonnement de ce
    /// `LspClient` — ex. diagnostics publiés dès l'ouverture du fichier par l'éditeur
    /// navigateur, avant qu'un tool MCP ne s'y intéresse — serait perdu ; observé en
    /// usage réel comme cause probable de "jamais de diagnostics Rust" côté tool MCP).
    ///
    /// `Some(vec)` (vide inclus) dès qu'une publication a été vue — `None` seulement
    /// si rien n'a jamais été publié dans le délai, distinct d'un vecteur vide
    /// (cf. doc `LspSession::wait_for_diagnostics` — un vecteur vide veut dire
    /// "propre", pas "pas encore su"). Erreur si `initialize`/`didOpen` échouent.
    pub async fn diagnostics(
        &mut self,
        uri: &str,
        language_id: &str,
        text: &str,
    ) -> anyhow::Result<Option<Vec<Value>>> {
        let _ = self.initialize().await?;
        self.ensure_open(uri, language_id, text).await?;
        Ok(self
            .session
            .wait_for_diagnostics(uri, DIAGNOSTICS_TIMEOUT)
            .await)
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.session.unsubscribe(self.client_id);
    }
}

/// Fakes LSP partagés entre les tests de `lsp_client` et `tools_impl`.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub mod lsp_test_fakes {
    use crate::lsp::{LspManager, LspToolchain};

    /// Script Python factice : implémente un mini LSP (initialize, hover, definition,
    /// references, rename, documentSymbol, didOpen→publishDiagnostics).
    /// Utilise le framing LSP (Content-Length) en entrée et en sortie.
    pub const FAKE_LSP_PY: &str = r#"
import sys, json, re, os

def read_frame():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    length = 0
    for line in text.strip().split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b""
    data = b""
    while len(data) < length:
        chunk = sys.stdin.buffer.read(length - len(data))
        if not chunk:
            break
        data += chunk
    return data

def write_frame(data):
    sys.stdout.buffer.write(f"Content-Length: {len(data)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

count = 0
opened_uris = set()
while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
        msg_id = msg.get("id")
        method = msg.get("method", "")
        params = msg.get("params", {})

        if msg_id is None:
            # Notification : pas de réponse JSON-RPC. didOpen publie des diagnostics —
            # UNE SEULE FOIS par URI (comme un vrai serveur), pas à chaque appel : un
            # test qui s'attendrait à un nouveau push sur un didOpen redondant sur la
            # même URI masquerait le bug réel (cf. try_mark_uri_open côté Rust).
            if method == "textDocument/didOpen":
                uri = params.get("textDocument", {}).get("uri", "")
                if uri not in opened_uris:
                    opened_uris.add(uri)
                    notif = {
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {
                            "uri": uri,
                            "diagnostics": [{"message": "fake diag", "severity": 1, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 10}}}]
                        }
                    }
                    write_frame(json.dumps(notif).encode("utf-8"))
            # initialized et autres notifications : aucune réponse.
            continue

        if method == "initialize":
            count += 1
            result = {
                "capabilities": {},
                "serverInfo": {"name": "fake", "version": "1"},
                "initializeCount": count
            }
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id, "result": result
            }).encode("utf-8"))
        elif method == "textDocument/hover":
            # MarkupContent (objet direct, PAS un array) — la forme réellement rendue
            # par rust-analyzer/typescript-language-server. Un array ici masquerait le
            # vrai bug de parsing (cf. hover_contents_to_text côté tools_impl.rs).
            uri = params.get("textDocument", {}).get("uri", "")
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {
                    "contents": {"kind": "plaintext", "value": f"hover:{uri}"},
                    "range": None
                }
            }).encode("utf-8"))
        elif method == "textDocument/definition":
            uri = params.get("textDocument", {}).get("uri", "")
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": [
                    {"uri": uri, "range": {"start": {"line": 0}, "end": {"line": 0}}},
                    {"uri": "file:///external/lib.rs", "range": {"start": {"line": 41}, "end": {"line": 41}}}
                ]
            }).encode("utf-8"))
        elif method == "textDocument/references":
            # references — scanner workspace-wide (tâche 04) reflétant le
            # comportement réel : résoudre le MOT (identifiant [A-Za-z0-9_]+)
            # couvrant params.position dans le fichier demandé (aucun mot →
            # []), puis renvoyer toute occurrence \b<mot>\b des fichiers .rs
            # de os.walk(".") triés (cwd = sandbox_root, cf. lsp.rs::spawn) +
            # UNE entrée synthétique fixe hors workspace (groupe R5 à tester :
            # rendue brute, jamais lue, jamais documentSymbol'ée). Cap 20.
            uri = params.get("textDocument", {}).get("uri", "")
            pos = params.get("position", {})
            line0 = pos.get("line", 0)
            char0 = pos.get("character", 0)
            word = None
            try:
                text = open(uri.removeprefix("file://")).read()
                lines = text.splitlines()
                if line0 < len(lines):
                    for m in re.finditer(r"[A-Za-z0-9_]+", lines[line0]):
                        if m.start() <= char0 < m.end():
                            word = m.group(0)
                            break
            except Exception:
                word = None
            result = []
            if word is not None:
                files = []
                for dirpath, _dirnames, filenames in os.walk("."):
                    for filename in filenames:
                        if filename.endswith(".rs"):
                            files.append(os.path.join(dirpath, filename))
                for path in sorted(files):
                    if len(result) >= 20:
                        break
                    try:
                        text = open(path).read()
                    except Exception:
                        continue
                    for m in re.finditer(r"\b" + re.escape(word) + r"\b", text):
                        line_start = text.rfind("\n", 0, m.start()) + 1
                        l0 = text.count("\n", 0, m.start())
                        result.append({
                            "uri": "file://" + os.path.abspath(path),
                            "range": {
                                "start": {"line": l0, "character": m.start() - line_start},
                                "end": {"line": l0, "character": m.end() - line_start}
                            }
                        })
                        if len(result) >= 20:
                            break
                result.append({
                    "uri": "file:///external/lib.rs",
                    "range": {"start": {"line": 10, "character": 4}, "end": {"line": 10, "character": 8}}
                })
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id, "result": result
            }).encode("utf-8"))
        elif method == "textDocument/rename":
            uri = params.get("textDocument", {}).get("uri", "")
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {
                    "changes": {
                        uri: [
                            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}, "newText": "X"}
                        ]
                    }
                }
            }).encode("utf-8"))
        elif method == "textDocument/documentSymbol":
            # documentSymbol — le fake couvre les DEUX formes du contrat : si le
            # fichier contient HIER, forme DocumentSymbol (hiérarchique : range +
            # selectionRange + detail = signature + children) ; sinon forme plate
            # SymbolInformation — celle que rust-analyzer ET
            # typescript-language-server rendent réellement avec capabilities: {}
            # (vérification R2 sur cluster). Réponse toujours un tableau JSON.
            uri = params.get("textDocument", {}).get("uri", "")
            try:
                text = open(uri.removeprefix("file://")).read()
            except Exception:
                text = ""
            if "HIER" in text:
                result = [{
                    "name": "Outer", "kind": 23, "detail": "struct Outer",
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 1}},
                    "selectionRange": {"start": {"line": 0, "character": 7}, "end": {"line": 0, "character": 12}},
                    "children": [{
                        "name": "run", "kind": 6, "detail": "() -> ()",
                        "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 14}},
                        "selectionRange": {"start": {"line": 2, "character": 7}, "end": {"line": 2, "character": 10}}
                    }]
                }]
            else:
                result = []
                for m in re.finditer(r"(fn|struct) (\w+)", text):
                    line0 = text[:m.start()].count("\n")
                    result.append({
                        "name": m.group(2),
                        "kind": 12 if m.group(1) == "fn" else 23,
                        "location": {
                            "uri": uri,
                            "range": {
                                "start": {"line": line0, "character": 0},
                                "end": {"line": line0, "character": 1}
                            }
                        }
                    })
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": result
            }).encode("utf-8"))
        elif method == "workspace/symbol":
            # workspace/symbol (tâche 03b) — erreur -32601 pour query vide ou
            # "NOSUPPORT" (sentinelle de test pour la dégradation ; le query
            # vide n'arrive jamais par le tool, requis par le schéma), sinon
            # scan os.walk(".") (cwd = sandbox_root, cf. lsp.rs::spawn) des
            # fichiers .rs/.ts TRIÉS (ordre déterministe), symboles dont
            # query in name, rendu plat SymbolInformation limité à 20 entrées.
            # Rend TOUJOURS un tableau (jamais null).
            query = params.get("query", "")
            if query == "" or query == "NOSUPPORT":
                write_frame(json.dumps({
                    "jsonrpc": "2.0", "id": msg_id,
                    "error": {"code": -32601, "message": "unknown request"}
                }).encode("utf-8"))
            else:
                files = []
                for dirpath, _dirnames, filenames in os.walk("."):
                    for filename in filenames:
                        if filename.endswith(".rs") or filename.endswith(".ts"):
                            files.append(os.path.join(dirpath, filename))
                result = []
                for path in sorted(files):
                    if len(result) >= 20:
                        break
                    try:
                        text = open(path).read()
                    except Exception:
                        continue
                    for m in re.finditer(r"(fn|struct) (\w+)", text):
                        if query not in m.group(2):
                            continue
                        line0 = text[:m.start()].count("\n")
                        result.append({
                            "name": m.group(2),
                            "kind": 12 if m.group(1) == "fn" else 23,
                            "location": {
                                "uri": "file://" + os.path.abspath(path),
                                "range": {
                                    "start": {"line": line0, "character": 0},
                                    "end": {"line": line0, "character": 1}
                                }
                            }
                        })
                        if len(result) >= 20:
                            break
                write_frame(json.dumps({
                    "jsonrpc": "2.0", "id": msg_id,
                    "result": result
                }).encode("utf-8"))
        else:
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {"echo": method}
            }).encode("utf-8"))
    except Exception:
        pass
"#;

    /// Script Python factice : ignore didOpen, ne publie aucun diagnostic. Timeout → vec![].
    pub const FAKE_LSP_NODIAG_PY: &str = r#"
import sys, json

def read_frame():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    length = 0
    for line in text.strip().split("\r\n"):
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b""
    data = b""
    while len(data) < length:
        chunk = sys.stdin.buffer.read(length - len(data))
        if not chunk:
            break
        data += chunk
    return data

def write_frame(data):
    sys.stdout.buffer.write(f"Content-Length: {len(data)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()

while True:
    raw = read_frame()
    if not raw:
        break
    try:
        msg = json.loads(raw)
        msg_id = msg.get("id")
        method = msg.get("method", "")
        params = msg.get("params", {})
        if msg_id is None:
            # Notification : aucune réponse (ce script ne publie jamais de diagnostics).
            continue
        if method == "initialize":
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {"capabilities": {}, "serverInfo": {"name": "fake", "version": "1"}}
            }).encode("utf-8"))
        else:
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {"echo": method}
            }).encode("utf-8"))
    except Exception:
        pass
"#;

    /// Crée un LspManager avec un script Python factice.
    pub async fn make_manager(name: &str, script: &str) -> (LspManager, tempfile::TempDir) {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let script_path = tmpdir.path().join(format!("fake_lsp_{name}.py"));
        std::fs::write(&script_path, script).unwrap();
        let manager = LspManager::new(
            vec![LspToolchain {
                name: name.to_string(),
                bin: "python3".to_string(),
                args: vec![script_path.to_string_lossy().to_string()],
            }],
            tmpdir.path().to_path_buf(),
        );
        (manager, tmpdir)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::lsp_client::lsp_test_fakes::*;
    use std::time::Duration;

    // ── Tests de LspClient ──────────────────────────────────────────────────

    /// Test 1: client_initialize_once — deux clients sur la même session,
    /// seuls le premier envoie le initialize.
    #[tokio::test]
    async fn client_initialize_once() {
        let (manager, _tmpdir) = make_manager("fake", FAKE_LSP_PY).await;
        let session = manager
            .get_or_spawn("fake")
            .await
            .expect("spawn ok")
            .expect("should have session");

        let root = "file:///workspace";
        let mut client_a = LspClient::new(Arc::clone(&session), root.to_string());
        let mut client_b = LspClient::new(Arc::clone(&session), root.to_string());

        let result_a = client_a.initialize().await.expect("A initialize ok");
        assert!(
            result_a.is_object(),
            "result_a should be object, got: {:?}",
            result_a
        );
        assert_eq!(
            result_a["initializeCount"].as_i64().unwrap(),
            1,
            "first client should get initializeCount == 1"
        );

        let result_b = client_b.initialize().await.expect("B initialize ok");
        assert!(
            result_b.is_null(),
            "second client should get Value::Null (already initialized)"
        );

        assert!(
            session.is_initialized(),
            "session should be initialized after A"
        );
    }

    /// Test 2: client_request_returns_hover — didOpen émet un diagnostic push
    /// avant la requête hover : request() doit sauter la notification.
    #[tokio::test]
    async fn client_request_returns_hover() {
        let (manager, _tmpdir) = make_manager("fake", FAKE_LSP_PY).await;
        let session = manager
            .get_or_spawn("fake")
            .await
            .expect("spawn ok")
            .expect("should have session");

        let mut client = LspClient::new(session, "file:///workspace".to_string());

        let timeout = tokio::time::timeout(Duration::from_secs(10), async {
            client.initialize().await.expect("init ok");
            let uri = "file:///workspace/main.rs";
            client
                .did_open(uri, "rust", "fn main(){}")
                .await
                .expect("did_open ok");
            let hover_params = serde_json::json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 0}
            });
            let result = client
                .request("textDocument/hover", hover_params)
                .await
                .expect("hover request ok");
            assert_eq!(
                result["contents"]["value"].as_str().unwrap(),
                "hover:file:///workspace/main.rs",
                "hover result value must match"
            );
        })
        .await;

        assert!(timeout.is_ok(), "hover test must complete within timeout");
    }

    /// Test 3: client_diagnostics_captures_publish — diagnostics() collecte
    /// la notification publishDiagnostics émise par didOpen.
    #[tokio::test]
    async fn client_diagnostics_captures_publish() {
        let (manager, _tmpdir) = make_manager("fake", FAKE_LSP_PY).await;
        let session = manager
            .get_or_spawn("fake")
            .await
            .expect("spawn ok")
            .expect("should have session");

        let mut client = LspClient::new(session, "file:///workspace".to_string());

        let timeout = tokio::time::timeout(
            Duration::from_secs(10),
            client.diagnostics("file:///workspace/main.rs", "rust", "fn main(){}"),
        )
        .await;

        assert!(
            timeout.is_ok(),
            "diagnostics test must complete within timeout"
        );
        let diags = timeout
            .unwrap()
            .expect("diagnostics ok")
            .expect("should be Some — a publish was seen");
        assert!(!diags.is_empty(), "should capture at least one diagnostic");
        assert_eq!(
            diags[0]["message"].as_str().unwrap(),
            "fake diag",
            "diagnostic message must match"
        );
        assert_eq!(
            diags[0]["severity"].as_i64().unwrap(),
            1,
            "diagnostic severity must match"
        );
    }

    /// Bug réel : chaque appel de tool MCP crée un nouveau `LspClient` (nouvel
    /// abonnement) et appelait jusqu'ici `did_open` sans condition — un second appel
    /// de `lsp_diagnostics` sur le MÊME fichier renvoyait `[]` (un didOpen redondant
    /// n'a aucune raison de republier chez un serveur réel, cf. fake ci-dessus
    /// modifié pour ne publier qu'une fois par URI). Avec `ensure_open` +
    /// `LspSession::diagnostics_cache`, un second client sur la même URI doit obtenir
    /// les MÊMES diagnostics, lus depuis le cache, sans déclencher un nouveau
    /// `didOpen`.
    #[tokio::test]
    async fn client_diagnostics_second_call_same_uri_reads_cache() {
        let (manager, _tmpdir) = make_manager("fake", FAKE_LSP_PY).await;
        let session = manager
            .get_or_spawn("fake")
            .await
            .expect("spawn ok")
            .expect("should have session");
        let uri = "file:///workspace/main.rs";

        let mut client_a = LspClient::new(Arc::clone(&session), "file:///workspace".to_string());
        let diags_a = tokio::time::timeout(
            Duration::from_secs(10),
            client_a.diagnostics(uri, "rust", "fn main(){}"),
        )
        .await
        .expect("first call must complete within timeout")
        .expect("diagnostics ok")
        .expect("should be Some — a publish was seen");
        assert!(
            !diags_a.is_empty(),
            "first call should capture a diagnostic"
        );

        // Nouveau LspClient — même scénario qu'un deuxième appel de tool MCP.
        let mut client_b = LspClient::new(Arc::clone(&session), "file:///workspace".to_string());
        let diags_b = tokio::time::timeout(
            Duration::from_secs(10),
            client_b.diagnostics(uri, "rust", "fn main(){}"),
        )
        .await
        .expect("second call must complete within timeout — not hang on a push that never comes")
        .expect("diagnostics ok")
        .expect("should be Some — read from cache, not a fresh timeout");
        assert!(
            !diags_b.is_empty(),
            "second call on the same uri must also return the cached diagnostic, not []"
        );
        assert_eq!(
            diags_a, diags_b,
            "both calls should see the same diagnostic"
        );
    }

    /// Test 4: client_diagnostics_empty_on_no_publish — script sans publication de
    /// diags → timeout → `None`, PAS `Some(vec![])` : "jamais reçu" est distinct de
    /// "reçu un vecteur vide" (le serveur a explicitement publié "rien à signaler"),
    /// cf. doc `LspSession::wait_for_diagnostics` — un agent ne doit pas confondre
    /// les deux (trouvé dans un retour d'usage réel).
    #[tokio::test]
    async fn client_diagnostics_none_on_no_publish() {
        let (manager, _tmpdir) = make_manager("nodiag", FAKE_LSP_NODIAG_PY).await;
        let session = manager
            .get_or_spawn("nodiag")
            .await
            .expect("spawn ok")
            .expect("should have session");

        let mut client = LspClient::new(session, "file:///workspace".to_string());

        let timeout = tokio::time::timeout(
            Duration::from_secs(5),
            client.diagnostics("file:///workspace/main.rs", "rust", "fn main(){}"),
        )
        .await;

        assert!(
            timeout.is_ok(),
            "diagnostics test must complete within timeout"
        );
        let diags = timeout.unwrap().expect("diagnostics ok");
        assert!(
            diags.is_none(),
            "should return None when nothing was ever published, not Some(vec![])"
        );
    }
}
