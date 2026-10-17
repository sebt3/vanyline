//! Serveur MCP HTTP de la sandbox : point d'entrée unique `POST /mcp`, JSON-RPC
//! 2.0 « HTTP streamable » POST-only, strictement sans état ; l'exécution des
//! tools est déléguée à `crate::tools_impl`, l'authentification au middleware
//! `crate::auth::require_auth` (spec : `./mcp.sdd`).

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AppState, tools_impl};

// ── JSON-RPC 2.0 types ───────────────────────────────────────────────────────

/// Requête JSON-RPC 2.0 entrante (params optionnels, `id` absent = notification).
#[derive(Deserialize)]
pub struct JsonRpcRequest {
    /// Version du protocole, vérifiée par le client (jamais relue ici).
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Identifiant de la requête ; `None` = notification (réponse HTTP 202 nue).
    pub id: Option<Value>,
    /// Méthode appelée (`initialize`, `tools/list`, `tools/call`, …).
    pub method: String,
    /// Paramètres de la méthode (absents = `Value::Null` par défaut serde).
    #[serde(default)]
    pub params: Value,
}

/// Réponse JSON-RPC 2.0 : `result` OU `error` (les champs `None` sont exclus
/// de la sérialisation, conformément au protocole).
#[derive(Serialize)]
pub struct JsonRpcResponse {
    /// Toujours `"2.0"`.
    pub jsonrpc: &'static str,
    /// Identifiant écho de la requête (absent en cas d'erreur d'enveloppe).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Résultat de la méthode, absent en cas d'erreur.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Objet d'erreur JSON-RPC, absent en cas de succès.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// Objet `error` d'une réponse JSON-RPC 2.0.
#[derive(Serialize)]
pub struct RpcError {
    /// Code d'erreur JSON-RPC (`-32601` méthode inconnue, `-32602` params
    /// invalides, …).
    pub code: i32,
    /// Message lisible de l'erreur.
    pub message: String,
}

impl JsonRpcResponse {
    pub(crate) fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(crate) fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// ── MCP POST /mcp ─────────────────────────────────────────────────────────────

/// Handler `POST /mcp` : notification (pas d'`id`) → HTTP 202 sans corps ;
/// sinon dispatch de la méthode (`initialize` / `tools/list` / `tools/call`)
/// et réponse JSON-RPC. L'authentification est en amont (`require_auth`).
pub async fn handle(
    State(state): State<AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    // Notifications have no id and expect HTTP 202, no body
    if req.id.is_none() {
        metrics::counter!("mcp_requests_total", "method" => "notification").increment(1);
        return StatusCode::ACCEPTED.into_response();
    }

    metrics::counter!("mcp_requests_total", "method" => req.method.clone()).increment(1);

    let response = match req.method.as_str() {
        "initialize" => handle_initialize(req.id, req.params),
        "notifications/initialized" => {
            // Should not happen (no id), but handle gracefully
            return StatusCode::ACCEPTED.into_response();
        }
        "tools/list" => handle_tools_list(req.id),
        "tools/call" => handle_tools_call(req.id, req.params, &state).await,
        _ => JsonRpcResponse::err(req.id, -32601, format!("Method not found: {}", req.method)),
    };

    Json(response).into_response()
}

pub(crate) fn handle_initialize(id: Option<Value>, _params: Value) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": env!("CARGO_PKG_NAME"),
                "version": env!("CARGO_PKG_VERSION"),
            }
        }),
    )
}

pub(crate) fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    let mut tools = vanyline_tools::mcp::filesystem_tools();
    tools.extend(vanyline_tools::mcp::search_tools());
    tools.extend(vanyline_tools::mcp::command_tools());
    tools.extend(tools_impl::lsp_tools());
    // `code_health` (health.sdd `Exposes`) : déclaré avec les outils LSP, dans
    // la même famille de déclarations de ./tools_impl.rs.
    tools.push(tools_impl::code_health_tool());
    JsonRpcResponse::ok(id, serde_json::json!({ "tools": tools }))
}

