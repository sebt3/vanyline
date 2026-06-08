use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::middleware::AdminAuth,
    db::models::McpServer,
    error::AppError,
    AppState,
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
    _admin: AdminAuth,
) -> Result<Json<Vec<McpServer>>, AppError> {
    let servers = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(servers))
}

pub async fn create_server(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Json(body): Json<CreateMcpServer>,
) -> Result<(StatusCode, Json<McpServer>), AppError> {
    validate_server_type(&body.server_type)?;

    let server = sqlx::query_as::<_, McpServer>(
        r#"INSERT INTO mcp_servers (name, server_type, url, headers)
           VALUES ($1, $2, $3, $4)
           RETURNING *"#,
    )
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
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<McpServer>, AppError> {
    let server = sqlx::query_as::<_, McpServer>("SELECT * FROM mcp_servers WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::McpError("VNL-MCP-002: MCP server not found".to_string()))?;
    Ok(Json(server))
}

pub async fn update_server(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMcpServer>,
) -> Result<Json<McpServer>, AppError> {
    if let Some(ref t) = body.server_type {
        validate_server_type(t)?;
    }

    let server = sqlx::query_as::<_, McpServer>(
        r#"UPDATE mcp_servers SET
            name = COALESCE($2, name),
            server_type = COALESCE($3, server_type),
            url = COALESCE($4, url),
            headers = COALESCE($5, headers),
            updated_at = NOW()
           WHERE id = $1
           RETURNING *"#,
    )
    .bind(id)
    .bind(&body.name)
    .bind(&body.server_type)
    .bind(&body.url)
    .bind(&body.headers)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::McpError("VNL-MCP-002: MCP server not found".to_string()))?;

    Ok(Json(server))
}

pub async fn delete_server(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let rows = sqlx::query("DELETE FROM mcp_servers WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::McpError("VNL-MCP-002: MCP server not found".to_string()));
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
