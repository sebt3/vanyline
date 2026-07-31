use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use vanyline_lib::domain::{McpSelection, Toolset};
use vanyline_lib::store::ConfigStore;
use vanyline_lib::VnyError;

use crate::{
    api::conversations::get_or_create_user, auth::middleware::AuthUser,
    config_store::PgConfigStore, error::AppError, AppState,
};

#[derive(Deserialize)]
pub struct CreateToolset {
    pub name: String,
    pub description: Option<String>,
    pub prompt: Option<String>,
    #[serde(default)]
    pub local_tools: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<McpSelection>,
}

#[derive(Deserialize)]
pub struct UpdateToolset {
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub local_tools: Option<Vec<String>>,
    pub mcp: Option<Vec<McpSelection>>,
}

pub async fn list_toolsets(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Toolset>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    Ok(Json(store.list_toolsets().await?))
}

pub async fn create_toolset(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateToolset>,
) -> Result<(StatusCode, Json<Toolset>), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    validate_mcp_servers(&state, db_user.id, &body.mcp).await?;

    sqlx::query(
        r#"INSERT INTO toolsets (user_id, name, description, prompt, local_tools, mcp)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
    )
    .bind(db_user.id)
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.prompt)
    .bind(serde_json::to_value(&body.local_tools).unwrap_or_else(|_| serde_json::json!([])))
    .bind(serde_json::to_value(&body.mcp).unwrap_or_else(|_| serde_json::json!([])))
    .execute(&state.pool)
    .await?;

    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let toolset = store
        .get_toolset(&body.name)
        .await
        .map_err(|_| AppError::ToolsetNotFound)?;
    Ok((StatusCode::CREATED, Json(toolset)))
}

pub async fn get_toolset(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<Toolset>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let toolset = store
        .get_toolset(&name)
        .await
        .map_err(|_| AppError::ToolsetNotFound)?;
    Ok(Json(toolset))
}

pub async fn update_toolset(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<UpdateToolset>,
) -> Result<Json<Toolset>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;

    if let Some(mcp) = &body.mcp {
        validate_mcp_servers(&state, db_user.id, mcp).await?;
    }

    let local_tools_json = body
        .local_tools
        .as_ref()
        .map(|v| serde_json::to_value(v).unwrap_or_else(|_| serde_json::json!([])));
    let mcp_json = body
        .mcp
        .as_ref()
        .map(|v| serde_json::to_value(v).unwrap_or_else(|_| serde_json::json!([])));

    let rows = sqlx::query(
        r#"UPDATE toolsets SET
            description = COALESCE($3, description),
            prompt = COALESCE($4, prompt),
            local_tools = COALESCE($5, local_tools),
            mcp = COALESCE($6, mcp),
            updated_at = NOW()
           WHERE user_id = $1 AND name = $2"#,
    )
    .bind(db_user.id)
    .bind(&name)
    .bind(&body.description)
    .bind(&body.prompt)
    .bind(local_tools_json)
    .bind(mcp_json)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(AppError::ToolsetNotFound);
    }

    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let toolset = store
        .get_toolset(&name)
        .await
        .map_err(|_| AppError::ToolsetNotFound)?;
    Ok(Json(toolset))
}

pub async fn delete_toolset(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let rows = sqlx::query("DELETE FROM toolsets WHERE user_id = $1 AND name = $2")
        .bind(db_user.id)
        .bind(&name)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::ToolsetNotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn validate_mcp_servers(
    state: &AppState,
    user_id: Uuid,
    mcp: &[McpSelection],
) -> Result<(), AppError> {
    for sel in mcp {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE user_id = $1 AND name = $2)",
        )
        .bind(user_id)
        .bind(&sel.server)
        .fetch_one(&state.pool)
        .await?;
        if !exists {
            return Err(AppError::UnprocessableReference(
                VnyError::UnknownReference("mcp_server", sel.server.clone()),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::MockOidcClient;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    fn test_key() -> cookie::Key {
        cookie::Key::from(&[0u8; 64])
    }

    fn make_app(cookie_key: cookie::Key) -> Router {
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
        };

        let state = AppState {
            config,
            oidc_client: std::sync::Arc::new(MockOidcClient),
            cookie_key,
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        };

        Router::new()
            .route("/toolsets", get(list_toolsets))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_toolsets_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/toolsets")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
