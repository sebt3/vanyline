use std::sync::Arc;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use vanyline_sandbox::{AppState, auth::AuthState, build_app, build_metrics_app, config::Config};
use tower::ServiceExt;

// ── Prometheus recorder (one-time global init for the test process) ───────────

static PROMETHEUS_INIT: OnceLock<()> = OnceLock::new();

fn ensure_prometheus() {
    PROMETHEUS_INIT.get_or_init(|| {
        vanyline_sandbox::telemetry::init_prometheus()
            .expect("Prometheus recorder init failed in tests");
    });
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn no_auth_state() -> AppState {
    let config = Arc::new(Config {
        listen: "0.0.0.0:3000".into(),
        tls_cert: None,
        tls_key: None,
        oidc_issuer: None,
        oidc_audience: None,
        auth_groups_admin: "kubernetes-admin".into(),
        auth_groups_read: "kubernetes-view,kubernetes-edit".into(),
        no_auth: true,
        static_token: None,
        public_url: None,
        oidc_ca_cert: None,
        metrics_listen: "0.0.0.0:9090".into(),
        otel_endpoint: None,
        sandbox_root: std::path::Path::new("/workspace").into(),
    });
    let auth = Arc::new(AuthState::new(config.clone()));
    AppState { config, auth }
}

fn no_auth_state_with_root(root: &std::path::Path) -> AppState {
    let config = Arc::new(Config {
        listen: "0.0.0.0:3000".into(),
        tls_cert: None,
        tls_key: None,
        oidc_issuer: None,
        oidc_audience: None,
        auth_groups_admin: "kubernetes-admin".into(),
        auth_groups_read: "kubernetes-view,kubernetes-edit".into(),
        no_auth: true,
        static_token: None,
        public_url: None,
        oidc_ca_cert: None,
        metrics_listen: "0.0.0.0:9090".into(),
        otel_endpoint: None,
        sandbox_root: root.to_path_buf(),
    });
    let auth = Arc::new(AuthState::new(config.clone()));
    AppState { config, auth }
}

fn auth_state() -> AppState {
    let config = Arc::new(Config {
        listen: "0.0.0.0:3000".into(),
        tls_cert: None,
        tls_key: None,
        oidc_issuer: Some("https://authentik.example.com/application/o/mcp/".into()),
        oidc_audience: Some("mcp-client".into()),
        auth_groups_admin: "kubernetes-admin".into(),
        auth_groups_read: "kubernetes-view,kubernetes-edit".into(),
        no_auth: false,
        static_token: None,
        public_url: Some("https://mcp.example.com".into()),
        oidc_ca_cert: None,
        metrics_listen: "0.0.0.0:9090".into(),
        otel_endpoint: None,
        sandbox_root: std::path::Path::new("/workspace").into(),
    });
    let auth = Arc::new(AuthState::new(config.clone()));
    AppState { config, auth }
}

fn static_token_state() -> AppState {
    let config = Arc::new(Config {
        listen: "0.0.0.0:3000".into(),
        tls_cert: None,
        tls_key: None,
        oidc_issuer: None,
        oidc_audience: None,
        auth_groups_admin: "kubernetes-admin".into(),
        auth_groups_read: "kubernetes-view,kubernetes-edit".into(),
        no_auth: false,
        static_token: Some("demo-token".into()),
        public_url: None,
        oidc_ca_cert: None,
        metrics_listen: "0.0.0.0:9090".into(),
        otel_endpoint: None,
        sandbox_root: std::path::Path::new("/workspace").into(),
    });
    let auth = Arc::new(AuthState::new(config.clone()));
    AppState { config, auth }
}

fn mcp_request(method: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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

// ── /health ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_200_ok() {
    let app = build_app(no_auth_state());
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
    assert_eq!(json["status"], "ok");
}

// ── Auth enforcement ──────────────────────────────────────────────────────────

#[tokio::test]
async fn mcp_without_token_returns_401() {
    let app = build_app(auth_state());
    let resp = app.oneshot(mcp_request("tools/list")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let header = resp
        .headers()
        .get("www-authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        header,
        r#"Bearer error="invalid_token", resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource""#
    );
}

#[tokio::test]
async fn mcp_with_malformed_bearer_returns_401() {
    let app = build_app(auth_state());
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
async fn mcp_with_valid_static_token_returns_200() {
    let app = build_app(static_token_state());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("authorization", "Bearer demo-token")
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
async fn mcp_with_wrong_static_token_returns_401() {
    let app = build_app(static_token_state());
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

// ── MCP protocol ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_returns_protocol_version() {
    let app = build_app(no_auth_state());
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
    let app = build_app(no_auth_state());
    let resp = app.oneshot(mcp_request("tools/list")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["jsonrpc"], "2.0");
    let tools = json["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 5);
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"edit_file"));
    assert!(names.contains(&"delete_file"));
    assert!(names.contains(&"list_directory"));
    assert!(json["error"].is_null());
}

// ── tools/call (filesystem) ──────────────────────────────────────────────────

#[tokio::test]
async fn tools_call_write_then_read_file() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(no_auth_state_with_root(tmpdir.path()));

    // Write a file
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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
    
    assert!(json["result"]["content"][0]["text"].as_str().unwrap().contains("wrote"));

    // Read it back
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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
    let app = build_app(no_auth_state_with_root(tmpdir.path()));

    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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
    assert!(text.contains("VNL-SBX-001"), "expected VNL-SBX-001 in: {text}");
}

#[tokio::test]
async fn tools_call_edit_file_nominal() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(no_auth_state_with_root(tmpdir.path()));

    // Write initial content
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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
    let app = build_app(no_auth_state_with_root(tmpdir.path()));

    // Write a file
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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
    assert!(text.contains("VNL-TLS-001"), "expected VNL-TLS-001 in: {text}");
}

#[tokio::test]
async fn tools_call_list_directory_nominal() {
    let tmpdir = tempfile::tempdir().unwrap();
    let app = build_app(no_auth_state_with_root(tmpdir.path()));

    // Write two files
    for name in ["alpha.txt", "beta.txt"] {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("content-type", "application/json")
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
    let app = build_app(no_auth_state_with_root(tmpdir.path()));

    // Call read_file without path
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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
    assert!(json["error"].is_null(), "expected tool error, not JSON-RPC error");
}

// ── tools/call (error codes) ──────────────────────────────────────────────────

#[tokio::test]
async fn tools_call_unknown() {
    let app = build_app(no_auth_state());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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
    let app = build_app(no_auth_state());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
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
    let app = build_app(no_auth_state());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"foobar"}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp.into_body()).await;
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Unknown tool: foobar"));
}

