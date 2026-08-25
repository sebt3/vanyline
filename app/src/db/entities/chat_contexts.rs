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
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chrono::Utc;

    #[test]
    fn chat_context_roundtrip_serializes() {
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
}