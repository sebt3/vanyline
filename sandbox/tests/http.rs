//! Tests d'intégration HTTP bout-en-bouche du serveur sandbox : routes de
//! `build_app` (`/mcp` protégé par jeton d'Owner — authentification et tools
//! MCP —, relais `/git/status` et `/git/unpushed`, absence de découverte
//! OAuth, aucune route publique) et du port d'exploitation (`build_ops_app` /
//! `spawn_ops_server` : `/health`, `/metrics` sans authentification) — motif
//! une-request-une-assertion sur l'app complète. Les sessions WS elles-mêmes
//! ne sont pas couvertes ici ; en revanche le scrub du paramètre `ticket`
//! par le `@TraceLayer` (lib.sdd `Must`) l'est, via un harnais de capture
//! tracing local (`log_capture`).
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::assert_is_empty
    )
)]

use std::process::Command;
use std::sync::Arc;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use vanyline_sandbox::auth::test_support;
use vanyline_sandbox::{AppState, build_app, build_ops_app, spawn_ops_server};

// ── Prometheus recorder (one-time global init for the test process) ───────────

static PROMETHEUS_INIT: OnceLock<()> = OnceLock::new();

fn ensure_prometheus() {
    PROMETHEUS_INIT.get_or_init(|| {
        vanyline_sandbox::telemetry::init_prometheus()
            .expect("Prometheus recorder init failed in tests");
    });
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn test_state(root: &std::path::Path) -> AppState {
    let config = Arc::new(test_support::test_config(root));
    let auth = Arc::new(test_support::auth_state(config.clone()));
    let (fs_events, fs_flush) = vanyline_sandbox::fs_push_channels();
    AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: std::sync::Arc::new(vanyline_sandbox::jobs::Jobs::new(std::sync::Arc::clone(
            &config,
        ))),
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: std::sync::Arc::new(vanyline_sandbox::lsp::LspManager::default()),
        fs_events,
        fs_flush,
    }
}

fn test_state_default() -> AppState {
    test_state(std::path::Path::new("/workspace"))
}

/// Jeton d'Owner de test signé par la clé de test — le seul mode d'accès
/// (aucun contournement : cf. auth.sdd).
fn owner_token() -> String {
    test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        3600,
    )
}

fn bearer() -> String {
    format!("Bearer {}", owner_token())
}

fn mcp_request(method: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": {}
            })
            .to_string(),
        ))
        .unwrap()
}

async fn body_json(body: axum::body::Body) -> serde_json::Value {
    let bytes = body.collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

// ── Port d'exploitation (`build_ops_app` / `spawn_ops_server`) ────────────────

/// Scenario `sante_repond_sans_autorisation_sur_le_port_d_exploitation`
/// (`lib.sdd`) — remplace `health_returns_200_ok`, qui interrogeait
/// `build_app` (le port principal ne porte plus `/health`).
#[tokio::test]
async fn sante_repond_sans_autorisation_sur_le_port_d_exploitation() {
    let app = build_ops_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json, serde_json::json!({"status": "ok"}));
}

/// Scenario `sante_absente_du_port_principal` (`lib.sdd`) : aucune des trois
/// routes non authentifiées n'est atteignable sur le routeur principal, avec
/// ou sans jeton d'Owner valide — trois 404.
#[tokio::test]
async fn sante_absente_du_port_principal() {
    let app = build_app(test_state_default());
    for uri in [
        "/health",
        "/metrics",
        "/.well-known/oauth-protected-resource",
    ] {
        for auth in [None, Some(bearer())] {
            let mut builder = Request::builder().uri(uri);
            if let Some(token) = &auth {
                builder = builder.header("authorization", token);
            }
            let resp = app
                .clone()
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{uri} ne doit pas être servie sur le port principal (jeton : {auth:?})"
            );
        }
    }
}

/// Scenario `spawn_ops_server_sur_port_occupe_rend_erreur` (`lib.sdd`) : un
/// bind sur adresse déjà liée rend `Err` (échec propagé, jamais avalé), sans
/// panic.
#[tokio::test]
async fn spawn_ops_server_sur_port_occupe_rend_erreur() {
    let holder = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = holder.local_addr().unwrap();
    let result = spawn_ops_server(addr).await;
    assert!(
        result.is_err(),
        "spawn_ops_server({addr}) devrait rendre Err sur port occupé, rendu : {result:?}"
    );
}

