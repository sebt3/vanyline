use miryad_core::resource::{AccessPolicy, MiryadResource};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_object() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = ModelProfile)]
#[sea_orm(table_name = "vanyline_model_profiles")]
pub struct Model {
    // `id`/`owner_id` sans valeur côté client (`resource_router` désérialise le body en
    // `Model` tel quel — `create()`/`update()` les écrasent de toute façon avant écriture) :
    // `#[serde(default)]` évite un 400 "missing field" sur les payloads de création envoyés
    // par le frontend, qui n'incluent ni l'un ni l'autre.
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub owner_id: i32,
    pub name: String,
    pub provider_id: i32,
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    #[serde(default = "default_json_object")]
    pub options: serde_json::Value,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "model-profiles"
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
    fn model_profile_resource_contract() {
        assert_eq!(Entity::resource_name(), "model-profiles");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::OwnerOnly);
        // `Column` enum ne derive pas `PartialEq` (SeaORM 2) — on teste
        // la structure via `matches!` au lieu d'`assert_eq!`.
        assert!(
            matches!(Entity::owner_column(), Some(Column::OwnerId)),
            "owner_column should be Some(Column::OwnerId), got {:?}",
            Entity::owner_column()
        );
    }
}