#[tokio::test]
async fn unknown_method_returns_rpc_error() {
    let app = build_app(no_auth_state());
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
    let app = build_app(no_auth_state());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn response_id_matches_request_id_string() {
    let app = build_app(no_auth_state());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":"req-xyz","method":"tools/list","params":{}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["id"], "req-xyz");
}

// ── OAuth metadata ────────────────────────────────────────────────────────────

#[tokio::test]
async fn oauth_metadata_contains_resource_and_auth_servers() {
    let app = build_app(auth_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/oauth-protected-resource")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["resource"], "https://mcp.example.com");
    assert!(
        json["authorization_servers"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(
                "https://authentik.example.com/application/o/mcp/"
            ))
    );
    assert_eq!(
        json["bearer_methods_supported"],
        serde_json::json!(["header"])
    );
}

#[tokio::test]
async fn oauth_metadata_defaults_to_localhost_without_public_url() {
    let app = build_app(no_auth_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/oauth-protected-resource")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["resource"], "http://localhost:3000");
    assert_eq!(json["authorization_servers"], serde_json::json!([]));
}

// ── Metrics endpoint ──────────────────────────────────────────────────────────

#[tokio::test]
async fn metrics_returns_200_with_prometheus_content_type() {
    ensure_prometheus();

    let app = build_metrics_app();
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

    let app = build_metrics_app();
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