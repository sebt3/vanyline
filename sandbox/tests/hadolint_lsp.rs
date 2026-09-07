#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Tests d'intégration bout-en-bouche du binaire `vnl-hadolint-lsp` (motif
//! `tests/maint.rs`) : le wrapper est spawné via `CARGO_BIN_EXE_vnl-hadolint-lsp`
//! et un FAUX `hadolint` (script shell) est posé EN TÊTE du `PATH` du fils — un
//! hadolint réellement installé sur la machine de test n'est JAMAIS utilisé.
//! Framing `Content-Length` via `vanyline_sandbox::lsp::FrameReader` (pub,
//! motif des tests de `lsp.rs`).

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};

use vanyline_sandbox::lsp::FrameReader;

/// Nom exact des argv que le faux hadolint doit voir — le contrat de sécurité
/// du design : argv littéraux uniquement, jamais d'URI/chemin en argv, jamais
/// de shell côté wrapper.
const FAKE_EXPECTED_ARGS: &str = "--format\njson\n--no-color\n-\n";

/// Environnements de journalisation posés sur le process wrapper — hérités par
/// le faux `hadolint` qu'il spawn (le fils écrit ses argv et le stdin reçu).
const FAKE_ARGS_ENV: &str = "VNL_FAKE_HADOLINT_ARGS";
const FAKE_STDIN_ENV: &str = "VNL_FAKE_HADOLINT_STDIN";

/// Corps du faux `hadolint` : journalise argv et stdin dans les fichiers pointés
/// par l'environnement, puis renvoie un JSON déterministe piloté par le CONTENU
/// reçu : contient `BAD` ⟹ un diagnostic DL3008 (warning, ligne 1 colonne 1),
/// sinon tableau vide. `slow` = dort 300 ms avant d'émettre (test des résultats
/// périmés).
fn fake_hadolint_script(slow: bool) -> String {
    let slow_line = if slow { "sleep 0.3\n" } else { "" };
    format!(
        r#"#!/bin/sh
printf '%s\n' "$@" > "$VNL_FAKE_HADOLINT_ARGS"
cat > "$VNL_FAKE_HADOLINT_STDIN"
{slow_line}if grep -q BAD "$VNL_FAKE_HADOLINT_STDIN"; then
  printf '[{{"file":"<stdin>","line":1,"column":1,"level":"warning","code":"DL3008","message":"Pin versions in FROM"}}]'
  exit 1
fi
printf '[]'
"#
    )
}

