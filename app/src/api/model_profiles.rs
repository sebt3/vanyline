use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use uuid::Uuid;

use vanyline_lib::VnyError;
use vanyline_lib::domain::ModelProfile;
use vanyline_lib::store::ConfigStore;

use miryad_core::auth::AuthUser;

use crate::{
    AppState, api::conversations::get_or_create_user, config_store::PgConfigStore, error::AppError,
};

#[derive(Deserialize)]
pub struct CreateModelProfile {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    pub options: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct UpdateModelProfile {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    pub options: Option<serde_json::Value>,
}

pub async fn list_model_profiles(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<ModelProfile>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    Ok(Json(store.list_models().await?))
}

pub async fn create_model_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateModelProfile>,
) -> Result<(StatusCode, Json<ModelProfile>), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let provider_id = resolve_provider_id(&state, db_user.id, &body.provider).await?;

    sqlx::query(
        r"INSERT INTO model_profiles (user_id, name, provider_id, model, temperature, max_tokens, options)
           VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(db_user.id)
    .bind(&body.name)
    .bind(provider_id)
    .bind(&body.model)
    .bind(body.temperature)
    .bind(body.max_tokens)
    .bind(body.options.unwrap_or_else(|| serde_json::json!({})))
    .execute(&state.pool)
    .await?;

    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let profile = store
        .get_model(&body.name)
        .await
        .map_err(|_| AppError::ModelProfileNotFound)?;
    Ok((StatusCode::CREATED, Json(profile)))
}

pub async fn get_model_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<ModelProfile>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let profile = store
        .get_model(&name)
        .await
        .map_err(|_| AppError::ModelProfileNotFound)?;
    Ok(Json(profile))
}

pub async fn update_model_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<UpdateModelProfile>,
) -> Result<Json<ModelProfile>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;

    let provider_id = match &body.provider {
        Some(p) => Some(resolve_provider_id(&state, db_user.id, p).await?),
        None => None,
    };

    let rows = sqlx::query(
        r"UPDATE model_profiles SET
            provider_id = COALESCE($3, provider_id),
            model = COALESCE($4, model),
            temperature = COALESCE($5, temperature),
            max_tokens = COALESCE($6, max_tokens),
            options = COALESCE($7, options),
            updated_at = NOW()
           WHERE user_id = $1 AND name = $2",
    )
    .bind(db_user.id)
    .bind(&name)
    .bind(provider_id)
    .bind(&body.model)
    .bind(body.temperature)
    .bind(body.max_tokens)
    .bind(body.options)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(AppError::ModelProfileNotFound);
    }

    let store = PgConfigStore::new(state.pool.clone(), db_user.id);
    let profile = store
        .get_model(&name)
        .await
        .map_err(|_| AppError::ModelProfileNotFound)?;
    Ok(Json(profile))
}

pub async fn delete_model_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let rows = sqlx::query("DELETE FROM model_profiles WHERE user_id = $1 AND name = $2")
        .bind(db_user.id)
        .bind(&name)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::ModelProfileNotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve_provider_id(
    state: &AppState,
    user_id: Uuid,
    provider_name: &str,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM llm_providers WHERE user_id = $1 AND name = $2")
        .bind(user_id)
        .bind(provider_name)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| {
            AppError::UnprocessableReference(VnyError::UnknownReference(
                "provider",
                provider_name.to_string(),
            ))
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
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
            cookie_key,
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
        };

        Router::new()
            .route("/model-profiles", get(list_model_profiles))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_model_profiles_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/model-profiles")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
