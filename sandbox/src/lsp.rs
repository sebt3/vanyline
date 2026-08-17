//! Gestion des process LSP par toolchain : spawn, framing `Content-Length`,
//! multiplexage multi-clients. Un seul process par toolchain, partagé entre
//! l'éditeur (route WS /ws/lsp/:toolchain) et les tools MCP `lsp_*`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// Spec d'une toolchain LSP : `bin` est un chemin absolu dans le volume toolchain
/// monté. Lue depuis `VNL_LSP_TOOLCHAINS` (JSON array).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspToolchain {
    pub name: String,
    pub bin: String,
    pub args: Vec<String>,
}

/// Parse le JSON de `VNL_LSP_TOOLCHAINS` en specs. Erreur si le JSON est invalide
/// ou malformé (`anyhow::Error` avec contexte).
pub fn parse_lsp_toolchains(json: &str) -> anyhow::Result<Vec<LspToolchain>> {
    let specs: Vec<LspToolchain> = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("LSP toolchain parse error: {e}"))?;
    Ok(specs)
}

/// Lit `VNL_LSP_TOOLCHAINS` (env). Env absente → `Ok(vec![])` (aucune toolchain LSP).
pub fn lsp_toolchains_from_env() -> anyhow::Result<Vec<LspToolchain>> {
    let json = std::env::var("VNL_LSP_TOOLCHAINS").unwrap_or_default();
    if json.is_empty() {
        return Ok(vec![]);
    }
    parse_lsp_toolchains(&json)
}