/// Scenario `spawn_ops_server_expose_health_et_metrics_sur_port_lie`
/// (`lib.sdd`) : une fois lié, le serveur d'exploitation sert `/health` et
/// `/metrics` sur le port choisi, sans authentification. Convention
/// « ports sondés » (`main.sdd` Tasks) : se lier à `127.0.0.1:0`, lire
/// `local_addr()`, relâcher, passer l'adresse explicite.
#[tokio::test]
async fn spawn_ops_server_expose_health_et_metrics_sur_port_lie() {
    ensure_prometheus();

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    spawn_ops_server(addr).await.expect("bind sur port libre");

    let resp = reqwest::get(format!("http://{addr}/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json, serde_json::json!({"status": "ok"}));

    let resp = reqwest::get(format!("http://{addr}/metrics"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(ct.contains("text/plain"), "content-type inattendu : {ct}");
    assert!(
        ct.contains("0.0.4"),
        "version d'exposition absente du content-type : {ct}"
    );
    let body = resp.text().await.unwrap();
    assert!(
        !body.is_empty(),
        "corps Prometheus vide sur le port d'exploitation"
    );
}

// ── Auth enforcement ──────────────────────────────────────────────────────────

#[tokio::test]
async fn mcp_without_token_returns_401() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let header = resp
        .headers()
        .get("www-authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    // aucune découverte OAuth : pas de resource_metadata (auth.sdd)
    assert_eq!(header, r#"Bearer error="invalid_token""#);
}

#[tokio::test]
async fn mcp_with_malformed_bearer_returns_401() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", "NotBearer token")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_with_valid_owner_token_returns_200() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    let tools = json["result"]["tools"].as_array().unwrap();
    let tool_names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(tool_names.contains(&"read_file"));
}

#[tokio::test]
async fn mcp_with_garbage_token_returns_401() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", "Bearer wrong-token")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_with_foreign_owner_token_returns_403() {
    let app = build_app(test_state_default());
    let foreign = test_support::mint_owner_token(
        "system:serviceaccount:ns:owner-bob",
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {foreign}"))
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── MCP protocol ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_returns_protocol_version() {
    let app = build_app(test_state_default());
    let resp = app.oneshot(mcp_request("initialize")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert_eq!(json["result"]["protocolVersion"], "2024-11-05");
    assert!(json["result"]["capabilities"]["tools"].is_object());
    assert!(json["result"]["serverInfo"]["name"].is_string());
}

// ── tools/list ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn tools_list_advertises_filesystem_tools() {
    let app = build_app(test_state_default());
    let resp = app.oneshot(mcp_request("tools/list")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["jsonrpc"], "2.0");
    let tools = json["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 17);
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"edit_file"));
    assert!(names.contains(&"delete_file"));
    assert!(names.contains(&"list_directory"));
    assert!(names.contains(&"find_files"));
    assert!(names.contains(&"search"));
    assert!(names.contains(&"execute_command"));
    assert!(json["error"].is_null());
}

// ── tools/list — full list ────────────────────────────────────────────────────

#[tokio::test]
async fn tools_list_advertises_search_tools() {
    let app = build_app(test_state_default());
    let resp = app.oneshot(mcp_request("tools/list")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["jsonrpc"], "2.0");
    let tools = json["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 17);
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"edit_file"));
    assert!(names.contains(&"delete_file"));
    assert!(names.contains(&"list_directory"));
    assert!(names.contains(&"find_files"));
    assert!(names.contains(&"search"));
    assert!(names.contains(&"execute_command"));
    assert!(json["error"].is_null());
}

// ── tools/call (filesystem) ──────────────────────────────────────────────────

#[tokio::test]
async fn tools_call_write_then_read_file() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Write a file
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "write_file",
                    "arguments": { "path": "greeting.txt", "content": "Hello, world!\nSecond line.\n" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);

    assert!(
        json["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("wrote")
    );

    // Read it back
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": { "path": "greeting.txt" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.lines().any(|l| l.contains("Hello, world")));
}

#[tokio::test]
async fn tools_call_read_file_confinement_rejected() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": { "path": "../outside.txt" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], true);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("VNL-SBX-001"),
        "expected VNL-SBX-001 in: {text}"
    );
}

#[tokio::test]
async fn tools_call_edit_file_nominal() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Write initial content
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "write_file",
                    "arguments": { "path": "data.txt", "content": "Hello, world!\n" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // Edit the file
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "edit_file",
                    "arguments": { "path": "data.txt", "old_string": "Hello, world!", "new_string": "Goodbye, world!" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // Read back to confirm
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": { "path": "data.txt" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Goodbye, world!"));
}

