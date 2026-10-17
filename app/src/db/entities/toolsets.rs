use miryad_core::resource::{AccessPolicy, MiryadResource};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = Toolset)]
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

    use super::*;
    use miryad_core::resource::AccessPolicy;
    use sea_orm::Iterable;

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

    #[test]
    fn minimal_creation_payload_deserializes_with_defaults() {
        let model: Model =
            serde_json::from_str(r#"{"name":"fs"}"#).expect("minimal payload deserialises");
        assert_eq!(model.id, 0);
        assert_eq!(model.owner_id, 0);
        assert_eq!(model.description, None);
        assert_eq!(model.prompt, None);
        assert_eq!(model.local_tools, serde_json::json!([]));
        assert_eq!(model.mcp, serde_json::json!([]));
    }

    #[test]
    fn model_serializes_with_exact_snake_case_keys() {
        let model = Model {
            id: 3,
            owner_id: 9,
            name: "web".to_string(),
            description: Some("navigation".to_string()),
            prompt: Some("sois bref".to_string()),
            local_tools: serde_json::json!(["fs"]),
            mcp: serde_json::json!([{"server": "github"}]),
        };
        let json = serde_json::to_value(&model).expect("serialize toolset");
        let obj = json.as_object().expect("toolset serialises as an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "description",
                "id",
                "local_tools",
                "mcp",
                "name",
                "owner_id",
                "prompt"
            ]
        );
    }

    #[test]
    fn entity_declares_no_relations() {
        assert_eq!(Relation::iter().count(), 0);
    }
}
