use miryad_core::resource::{AccessPolicy, MiryadResource};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_toolsets")]
pub struct Model {
    // Cf. commentaire équivalent dans `model_profiles.rs` : `#[serde(default)]` sur
    // `id`/`owner_id` évite un 400 sur les payloads de création du frontend, qui n'envoie
    // ni l'un ni l'autre (écrasés de toute façon par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub owner_id: i32,
    pub name: String,
    pub description: Option<String>,
    pub prompt: Option<String>,
    #[serde(default = "default_json_array")]
    pub local_tools: serde_json::Value,
    #[serde(default = "default_json_array")]
    pub mcp: serde_json::Value,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "toolsets"
    }
    fn read_policy() -> AccessPolicy {
        AccessPolicy::OwnerOnly
    }
    fn write_policy() -> AccessPolicy {
        AccessPolicy::OwnerOnly
    }
    fn owner_column() -> Option<Column> {
        Some(Column::OwnerId)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use miryad_core::resource::AccessPolicy;

    #[test]
    fn toolset_resource_contract() {
        assert_eq!(Entity::resource_name(), "toolsets");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::OwnerOnly);
        let oc = Entity::owner_column();
        assert!(
            matches!(oc, Some(Column::OwnerId)),
            "owner_column should be Some(Column::OwnerId), got {oc:?}"
        );
    }
}
