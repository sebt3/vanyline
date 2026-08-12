use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use vanyline_lib::domain::SkillMeta;
use vanyline_lib::store::ConfigStore;

use crate::{
    api::conversations::get_or_create_user, auth::middleware::AuthUser,
    config_store::PgConfigStore, error::AppError, AppState,
};

#[derive(Deserialize)]
pub struct CreateSkill {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Deserialize)]
pub struct UpdateSkill {
    pub description: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct SkillDetail {
    pub name: String,
    pub description: String,
    pub body: String,
}

pub async fn list_skills(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<SkillMeta>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    Ok(Json(store.list_skills().await?))
}

pub async fn create_skill(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateSkill>,
) -> Result<(StatusCode, Json<SkillDetail>), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let skill = sqlx::query_as::<_, SkillDetail>(
        r"INSERT INTO skills (user_id, name, description, body)
           VALUES ($1, $2, $3, $4)
           RETURNING name, description, body",
    )
    .bind(db_user.id)
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.body)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(skill)))
}

pub async fn get_skill(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<SkillDetail>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let skill = sqlx::query_as::<_, SkillDetail>(
        "SELECT name, description, body FROM skills WHERE user_id = $1 AND name = $2",
    )
    .bind(db_user.id)
    .bind(&name)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::SkillNotFound)?;
    Ok(Json(skill))
}

pub async fn update_skill(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<UpdateSkill>,
) -> Result<Json<SkillDetail>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let skill = sqlx::query_as::<_, SkillDetail>(
        r"UPDATE skills SET
            description = COALESCE($3, description),
            body = COALESCE($4, body),
            updated_at = NOW()
           WHERE user_id = $1 AND name = $2
           RETURNING name, description, body",
    )
    .bind(db_user.id)
    .bind(&name)
    .bind(&body.description)
    .bind(&body.body)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::SkillNotFound)?;
    Ok(Json(skill))
}

pub async fn delete_skill(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let rows = sqlx::query("DELETE FROM skills WHERE user_id = $1 AND name = $2")
        .bind(db_user.id)
        .bind(&name)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::SkillNotFound);
    }
    Ok(StatusCode::NO_CONTENT)
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
            .route("/skills", get(list_skills))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_skills_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/skills")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
