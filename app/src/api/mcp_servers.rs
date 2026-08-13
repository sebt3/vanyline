use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use uuid::Uuid;

use vanyline_lib::domain::McpServer as DomainMcpServer;
use vanyline_lib::domain::McpTransport;
use vanyline_lib::VnyError;

use crate::{
    api::conversations::get_or_create_user, auth::middleware::AuthUser, db::models::McpServer,
    error::AppError, AppState,
};

#[derive(Deserialize)]
pub struct CreateMcpServer {
    pub name: String,
    pub server_type: String,
    pub url: String,
    pub headers: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct UpdateMcpServer {
    pub name: Option<String>,
    pub server_type: Option<String>,
    pub url: Option<String>,
    pub headers: Option<serde_json::Value>,
}

pub async fn list_servers(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<McpServer>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let servers = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(db_user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(servers))
}

pub async fn create_server(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateMcpServer>,
) -> Result<(StatusCode, Json<McpServer>), AppError> {
    validate_server_type(&body.server_type)?;
    let db_user = get_or_create_user(&state, &user).await?;

    let server = sqlx::query_as::<_, McpServer>(
        r"INSERT INTO mcp_servers (user_id, name, server_type, url, headers)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING *",
    )
    .bind(db_user.id)
    .bind(&body.name)
    .bind(&body.server_type)
    .bind(&body.url)
    .bind(body.headers.unwrap_or_else(|| serde_json::json!({})))
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(server)))
}

pub async fn get_server(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<McpServer>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let server =
        sqlx::query_as::<_, McpServer>("SELECT * FROM mcp_servers WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(db_user.id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::McpError(
                "VNL-MCP-002: MCP server not found".to_string(),
            ))?;
    Ok(Json(server))
}

pub async fn update_server(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMcpServer>,
) -> Result<Json<McpServer>, AppError> {
    if let Some(ref t) = body.server_type {
        validate_server_type(t)?;
    }
    let db_user = get_or_create_user(&state, &user).await?;

    let server = sqlx::query_as::<_, McpServer>(
        r"UPDATE mcp_servers SET
            name = COALESCE($3, name),
            server_type = COALESCE($4, server_type),
            url = COALESCE($5, url),
            headers = COALESCE($6, headers),
            updated_at = NOW()
           WHERE id = $1 AND user_id = $2
           RETURNING *",
    )
    .bind(id)
    .bind(db_user.id)
    .bind(&body.name)
    .bind(&body.server_type)
    .bind(&body.url)
    .bind(&body.headers)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::McpError(
        "VNL-MCP-002: MCP server not found".to_string(),
    ))?;

    Ok(Json(server))
}

pub async fn delete_server(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let rows = sqlx::query("DELETE FROM mcp_servers WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(db_user.id)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::McpError(
            "VNL-MCP-002: MCP server not found".to_string(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

fn validate_server_type(t: &str) -> Result<(), AppError> {
    if t != "sse" && t != "http-streamable" {
        return Err(AppError::McpError(format!(
            "VNL-MCP-003: server_type must be 'sse' or 'http-streamable', got: {t}"
        )));
    }
    Ok(())
}

#[derive(serde::Serialize)]
pub struct McpTestResult {
    pub tools: Vec<String>,
}

/// Mappe le modèle DB sur le type domaine `lib`. `server_type: "sse"` -> erreur
/// claire (VNL-MCP-004, transport SSE non implémenté — limite pré-existante,
/// pas corrigée dans cette feature). Pure, testable.
pub fn build_domain_server(server: &McpServer) -> Result<DomainMcpServer, AppError> {
    let transport = match server.server_type.as_str() {
        "http-streamable" => McpTransport::HttpStreamable,
        "sse" => return Err(AppError::McpError(VnyError::SseNotImplemented.to_string())),
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
        _ => BTreeMap::new(),
    };
    Ok(DomainMcpServer {
        name: server.name.clone(),
        transport,
        url: server.url.clone(),
        headers,
    })
}

pub async fn test_server(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<McpTestResult>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(db_user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| {
        AppError::McpError("VNL-MCP-002: MCP server not found".to_string())
    })?;

    let domain = build_domain_server(&server)?;
    let tools = vanyline_lib::prefixed_mcp::list_mcp_server_tools(&domain)
        .await
        .map_err(|e| AppError::McpError(e.to_string()))?;

    let tools_json = serde_json::to_value(&tools)
        .map_err(|e| AppError::InternalError(format!("VNL-MCP-007: serialization error: {e}")))?;

    sqlx::query(
        "UPDATE mcp_servers SET available_tools = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(&tools_json)
    .bind(id)
    .execute(&state.pool)
    .await?;

    Ok(Json(McpTestResult { tools }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::auth::MockOidcClient;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        Router,
    };
    use tower::ServiceExt;

    fn test_key() -> cookie::Key {
        cookie::Key::from(&[0u8; 64])
    }

    fn make_app() -> Router {
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
        };

        let state = AppState {
            config,
            oidc_client: std::sync::Arc::new(MockOidcClient),
            cookie_key: test_key(),
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        };

        Router::new()
            .route("/mcp-servers", axum::routing::get(list_servers))
            .route(
                "/mcp-servers/{id}/test",
                axum::routing::post(test_server),
            )
            .with_state(state)
    }

    #[test]
    fn validate_server_type_accepts_known() {
        assert!(validate_server_type("sse").is_ok());
        assert!(validate_server_type("http-streamable").is_ok());
    }

    #[test]
    fn validate_server_type_rejects_unknown() {
        let err = validate_server_type("bogus").unwrap_err();
        assert!(matches!(err, AppError::McpError(_)));
    }

    #[tokio::test]
    async fn list_servers_without_cookie_returns_401() {
        let app = make_app();
        let req = Request::builder()
            .uri("/mcp-servers")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn build_domain_server_http_streamable() {
        let server = McpServer {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            name: "test-server".to_string(),
            server_type: "http-streamable".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({"Authorization": "Bearer token"}),
            available_tools: serde_json::json!([]),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let result = build_domain_server(&server).unwrap();
        assert_eq!(result.name, "test-server");
        assert_eq!(result.transport, McpTransport::HttpStreamable);
        assert_eq!(result.url, "http://localhost:3000/mcp");
        assert_eq!(result.headers.get("Authorization"), Some(&"Bearer token".to_string()));
    }

    #[test]
    fn build_domain_server_sse_returns_error() {
        let server = McpServer {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            name: "test-server".to_string(),
            server_type: "sse".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let err = build_domain_server(&server).unwrap_err();
        match err {
            AppError::McpError(msg) => {
                assert!(msg.contains("VNL-MCP-004"));
            }
            _ => panic!("expected McpError"),
        }
    }

    #[test]
    fn build_domain_server_bogus_type_returns_error() {
        let server = McpServer {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            name: "test-server".to_string(),
            server_type: "bogus".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
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
            .uri("/mcp-servers/00000000-0000-0000-0000-000000000000/test")
            .method("POST")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