#[tokio::test]
async fn tools_call_delete_file_nominal() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Write a file
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "write_file",
                    "arguments": { "path": "to-delete.txt", "content": "delete me\n" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // Delete it
    let delete_req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "delete_file",
                    "arguments": { "path": "to-delete.txt" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    app.clone().oneshot(delete_req).await.unwrap();

    // Try to read it again — should fail
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": { "path": "to-delete.txt" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], true);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("VNL-TLS-001"),
        "expected VNL-TLS-001 in: {text}"
    );
}

#[tokio::test]
async fn tools_call_list_directory_nominal() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Write two files
    for name in ["alpha.txt", "beta.txt"] {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("authorization", bearer())
            .body(Body::from(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "write_file",
                        "arguments": { "path": name, "content": format!("content of {name}\n") }
                    }
                })
                .to_string(),
            ))
            .unwrap();
        app.clone().oneshot(req).await.unwrap();
    }

    // List directory
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "list_directory",
                    "arguments": { "path": "" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("alpha.txt"), "expected alpha.txt in: {text}");
    assert!(text.contains("beta.txt"), "expected beta.txt in: {text}");
}

#[tokio::test]
async fn tools_call_read_file_missing_argument() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Call read_file without path
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "read_file",
                    "arguments": {}
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    // Must be an isError:true tool-level error, NOT a JSON-RPC protocol error
    assert_eq!(json["result"]["isError"], true);
    assert!(
        json["error"].is_null(),
        "expected tool error, not JSON-RPC error"
    );
}

// ── tools/call (error codes) ──────────────────────────────────────────────────

#[tokio::test]
async fn tools_call_unknown() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope"}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["error"]["code"], -32602);
    assert!(json["result"].is_null());
}

// ── Other MCP protocol tests ──────────────────────────────────────────────────

#[tokio::test]
async fn initialize_echoes_request_id() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":"req-abc","method":"initialize","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["id"], "req-abc");
}

#[tokio::test]
async fn tools_call_unknown_returns_unknown_tool_message() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"foobar"}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp.into_body()).await;
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Unknown tool: foobar")
    );
}

#[tokio::test]
async fn unknown_method_returns_rpc_error() {
    let app = build_app(test_state_default());
    let resp = app.oneshot(mcp_request("unknown/method")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["error"]["code"], -32601);
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown/method")
    );
}

#[tokio::test]
async fn notification_without_id_returns_202() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn response_id_matches_request_id_string() {
    let app = build_app(test_state_default());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":"req-xyz","method":"tools/list","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["id"], "req-xyz");
}

// ── tools/call (search) ───────────────────────────────────────────────────────

#[tokio::test]
async fn tools_call_find_files_nominal() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Write a .rs and a .txt file
    let write_rs_req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "write_file", "arguments": { "path": "a.rs", "content": "fn main() {}" } }
            }).to_string(),
        )).unwrap();
    app.clone().oneshot(write_rs_req).await.unwrap();

    let write_txt_req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "write_file", "arguments": { "path": "b.txt", "content": "hello" } }
            }).to_string(),
        )).unwrap();
    app.clone().oneshot(write_txt_req).await.unwrap();

    // find_files with pattern "*.rs"
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "find_files", "arguments": { "pattern": "*.rs", "path": "" } }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("a.rs"), "expected a.rs in: {text}");
    assert!(
        !text.contains("b.txt"),
        "should not contain b.txt in: {text}"
    );
}

#[tokio::test]
async fn tools_call_find_files_default_path() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Write a .rs and a .txt file at root
    let write_rs_req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "write_file", "arguments": { "path": "rusty.rs", "content": "rusty" } }
            }).to_string(),
        )).unwrap();
    app.clone().oneshot(write_rs_req).await.unwrap();

    let write_txt_req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "write_file", "arguments": { "path": "plain.txt", "content": "plain" } }
            }).to_string(),
        )).unwrap();
    app.clone().oneshot(write_txt_req).await.unwrap();

    // find_files WITHOUT path in arguments — should default to sandbox root
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "find_files", "arguments": { "pattern": "*.rs" } }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("rusty.rs"), "expected rusty.rs in: {text}");
    assert!(
        !text.contains("plain.txt"),
        "should not contain plain.txt in: {text}"
    );
}