/// Encode un payload JSON-RPC en trame stdio LSP : `Content-Length: N\r\n\r\n` + payload.
pub fn encode_message(payload: &[u8]) -> Vec<u8> {
    let len = payload.len();
    let header = format!("Content-Length: {len}\r\n\r\n");
    let mut buf = Vec::with_capacity(header.len() + len);
    buf.extend_from_slice(header.as_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Décodeur incrémental de trames `Content-Length` sur un flux d'octets.
pub struct FrameReader {
    buf: Vec<u8>,
}

impl Default for FrameReader {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameReader {
    pub fn new() -> Self {
        FrameReader { buf: Vec::new() }
    }

    /// Ajoute un chunk d'octets au buffer interne.
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Rend la première trame complète (`payload` pur), ou `None`.
    pub fn next(&mut self) -> Option<Vec<u8>> {
        // Chercher le terminateur \r\n\r\n
        let pos = self.buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let header_bytes = &self.buf[..pos];
        let body_start = pos + 4;

        // Décoder le header (insensible à la casse)
        let header_str = String::from_utf8_lossy(header_bytes);

        // Chercher content-length: dans le header (insensible à la casse)
        let content_length = header_str
            .lines()
            .filter_map(|line| {
                let lower = line.trim().to_ascii_lowercase();
                if let Some(stripped) = lower.strip_prefix("content-length:") {
                    stripped.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .next()?;

        if body_start + content_length > self.buf.len() {
            return None; // pas assez de données
        }

        let payload = self.buf[body_start..body_start + content_length].to_vec();

        // Supprimer le frame lu du buffer
        self.buf.drain(..body_start + content_length);

        Some(payload)
    }
}

/// Identifiant d'un client abonné à une session LSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub u64);

// ── Session interne ──────────────────────────────────────────────────────────

/// Implémentation interne de la session LSP : process, multiplexage, communication.
struct LspSessionInner {
    cmd_tx: mpsc::Sender<Vec<u8>>,
    /// session_id -> (client who sent the request, original JSON-RPC id)
    pending: Mutex<HashMap<u64, (ClientId, i64)>>,
    subs: Mutex<HashMap<ClientId, mpsc::UnboundedSender<Vec<u8>>>>,
    alive: AtomicBool,
    child: Mutex<Option<Child>>,
    next_client: AtomicU64,
    next_req: AtomicU64,
    _toolchain_name: String,
}

/// Session LSP : possède un process, multiplexe les clients.
pub struct LspSession {
    inner: Arc<LspSessionInner>,
}

impl LspSession {
    /// Spawn le process LSP (`spec.bin` + `spec.args`) avec `cwd = sandbox_root`,
    /// stdio pipé (stdin/stdout), stderr pipé et loggé via `tracing`. Lance les
    /// tâches lectrice (stdout → FrameReader → dispatch) et écrivaine (canal →
    /// stdin encodé). Erreur si le spawn échoue ou si stdin/stdout/stderr sont absents.
    pub async fn spawn(spec: &LspToolchain, sandbox_root: &Path) -> anyhow::Result<Arc<Self>> {
        let mut cmd = Command::new(&spec.bin);
        cmd.args(&spec.args)
            .current_dir(sandbox_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("LSP spawn error for {}: {e}", spec.name))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stdout"))?;
        let stderr_handle = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing stderr"))?;
        // child handle still valid for kill

        let (cmd_tx, mut cmd_rx) = mpsc::channel(64);

        let session = Arc::new(LspSession {
            inner: Arc::new(LspSessionInner {
                cmd_tx,
                pending: Mutex::new(HashMap::new()),
                subs: Mutex::new(HashMap::new()),
                alive: AtomicBool::new(true),
                child: Mutex::new(Some(child)),
                next_client: AtomicU64::new(1),
                next_req: AtomicU64::new(1),
                _toolchain_name: spec.name.clone(),
            }),
        });

        // Écrivaine : canal → stdin encodé
        tokio::spawn(async move {
            let mut writer = stdin;
            while let Some(data) = cmd_rx.recv().await {
                let encoded = encode_message(&data);
                if writer.write_all(&encoded).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
            // Canal fermé → le process voit EOF stdin
        });

        // Lectrice : stdout → FrameReader → dispatch
        let reader_session = Arc::clone(&session);
        tokio::spawn(async move {
            let mut stdout = stdout;
            let mut frame_reader = FrameReader::new();
            let mut buf = [0u8; 8192];

            loop {
                let n = match stdout.read(&mut buf).await {
                    Ok(0) => break, // EOF → process mort
                    Ok(n) => n,
                    Err(_) => break,
                };

                frame_reader.push(&buf[..n]);
                while let Some(payload) = frame_reader.next() {
                    // Trame non-JSON → ignored with warn
                    let msg: Value = match serde_json::from_slice(&payload) {
                        Ok(msg) => msg,
                        Err(_) => {
                            tracing::warn!(
                                toolchain = reader_session.inner._toolchain_name.as_str(),
                                "LSP: non-JSON frame, ignoring"
                            );
                            continue;
                        }
                    };

                    // Trame avec `id` → réponse : router au client d'origine
                    if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
                        let session_id = id as u64;
                        let mut pending = match reader_session.inner.pending.lock() {
                            Ok(g) => g,
                            Err(g) => g.into_inner(),
                        };

                        if let Some((client, orig_id)) = pending.remove(&session_id) {
                            // Restaurer l'original id dans la réponse
                            let mut outbound = msg.clone();
                            outbound["id"] = Value::Number(serde_json::Number::from(orig_id));
                            // Envoi au client : JSON brut (pas de framing — le client n'utilise pas FrameReader)
                            if let Ok(subs) = reader_session.inner.subs.lock()
                                && let Some(tx) = subs.get(&client)
                            {
                                let _ = tx.send(outbound.to_string().into_bytes());
                            }
                        } else {
                            tracing::warn!(
                                session_id,
                                "LSP: response for unknown session_id, ignoring"
                            );
                        }
                    } else {
                        // Notification (pas d'id) : broadcast à tous les abonnés
                        let subs_map = match reader_session.inner.subs.lock() {
                            Ok(g) => g,
                            Err(g) => g.into_inner(),
                        };

                        let dead_clients: Vec<_> = subs_map
                            .iter()
                            .filter(|(_client_id, tx)| {
                                let raw_json = payload.clone();
                                if tx.send(raw_json).is_err() {
                                    return true; // client mort (canal fermé)
                                }
                                false
                            })
                            .map(|(id, _)| *id)
                            .collect();

                        // Nettoyer les clients morts
                        if !dead_clients.is_empty()
                            && let Ok(mut subs) = reader_session.inner.subs.lock()
                        {
                            for client_id in dead_clients {
                                subs.remove(&client_id);
                            }
                        }
                    }
                }
            }

            // EOF stdout → process mort
            reader_session.inner.alive.store(false, Ordering::SeqCst);
            if let Ok(mut subs) = reader_session.inner.subs.lock() {
                subs.clear();
            }
        });

        // Stderr logger : stderr → tracing
        let _stderr_session = Arc::clone(&session);
        tokio::spawn(async move {
            let mut stderr = stderr_handle;
            let mut buf = [0u8; 8192];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let line = String::from_utf8_lossy(&buf[..n]);
                tracing::debug!(
                    toolchain = _stderr_session.inner._toolchain_name.as_str(),
                    "LSP stderr: {}",
                    line.trim()
                );
            }
        });

        Ok(session)
    }

    /// Abonne un client. Rend `(ClientId, UnboundedReceiver<Vec<u8>>)`.
    /// Le receiver reçoit les réponses (id restauré) et notifications serveur.
    /// `None` quand le process meurt (canal fermé par la tâche lectrice).
    pub fn subscribe(&self) -> (ClientId, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let client_id = ClientId(self.inner.next_client.fetch_add(1, Ordering::SeqCst));
        if let Ok(mut subs) = self.inner.subs.lock() {
            subs.insert(client_id, tx);
        }
        (client_id, rx)
    }

    /// Désabonne un client : retire le canal de sortie et les entrées `pending`
    /// de ce client.
    pub fn unsubscribe(&self, client: ClientId) {
        if let Ok(mut subs) = self.inner.subs.lock() {
            subs.remove(&client);
        }
        // Retirer les pending requests de ce client
        let mut pending = match self.inner.pending.lock() {
            Ok(g) => g,
            Err(g) => g.into_inner(),
        };
        pending.retain(|_, (c, _)| *c != client);
    }

    /// Envoie un message JSON-RPC client → process.
    ///
    /// Erreurs :
    /// - JSON invalide → `VNL-SBX-LSP-001`
    /// - `id` présent mais non entier → `VNL-SBX-LSP-002`
    /// - process fermé (canal `cmd_tx` fermé) → `VNL-SBX-LSP-003`
    pub async fn send(&self, client: ClientId, payload: Vec<u8>) -> anyhow::Result<()> {
        // Valider JSON
        let msg: Value = serde_json::from_slice(&payload)
            .map_err(|_| anyhow::anyhow!("VNL-SBX-LSP-001: invalid JSON payload"))?;

        // Vérifier l'`id` : doit être un entier si présent
        if let Some(id_val) = msg.get("id")
            && !id_val.is_number()
        {
            return Err(anyhow::anyhow!(
                "VNL-SBX-LSP-002: JSON-RPC id must be integer, got string"
            ));
        }

        if msg.get("id").is_some() {
            // Requête : réécrire l'id en session id et mémoriser le mapping
            let session_req_id = self.inner.next_req.fetch_add(1, Ordering::SeqCst);
            let orig_id = msg["id"].as_i64().unwrap_or(0);

            // Réécrire l'id dans le payload pour le processus
            let mut rewritten = msg.clone();
            rewritten["id"] = Value::Number(serde_json::Number::from(session_req_id));
            let rewritten_payload = rewritten.to_string().into_bytes();

            // Mémoriser le mapping session_id -> (client, orig_id)
            if let Ok(mut pending) = self.inner.pending.lock() {
                pending.insert(session_req_id, (client, orig_id));
            }

            // Envoyer au process
            self.inner
                .cmd_tx
                .send(rewritten_payload)
                .await
                .map_err(|_| anyhow::anyhow!("VNL-SBX-LSP-003: LSP process is dead"))
        } else {
            // Notification : transmettre tel quel
            self.inner
                .cmd_tx
                .send(payload)
                .await
                .map_err(|_| anyhow::anyhow!("VNL-SBX-LSP-003: LSP process is dead"))
        }
    }

    /// `true` tant que le process n'a pas rendu EOF stdout.
    pub fn is_alive(&self) -> bool {
        self.inner.alive.load(Ordering::SeqCst)
    }
}

impl Drop for LspSessionInner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            drop(child.kill());
        }
    }
}

// ── Manager ──────────────────────────────────────────────────────────────────

/// Gestionnaire des sessions LSP d'une sandbox : une session par toolchain.
pub struct LspManager {
    specs: HashMap<String, LspToolchain>,
    sandbox_root: PathBuf,
    sessions: Mutex<HashMap<String, Arc<LspSession>>>,
}

impl Clone for LspManager {
    fn clone(&self) -> Self {
        LspManager {
            specs: self.specs.clone(),
            sandbox_root: self.sandbox_root.clone(),
            sessions: Mutex::new(
                self.sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ),
        }
    }
}

impl Default for LspManager {
    /// Aucune toolchain LSP, sandbox_root `/workspace`.
    fn default() -> Self {
        LspManager::new(vec![], PathBuf::from("/workspace"))
    }
}

impl LspManager {
    pub fn new(specs: Vec<LspToolchain>, sandbox_root: PathBuf) -> Self {
        let mut map = HashMap::new();
        for spec in specs {
            map.insert(spec.name.clone(), spec);
        }
        LspManager {
            specs: map,
            sandbox_root,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Construit depuis `lsp_toolchains_from_env()` et `sandbox_root`.
    pub fn from_env(sandbox_root: PathBuf) -> anyhow::Result<Self> {
        let specs = lsp_toolchains_from_env()?;
        Ok(LspManager::new(specs, sandbox_root))
    }

    /// `true` si une toolchain LSP de ce nom est configurée.
    pub fn has(&self, toolchain: &str) -> bool {
        self.specs.contains_key(toolchain)
    }

    /// Rend la session existante si vivante, sinon spawn une neuve.
    /// `Ok(None)` si aucune toolchain de ce nom n'est configurée.
    pub async fn get_or_spawn(&self, toolchain: &str) -> anyhow::Result<Option<Arc<LspSession>>> {
        // Vérifier si la toolchain est configurée
        let spec = if let Some(spec) = self.specs.get(toolchain) {
            spec.clone()
        } else {
            return Ok(None);
        };

        // Vérifier s'il existe une session vivante
        {
            if let Some(session) = self
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(toolchain)
                && session.is_alive()
            {
                return Ok(Some(Arc::clone(session)));
            }
        }

        // Session morte ou inexistante : spawn une nouvelle
        let new_session = LspSession::spawn(&spec, &self.sandbox_root).await?;

        // Réinsérer (remplace une éventuelle ancienne entrée morte).
        // Si une autre tâche a déjà remplacé par sa session, accepter la sienne.
        {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = sessions.get(toolchain)
                && existing.is_alive()
                && !Arc::ptr_eq(existing, &new_session)
            {
                // Une autre tâche a spawné — l'utiliser à la place
                return Ok(Some(Arc::clone(existing)));
            }
            // Notre session est la plus récente, l'insérer
            sessions.insert(toolchain.to_string(), Arc::clone(&new_session));
        }

        Ok(Some(new_session))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::time::Duration;

    /// Script Python factice : echo — lit des trames stdin et répond.
    const FAKE_LSP_PY: &str = r#"
import sys

def read_message():
    header = b""
    while True:
        ch = sys.stdin.buffer.read(1)
        if not ch:
            return None
        header += ch
        if header.endswith(b"\r\n\r\n"):
            break
    text = header.decode("ascii", errors="replace")
    lines = text.strip().split("\r\n")
    length = 0
    for line in lines:
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1].strip())
    if length <= 0:
        return b""
    payload = b""
    while len(payload) < length:
        chunk = sys.stdin.buffer.read(length - len(payload))
        if not chunk:
            break
        payload += chunk
    return payload

while True:
    msg = read_message()
    if not msg:
        break
    try:
        import json
        data = json.loads(msg)
        if "id" in data:
            reply = {"jsonrpc": "2.0", "id": data["id"], "result": f"pong:{data['id']}"}
        else:
            reply = {"jsonrpc": "2.0", "method": "fake/notify", "params": {}}
        out = json.dumps(reply).encode("utf-8")
        header = f"Content-Length: {len(out)}\r\n\r\n".encode("ascii")
        sys.stdout.buffer.write(header)
        sys.stdout.buffer.write(out)
        sys.stdout.buffer.flush()
    except Exception:
        pass
"#;

    /// Script Python factice : exit immédiat.
    const FAKE_LSP_EXIT_PY: &str = "import sys; sys.exit(0)\n";

    /// Crée une toolchain LSP pointant vers un script Python factice.
    async fn make_fake_toolchain(name: &str, script: &str) -> (LspToolchain, tempfile::TempDir) {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let script_path = tmpdir.path().join(format!("fake_lsp_{name}.py"));
        std::fs::write(&script_path, script).unwrap();
        let toolchain = LspToolchain {
            name: name.to_string(),
            bin: "python3".to_string(),
            args: vec![script_path.to_string_lossy().to_string()],
        };
        (toolchain, tmpdir)
    }

    /// Timeout de réception sur le channel d'un client.
    async fn recv_timeout<T>(rx: &mut mpsc::UnboundedReceiver<T>, timeout: Duration) -> Option<T> {
        tokio::time::timeout(timeout, rx.recv())
            .await
            .ok()
            .flatten()
    }

    // ── Tests unitaires (encode, FrameReader, parse) ───────────────────────

    /// Test 1: encode_has_content_length_header
    #[test]
    fn encode_has_content_length_header() {
        let payload = b"{\"jsonrpc\":\"2.0\"}";
        let encoded = encode_message(payload);
        let prefix = format!("Content-Length: {}\r\n\r\n", payload.len());
        assert!(
            encoded.starts_with(prefix.as_bytes()),
            "expected Content-Length header"
        );
        assert_eq!(
            &encoded[prefix.len()..],
            payload,
            "payload must end with the raw payload"
        );
    }

    /// Test 2: frame_reader_reassembles_split_chunks
    #[test]
    fn frame_reader_reassembles_split_chunks() {
        let payload = b"{\"jsonrpc\":\"2.0\"}";
        let encoded = encode_message(payload);

        let mut reader = FrameReader::new();
        // Découper en chunks de 3 octets
        let chunks: Vec<Vec<u8>> = encoded.chunks(3).map(|c| c.to_vec()).collect();
        // Pousser un par un
        for chunk in &chunks {
            reader.push(&chunk);
        }

        // next() doit rendre le payload
        let result = reader.next().expect("should yield the full payload");
        assert_eq!(result, payload, "decoded payload must match original");

        // next() supplémentaire → None
        assert!(
            reader.next().is_none(),
            "should return None after full frame decoded"
        );
    }

    /// Test 3: frame_reader_reads_two_messages
    #[test]
    fn frame_reader_reads_two_messages() {
        let p1 = encode_message(b"hello");
        let p2 = encode_message(b"world");
        let mut reader = FrameReader::new();
        reader.push(&p1);
        reader.push(&p2);

        assert_eq!(reader.next().expect("msg1"), b"hello");
        assert_eq!(reader.next().expect("msg2"), b"world");
        assert!(
            reader.next().is_none(),
            "should return None after both frames"
        );
    }

    /// Test 4: parse_lsp_toolchains_valid_json
    #[test]
    fn parse_lsp_toolchains_valid_json() {
        let json = r#"[{"name":"rust","bin":"/toolchains/rust/bin/rust-analyzer","args":[]}]"#;
        let specs = parse_lsp_toolchains(json).expect("should parse valid JSON");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "rust");
        assert_eq!(specs[0].bin, "/toolchains/rust/bin/rust-analyzer");
        assert!(specs[0].args.is_empty());
    }

    /// Test 5: parse_lsp_toolchains_empty_array
    #[test]
    fn parse_lsp_toolchains_empty_array() {
        let specs = parse_lsp_toolchains("[]").expect("empty array should be valid");
        assert!(specs.is_empty(), "empty array should yield zero specs");
    }

    /// Test 6: parse_lsp_toolchains_invalid_json_errors
    #[test]
    fn parse_lsp_toolchains_invalid_json_errors() {
        let result = parse_lsp_toolchains("not json");
        assert!(result.is_err(), "invalid JSON should return Err");
    }

    // ── Tests manager ──────────────────────────────────────────────────────

    /// Test 7: manager_unknown_toolchain_returns_none
    #[tokio::test]
    async fn manager_unknown_toolchain_returns_none() {
        let root = PathBuf::from("/tmp/test-root");
        let manager = LspManager::new(vec![], root);
        let result = manager
            .get_or_spawn("nope")
            .await
            .expect("get_or_spawn should succeed");
        assert!(result.is_none(), "unknown toolchain should return None");
    }

    /// Test 8: manager_reuses_alive_session
    #[tokio::test]
    async fn manager_reuses_alive_session() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let manager = LspManager::new(vec![spec], root);

        let s1 = manager
            .get_or_spawn("fake")
            .await
            .expect("first spawn should succeed");
        assert!(s1.is_some(), "should return Some");
        let s1 = s1.unwrap();
        assert!(s1.is_alive(), "session should be alive");

        // Deuxième appel → same session (réutilisée)
        let s2 = manager
            .get_or_spawn("fake")
            .await
            .expect("reuse should succeed");
        assert!(s2.is_some(), "should return Some on reuse");
        assert!(
            Arc::ptr_eq(&s1, &s2.unwrap()),
            "should return the same session (same Arc)"
        );
        drop(tmpdir);
    }

    // ── Tests session ──────────────────────────────────────────────────────

    /// Test 9: session_request_response
    #[tokio::test]
    async fn session_request_response() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let (client_id, mut rx) = session.subscribe();

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping",
            "params": {}
        });
        let payload = request.to_string().into_bytes();
        session
            .send(client_id, payload)
            .await
            .expect("send should succeed");

        // Attendre la réponse (timeout 5s)
        let resp = recv_timeout(&mut rx, Duration::from_secs(5)).await;
        assert!(resp.is_some(), "should receive response within timeout");

        let raw = resp.unwrap();
        let resp = serde_json::from_slice::<Value>(&raw).expect("response should be valid JSON");
        assert_eq!(
            resp["id"].as_i64().unwrap(),
            1,
            "id should be restored to original"
        );
        // resultat = pong:<session_id> (l'id réécrit par la session que le fake LSP echo)
        assert!(
            resp["result"].as_str().unwrap().starts_with("pong:"),
            "result must be pong:<session_id>"
        );

        drop(tmpdir);
    }

    /// Test 10: session_routes_same_id_to_each_client
    #[tokio::test]
    async fn session_routes_same_id_to_each_client() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let (client_a, mut rx_a) = session.subscribe();
        let (client_b, mut rx_b) = session.subscribe();

        // A envoie id:1
        let req_a = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}})
            .to_string()
            .into_bytes();
        session.send(client_a, req_a).await.expect("A send ok");

        // B envoie id:1
        let req_b = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}})
            .to_string()
            .into_bytes();
        session.send(client_b, req_b).await.expect("B send ok");

        // Chaque client reçoit sa réponse avec id == 1
        let resp_a = recv_timeout(&mut rx_a, Duration::from_secs(5)).await;
        let resp_b = recv_timeout(&mut rx_b, Duration::from_secs(5)).await;

        assert!(resp_a.is_some(), "A should receive response");
        assert!(resp_b.is_some(), "B should receive response");

        let r_a = serde_json::from_slice::<Value>(&resp_a.unwrap()).unwrap();
        let r_b = serde_json::from_slice::<Value>(&resp_b.unwrap()).unwrap();

        assert_eq!(
            r_a["id"].as_i64().unwrap(),
            1,
            "A's response id should be 1"
        );
        assert_eq!(
            r_b["id"].as_i64().unwrap(),
            1,
            "B's response id should be 1"
        );

        // result DIFFÈRE → ids de session différents → pas de cross-routing
        let result_a = r_a["result"].as_str().unwrap();
        let result_b = r_b["result"].as_str().unwrap();
        assert_ne!(
            result_a, result_b,
            "results must differ for different clients"
        );

        drop(tmpdir);
    }

    /// Test 11: session_broadcasts_notifications
    #[tokio::test]
    async fn session_broadcasts_notifications() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();

        let (client_a, mut rx_a) = session.subscribe();
        let (_, mut rx_b) = session.subscribe();

        // A envoie une notification (pas d'id)
        let notif = serde_json::json!(
            {"jsonrpc":"2.0","method":"fake/notify","params":{}}
        )
        .to_string()
        .into_bytes();
        session
            .send(client_a, notif)
            .await
            .expect("send notification ok");

        // B ET A reçoivent la notification
        let notif_b = recv_timeout(&mut rx_b, Duration::from_secs(5))
            .await
            .expect("B must receive notification");
        let notif_a = recv_timeout(&mut rx_a, Duration::from_secs(5))
            .await
            .expect("A must also receive its own notification");

        let n_b = serde_json::from_slice::<Value>(&notif_b).unwrap();
        let n_a = serde_json::from_slice::<Value>(&notif_a).unwrap();

        assert_eq!(
            n_b["method"].as_str().unwrap(),
            "fake/notify",
            "B: method should match"
        );
        assert_eq!(
            n_a["method"].as_str().unwrap(),
            "fake/notify",
            "A: method should match"
        );

        drop(tmpdir);
    }

    /// Test 12: send_rejects_invalid_json
    #[tokio::test]
    async fn send_rejects_invalid_json() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();
        let (client, _) = session.subscribe();

        let result = session.send(client, b"not json".to_vec()).await;
        assert!(result.is_err(), "invalid JSON should return Err");
        assert!(
            result.unwrap_err().to_string().contains("VNL-SBX-LSP-001"),
            "error message must contain VNL-SBX-LSP-001"
        );

        drop(tmpdir);
    }

    /// Test 13: send_rejects_non_integer_id
    #[tokio::test]
    async fn send_rejects_non_integer_id() {
        let (spec, tmpdir) = make_fake_toolchain("fake", FAKE_LSP_PY).await;
        let root = tmpdir.path().to_path_buf();
        let session = LspSession::spawn(&spec, &root).await.unwrap();
        let (client, _) = session.subscribe();

        let payload = serde_json::json!({"jsonrpc":"2.0","id":"str","method":"ping","params":{}})
            .to_string()
            .into_bytes();
        let result = session.send(client, payload).await;
        assert!(result.is_err(), "non-integer id should return Err");
        assert!(
            result.unwrap_err().to_string().contains("VNL-SBX-LSP-002"),
            "error must contain VNL-SBX-LSP-002"
        );

        drop(tmpdir);
    }

    /// Test 14: manager_respawns_dead_session
    #[tokio::test]
    async fn manager_respawns_dead_session() {
        let (spec, tmpdir) = make_fake_toolchain("die", FAKE_LSP_EXIT_PY).await;
        let root = tmpdir.path().to_path_buf();
        let manager = LspManager::new(vec![spec], root);

        // Spawn la session morte
        let s1 = manager
            .get_or_spawn("die")
            .await
            .expect("first spawn ok")
            .unwrap();

        // Attendre que le process meure (poll 10ms, timeout 5s)
        for _ in 0..500 {
            if !s1.is_alive() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!s1.is_alive(), "process should be dead by now");

        // get_or_spawn doit respawn
        let s2 = manager
            .get_or_spawn("die")
            .await
            .expect("respawn ok")
            .unwrap();
        assert!(
            !Arc::ptr_eq(&s1, &s2),
            "should return a new session (different Arc)"
        );

        drop(tmpdir);
    }
}