/// Écrase la racine `tmp/bin` avec le faux `hadolint` (exécutable 755).
fn write_fake_hadolint(root: &Path, slow: bool) {
    let dir = root.join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("hadolint");
    std::fs::write(&script, fake_hadolint_script(slow)).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Le process wrapper + ses pipes ; les trames stdout sont décodées avec le
/// FrameReader de la lib (motif des tests de `lsp.rs`).
struct Harness {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    reader: FrameReader,
    stderr_path: std::path::PathBuf,
}

impl Harness {
    /// Spawn du binaire : `path` = contenu EXACT du `PATH` du fils, posé par
    /// les helpers (faux hadolint EN TÊTE, ou tmpdir vide pour la dégradation
    /// — jamais de PATH implicite : un hadolint système ne doit jamais être
    /// atteint).
    fn spawn(t: &TempDir, path: &str) -> Harness {
        let stderr_path = t.path().join("wrapper-stderr.log");
        let stderr_file = std::fs::File::create(&stderr_path).unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_vnl-hadolint-lsp"));
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr_file))
            .env("PATH", path)
            .env(FAKE_ARGS_ENV, t.path().join("fake-hadolint-args.log"))
            .env(FAKE_STDIN_ENV, t.path().join("fake-hadolint-stdin.log"));
        let mut child = cmd.spawn().unwrap();
        Harness {
            stdin: child.stdin.take().unwrap(),
            stdout: child.stdout.take().unwrap(),
            child,
            reader: FrameReader::new(),
            stderr_path,
        }
    }

    /// Enveloppe une requête/notification en trame et l'écrit sur le stdin du
    /// fils.
    async fn send(&mut self, msg: &Value) {
        let frame = vanyline_sandbox::lsp::encode_message(&serde_json::to_vec(msg).unwrap());
        self.stdin.write_all(&frame).await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn request(&mut self, id: i64, method: &str, params: Value) {
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
    }

    async fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await;
    }

    async fn did_open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "dockerfile", "version": 1, "text": text}}),
        )
        .await;
    }

    /// `didChange` full-sync (sans `range`) : motif du client MCP — le wrapper
    /// doit le tolérer aux côtés de l'incrémental du navigateur.
    async fn did_change_full(&mut self, uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}]
            }),
        )
        .await;
    }

    async fn did_change_incremental(
        &mut self,
        uri: &str,
        version: i64,
        start: (i64, i64),
        end: (i64, i64),
        text: &str,
    ) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{
                    "range": {
                        "start": {"line": start.0, "character": start.1},
                        "end": {"line": end.0, "character": end.1}
                    },
                    "text": text
                }]
            }),
        )
        .await;
    }

    async fn did_save(&mut self, uri: &str) {
        self.notify(
            "textDocument/didSave",
            json!({"textDocument": {"uri": uri}}),
        )
        .await;
    }

    async fn did_close(&mut self, uri: &str) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        )
        .await;
    }

    /// Trame JSON suivante sur stdout, bornée par `timeout` (`None` = timeout
    /// ou EOF).
    async fn next_frame(&mut self, timeout: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut chunk = [0_u8; 8192];
        loop {
            while let Some(frame) = self.reader.next_frame() {
                if let Ok(msg) = serde_json::from_slice::<Value>(&frame) {
                    return Some(msg);
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match tokio::time::timeout(remaining, self.stdout.read(&mut chunk)).await {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return None,
                Ok(Ok(n)) => self.reader.push(&chunk[..n]),
            }
        }
    }

    /// Prochaine notification `publishDiagnostics` (les réponses aux requêtes
    /// sont consommées et ignorées) ; `None` = timeout.
    async fn next_publish(&mut self, timeout: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let frame = self.next_frame(remaining).await?;
            if frame["method"].as_str() == Some("textDocument/publishDiagnostics") {
                return Some(frame["params"].clone());
            }
        }
    }

    /// `Some` si une publication est arrivée dans la fenêtre.
    async fn no_publish(&mut self, window: Duration) -> Option<Value> {
        self.next_publish(window).await
    }

    async fn expect_publish(&mut self, timeout: Duration) -> Value {
        self.next_publish(timeout)
            .await
            .expect("timeout en attente d'une publication")
    }

    /// `shutdown` + `exit`, stdin fermé : le process doit finir code 0.
    async fn shutdown_and_exit(mut self) {
        self.request(1, "shutdown", Value::Null).await;
        let _resp = self
            .next_frame(Duration::from_secs(5))
            .await
            .expect("pas de réponse à shutdown");
        self.notify("exit", json!({})).await;
        drop(self.stdin);
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .expect("le process ne termine pas après exit")
            .unwrap();
        assert!(status.success(), "code de sortie non nul : {status}");
    }
}

/// PATH standard + `tmp/bin` en tête (faux hadolint prioritaire, jamais un
/// hadolint système).
fn path_with_fake(t: &TempDir) -> String {
    let fake = t.path().join("bin").to_string_lossy().into_owned();
    match std::env::var("PATH") {
        Ok(system) if !system.is_empty() => format!("{fake}:{system}"),
        _ => fake,
    }
}

/// `tmp/bin` en tête d'un `PATH` réduit (le répertoire est créé vide — ex :
/// test de spawn en échec).
fn path_fake_only(t: &TempDir) -> String {
    std::fs::create_dir_all(t.path().join("bin")).unwrap();
    t.path().join("bin").to_string_lossy().into_owned()
}

fn uri() -> String {
    "file:///w/Dockerfile".to_string()
}