#[tokio::test]
async fn tools_call_search_nominal() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Write a file containing "fn foo() {}"
    let write_req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "write_file", "arguments": { "path": "mod.rs", "content": "fn foo() {}\n" } }
            }).to_string(),
        )).unwrap();
    app.clone().oneshot(write_req).await.unwrap();

    // search with regex pattern
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "search", "arguments": { "pattern": "fn \\w+", "path": "" } }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("foo"), "expected 'foo' in match: {text}");
}

#[tokio::test]
async fn tools_call_search_confinement_rejected() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));
    // search with path escaping sandbox root
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "search", "arguments": { "pattern": "foo", "path": "../../etc" } }
            }).to_string(),
        )).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], true);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("VNL-SBX-001"),
        "expected VNL-SBX-001 in: {text}"
    );
}

#[tokio::test]
async fn tools_call_find_files_invalid_pattern() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));
    // find_files with an invalid glob pattern — confinement succeeds (empty path → sandbox_root)
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "find_files", "arguments": { "pattern": "[" } }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], true);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("VNL-TLS-005"),
        "expected VNL-TLS-005 in: {text}"
    );
}

// ── tools/call (command) ─────────────────────────────────────────────────────

#[tokio::test]
async fn tools_list_advertises_command_tool() {
    let app = build_app(test_state_default());
    let resp = app.oneshot(mcp_request("tools/list")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["jsonrpc"], "2.0");
    let tools = json["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 17);
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"execute_command"));
    assert!(json["error"].is_null());
}

#[tokio::test]
async fn tools_call_execute_command_default_cwd_is_sandbox_root() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // execute_command sans cwd → doit tourner dans sandbox_root
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "execute_command",
                    "arguments": { "command": "pwd" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    let expected = tmpdir.path().canonicalize().unwrap();
    assert!(
        text.contains(&expected.to_string_lossy().into_owned()),
        "expected sandbox root {expected:?} in output: {text}"
    );
}

#[tokio::test]
async fn tools_call_execute_command_nominal() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "execute_command",
                    "arguments": { "command": "echo hello" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("hello"), "expected 'hello' in: {text}");
}

#[tokio::test]
async fn tools_call_execute_command_confinement_rejected() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "execute_command",
                    "arguments": { "command": "pwd", "cwd": "../../etc" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], true);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("VNL-SBX-001"),
        "expected VNL-SBX-001 in: {text}"
    );
}

#[tokio::test]
async fn tools_call_execute_command_empty_command_rejected() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "execute_command",
                    "arguments": { "command": "" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], true);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("VNL-TLS-005"),
        "expected VNL-TLS-005 in: {text}"
    );
}

#[tokio::test]
async fn tools_call_execute_command_missing_argument() {
    let app = build_app(test_state_default());

    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "execute_command",
                    "arguments": {}
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    // Must be a tool-level error, NOT a JSON-RPC protocol error
    assert_eq!(json["result"]["isError"], true);
    assert!(
        json["error"].is_null(),
        "expected tool error, not JSON-RPC error"
    );
}

// ── tools/call (command) — activation .venv (feature python-support) ─────────

#[tokio::test]
async fn tools_call_execute_command_active_le_venv_du_workspace() {
    use std::os::unix::fs::PermissionsExt;

    let tmpdir = tempfile::tempdir().unwrap();
    let venv = tmpdir.path().join(".venv");
    std::fs::create_dir_all(venv.join("bin")).unwrap();
    std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    let script = venv.join("bin").join("vnl-venv-marker");
    std::fs::write(&script, "#!/bin/sh\necho vnl-venv-ok\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let app = build_app(test_state(tmpdir.path()));

    // VIRTUAL_ENV posée (assertion sur `/.venv]` : le chemin du venv est
    // tmpdir/.venv) ET la commande résolue depuis .venv/bin (PATH en tête).
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "execute_command",
                    "arguments": { "command": "echo \"[$VIRTUAL_ENV]\" && vnl-venv-marker" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("/.venv]"),
        "expected VIRTUAL_ENV '…/.venv]' in: {text}"
    );
    assert!(
        text.contains("vnl-venv-ok"),
        "expected .venv/bin on PATH (vnl-venv-ok) in: {text}"
    );
}

#[tokio::test]
async fn tools_call_execute_command_sans_venv() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Pas de `.venv/pyvenv.cfg` ⟹ aucun overlay : VIRTUAL_ENV unset ⟹ `[]`.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "execute_command",
                    "arguments": { "command": "echo \"[$VIRTUAL_ENV]\"" }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("[]"), "expected empty VIRTUAL_ENV in: {text}");
}