pub(crate) async fn handle_tools_call(
    id: Option<Value>,
    params: Value,
    state: &AppState,
) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return JsonRpcResponse::err(id, -32602, "Missing tool name in params");
    };

    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    if let Some(result) = tools_impl::dispatch_filesystem(
        &state.config.sandbox_root,
        &state.health,
        name,
        arguments.clone(),
    )
    .await
    {
        JsonRpcResponse::ok(id, result)
    } else if let Some(result) =
        tools_impl::dispatch_search(&state.config.sandbox_root, name, arguments.clone()).await
    {
        JsonRpcResponse::ok(id, result)
    } else if let Some(result) =
        tools_impl::dispatch_command(&state.config.sandbox_root, name, arguments.clone()).await
    {
        JsonRpcResponse::ok(id, result)
    } else if let Some(result) = tools_impl::dispatch_lsp(state, name, arguments.clone()).await {
        JsonRpcResponse::ok(id, result)
    } else if let Some(result) =
        tools_impl::dispatch_edit_and_check(state, name, arguments.clone()).await
    {
        // `edit_and_check` (tâche 08d) : frère de `dispatch_lsp`, même forme
        // `Option<Value>` — `None` si le nom est étranger à cette famille.
        JsonRpcResponse::ok(id, result)
    } else if let Some(result) = tools_impl::dispatch_code_health(state, name, arguments) {
        // `code_health` (health.sdd) : lecture pure du snapshot de santé, forme
        // synchrone `Option<Value>` — `None` si le nom est étranger.
        JsonRpcResponse::ok(id, result)
    } else {
        JsonRpcResponse::err(id, -32602, format!("Unknown tool: {name}"))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::auth::test_support;
    use std::sync::Arc;

    fn make_state() -> AppState {
        let config = Arc::new(test_support::test_config(std::path::Path::new("/tmp")));
        let auth = test_support::auth_state(config.clone());
        let (fs_events, fs_flush) = crate::fs_push_channels();
        AppState {
            health: crate::health::CodeHealth::new(config.clone()),
            jobs: Arc::new(crate::jobs::Jobs::new(Arc::clone(&config))),
            config,
            auth: Arc::new(auth),
            tickets: crate::ws::ticket::TicketStore::new(),
            lsp: std::sync::Arc::new(crate::lsp::LspManager::default()),
            fs_events,
            fs_flush,
        }
    }

    #[test]
    fn initialize_returns_correct_protocol_version() {
        let resp = handle_initialize(Some(serde_json::json!(1)), Value::Null);
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["serverInfo"]["name"].is_string());
        assert!(result["serverInfo"]["version"].is_string());
    }

    #[test]
    fn initialize_echoes_request_id() {
        let id = serde_json::json!("req-abc");
        let resp = handle_initialize(Some(id.clone()), Value::Null);
        assert_eq!(resp.id.unwrap(), id);
    }

    #[test]
    fn tools_list_returns_all_tools() {
        let resp = handle_tools_list(Some(serde_json::json!(1)));
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
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
        assert!(names.contains(&"lsp_diagnostics"));
        assert!(names.contains(&"lsp_definition"));
        assert!(names.contains(&"lsp_references"));
        assert!(names.contains(&"lsp_rename"));
        assert!(names.contains(&"lsp_document_symbols"));
        assert!(names.contains(&"lsp_workspace_symbols"));
        assert!(names.contains(&"inspect_symbol"));
        assert!(names.contains(&"edit_and_check"));
        assert!(names.contains(&"code_health"));
    }

    #[tokio::test]
    async fn tools_call_unknown_tool_returns_error() {
        let state = make_state();
        let params = serde_json::json!({ "name": "nope", "arguments": {} });
        let resp = handle_tools_call(Some(serde_json::json!(1)), params, &state).await;
        let error = resp.error.unwrap();
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, "Unknown tool: nope");
    }

    #[test]
    fn tools_call_missing_name_returns_error() {
        let state = make_state();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let resp = rt.block_on(async {
            let params = serde_json::json!({ "arguments": {} });
            handle_tools_call(Some(serde_json::json!(1)), params, &state).await
        });
        let error = resp.error.unwrap();
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, "Missing tool name in params");
    }

    #[test]
    fn rpc_ok_has_no_error() {
        let resp = JsonRpcResponse::ok(Some(serde_json::json!(42)), serde_json::json!("data"));
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), "data");
        assert_eq!(resp.jsonrpc, "2.0");
    }

    #[test]
    fn rpc_err_has_no_result() {
        let resp = JsonRpcResponse::err(Some(serde_json::json!(42)), -32601, "not found");
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "not found");
    }

    #[test]
    fn rpc_err_code_method_not_found() {
        let resp = JsonRpcResponse::err(None, -32601, "Method not found: foo");
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // ── Tests par Scenario (titres de `mcp.sdd`, niveau HTTP) ──────────────────

    fn app_with_root(root: &std::path::Path) -> (axum::Router, String) {
        let config = Arc::new(test_support::test_config(root));
        let auth = test_support::auth_state(config.clone());
        let (fs_events, fs_flush) = crate::fs_push_channels();
        let state = AppState {
            health: crate::health::CodeHealth::new(config.clone()),
            jobs: Arc::new(crate::jobs::Jobs::new(Arc::clone(&config))),
            config,
            auth: Arc::new(auth),
            tickets: crate::ws::ticket::TicketStore::new(),
            lsp: Arc::new(crate::lsp::LspManager::default()),
            fs_events,
            fs_flush,
        };
        let token = test_support::mint_owner_token(
            test_support::TEST_OWNER,
            test_support::TEST_AUDIENCE,
            test_support::TEST_ISSUER,
            600,
        );
        (crate::build_app(state), token)
    }

    async fn post_mcp_authed(root: &std::path::Path, body: &str) -> axum::response::Response {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let (app, token) = app_with_root(root);
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        app.oneshot(req).await.unwrap()
    }

    async fn json_body(resp: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    // Scenario: initialize_rend_version_protocole_et_infos_serveur
    #[tokio::test]
    async fn initialize_rend_version_protocole_et_infos_serveur() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":null}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert!(resp.headers().get("mcp-session-id").is_none());
        let v = json_body(resp).await;
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert!(
            !v["result"]["serverInfo"]["name"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
        assert!(
            !v["result"]["serverInfo"]["version"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
    }

    // Scenario: initialize_echoe_identifiant_non_numberique
    #[test]
    fn initialize_echoe_identifiant_non_numberique() {
        let resp = handle_initialize(Some(serde_json::json!("req-abc")), Value::Null);
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["id"], "req-abc");
        assert!(v.get("error").is_none());
    }

    // Scenario: notification_sans_id_recoit_202_sans_corps
    #[tokio::test]
    async fn notification_sans_id_recoit_202_sans_corps() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(bytes.is_empty());
    }

    // Scenario: id_explicite_null_traite_comme_notification
    #[tokio::test]
    async fn id_explicite_null_traite_comme_notification() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":null,"method":"tools/list"}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(bytes.is_empty(), "aucun listing d'outils attendu");
    }

    // Scenario: initialized_avec_id_porteur_recoit_202
    #[tokio::test]
    async fn initialized_avec_id_porteur_recoit_202() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":7,"method":"notifications/initialized"}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(bytes.is_empty(), "aucune réponse pour l'id 7");
    }

    // Scenario: methode_inconnue_avec_id_rend_32601
    #[tokio::test]
    async fn methode_inconnue_avec_id_rend_32601() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":3,"method":"foo/bar"}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let v = json_body(resp).await;
        assert_eq!(v["error"]["code"], -32601);
        assert_eq!(v["error"]["message"], "Method not found: foo/bar");
        assert!(v.get("result").is_none());
    }

    // Scenario: outils_listes_en_noms_origine_et_17_entrees
    #[tokio::test]
    async fn outils_listes_en_noms_origine_et_17_entrees() {
        let dir = tempfile::tempdir().unwrap();
        let body =
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"cursor":"n'importe"}}"#;
        let v = json_body(post_mcp_authed(dir.path(), body).await).await;
        let tools = v["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 17);
        assert!(names.iter().all(|n| !n.starts_with('/')));
        assert_eq!(names[0], "read_file");
        assert_eq!(names[7], "execute_command");
        assert_eq!(names[15], "edit_and_check");
        assert_eq!(names[16], "code_health");
        assert!(v["result"].get("nextCursor").is_none());
    }

    // Scenario: outils_appelles_par_nom_origine_sans_initialisation
    #[tokio::test]
    async fn outils_appelles_par_nom_origine_sans_initialisation() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_directory","arguments":{"path":""}}}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let v = json_body(resp).await;
        assert!(v["result"]["content"].is_array(), "{v}");
        assert!(v.get("error").is_none(), "{v}");
    }

    // Scenario: tools_call_sans_nom_rend_32602
    #[tokio::test]
    async fn tools_call_sans_nom_rend_32602() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"arguments":{}}}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let v = json_body(resp).await;
        assert_eq!(v["error"]["code"], -32602);
        assert_eq!(v["error"]["message"], "Missing tool name in params");
    }

    // Scenario: outils_inconnu_de_toutes_familles_rend_32602
    #[tokio::test]
    async fn outils_inconnu_de_toutes_familles_rend_32602() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let v = json_body(resp).await;
        assert_eq!(v["error"]["code"], -32602);
        assert_eq!(v["error"]["message"], "Unknown tool: nope");
    }

    // Scenario: erreur_execution_outil_reste_enveloppe_is_error
    #[tokio::test]
    async fn erreur_execution_outil_reste_enveloppe_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"../outside"}}}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let v = json_body(resp).await;
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(text.contains("VNL-SBX-001"), "{text}");
        assert!(v.get("error").is_none());
    }

    // Scenario: arguments_absents_produisent_erreur_outil
    #[tokio::test]
    async fn arguments_absents_produisent_erreur_outil() {
        let dir = tempfile::tempdir().unwrap();
        let body =
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_file"}}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let v = json_body(resp).await;
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(
            text.starts_with("invalid arguments for read_file"),
            "{text}"
        );
    }

    // Scenario: erreur_rpc_rend_http_200
    #[tokio::test]
    async fn erreur_rpc_rend_http_200() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"2.0","id":9,"method":"inconnue"}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(ct.starts_with("application/json"), "{ct}");
        let v = json_body(resp).await;
        assert!(v.get("error").is_some());
        assert!(v.get("result").is_none());
    }

    // Scenario: jsonrpc_non_2_0_accepte
    #[tokio::test]
    async fn jsonrpc_non_2_0_accepte() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"jsonrpc":"1.0","id":1,"method":"initialize","params":null}"#;
        let resp = post_mcp_authed(dir.path(), body).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let v = json_body(resp).await;
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
    }

    // Scenario: corps_invalide_rejete_sans_enveloppe_rpc
    #[tokio::test]
    async fn corps_invalide_rejete_sans_enveloppe_rpc() {
        let dir = tempfile::tempdir().unwrap();
        for body in [
            r#"{"jsonrpc":"2.0","method":"#,
            r#"[{"jsonrpc":"2.0","id":1,"method":"initialize"}]"#,
        ] {
            let resp = post_mcp_authed(dir.path(), body).await;
            assert!(
                resp.status().is_client_error(),
                "{body} → {}",
                resp.status()
            );
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains("\"jsonrpc\""), "{text}");
            assert!(!text.contains("-32700"), "{text}");
        }
    }

    // Scenario: aucune_decouverte_oauth
    #[tokio::test]
    async fn aucune_decouverte_oauth() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let (app, token) = app_with_root(dir.path());
        for auth in [None, Some(format!("Bearer {token}"))] {
            let mut b = Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-protected-resource");
            if let Some(a) = auth {
                b = b.header("authorization", a);
            }
            let resp = app
                .clone()
                .oneshot(b.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
        }
    }
}
