use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Contexte d'une conversation : décrit la nature du contexte
/// (ex: `"sandbox"` avec `data` contenant les infos du sandbox).
/// Sans colonne `owner_id` — le contexte est réutilisable par plusieurs
/// conversations d'un même owner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_chat_contexts")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub kind: String,
    pub data: serde_json::Value,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use super::*;
    use chrono::Utc;
    use sea_orm::Iterable;

    #[test]
    fn chat_context_model_roundtrip_serde() {
        let ctx = Model {
            id: 1,
            kind: "sandbox".to_string(),
            data: serde_json::json!({"sandbox_name": "my-sandbox"}),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&ctx).expect("serialize ChatContext");
        let deserialized: Model = serde_json::from_str(&json).expect("deserialize ChatContext");

        assert_eq!(deserialized.id, 1);
        assert_eq!(deserialized.kind, "sandbox");
        assert_eq!(
            deserialized.data["sandbox_name"].as_str(),
            Some("my-sandbox")
        );
        // created_at may differ due to serialization — just check it's valid
        assert!(deserialized.created_at.timestamp() > 0);
    }

    #[test]
    fn chat_context_model_se_serialize_en_quatre_cles_snake_case() {
        let ctx = Model {
            id: 7,
            kind: "sandbox".to_string(),
            data: serde_json::json!({"sandbox_name": "sb"}),
            created_at: "2026-08-25T00:00:00Z"
                .parse()
                .expect("fixed RFC 3339 timestamp"),
        };
        let json = serde_json::to_value(&ctx).expect("serialize ChatContext");
        let obj = json.as_object().expect("context serialises as an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["created_at", "data", "id", "kind"]);
        let stamp = obj["created_at"].as_str().expect("created_at is a string");
        assert!(
            chrono::DateTime::parse_from_rfc3339(stamp).is_ok(),
            "created_at is RFC 3339, got {stamp}"
        );
    }

    #[test]
    fn chat_context_relation_enum_vide() {
        assert_eq!(Relation::iter().count(), 0);
    }
}
