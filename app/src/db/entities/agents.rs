use miryad_core::resource::{AccessPolicy, MiryadResource};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

fn default_auto() -> serde_json::Value {
    serde_json::json!("auto")
}

fn default_mode() -> String {
    "primary".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_agents")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub owner_id: i32,
    pub name: String,
    pub description: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: String,
    pub model_profile_id: i32,
    #[serde(default = "default_json_array")]
    pub toolsets: serde_json::Value,
    #[serde(default = "default_auto")]
    pub skills: serde_json::Value,
    #[serde(default)]
    pub system_prompt: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "agents"
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use miryad_core::resource::AccessPolicy;

    #[test]
    fn agent_record_resource_contract() {
        assert_eq!(Entity::resource_name(), "agents");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::OwnerOnly);
        assert!(
            matches!(Entity::owner_column(), Some(Column::OwnerId)),
            "owner_column should be Some(Column::OwnerId), got {:?}",
            Entity::owner_column()
        );
    }
}