use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::middleware::{AdminAuth, AuthUser},
    db::models::{Agent, AgentRow, McpServer},
    error::AppError,
    AppState,
};

#[derive(Deserialize)]
pub struct CreateAgent {
    pub name: String,
    pub description: Option<String>,
    pub system_prompt: Option<String>,
    pub llm_provider_id: Option<Uuid>,
    pub model: Option<String>,
    pub mcp_server_ids: Option<Vec<Uuid>>,
}

#[derive(Deserialize)]
pub struct UpdateAgent {
    pub name: Option<String>,
    pub description: Option<String>,
    pub system_prompt: Option<String>,
    pub llm_provider_id: Option<Uuid>,
    pub model: Option<String>,
    pub mcp_server_ids: Option<Vec<Uuid>>,
}

pub async fn list_agents(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<Json<Vec<Agent>>, AppError> {
    let rows = sqlx::query_as::<_, AgentRow>("SELECT * FROM agents ORDER BY created_at DESC")
        .fetch_all(&state.pool)
        .await?;

    let mut agents = Vec::with_capacity(rows.len());
    for row in rows {
        let mcp_servers = fetch_agent_mcp_servers(&state, row.id).await?;
        agents.push(row.into_agent(mcp_servers));
    }
    Ok(Json(agents))
}

pub async fn create_agent(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Json(body): Json<CreateAgent>,
) -> Result<(StatusCode, Json<Agent>), AppError> {
    let row = sqlx::query_as::<_, AgentRow>(
        r#"INSERT INTO agents (name, description, system_prompt, llm_provider_id, model)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING *"#,
    )
    .bind(&body.name)
    .bind(&body.description)
    .bind(body.system_prompt.as_deref().unwrap_or(""))
    .bind(body.llm_provider_id)
    .bind(&body.model)
    .fetch_one(&state.pool)
    .await?;

    if let Some(ref ids) = body.mcp_server_ids {
        set_agent_mcp_servers(&state, row.id, ids).await?;
    }

    let mcp_servers = fetch_agent_mcp_servers(&state, row.id).await?;
    Ok((StatusCode::CREATED, Json(row.into_agent(mcp_servers))))
}

pub async fn get_agent(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Agent>, AppError> {
    let row = sqlx::query_as::<_, AgentRow>("SELECT * FROM agents WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::AgentNotFound)?;

    let mcp_servers = fetch_agent_mcp_servers(&state, row.id).await?;
    Ok(Json(row.into_agent(mcp_servers)))
}

pub async fn update_agent(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAgent>,
) -> Result<Json<Agent>, AppError> {
    let row = sqlx::query_as::<_, AgentRow>(
        r#"UPDATE agents SET
            name = COALESCE($2, name),
            description = COALESCE($3, description),
            system_prompt = COALESCE($4, system_prompt),
            llm_provider_id = COALESCE($5, llm_provider_id),
            model = COALESCE($6, model),
            updated_at = NOW()
           WHERE id = $1
           RETURNING *"#,
    )
    .bind(id)
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.system_prompt)
    .bind(body.llm_provider_id)
    .bind(&body.model)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::AgentNotFound)?;

    if let Some(ref ids) = body.mcp_server_ids {
        set_agent_mcp_servers(&state, id, ids).await?;
    }

    let mcp_servers = fetch_agent_mcp_servers(&state, row.id).await?;
    Ok(Json(row.into_agent(mcp_servers)))
}

pub async fn delete_agent(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let rows = sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::AgentNotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn fetch_agent_mcp_servers(
    state: &AppState,
    agent_id: Uuid,
) -> Result<Vec<McpServer>, AppError> {
    let servers = sqlx::query_as::<_, McpServer>(
        r#"SELECT m.* FROM mcp_servers m
           JOIN agent_mcp_servers ams ON ams.mcp_server_id = m.id
           WHERE ams.agent_id = $1
           ORDER BY m.name"#,
    )
    .bind(agent_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(servers)
}

async fn set_agent_mcp_servers(
    state: &AppState,
    agent_id: Uuid,
    mcp_server_ids: &[Uuid],
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM agent_mcp_servers WHERE agent_id = $1")
        .bind(agent_id)
        .execute(&state.pool)
        .await?;

    for mcp_id in mcp_server_ids {
        sqlx::query(
            "INSERT INTO agent_mcp_servers (agent_id, mcp_server_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(agent_id)
        .bind(mcp_id)
        .execute(&state.pool)
        .await?;
    }
    Ok(())
}
