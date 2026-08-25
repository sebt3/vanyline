use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Message d'une conversation : rôle (user/assistant/system) + payload JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_messages")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub owner_id: i32,
    pub conversation_id: i32,
    pub role: String,
    pub payload: serde_json::Value,
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
    fn message_roundtrip_serializes() {
        let msg = Model {
            id: 7,
            owner_id: 42,
            conversation_id: 5,
            role: "assistant".to_string(),
            payload: serde_json::json!({"text": "Hello!", "attachments": []}),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&msg).expect("serialize Message");
        let deserialized: Model = serde_json::from_str(&json).expect("deserialize Message");

        assert_eq!(deserialized.id, 7);
        assert_eq!(deserialized.owner_id, 42);
        assert_eq!(deserialized.conversation_id, 5);
        assert_eq!(deserialized.role, "assistant");
        assert_eq!(deserialized.payload["text"].as_str(), Some("Hello!"));
    }
}