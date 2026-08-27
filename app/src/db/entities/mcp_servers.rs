use miryad_core::auth::AuthPrincipal;
use miryad_core::resource::{AccessPolicy, HookError, MiryadResource};
use sea_orm::ActiveValue;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_object() -> serde_json::Value {
    serde_json::json!({})
}

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_mcp_servers")]
pub struct Model {
    // Cf. commentaire équivalent dans `model_profiles.rs` : `#[serde(default)]` évite un 400
    // sur les payloads de création du frontend, qui n'envoie pas `id` (écrasé de toute façon
    // par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    pub name: String,
    pub server_type: String,
    pub url: String,
    #[serde(default = "default_json_object")]
    pub headers: serde_json::Value,
    #[serde(default = "default_json_array")]
    pub available_tools: serde_json::Value,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "mcp-servers"
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

    /// Validation perdue lors de la bascule miryad-core (CHECK en base + `validate_server_type`
    /// sur chaque create/update, cf. `docs/features/miryad-core-integration.md`) — restaurée ici
    /// pour la création (seule surface que `before_create` couvre).
    fn before_create(
        active: Self::ActiveModel,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        let server_type = match &active.server_type {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_str(),
            ActiveValue::NotSet => "",
        };
        if server_type != "sse" && server_type != "http-streamable" {
            return Err(HookError::with_code(
                "VNL-MCP-003",
                format!("server_type must be 'sse' or 'http-streamable', got: {server_type}"),
            ));
        }
        Ok(active)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use miryad_core::resource::AccessPolicy;
    use sea_orm::IntoActiveModel;

    #[test]
    fn mcp_server_resource_contract() {
        assert_eq!(Entity::resource_name(), "mcp-servers");
        assert_eq!(Entity::read_policy(), AccessPolicy::Public);
        assert_eq!(Entity::write_policy(), AccessPolicy::AdminOnly);
        assert!(Entity::owner_column().is_none());
    }

    fn sample(server_type: &str) -> ActiveModel {
        Model {
            id: 0,
            name: "test".to_string(),
            server_type: server_type.to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
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
    fn before_create_accepts_known_server_types() {
        assert!(Entity::before_create(sample("sse"), &test_principal()).is_ok());
        assert!(Entity::before_create(sample("http-streamable"), &test_principal()).is_ok());
    }

    #[test]
    fn before_create_rejects_unknown_server_type() {
        let err = Entity::before_create(sample("bogus"), &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-MCP-003"));
    }
}
