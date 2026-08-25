use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use miryad_core::auth::AuthUser;

use crate::{
    AppState, api::conversations::get_or_create_user,
    db::models::LlmProvider, error::AppError,
};

#[derive(Deserialize)]
pub struct CreateLlmProvider {
    pub name: String,
    pub provider_type: String,
    pub endpoint: String,
    pub api_key: Option<String>,
    pub is_default: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateLlmProvider {
    pub name: Option<String>,
    pub provider_type: Option<String>,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub is_default: Option<bool>,
}

#[derive(Serialize)]
pub struct TestResult {
    pub models: Vec<String>,
}

pub async fn list_providers(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<LlmProvider>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let providers = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(db_user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(providers))
}

pub async fn create_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateLlmProvider>,
) -> Result<(StatusCode, Json<LlmProvider>), AppError> {
    validate_provider_type(&body.provider_type)?;
    let db_user = get_or_create_user(&state, &user).await?;

    if body.is_default == Some(true) {
        sqlx::query(
            "UPDATE llm_providers SET is_default = FALSE, updated_at = NOW() WHERE user_id = $1",
        )
        .bind(db_user.id)
        .execute(&state.pool)
        .await?;
    }

    let provider = sqlx::query_as::<_, LlmProvider>(
        r"INSERT INTO llm_providers (user_id, name, provider_type, endpoint, api_key, is_default)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING *",
    )
    .bind(db_user.id)
    .bind(&body.name)
    .bind(&body.provider_type)
    .bind(&body.endpoint)
    .bind(&body.api_key)
    .bind(body.is_default.unwrap_or(false))
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(provider)))
}

pub async fn get_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<LlmProvider>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(db_user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LlmProviderNotFound)?;
    Ok(Json(provider))
}

pub async fn update_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLlmProvider>,
) -> Result<Json<LlmProvider>, AppError> {
    if let Some(ref t) = body.provider_type {
        validate_provider_type(t)?;
    }
    let db_user = get_or_create_user(&state, &user).await?;

    if body.is_default == Some(true) {
        sqlx::query(
            "UPDATE llm_providers SET is_default = FALSE, updated_at = NOW() WHERE user_id = $1 AND id != $2",
        )
        .bind(db_user.id)
        .bind(id)
        .execute(&state.pool)
        .await?;
    }

    let provider = sqlx::query_as::<_, LlmProvider>(
        r"UPDATE llm_providers SET
            name = COALESCE($3, name),
            provider_type = COALESCE($4, provider_type),
            endpoint = COALESCE($5, endpoint),
            api_key = COALESCE($6, api_key),
            is_default = COALESCE($7, is_default),
            updated_at = NOW()
           WHERE id = $1 AND user_id = $2
           RETURNING *",
    )
    .bind(id)
    .bind(db_user.id)
    .bind(&body.name)
    .bind(&body.provider_type)
    .bind(&body.endpoint)
    .bind(&body.api_key)
    .bind(body.is_default)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LlmProviderNotFound)?;

    Ok(Json(provider))
}

pub async fn delete_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let rows = sqlx::query("DELETE FROM llm_providers WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(db_user.id)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::LlmProviderNotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_default_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<LlmProvider>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;

    sqlx::query(
        "UPDATE llm_providers SET is_default = FALSE, updated_at = NOW() WHERE user_id = $1",
    )
    .bind(db_user.id)
    .execute(&state.pool)
    .await?;

    let provider = sqlx::query_as::<_, LlmProvider>(
        "UPDATE llm_providers SET is_default = TRUE, updated_at = NOW() WHERE id = $1 AND user_id = $2 RETURNING *",
    )
    .bind(id)
    .bind(db_user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LlmProviderNotFound)?;

    Ok(Json(provider))
}

pub async fn test_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<TestResult>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(db_user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LlmProviderNotFound)?;

    let models = discover_models(&provider).await?;

    let models_json = serde_json::to_value(&models)
        .map_err(|e| AppError::InternalError(format!("VNL-LLM-002: serialization error: {e}")))?;

    sqlx::query("UPDATE llm_providers SET available_models = $1, updated_at = NOW() WHERE id = $2")
        .bind(&models_json)
        .bind(id)
        .execute(&state.pool)
        .await?;

    Ok(Json(TestResult { models }))
}

async fn discover_models(provider: &LlmProvider) -> Result<Vec<String>, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::LlmError(format!("VNL-LLM-003: {e}")))?;

    match provider.provider_type.as_str() {
        "ollama" => {
            let url = format!("{}/api/tags", provider.endpoint.trim_end_matches('/'));
            let resp: serde_json::Value = client
                .get(&url)
                .send()
                .await
                .map_err(|e| AppError::LlmError(format!("VNL-LLM-004: {e}")))?
                .json()
                .await
                .map_err(|e| AppError::LlmError(format!("VNL-LLM-004: {e}")))?;

            let models = resp["models"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["name"].as_str().map(std::string::ToString::to_string))
                .collect();
            Ok(models)
        }
        "openai-compatible" => {
            let url = format!("{}/v1/models", provider.endpoint.trim_end_matches('/'));
            let mut req = client.get(&url);
            if let Some(ref key) = provider.api_key {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
            let resp: serde_json::Value = req
                .send()
                .await
                .map_err(|e| AppError::LlmError(format!("VNL-LLM-004: {e}")))?
                .json()
                .await
                .map_err(|e| AppError::LlmError(format!("VNL-LLM-004: {e}")))?;

            let models = resp["data"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|m| m["id"].as_str().map(std::string::ToString::to_string))
                .collect();
            Ok(models)
        }
        _ => Err(AppError::LlmError(format!(
            "VNL-LLM-005: unknown provider type: {}",
            provider.provider_type
        ))),
    }
}

fn validate_provider_type(t: &str) -> Result<(), AppError> {
    if t != "ollama" && t != "openai-compatible" {
        return Err(AppError::LlmError(format!(
            "VNL-LLM-005: provider_type must be 'ollama' or 'openai-compatible', got: {t}"
        )));
    }
    Ok(())
}

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
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
        };

        Router::new()
            .route("/llm-providers", axum::routing::get(list_providers))
            .with_state(state)
    }

    #[test]
    fn validate_provider_type_accepts_known() {
        assert!(validate_provider_type("ollama").is_ok());
        assert!(validate_provider_type("openai-compatible").is_ok());
    }

    #[test]
    fn validate_provider_type_rejects_unknown() {
        let err = validate_provider_type("bogus").unwrap_err();
        assert!(matches!(err, AppError::LlmError(_)));
    }

    #[tokio::test]
    async fn list_providers_without_cookie_returns_401() {
        let app = make_app();
        let req = Request::builder()
            .uri("/llm-providers")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
