use axum::{
    Json,
    extract::{Path, State},
};
use miryad_core::auth::AuthUser;
use miryad_core::rbac::can_write;
use miryad_core::users::resolve_user;
use sea_orm::{EntityTrait, IntoActiveModel};

use crate::{
    AppState, db::entities::llm_providers::Entity as LlmProviderEntity,
    db::entities::llm_providers::Model as LlmProviderModel, error::AppError,
};

fn db_err(e: sea_orm::DbErr) -> AppError {
    AppError::InternalError(format!("VNL-DB-006: {e}"))
}

#[derive(serde::Serialize)]
pub struct TestResult {
    pub models: Vec<String>,
}

pub async fn set_default_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<LlmProviderModel>, AppError> {
    let db = state.auth.db.clone();
    let principal_user = resolve_user(&db, &user.subject, user.email.as_deref())
        .await
        .map_err(db_err)?;
    let provider = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(AppError::LlmProviderNotFound)?;
    if !can_write::<LlmProviderEntity>(&db, &principal_user, &provider)
        .await
        .map_err(db_err)?
    {
        return Err(AppError::Forbidden);
    }
    // Comportement : is_default = FALSE sur tous les providers, puis is_default = TRUE
    // sur `id` (opérations SeaORM via EntityTrait/ActiveModelTrait), puis retourne le
    // provider mis à jour.

    let all = LlmProviderEntity::find().all(&db).await.map_err(db_err)?;
    for p in all {
        let mut active = p.into_active_model();
        active.is_default = sea_orm::ActiveValue::Set(false);
        LlmProviderEntity::update(active).exec(&db).await.map_err(db_err)?;
    }

    let updated = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(AppError::LlmProviderNotFound)?;
    let mut active = updated.into_active_model();
    active.is_default = sea_orm::ActiveValue::Set(true);
    let _saved = LlmProviderEntity::update(active).exec(&db).await.map_err(db_err)?;
    let result = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(AppError::LlmProviderNotFound)?;

    Ok(Json(result))
}

pub async fn test_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<TestResult>, AppError> {
    let db = state.auth.db.clone();
    let principal_user = resolve_user(&db, &user.subject, user.email.as_deref())
        .await
        .map_err(db_err)?;
    let provider = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(AppError::LlmProviderNotFound)?;
    if !can_write::<LlmProviderEntity>(&db, &principal_user, &provider)
        .await
        .map_err(db_err)?
    {
        return Err(AppError::Forbidden);
    }
    let models = discover_models(&provider).await?;
    // Comportement : persiste available_models = models sur `id` (SeaORM), retourne
    // TestResult { models }.

    let mut active = provider.into_active_model();
    active.available_models = sea_orm::ActiveValue::Set(models.clone().into());
    LlmProviderEntity::update(active).exec(&db).await.map_err(db_err)?;

    let _result = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(db_err)?
        .ok_or(AppError::LlmProviderNotFound)?;

    Ok(Json(TestResult { models }))
}

async fn discover_models(provider: &LlmProviderModel) -> Result<Vec<String>, AppError> {
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