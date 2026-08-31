use axum::{
    Json,
    extract::{Path, State},
};
use miryad_core::auth::AuthUser;
use miryad_core::rbac::can_write;
use miryad_core::users::resolve_user;
use sea_orm::{EntityTrait, IntoActiveModel};

use crate::{
    AppState,
    db::entities::mcp_servers::{Entity as McpServerEntity, Model as McpServerModel},
    error::AppError,
};
use vanyline_lib::domain::{McpServer as DomainMcpServer, McpTransport};

/// Mappe le Model SeaORM sur le type domaine `lib`. `server_type: "sse"` -> erreur
/// claire. `server_type: "sse"` mappe sur `McpTransport::Sse` — accepté, mais la
/// découverte de tools remonte `VNL-MCP-004` via `prefixed_mcp` (connexion SSE
/// pas encore implémentée). Pure, testable.
pub fn build_domain_server(server: &McpServerModel) -> Result<DomainMcpServer, AppError> {
    let transport = match server.server_type.as_str() {
        "http-streamable" => McpTransport::HttpStreamable,
        "sse" => McpTransport::Sse,
        other => {
            return Err(AppError::McpError(format!(
                "VNL-MCP-003: server_type must be 'sse' or 'http-streamable', got: {other}"
            )));
        }
    };
    let headers = match &server.headers {
        serde_json::Value::Object(map) => map
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
        _ => std::collections::BTreeMap::new(),
    };
    Ok(DomainMcpServer {
        name: server.name.clone(),
        transport,
        url: server.url.clone(),
        headers,
    })
}

#[derive(serde::Serialize)]
pub struct McpTestResult {
    pub tools: Vec<String>,
}

pub async fn test_server(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<McpTestResult>, AppError> {
    let db = state.auth.db.clone();
    let principal_user = resolve_user(&db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let server = McpServerEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::McpError("VNL-MCP-002: MCP server not found".to_string()))?;
    if !can_write::<McpServerEntity>(&db, &principal_user, &server)
        .await
        .map_err(AppError::from)?
    {
        return Err(AppError::Forbidden);
    }
    let domain = build_domain_server(&server)?;
    let tools = vanyline_lib::prefixed_mcp::list_mcp_server_tools(&domain)
        .await
        .map_err(|e| AppError::McpError(e.to_string()))?;

    // Persist available_tools = tools
    let mut active = server.clone().into_active_model();
    active.available_tools = sea_orm::ActiveValue::Set(tools.clone().into());
    McpServerEntity::update(active)
        .exec(&db)
        .await
        .map_err(AppError::from)?;

    Ok(Json(McpTestResult { tools }))
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    fn test_key() -> cookie::Key {
        cookie::Key::from(&[0u8; 64])
    }

    fn make_app() -> Router {
        // make_app() ne monte que /mcp-servers/{id}/test (POST) — les routes CRUD
        // sont gérées désormais par resource_router.
        let config = crate::config::Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
            cookie_secret: "0".repeat(64),
            database_url: "postgres://localhost/test".to_string(),
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
            k8s_namespace: None,
            application_name: None,
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
        };

        let state = AppState {
            config,
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
        };

        Router::new()
            .route("/mcp-servers/{id}/test", axum::routing::post(test_server))
            .with_state(state)
    }

    #[test]
    fn build_domain_server_http_streamable() {
        let server = McpServerModel {
            id: 1,
            name: "test-server".to_string(),
            server_type: "http-streamable".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({"Authorization": "Bearer token"}),
            available_tools: serde_json::json!([]),
        };
        let result = build_domain_server(&server).unwrap();
        assert_eq!(result.name, "test-server");
        assert_eq!(result.transport, McpTransport::HttpStreamable);
        assert_eq!(result.url, "http://localhost:3000/mcp");
        assert_eq!(
            result.headers.get("Authorization"),
            Some(&"Bearer token".to_string())
        );
    }

    #[test]
    fn build_domain_server_sse_maps_to_sse_transport() {
        let server = McpServerModel {
            id: 2,
            name: "sse-server".to_string(),
            server_type: "sse".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
        };
        let result = build_domain_server(&server).expect("sse accepté");
        assert_eq!(result.transport, McpTransport::Sse);
    }

    #[test]
    fn build_domain_server_bogus_type_returns_error() {
        let server = McpServerModel {
            id: 3,
            name: "bogus-server".to_string(),
            server_type: "bogus".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
        };
        let err = build_domain_server(&server).unwrap_err();
        match err {
            AppError::McpError(msg) => {
                assert!(msg.contains("VNL-MCP-003"));
            }
            _ => panic!("expected McpError"),
        }
    }

    #[tokio::test]
    async fn test_server_without_cookie_returns_401() {
        let app = make_app();
        let req = Request::builder()
            .uri("/mcp-servers/1/test")
            .method("POST")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
