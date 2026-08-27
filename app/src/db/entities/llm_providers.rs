use miryad_core::auth::AuthPrincipal;
use miryad_core::resource::{AccessPolicy, HookError, MiryadResource};
use sea_orm::ActiveValue;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_llm_providers")]
pub struct Model {
    // Cf. commentaire équivalent dans `model_profiles.rs` : `#[serde(default)]` évite un 400
    // sur les payloads de création du frontend, qui n'envoie pas `id` (écrasé de toute façon
    // par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    pub name: String,
    pub provider_type: String,
    pub endpoint: String,
    pub api_key: Option<String>,
    #[serde(default = "default_json_array")]
    pub available_models: serde_json::Value,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "llm-providers"
    }
    fn read_policy() -> AccessPolicy {
        AccessPolicy::Public
    }
    fn write_policy() -> AccessPolicy {
        AccessPolicy::AdminOnly
    }
    fn owner_column() -> Option<Column> {
        None
    }

    /// Validation perdue lors de la bascule miryad-core (CHECK en base + `validate_provider_type`
    /// sur chaque create/update, cf. `docs/features/miryad-core-integration.md`) — restaurée ici
    /// pour la création (seule surface que `before_create` couvre ; pas d'équivalent
    /// `before_update` dans le trait, limite du framework, pas de cette entité).
    fn before_create(
        active: Self::ActiveModel,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        let provider_type = match &active.provider_type {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_str(),
            ActiveValue::NotSet => "",
        };
        if provider_type != "ollama" && provider_type != "openai-compatible" {
            return Err(HookError::with_code(
                "VNL-LLM-005",
                format!(
                    "provider_type must be 'ollama' or 'openai-compatible', got: {provider_type}"
                ),
            ));
        }
        Ok(active)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use miryad_core::resource::AccessPolicy;
    use sea_orm::IntoActiveModel;

    #[test]
    fn llm_provider_resource_contract() {
        assert_eq!(Entity::resource_name(), "llm-providers");
        assert_eq!(Entity::read_policy(), AccessPolicy::Public);
        assert_eq!(Entity::write_policy(), AccessPolicy::AdminOnly);
        assert!(Entity::owner_column().is_none());
    }

    fn sample(provider_type: &str) -> ActiveModel {
        Model {
            id: 0,
            name: "test".to_string(),
            provider_type: provider_type.to_string(),
            endpoint: "http://localhost:11434".to_string(),
            api_key: None,
            available_models: serde_json::json!([]),
            is_default: false,
        }
        .into_active_model()
    }

    fn test_principal() -> AuthPrincipal {
        AuthPrincipal {
            subject: "alice".to_string(),
            email: None,
            source: miryad_core::auth::PrincipalSource::ApiToken { token_id: 1 },
        }
    }

    #[test]
    fn before_create_accepts_known_provider_types() {
        assert!(Entity::before_create(sample("ollama"), &test_principal()).is_ok());
        assert!(Entity::before_create(sample("openai-compatible"), &test_principal()).is_ok());
    }

    #[test]
    fn before_create_rejects_unknown_provider_type() {
        let err = Entity::before_create(sample("bogus"), &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-LLM-005"));
    }
}