// ===== 1. initialize_handshake =====
#[tokio::test]
async fn initialize_handshake() {
    let t = TempDir::new().unwrap();
    write_fake_hadolint(t.path(), false);
    let mut h = Harness::spawn(&t, &path_with_fake(&t));

    h.request(1, "initialize", json!({"capabilities": {}}))
        .await;
    let resp = h
        .next_frame(Duration::from_secs(5))
        .await
        .expect("pas de réponse à initialize");
    assert_eq!(resp["id"], json!(1));
    assert_eq!(
        resp["result"]["capabilities"]["textDocumentSync"]["change"],
        json!(2),
        "capacité textDocumentSync.change = 2 (incrémental)"
    );
    assert_eq!(
        resp["result"]["serverInfo"]["name"],
        json!("vnl-hadolint-lsp")
    );
    assert!(
        resp["result"]["serverInfo"]["version"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "serverInfo.version = CARGO_PKG_VERSION non vide"
    );

    h.request(2, "shutdown", Value::Null).await;
    let resp = h
        .next_frame(Duration::from_secs(5))
        .await
        .expect("pas de réponse à shutdown");
    assert_eq!(resp["id"], json!(2));
    assert!(
        resp.get("result").is_some_and(Value::is_null),
        "résultat shutdown = null : {resp}"
    );
    assert!(
        resp.get("error").is_none(),
        "shutdown ne doit pas être une erreur"
    );

    h.notify("exit", json!({})).await;
    drop(h.stdin);
    let status = tokio::time::timeout(Duration::from_secs(5), h.child.wait())
        .await
        .expect("le process ne termine pas après exit")
        .unwrap();
    assert!(status.success(), "exit doit finir code 0 : {status}");
}

// ===== 2. did_open_publishes_converted =====
#[tokio::test]
async fn did_open_publishes_converted() {
    let t = TempDir::new().unwrap();
    write_fake_hadolint(t.path(), false);
    let mut h = Harness::spawn(&t, &path_with_fake(&t));

    // Contenu dont la 1re ligne fait 19 unités UTF-16 → end attendu (0, 19).
    h.did_open(&uri(), "FROM BAD:tag latest\nRUN echo x\n")
        .await;
    let p = h.expect_publish(Duration::from_secs(3)).await;
    assert_eq!(p["uri"], json!("file:///w/Dockerfile"), "uri publiée");
    assert_eq!(p["version"], json!(1), "version du doc au moment du rideau");
    let diags = p["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1, "le DL du faux : {p}");
    assert_eq!(diags[0]["source"], json!("hadolint"));
    assert_eq!(diags[0]["code"], json!("DL3008"));
    assert_eq!(diags[0]["severity"], json!(2));
    assert_eq!(
        diags[0]["range"]["start"],
        json!({"line": 0, "character": 0}),
        "1-based (1,1) → 0-based (0,0)"
    );
    assert_eq!(
        diags[0]["range"]["end"],
        json!({"line": 0, "character": 19}),
        "fin de range = fin de ligne 1"
    );

    // Le faux a reçu sur stdin le CONTENU du buffer, et des argv strictement
    // littéraux (aucun argument-chemin — contrat de sécurité).
    let got_stdin = std::fs::read_to_string(t.path().join("fake-hadolint-stdin.log")).unwrap();
    assert_eq!(got_stdin, "FROM BAD:tag latest\nRUN echo x\n");
    let got_args = std::fs::read_to_string(t.path().join("fake-hadolint-args.log")).unwrap();
    assert_eq!(got_args, FAKE_EXPECTED_ARGS, "argv littéraux uniquement");

    h.shutdown_and_exit().await;
}

// ===== 3. clean_content_publishes_empty =====
#[tokio::test]
async fn clean_content_publishes_empty() {
    let t = TempDir::new().unwrap();
    write_fake_hadolint(t.path(), false);
    let mut h = Harness::spawn(&t, &path_with_fake(&t));

    h.did_open(&uri(), "FROM pinned:1.27\n").await;
    let p = h.expect_publish(Duration::from_secs(3)).await;
    assert_eq!(p["uri"], json!("file:///w/Dockerfile"));
    assert_eq!(
        p["diagnostics"].as_array().unwrap().len(),
        0,
        "contenu propre ⟹ publication [] (jamais de silence) : {p}"
    );

    h.shutdown_and_exit().await;
}

// ===== 4. did_change_debounced =====
#[tokio::test]
async fn did_change_debounced() {
    let t = TempDir::new().unwrap();
    write_fake_hadolint(t.path(), false);
    let mut h = Harness::spawn(&t, &path_with_fake(&t));

    // Publication initiale (didOpen propre) consommée d'abord : la fenêtre de
    // silence qui suit ne porte QUE sur le didChange.
    h.did_open(&uri(), "FROM ubuntu:latest\n").await;
    let p = h.expect_publish(Duration::from_secs(3)).await;
    assert!(p["diagnostics"].as_array().unwrap().is_empty());

    // Édit incrémental (motif navigateur) introduisant BAD : « latest »
    // (caractères 12..18 de la ligne 0) → « BAD:tag ».
    let t0 = Instant::now();
    h.did_change_incremental(&uri(), 2, (0, 12), (0, 18), "BAD:tag")
        .await;
    assert!(
        h.no_publish(Duration::from_millis(300)).await.is_none(),
        "aucune publication dans les ~300 ms qui suivent le didChange"
    );
    let p = h.expect_publish(Duration::from_secs(3)).await;
    let elapsed = t0.elapsed();
    assert!(
        elapsed >= Duration::from_millis(400),
        "le debounce de 500 ms n'a pas été respecté : {elapsed:?}"
    );
    assert!(
        elapsed <= Duration::from_secs(3),
        "publication trop tardive : {elapsed:?}"
    );
    assert_eq!(p["uri"], json!("file:///w/Dockerfile"));
    let diags = p["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1, "le DL post-édit : {p}");
    assert_eq!(diags[0]["code"], json!("DL3008"));
    assert_eq!(diags[0]["source"], json!("hadolint"));
    assert_eq!(diags[0]["severity"], json!(2));

    // UNE SEULE publication finale : rien après (pas de re-déclenchement).
    assert!(
        h.no_publish(Duration::from_secs(1)).await.is_none(),
        "une seule publication finale attendue"
    );
    // Le buffer passé au faux est le contenu POST-édit.
    let got_stdin = std::fs::read_to_string(t.path().join("fake-hadolint-stdin.log")).unwrap();
    assert_eq!(got_stdin, "FROM ubuntu:BAD:tag\n");

    h.shutdown_and_exit().await;
}

// ===== 5. did_save_immediate =====
#[tokio::test]
async fn did_save_immediate() {
    let t = TempDir::new().unwrap();
    write_fake_hadolint(t.path(), false);
    let mut h = Harness::spawn(&t, &path_with_fake(&t));

    // Publication propre du didOpen consommée d'abord.
    h.did_open(&uri(), "FROM x\n").await;
    let p = h.expect_publish(Duration::from_secs(3)).await;
    assert!(p["diagnostics"].as_array().unwrap().is_empty());

    // didChange (BAD introduit, debounce armé) PUIS didSave : la publication
    // du save doit arriver nettement avant la fin du debounce.
    let t0 = Instant::now();
    h.did_change_full(&uri(), 2, "FROM BAD\n").await;
    h.did_save(&uri()).await;
    let p = h
        .next_publish(Duration::from_millis(400))
        .await
        .expect("le didSave doit publier immédiatement (< 400 ms)");
    assert!(t0.elapsed() < Duration::from_millis(400));
    let diags = p["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1, "le DL du contenu courant : {p}");
    assert_eq!(diags[0]["code"], json!("DL3008"));
    assert_eq!(diags[0]["source"], json!("hadolint"));

    // Le debounce en sommeil s'est disqualifié : plus aucune publication
    // (fenêtre large, au-delà des 500 ms du debounce).
    assert!(
        h.no_publish(Duration::from_secs(1)).await.is_none(),
        "le debounce disqualifié ne doit rien publier de plus"
    );

    h.shutdown_and_exit().await;
}

// ===== 6. stale_result_discarded =====
#[tokio::test]
async fn stale_result_discarded() {
    let t = TempDir::new().unwrap();
    // Faux SLOW : dort 300 ms avant d'émettre — le lint du didOpen (v1, BAD)
    // est encore en vol quand le didChange (v2, propre) arrive.
    write_fake_hadolint(t.path(), true);
    let mut h = Harness::spawn(&t, &path_with_fake(&t));

    h.did_open(&uri(), "FROM BAD\n").await;
    h.did_change_full(&uri(), 2, "FROM good\n").await;

    // La publication FINALE (dernière trame) est [] : le résultat v1 (avec le
    // DL) n'est JAMAIS publié — le buffer courant est sans BAD.
    let p = h
        .next_publish(Duration::from_secs(5))
        .await
        .expect("le lint debouncé du contenu courant doit publier");
    assert_eq!(p["uri"], json!("file:///w/Dockerfile"));
    assert_eq!(
        p["diagnostics"].as_array().unwrap().len(),
        0,
        "le résultat périmé (v1 avec DL) ne doit jamais être publié : {p}"
    );
    let extra = h.no_publish(Duration::from_secs(1)).await;
    assert!(
        extra.is_none(),
        "rien après la publication du contenu courant : {extra:?}"
    );

    h.shutdown_and_exit().await;
}

// ===== 7. did_close_publishes_empty =====
#[tokio::test]
async fn did_close_publishes_empty() {
    let t = TempDir::new().unwrap();
    write_fake_hadolint(t.path(), false);
    let mut h = Harness::spawn(&t, &path_with_fake(&t));

    h.did_open(&uri(), "FROM BAD\n").await;
    let p = h.expect_publish(Duration::from_secs(3)).await;
    assert_eq!(p["diagnostics"].as_array().unwrap().len(), 1);

    // didClose ⟹ la part du wrapper est retirée de la fusion session : [].
    h.did_close(&uri()).await;
    let p = h.expect_publish(Duration::from_secs(3)).await;
    assert_eq!(p["uri"], json!("file:///w/Dockerfile"));
    assert_eq!(p["version"], json!(1));
    assert_eq!(
        p["diagnostics"].as_array().unwrap().len(),
        0,
        "didClose publie [] pour l'URI : {p}"
    );
    assert!(
        h.no_publish(Duration::from_millis(800)).await.is_none(),
        "plus rien après la clôture"
    );

    h.shutdown_and_exit().await;
}

// ===== 8. unknown_request_rejected_not_hung =====
#[tokio::test]
async fn unknown_request_rejected_not_hung() {
    let t = TempDir::new().unwrap();
    write_fake_hadolint(t.path(), false);
    let mut h = Harness::spawn(&t, &path_with_fake(&t));

    h.request(
        7,
        "textDocument/hover",
        json!({"textDocument": {"uri": "file:///w/Dockerfile", "position": {"line": 0, "character": 0}}}),
    )
    .await;
    let resp = h
        .next_frame(Duration::from_secs(5))
        .await
        .expect("une requête inconnue doit recevoir une réponse, jamais pendre");
    assert_eq!(resp["id"], json!(7));
    assert_eq!(
        resp["error"]["code"].as_i64(),
        Some(-32601),
        "Method not found : {resp}"
    );

    // Le serveur vit : une requête valide suivante répond.
    h.request(8, "initialize", json!({"capabilities": {}}))
        .await;
    let resp = h
        .next_frame(Duration::from_secs(5))
        .await
        .expect("le serveur doit rester vivant après un -32601");
    assert_eq!(resp["id"], json!(8));
    assert!(resp.get("result").is_some());

    h.shutdown_and_exit().await;
}

// ===== 9. spawn_failure_degrades_empty =====
#[tokio::test]
async fn spawn_failure_degrades_empty() {
    let t = TempDir::new().unwrap();
    // PATH réduit à un tmpdir VIDE : hadolint introuvable ⟹ spawn en échec.
    // Dégradation silencieuse du design : contribution [], message sur stderr,
    // process vivant — jamais de panic ni de faux-positifs périmés.
    let mut h = Harness::spawn(&t, &path_fake_only(&t));

    h.did_open(&uri(), "FROM BAD\n").await;
    let p = h.expect_publish(Duration::from_secs(3)).await;
    assert_eq!(p["uri"], json!("file:///w/Dockerfile"));
    assert_eq!(
        p["diagnostics"].as_array().unwrap().len(),
        0,
        "spawn en échec ⟹ [] : {p}"
    );

    // Message sur stderr du wrapper (le multiplexeur draine dans tracing).
    let stderr = std::fs::read_to_string(&h.stderr_path).unwrap();
    assert!(
        stderr.to_lowercase().contains("hadolint"),
        "message d'absence de hadolint attendu sur stderr : {stderr:?}"
    );

    // Le serveur vit toujours.
    h.request(3, "initialize", json!({"capabilities": {}}))
        .await;
    let resp = h
        .next_frame(Duration::from_secs(5))
        .await
        .expect("le serveur doit survivre au spawn en échec");
    assert_eq!(resp["id"], json!(3));
    assert!(resp.get("result").is_some());

    h.shutdown_and_exit().await;
}
