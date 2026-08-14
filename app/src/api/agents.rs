use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use vanyline_lib::domain::{Agent, AgentMode, SkillSelection};
use vanyline_lib::store::ConfigStore;
use vanyline_lib::VnyError;

use crate::{
    api::conversations::get_or_create_user, auth::middleware::AuthUser,
    config_store::PgConfigStore, error::AppError, AppState,
};

const fn default_mode() -> AgentMode {
    AgentMode::Primary
}

const fn mode_to_str(mode: &AgentMode) -> &'static str {
    match mode {
        AgentMode::Primary => "primary",
        AgentMode::Subagent => "subagent",
        AgentMode::All => "all",
    }
}

#[derive(Deserialize)]
pub struct CreateAgent {
    pub name: String,
    pub description: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: AgentMode,
    pub model: String,
    #[serde(default)]
    pub toolsets: Vec<String>,
    #[serde(default)]
    pub skills: SkillSelection,
    #[serde(default)]
    pub system_prompt: String,
}

#[derive(Deserialize)]
pub struct UpdateAgent {
    pub description: Option<String>,
    pub mode: Option<AgentMode>,
    pub model: Option<String>,
    pub toolsets: Option<Vec<String>>,
    pub skills: Option<SkillSelection>,
    pub system_prompt: Option<String>,
}

pub async fn list_agents(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Agent>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    Ok(Json(store.list_agents().await?))
}

pub async fn create_agent(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateAgent>,
) -> Result<(StatusCode, Json<Agent>), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let model_profile_id = resolve_model_profile_id(&state, db_user.id, &body.model).await?;
    validate_toolsets(&state, db_user.id, &body.toolsets).await?;
    validate_skills(&state, db_user.id, &body.skills).await?;

    sqlx::query(
        r"INSERT INTO agents (user_id, name, description, mode, model_profile_id, toolsets, skills, system_prompt)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(db_user.id)
    .bind(&body.name)
    .bind(&body.description)
    .bind(mode_to_str(&body.mode))
    .bind(model_profile_id)
    .bind(serde_json::to_value(&body.toolsets).unwrap_or_else(|_| serde_json::json!([])))
    .bind(serde_json::to_value(&body.skills).unwrap_or_else(|_| serde_json::json!("auto")))
    .bind(&body.system_prompt)
    .execute(&state.pool)
    .await?;

    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let agent = store
        .get_agent(&body.name)
        .await
        .map_err(|_| AppError::AgentNotFound)?;
    Ok((StatusCode::CREATED, Json(agent)))
}

pub async fn get_agent(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<Agent>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let agent = store
        .get_agent(&name)
        .await
        .map_err(|_| AppError::AgentNotFound)?;
    Ok(Json(agent))
}

pub async fn update_agent(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<UpdateAgent>,
) -> Result<Json<Agent>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;

    let model_profile_id = match &body.model {
        Some(m) => Some(resolve_model_profile_id(&state, db_user.id, m).await?),
        None => None,
    };
    if let Some(toolsets) = &body.toolsets {
        validate_toolsets(&state, db_user.id, toolsets).await?;
    }
    if let Some(skills) = &body.skills {
        validate_skills(&state, db_user.id, skills).await?;
    }

    let mode_str = body.mode.as_ref().map(mode_to_str);
    let toolsets_json = body
        .toolsets
        .as_ref()
        .map(|v| serde_json::to_value(v).unwrap_or_else(|_| serde_json::json!([])));
    let skills_json = body
        .skills
        .as_ref()
        .map(|v| serde_json::to_value(v).unwrap_or_else(|_| serde_json::json!("auto")));

    let rows = sqlx::query(
        r"UPDATE agents SET
            description = COALESCE($3, description),
            mode = COALESCE($4, mode),
            model_profile_id = COALESCE($5, model_profile_id),
            toolsets = COALESCE($6, toolsets),
            skills = COALESCE($7, skills),
            system_prompt = COALESCE($8, system_prompt),
            updated_at = NOW()
           WHERE user_id = $1 AND name = $2",
    )
    .bind(db_user.id)
    .bind(&name)
    .bind(&body.description)
    .bind(mode_str)
    .bind(model_profile_id)
    .bind(toolsets_json)
    .bind(skills_json)
    .bind(&body.system_prompt)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(AppError::AgentNotFound);
    }

    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let agent = store
        .get_agent(&name)
        .await
        .map_err(|_| AppError::AgentNotFound)?;
    Ok(Json(agent))
}

pub async fn delete_agent(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let rows = sqlx::query("DELETE FROM agents WHERE user_id = $1 AND name = $2")
        .bind(db_user.id)
        .bind(&name)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::AgentNotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve_model_profile_id(
    state: &AppState,
    user_id: Uuid,
    model_name: &str,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM model_profiles WHERE user_id = $1 AND name = $2")
        .bind(user_id)
        .bind(model_name)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| {
            AppError::UnprocessableReference(VnyError::UnknownReference(
                "model",
                model_name.to_string(),
            ))
        })
}

async fn validate_toolsets(
    state: &AppState,
    user_id: Uuid,
    names: &[String],
) -> Result<(), AppError> {
    for name in names {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM toolsets WHERE user_id = $1 AND name = $2)",
        )
        .bind(user_id)
        .bind(name)
        .fetch_one(&state.pool)
        .await?;
        if !exists {
            return Err(AppError::UnprocessableReference(
                VnyError::UnknownReference("toolset", name.clone()),
            ));
        }
    }
    Ok(())
}

async fn validate_skills(
    state: &AppState,
    user_id: Uuid,
    skills: &SkillSelection,
) -> Result<(), AppError> {
    let SkillSelection::Named(names) = skills else {
        return Ok(()); // Auto / None : rien à valider, même règle que cli/src/config_check.rs
    };
    for name in names {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM skills WHERE user_id = $1 AND name = $2)",
        )
        .bind(user_id)
        .bind(name)
        .fetch_one(&state.pool)
        .await?;
        if !exists {
            return Err(AppError::UnprocessableReference(
                VnyError::UnknownReference("skill", name.clone()),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
            k8s_namespace: None,
            application_name: None,
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
        };

        let state = AppState {
            config,
            oidc_client: std::sync::Arc::new(MockOidcClient),
            cookie_key,
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        };

        Router::new()
            .route("/agents", get(list_agents))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_agents_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/agents")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
