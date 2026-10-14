//! Tests process du one-shot `vanyline run` (`../chat.sdd` Scenarios
//! `timeout_zero_desactive_la_limite` et
//! `tour_reussi_persiste_deux_messages_et_le_todo`). Le modèle est simulé par
//! un serveur SSE de test en loopback uniquement (127.0.0.1, port éphémère)
//! servant le format OpenAI-compatible attendu par rig — aucun réseau externe,
//! aucune dépendance de plus. XDG isolé par tempdir injecté au child
//! uniquement (convention de `rpc_stdio_smoke.rs` : la config de la machine
//! n'est jamais lue ni touchée).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::tempdir;

/// Corps SSE (format `OpenAI` `chat.completions.chunk`) : une ligne `data:` par
/// événement, terminée par `data: [DONE]`.
fn sse_body(events: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for ev in events {
        out.push_str("data: ");
        out.push_str(&ev.to_string());
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

/// Événement de contenu textuel puis fin de tour (`finish_reason` `stop`).
fn text_events(text: &str) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"choices": [{"delta": {"content": text}, "finish_reason": null}]}),
        serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
    ]
}

/// Événement d'appel d'outil (arguments en chaîne JSON échappée, comme
/// l'API `OpenAI`) puis fin de tour (`finish_reason` `tool_calls`).
fn tool_call_event(name: &str, arguments: &str) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"choices": [{"delta": {"tool_calls": [{
            "index": 0,
            "id": "appel-de-test",
            "function": {"name": name, "arguments": arguments}
        }]}, "finish_reason": null}]}),
        serde_json::json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
    ]
}

/// Lit et évacue la requête HTTP (entêtes puis corps de `Content-Length`
/// octets) pour ne jamais laisser le client bloqué.
fn drain_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<()> {
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
    }
    Ok(())
}

/// Sert une requête : lit la requête, attend `delay` (simule un tour de
/// modèle long), rend le corps SSE puis ferme la connexion.
fn serve_one(sock: TcpStream, delay: Duration, body: &str) -> std::io::Result<()> {
    let mut writer = sock.try_clone()?;
    let mut reader = BufReader::new(sock);
    drain_request(&mut reader)?;
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    writer.write_all(head.as_bytes())?;
    writer.write_all(body.as_bytes())?;
    writer.flush()
}

/// Lance le mock SSE : une réponse par élément du script, une connexion
/// séquentielle par réponse (le client est en `connection: close`). Rend le
/// port écouté ; le `TcpListener` de retour est la boucle d'arrêt (le test s'y
/// connecte à la fin pour libérer le fil mock — sans jamais ré-accepter sur un
/// port rendu à l'OS).
fn spawn_sse_mock(responses: Vec<(Duration, String)>) -> (u16, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("port du mock").port();
    let shutdown = TcpListener::bind("127.0.0.1:0").expect("bind boucle d'arret");
    let shutdown_addr = shutdown.local_addr().expect("adresse de la boucle");
    std::thread::spawn(move || {
        for (delay, body) in responses {
            let Ok((accepted, _)) = listener.accept() else {
                return;
            };
            if serve_one(accepted, delay, &body).is_err() {
                return;
            }
        }
        // Attend la notification de fin du test (connexion sur la boucle
        // interne) pour ne pas ré-accepter sur un port rendu à l'OS.
        let _ = TcpStream::connect(shutdown_addr);
    });
    (port, shutdown)
}

/// Config isolée minimale : provider `openai-compatible` pointant sur le mock
/// `/v1/chat/completions` du loopback, modèle `m1`, agent unique `a1`.
fn write_child_config(home: &Path, port: u16) {
    let cfg = home.join("vanyline");
    std::fs::create_dir_all(cfg.join("agents")).expect("repertoire agents");
    std::fs::write(
        cfg.join("agents").join("a1.md"),
        "---\nmodel: m1\n---\nagent de test\n",
    )
    .expect("agent a1.md");
    std::fs::write(
        cfg.join("config.yaml"),
        format!(
            "providers:\n  mock:\n    type: openai-compatible\n    endpoint: http://127.0.0.1:{port}\n    api_key: cle-test\nmodels:\n  m1:\n    provider: mock\n    model: modele-test\n"
        ),
    )
    .expect("config.yaml");
}

