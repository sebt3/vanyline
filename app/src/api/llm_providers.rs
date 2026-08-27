use axum::{
    Json,
    extract::{Path, State},
};
use miryad_core::auth::AuthUser;
use miryad_core::rbac::can_write;
use miryad_core::users::resolve_user;
use sea_orm::{ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, sea_query::Expr};

use crate::{
    AppState, db::entities::llm_providers::Column,
    db::entities::llm_providers::Entity as LlmProviderEntity,
    db::entities::llm_providers::Model as LlmProviderModel, error::AppError,
};

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
        .map_err(AppError::from)?;
    let provider = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(AppError::from)?
        .ok_or(AppError::LlmProviderNotFound)?;
    if !can_write::<LlmProviderEntity>(&db, &principal_user, &provider)
        .await
        .map_err(AppError::from)?
    {
        return Err(AppError::Forbidden);
    }
    // Deux `UPDATE` bulk (pas un fetch-all + une boucle par ligne + deux re-fetches
    // redondants du même provider, cf. revue Phase 3 miryad-core-integration) : is_default =
    // FALSE partout, puis TRUE sur `id`. Le provider déjà en main (RBAC ci-dessus) est réutilisé
    // pour la réponse plutôt que refetché.
    LlmProviderEntity::update_many()
        .col_expr(Column::IsDefault, Expr::value(false))
        .exec(&db)
        .await
        .map_err(AppError::from)?;
    LlmProviderEntity::update_many()
        .col_expr(Column::IsDefault, Expr::value(true))
        .filter(Column::Id.eq(id))
        .exec(&db)
        .await
        .map_err(AppError::from)?;

    let mut result = provider;
    result.is_default = true;
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
        .map_err(AppError::from)?;
    let provider = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(AppError::from)?
        .ok_or(AppError::LlmProviderNotFound)?;
    if !can_write::<LlmProviderEntity>(&db, &principal_user, &provider)
        .await
        .map_err(AppError::from)?
    {
        return Err(AppError::Forbidden);
    }
    let models = discover_models(&provider).await?;

    // Persiste `available_models`, sans re-fetch mort après coup (la réponse ne porte que
    // `models`, cf. revue Phase 3 miryad-core-integration).
    let mut active = provider.into_active_model();
    active.available_models = sea_orm::ActiveValue::Set(models.clone().into());
    LlmProviderEntity::update(active)
        .exec(&db)
        .await
        .map_err(AppError::from)?;

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::db::entities::llm_providers::ActiveModel;
    use miryad_core::users::sync_group_memberships;
    use sea_orm::ActiveValue::{NotSet, Set};

    fn test_config() -> crate::config::Config {
        crate::config::Config {
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
        }
    }

    async fn test_state() -> AppState {
        let db = crate::db::test_support::real_db().await;
        AppState {
            config: test_config(),
            cookie_key: cookie::Key::from(&[0u8; 64]),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
        }
    }

    fn admin_user() -> AuthUser {
        AuthUser {
            subject: "admin-user".to_string(),
            email: None,
            id_token: "test-id-token".to_string(),
        }
    }

    /// Régression Phase 3 (miryad-core-integration) : `set_default_provider` doit désactiver
    /// l'ancien défaut et activer le nouveau via deux `UPDATE` bulk, sans dépendre d'une boucle
    /// par ligne ni de re-fetches redondants.
    #[tokio::test]
    async fn set_default_provider_switches_the_flag() {
        let state = test_state().await;
        let db = &state.auth.db;
        let admin = resolve_user(db, "admin-user", None)
            .await
            .expect("admin resolves");
        sync_group_memberships(db, admin.id, &["admin".to_string()])
            .await
            .expect("sync admin group");

        let provider_a = LlmProviderEntity::insert(ActiveModel {
            id: NotSet,
            name: Set("a".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(true),
        })
        .exec(db)
        .await
        .expect("provider a inserts")
        .last_insert_id;

        let provider_b = LlmProviderEntity::insert(ActiveModel {
            id: NotSet,
            name: Set("b".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(false),
        })
        .exec(db)
        .await
        .expect("provider b inserts")
        .last_insert_id;

        let Json(result) =
            set_default_provider(State(state.clone()), admin_user(), Path(provider_b))
                .await
                .expect("set_default_provider succeeds");
        assert!(result.is_default);

        let a = LlmProviderEntity::find_by_id(provider_a)
            .one(db)
            .await
            .expect("query a")
            .expect("a exists");
        assert!(!a.is_default, "previous default must be unset");
    }
}
