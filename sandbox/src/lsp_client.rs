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

    /// `initialize` + `did_open(uri, language_id, text)`, puis collecte la première
    /// notification `textDocument/publishDiagnostics` avec `params.uri == uri`. Rend le
    /// tableau `params.diagnostics` (vide si timeout ou si le serveur n'en publie pas).
    /// Erreur si `initialize`/`didOpen` échouent.
    pub async fn diagnostics(
        &mut self,
        uri: &str,
        language_id: &str,
        text: &str,
    ) -> anyhow::Result<Vec<Value>> {
        let _ = self.initialize().await?;
        self.did_open(uri, language_id, text).await?;

        let deadline = tokio::time::Instant::now() + DIAGNOSTICS_TIMEOUT;

        loop {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(raw)) => {
                    let msg: Value = match serde_json::from_slice(&raw) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::debug!("LSP client: failed to parse diagnostic message: {e}");
                            continue;
                        }
                    };

                    if let (Some(method_value), Some(params)) =
                        (msg.get("method"), msg.get("params"))
                        && matches!(
                            method_value.as_str(),
                            Some("textDocument/publishDiagnostics")
                        )
                        && let Some(params_uri) = params.get("uri").and_then(|u| u.as_str())
                        && let Some(diag) = params
                            .get("diagnostics")
                            .and_then(|d| d.as_array().cloned())
                        && params_uri == uri
                    {
                        return Ok(diag);
                    }
                }
                Ok(None) => {
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-004: LSP process closed while waiting for diagnostics"
                    ));
                }
                Err(_) => {
                    return Ok(Vec::new());
                }
            }
        }
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
    /// references, didOpen→publishDiagnostics).
    /// Utilise le framing LSP (Content-Length) en entrée et en sortie.
    pub const FAKE_LSP_PY: &str = r#"
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

count = 0
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
            # Notification : pas de réponse JSON-RPC. didOpen publie des diagnostics.
            if method == "textDocument/didOpen":
                uri = params.get("textDocument", {}).get("uri", "")
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
            uri = params.get("textDocument", {}).get("uri", "")
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": {
                    "contents": [{"kind": "plaintext", "value": f"hover:{uri}"}],
                    "range": None
                }
            }).encode("utf-8"))
        elif method == "textDocument/definition":
            uri = params.get("textDocument", {}).get("uri", "")
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": [{"uri": uri, "range": {"start": {"line": 0}, "end": {"line": 0}}}]
            }).encode("utf-8"))
        elif method == "textDocument/references":
            uri = params.get("textDocument", {}).get("uri", "")
            write_frame(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "result": [{"uri": uri, "range": {"start": {"line": 0}, "end": {"line": 0}}}]
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
                result["contents"][0]["value"].as_str().unwrap(),
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
        let diags = timeout.unwrap().expect("diagnostics ok");
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

    /// Test 4: client_diagnostics_empty_on_no_publish — script sans
    /// publication de diags → timeout → vec![].
    #[tokio::test]
    async fn client_diagnostics_empty_on_no_publish() {
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
            diags.is_empty(),
            "should return empty vec when no diagnostics published"
        );
    }
}