/// Scenario: `timeout_zero_desactive_la_limite` — one-shot lancé avec
/// `timeout_secs` `Some(0)` et un tour de modèle simulé durant plus d'une
/// seconde : le tour va à son terme (aucun `[VNL-CLI-001]`, code de sortie 0)
/// — garde `secs > 0` de `run_one_shot`, contrat de l'aide clap.
#[test]
fn timeout_zero_desactive_la_limite() {
    let home = tempdir().expect("tempdir XDG");
    let ws = tempdir().expect("tempdir cwd");
    let mock_sse = sse_body(&text_events("ok"));
    let (port, shutdown) = spawn_sse_mock(vec![(Duration::from_millis(1200), mock_sse)]);
    write_child_config(home.path(), port);

    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_vanyline"))
        .args(["run", "--timeout", "0", "bonjour"])
        .env("XDG_CONFIG_HOME", home.path())
        .env("XDG_DATA_HOME", home.path())
        .current_dir(ws.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de vanyline run --timeout 0")
        .wait_with_output()
        .expect("attente de vanyline run --timeout 0");
    let elapsed = started.elapsed();
    let _ = TcpStream::connect(shutdown.local_addr().expect("boucle d'arret"));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        elapsed >= Duration::from_millis(1000),
        "le tour simulé doit avoir duré plus d'une seconde (obtenu: {elapsed:?})"
    );
    assert!(
        !stderr.contains("VNL-CLI-001"),
        "aucun timeout attendu avec --timeout 0 ; stderr: {stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "code de sortie 0 attendu ; stderr: {stderr}, stdout: {stdout}"
    );
    assert!(
        stdout.contains("ok"),
        "le tour doit aller à son terme (texte du modèle visible) ; stdout: {stdout}"
    );
}

/// Scenario: `tour_reussi_persiste_deux_messages_et_le_todo` — une
/// conversation neuve, un tour simulé réussi (réponse `"ok"`, un tool call de
/// `todowrite`) : le fichier de conversation porte deux messages (user
/// `"bonjour"`, assistant `"ok"` avec `tool_calls` d'un enregistrement) et
/// `todo` égal à l'état lu dans `todo_state` (l'état posé par `todowrite`
/// pendant le tour, relis après celui-ci).
#[test]
fn tour_reussi_persiste_deux_messages_et_le_todo() {
    let home = tempdir().expect("tempdir XDG");
    let ws = tempdir().expect("tempdir cwd");
    let todos = serde_json::json!([{"content": "tache-x", "status": "in_progress"}]);
    let mock1 = sse_body(&tool_call_event(
        "todowrite",
        &serde_json::json!({ "todos": todos }).to_string(),
    ));
    let mock2 = sse_body(&text_events("ok"));
    let (port, shutdown) = spawn_sse_mock(vec![(Duration::ZERO, mock1), (Duration::ZERO, mock2)]);
    write_child_config(home.path(), port);

    let out = Command::new(env!("CARGO_BIN_EXE_vanyline"))
        .args(["run", "bonjour"])
        .env("XDG_CONFIG_HOME", home.path())
        .env("XDG_DATA_HOME", home.path())
        .current_dir(ws.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de vanyline run")
        .wait_with_output()
        .expect("attente de vanyline run");
    let _ = TcpStream::connect(shutdown.local_addr().expect("boucle d'arret"));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "le tour doit réussir ; stderr: {stderr}, stdout: {stdout}"
    );

    let conv_dir = home.path().join("vanyline").join("conversations");
    let files: Vec<_> = std::fs::read_dir(&conv_dir)
        .expect("repertoire de conversations")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert_eq!(files.len(), 1, "exactement une conversation persistée");
    let raw = std::fs::read_to_string(files[0].path()).expect("lecture de la conversation");
    let conv: serde_json::Value = serde_json::from_str(&raw).expect("conversation en JSON");

    let messages = conv["messages"].as_array().expect("messages en tableau");
    assert_eq!(
        messages.len(),
        2,
        "[user, assistant] attendus, obtenu: {raw}"
    );
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "bonjour");
    assert!(
        messages[0].get("tool_calls").is_none(),
        "le message user ne porte pas de tool_calls ; obtenu: {raw}"
    );
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"], "ok");
    let calls = messages[1]["tool_calls"]
        .as_array()
        .expect("tool_calls de l'assistant en tableau");
    assert_eq!(calls.len(), 1, "tool_calls d'un enregistrement attendu");
    assert_eq!(calls[0]["name"], "todowrite");
    assert_eq!(calls[0]["arguments"]["todos"], todos);

    let todo_raw = conv["todo"].as_str().expect("todo persisté (Some)");
    let todo: serde_json::Value = serde_json::from_str(todo_raw).expect("todo en JSON");
    assert_eq!(
        todo, todos,
        "todo persisté = état lu dans todo_state (posé par todowrite)"
    );
}