#[tokio::test]
async fn tools_call_execute_command_envs_non_injectables() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(test_state(tmpdir.path()));

    // Sécurité (le point clé du `#[serde(skip)]` sur `envs`) : un champ
    // "envs" dans les arguments du tool est IGNOREE à la désérialisation —
    // ici la forme aurait même été désérialisable (tableau de paires),
    // seule la skip l'empêche. Tmpdir SANS venv : aucun env additionnel
    // n'est posé par le sandbox lui-même, donc `$X` doit rester vide.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "execute_command",
                    "arguments": { "command": "echo \"[$X]\"", "envs": [["X", "INJECTEE"]] }
                }
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["result"]["isError"], false);
    let text = json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("[]"),
        "expected empty $X (envs ignored) in: {text}"
    );
    assert!(
        !text.contains("INJECTEE"),
        "envs must NEVER be injectable from tool arguments, got: {text}"
    );
}

// ── Aucune découverte OAuth (suppression RFC 9728, mcp.sdd) ──────────────────

/// Scénario `aucune_decouverte_oauth` (`mcp.sdd`).
#[tokio::test]
async fn aucune_decouverte_oauth() {
    let app = build_app(test_state_default());

    // avec jeton d'Owner valide : la ressource n'existe plus
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/.well-known/oauth-protected-resource")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // sans jeton : idem
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/oauth-protected-resource")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Metrics endpoint (servi par `build_ops_app` sur le port d'exploitation) ───

#[tokio::test]
async fn metrics_returns_200_with_prometheus_content_type() {
    ensure_prometheus();

    let app = build_ops_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/plain"), "unexpected content-type: {ct}");
    assert!(
        ct.contains("0.0.4"),
        "missing exposition version in content-type: {ct}"
    );
}

#[tokio::test]
async fn metrics_body_is_valid_prometheus_text() {
    ensure_prometheus();

    // Emit a known counter so we have at least one metric to assert on.
    metrics::counter!("test_counter_total", "label" => "value").increment(1);

    let app = build_ops_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(
        text.contains("test_counter_total"),
        "metric not found in output:\n{text}"
    );
}

// ── git status ────────────────────────────────────────────────────────────────

/// Helper: create a git repo in `root` with an initial commit on `main`.
fn init_repo(root: &std::path::Path) {
    std::fs::create_dir_all(root).expect("create dir");
    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(root)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );
    std::fs::write(root.join("README.md"), "# test\n").expect("write README");
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(root)
            .status()
            .unwrap()
            .success(),
        "git add failed"
    );
    let output = Command::new("git")
        .args([
            "-c",
            "user.email=test@test",
            "-c",
            "user.name=test",
            "commit",
            "-m",
            "init",
        ])
        .current_dir(root)
        .output()
        .expect("git commit failed");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn git_status_endpoint_nominal() {
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use vanyline_sandbox::build_app;

    let tmpdir = tempfile::tempdir().unwrap();
    init_repo(tmpdir.path());

    // Create an untracked file after the commit
    std::fs::write(tmpdir.path().join("new.txt"), "hello\n").expect("write new.txt");

    let config = std::sync::Arc::new(test_support::test_config(tmpdir.path()));
    let auth = std::sync::Arc::new(test_support::auth_state(config.clone()));
    let (fs_events, fs_flush) = vanyline_sandbox::fs_push_channels();
    let state = vanyline_sandbox::AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: std::sync::Arc::new(vanyline_sandbox::jobs::Jobs::new(std::sync::Arc::clone(
            &config,
        ))),
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: std::sync::Arc::new(vanyline_sandbox::lsp::LspManager::default()),
        fs_events,
        fs_flush,
    };

    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/status")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();

    assert_eq!(json["clean"], false);
    let files = json["files"].as_array().unwrap();
    assert!(
        !files.is_empty(),
        "expected at least one file in status: {json}"
    );
}

