use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::middleware::AdminAuth,
    db::models::LlmProvider,
    error::AppError,
    AppState,
};

#[derive(Deserialize)]
pub struct CreateLlmProvider {
    pub name: String,
    pub provider_type: String,
    pub endpoint: String,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub is_default: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateLlmProvider {
    pub name: Option<String>,
    pub provider_type: Option<String>,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub is_default: Option<bool>,
}

#[derive(Serialize)]
pub struct TestResult {
    pub models: Vec<String>,
}

pub async fn list_providers(
    State(state): State<AppState>,
    _admin: AdminAuth,
) -> Result<Json<Vec<LlmProvider>>, AppError> {
    let providers = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(providers))
}

pub async fn create_provider(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Json(body): Json<CreateLlmProvider>,
) -> Result<(StatusCode, Json<LlmProvider>), AppError> {
    validate_provider_type(&body.provider_type)?;

    if body.is_default == Some(true) {
        sqlx::query("UPDATE llm_providers SET is_default = FALSE, updated_at = NOW()")
            .execute(&state.pool)
            .await?;
    }

    let provider = sqlx::query_as::<_, LlmProvider>(
        r#"INSERT INTO llm_providers (name, provider_type, endpoint, api_key, default_model, is_default)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING *"#,
    )
    .bind(&body.name)
    .bind(&body.provider_type)
    .bind(&body.endpoint)
    .bind(&body.api_key)
    .bind(&body.default_model)
    .bind(body.is_default.unwrap_or(false))
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(provider)))
}

pub async fn get_provider(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<LlmProvider>, AppError> {
    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LlmProviderNotFound)?;
    Ok(Json(provider))
}

pub async fn update_provider(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLlmProvider>,
) -> Result<Json<LlmProvider>, AppError> {
    if let Some(ref t) = body.provider_type {
        validate_provider_type(t)?;
    }

    if body.is_default == Some(true) {
        sqlx::query("UPDATE llm_providers SET is_default = FALSE, updated_at = NOW() WHERE id != $1")
            .bind(id)
            .execute(&state.pool)
            .await?;
    }

    let provider = sqlx::query_as::<_, LlmProvider>(
        r#"UPDATE llm_providers SET
            name = COALESCE($2, name),
            provider_type = COALESCE($3, provider_type),
            endpoint = COALESCE($4, endpoint),
            api_key = COALESCE($5, api_key),
            default_model = COALESCE($6, default_model),
            is_default = COALESCE($7, is_default),
            updated_at = NOW()
           WHERE id = $1
           RETURNING *"#,
    )
    .bind(id)
    .bind(&body.name)
    .bind(&body.provider_type)
    .bind(&body.endpoint)
    .bind(&body.api_key)
    .bind(&body.default_model)
    .bind(body.is_default)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LlmProviderNotFound)?;

    Ok(Json(provider))
}

pub async fn delete_provider(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let rows = sqlx::query("DELETE FROM llm_providers WHERE id = $1")
        .bind(id)
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
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<LlmProvider>, AppError> {
    sqlx::query("UPDATE llm_providers SET is_default = FALSE, updated_at = NOW()")
        .execute(&state.pool)
        .await?;

    let provider = sqlx::query_as::<_, LlmProvider>(
        "UPDATE llm_providers SET is_default = TRUE, updated_at = NOW() WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LlmProviderNotFound)?;

    Ok(Json(provider))
}

pub async fn test_provider(
    State(state): State<AppState>,
    _admin: AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<TestResult>, AppError> {
    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::LlmProviderNotFound)?;

    let models = discover_models(&provider).await?;

    let models_json = serde_json::to_value(&models)
        .map_err(|e| AppError::InternalError(format!("VNL-LLM-002: serialization error: {e}")))?;

    sqlx::query(
        "UPDATE llm_providers SET available_models = $1, updated_at = NOW() WHERE id = $2",
    )
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
                .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
                .collect();
            Ok(models)
        }
        "openai-compatible" => {
            let url = format!("{}/v1/models", provider.endpoint.trim_end_matches('/'));
            let mut req = client.get(&url);
            if let Some(ref key) = provider.api_key {
                req = req.header("Authorization", format!("Bearer {}", key));
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
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
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