#[tokio::test]
async fn git_status_endpoint_clean() {
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let tmpdir = tempfile::tempdir().unwrap();
    init_repo(tmpdir.path());

    let config = std::sync::Arc::new(test_support::test_config(tmpdir.path()));
    let auth = std::sync::Arc::new(test_support::auth_state(config.clone()));
    let (fs_events, fs_flush) = vanyline_sandbox::fs_push_channels();
    let state = vanyline_sandbox::AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: std::sync::Arc::new(vanyline_sandbox::jobs::Jobs::new(std::sync::Arc::clone(
            &config,
        ))),
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: std::sync::Arc::new(vanyline_sandbox::lsp::LspManager::default()),
        fs_events,
        fs_flush,
    };

    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/status")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();

    assert_eq!(json["clean"], true);
    assert!(json["files"].is_array());
    assert_eq!(json["files"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn git_status_endpoint_not_a_repo() {
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let tmpdir = tempfile::tempdir().unwrap();
    // Do NOT init a git repo here

    let config = std::sync::Arc::new(test_support::test_config(tmpdir.path()));
    let auth = std::sync::Arc::new(test_support::auth_state(config.clone()));
    let (fs_events, fs_flush) = vanyline_sandbox::fs_push_channels();
    let state = vanyline_sandbox::AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: std::sync::Arc::new(vanyline_sandbox::jobs::Jobs::new(std::sync::Arc::clone(
            &config,
        ))),
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: std::sync::Arc::new(vanyline_sandbox::lsp::LspManager::default()),
        fs_events,
        fs_flush,
    };

    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/status")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let error_text = json["error"].as_str().unwrap();
    assert!(
        error_text.contains("VNL-SBX-004"),
        "expected VNL-SBX-004 in: {error_text}"
    );
}

// ── git unpushed ──────────────────────────────────────────────────────────────

fn run_git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// Construit `<tmp>/remote` (dépôt normal, un commit initial sur `main`),
/// `<tmp>/bare.git` (clone bare avec la refspec de fetch posée + fetch —
/// mêmes étapes que `vanyline-maint init`) et `<tmp>/wt` (worktree créé
/// depuis le bare — mêmes étapes que `vanyline-maint checkout`).
/// `worktree_branch == "main"` réutilise la branche existante (cas
/// upstream) ; toute autre valeur crée une nouvelle branche locale depuis
/// `main` (cas sans upstream). Retourne le chemin du worktree.
fn make_worktree_topology(tmp: &std::path::Path, worktree_branch: &str) -> std::path::PathBuf {
    let remote = tmp.join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    run_git(&remote, &["init", "-q", "-b", "main"]);
    run_git(&remote, &["config", "user.email", "t@t"]);
    run_git(&remote, &["config", "user.name", "t"]);
    run_git(&remote, &["commit", "-q", "--allow-empty", "-m", "c1"]);

    let bare = tmp.join("bare.git");
    run_git(
        tmp,
        &[
            "clone",
            "-q",
            "--bare",
            remote.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    run_git(
        &bare,
        &[
            "config",
            "--replace-all",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    run_git(&bare, &["fetch", "-q", "--prune"]);

    let wt = tmp.join("wt");
    if worktree_branch == "main" {
        run_git(
            tmp,
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "worktree",
                "add",
                wt.to_str().unwrap(),
                "main",
            ],
        );
    } else {
        run_git(
            tmp,
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "worktree",
                "add",
                "-b",
                worktree_branch,
                wt.to_str().unwrap(),
                "main",
            ],
        );
    }
    // Worktree commits need author identity too.
    run_git(&wt, &["config", "user.email", "t@t"]);
    run_git(&wt, &["config", "user.name", "t"]);

    wt
}

#[tokio::test]
async fn git_unpushed_endpoint_with_upstream() {
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use vanyline_sandbox::build_app;

    let tmpdir = tempfile::tempdir().unwrap();
    let wt = make_worktree_topology(tmpdir.path(), "main");

    // Add a local commit on the worktree
    run_git(
        &wt,
        &["commit", "-q", "--allow-empty", "-m", "local change"],
    );

    let config = std::sync::Arc::new(test_support::test_config(&wt.clone()));
    let auth = std::sync::Arc::new(test_support::auth_state(config.clone()));
    let (fs_events, fs_flush) = vanyline_sandbox::fs_push_channels();
    let state = vanyline_sandbox::AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: std::sync::Arc::new(vanyline_sandbox::jobs::Jobs::new(std::sync::Arc::clone(
            &config,
        ))),
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: std::sync::Arc::new(vanyline_sandbox::lsp::LspManager::default()),
        fs_events,
        fs_flush,
    };

    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/unpushed")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();

    assert_eq!(json["branch"], "main");
    assert_eq!(json["upstream"], "origin/main");
    let commits = json["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["title"], "local change");
    assert_eq!(json["truncated"], false);
}

#[tokio::test]
async fn git_unpushed_endpoint_without_upstream() {
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use vanyline_sandbox::build_app;

    let tmpdir = tempfile::tempdir().unwrap();
    let wt = make_worktree_topology(tmpdir.path(), "feat-new");

    // Add a local commit on the worktree
    run_git(
        &wt,
        &["commit", "-q", "--allow-empty", "-m", "wip on feature"],
    );

    let config = std::sync::Arc::new(test_support::test_config(&wt.clone()));
    let auth = std::sync::Arc::new(test_support::auth_state(config.clone()));
    let (fs_events, fs_flush) = vanyline_sandbox::fs_push_channels();
    let state = vanyline_sandbox::AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: std::sync::Arc::new(vanyline_sandbox::jobs::Jobs::new(std::sync::Arc::clone(
            &config,
        ))),
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: std::sync::Arc::new(vanyline_sandbox::lsp::LspManager::default()),
        fs_events,
        fs_flush,
    };

    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/unpushed")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();

    assert_eq!(json["branch"], "feat-new");
    assert!(json["upstream"].is_null());
    let commits = json["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["title"], "wip on feature");
}

#[tokio::test]
async fn git_unpushed_endpoint_no_local_commits() {
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use vanyline_sandbox::build_app;

    let tmpdir = tempfile::tempdir().unwrap();
    let wt = make_worktree_topology(tmpdir.path(), "main");
    // No additional commit — worktree is at same point as origin/main

    let config = std::sync::Arc::new(test_support::test_config(&wt.clone()));
    let auth = std::sync::Arc::new(test_support::auth_state(config.clone()));
    let (fs_events, fs_flush) = vanyline_sandbox::fs_push_channels();
    let state = vanyline_sandbox::AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: std::sync::Arc::new(vanyline_sandbox::jobs::Jobs::new(std::sync::Arc::clone(
            &config,
        ))),
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: std::sync::Arc::new(vanyline_sandbox::lsp::LspManager::default()),
        fs_events,
        fs_flush,
    };

    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/unpushed")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();

    assert_eq!(json["branch"], "main");
    assert_eq!(json["upstream"], "origin/main");
    let commits = json["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 0);
}

#[tokio::test]
async fn git_unpushed_endpoint_detached_head() {
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use vanyline_sandbox::build_app;

    let tmpdir = tempfile::tempdir().unwrap();
    let wt = make_worktree_topology(tmpdir.path(), "main");
    // Detach HEAD
    run_git(&wt, &["checkout", "-q", "--detach", "HEAD"]);

    let config = std::sync::Arc::new(test_support::test_config(&wt.clone()));
    let auth = std::sync::Arc::new(test_support::auth_state(config.clone()));
    let (fs_events, fs_flush) = vanyline_sandbox::fs_push_channels();
    let state = vanyline_sandbox::AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: std::sync::Arc::new(vanyline_sandbox::jobs::Jobs::new(std::sync::Arc::clone(
            &config,
        ))),
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: std::sync::Arc::new(vanyline_sandbox::lsp::LspManager::default()),
        fs_events,
        fs_flush,
    };

    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/git/unpushed")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let error_text = json["error"].as_str().unwrap();
    assert!(
        error_text.contains("VNL-SBX-006"),
        "expected VNL-SBX-006 in: {error_text}"
    );
}

// ── Scrub du paramètre `ticket` dans le TraceLayer (lib.sdd `Must`) ──────────

mod log_capture {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    /// Buffer partagé : la couche `fmt` y écrit chaque ligne déjà formatée
    /// (spans avec leurs champs — dont `uri` — et events avec leur message).
    #[derive(Clone)]
    struct Shared(Arc<Mutex<String>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut sink = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let s = String::from_utf8_lossy(buf);
            sink.push_str(&s);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl MakeWriter<'_> for Shared {
        type Writer = Self;
        fn make_writer(&self) -> Self {
            self.clone()
        }
    }

    /// Exécute `fut` avec la capture installée comme subscriber par défaut
    /// (garde thread-local `set_default`) et rend les lignes capturées (une
    /// par span — dont le span du `TraceLayer` avec son champ `uri` — et une
    /// par event). Le runtime monothreadé de `#[tokio::test]` garantit que
    /// `fut` tourne sur CE thread, donc sous la garde.
    pub async fn capture<T, F: std::future::Future<Output = T>>(fut: F) -> (T, Vec<String>) {
        let buf = Arc::new(Mutex::new(String::new()));
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_writer(Shared(buf.clone()))
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW);
        let sub = tracing_subscriber::registry::Registry::default().with(layer);
        // Garde thread-local : la capture ne vit que dans cette fonction, le
        // runtime monothreadé de `#[tokio::test]` garantit que `fut` tourne
        // sur CE thread. Les lignes sont clonées avant la sortie de scope.
        let _guard = tracing::subscriber::set_default(sub);
        let out = fut.await;
        let all = buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        (out, all.lines().map(str::to_owned).collect::<Vec<_>>())
    }
}

/// Scenario `trace_ne_jamais_journalise_la_valeur_d_un_ticket` (`lib.sdd`
/// `Must`) : la valeur du paramètre de query `ticket` ne doit JAMAIS
/// apparaître dans un log — sur `GET /ws/fs?ticket=…` (l'upgrade est rejeté
/// 401 par la garde d'auth, mais le span du `TraceLayer`, couche la plus
/// externe, est créé AVANT la garde) et sur `GET /inconnu?ticket=…`
/// (fallback 404 : le scrub est plus externe que le routage, l'URI est
/// loggée scrubée quel que soit le sort de la requête). Le marqueur de
/// contrôle (path rendu + `ticket=***`) prouve que le log part, pas
/// seulement qu'il tait le secret.
#[tokio::test]
async fn trace_ne_jamais_journalise_la_valeur_d_un_ticket() {
    let app = build_app(test_state_default());
    let (_resp, lines) = log_capture::capture(
        app.clone().oneshot(
            Request::builder()
                .method("GET")
                .uri("/ws/fs?ticket=SECRET-INTROUVABLE")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await;

    let joined = lines.join("\n");
    assert!(
        !joined.contains("SECRET-INTROUVABLE"),
        "la valeur du ticket a fuité dans les logs:\n{joined}"
    );
    // Marqueur contrôle : le log part bien, avec le path intact et la valeur
    // du ticket masquée.
    assert!(
        joined.contains("/ws/fs"),
        "aucune trace de la requête /ws/fs (le TraceLayer ne loggue plus ?):\n{joined}"
    );
    assert!(
        joined.contains("ticket=***"),
        "le paramètre ticket n'est pas masqué dans l'URI loggée:\n{joined}"
    );

    // Volet fallback 404 (`Must` ordre des couches : @TraceLayer plus
    // externe que le routage) : une route inconnue loggue aussi son URI
    // scrubée, sans jamais porter la valeur du ticket.
    let (resp, lines) = log_capture::capture(
        app.oneshot(
            Request::builder()
                .method("GET")
                .uri("/inconnu?ticket=SECRET-INTROUVABLE")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await;
    let resp = resp.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "l'URI doit bien tomber sur le fallback 404 pour tester ce volet"
    );

    let joined = lines.join("\n");
    assert!(
        !joined.contains("SECRET-INTROUVABLE"),
        "la valeur du ticket a fuité dans les logs du fallback 404:\n{joined}"
    );
    assert!(
        joined.contains("/inconnu"),
        "aucune trace de la requête /inconnu (le TraceLayer ne loggue plus le fallback ?):\n{joined}"
    );
    assert!(
        joined.contains("ticket=***"),
        "le paramètre ticket n'est pas masqué dans l'URI du fallback:\n{joined}"
    );
}

/// Volet « requête sans ticket inchangée » : le scrub ne casse pas le log
/// normal — l'URI complète (avec un autre paramètre de query) est journalisée
/// telle quelle.
#[tokio::test]
async fn trace_requete_sans_ticket_loggue_l_uri_intacte() {
    let app = build_app(test_state_default());
    let (_resp, lines) = log_capture::capture(
        app.oneshot(
            Request::builder()
                .method("GET")
                .uri("/ws/fs?other=val1")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await;

    let joined = lines.join("\n");
    assert!(
        joined.contains("/ws/fs?other=val1"),
        "l'URI sans ticket doit rester intacte dans le log:\n{joined}"
    );
}
