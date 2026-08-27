use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Conversation : une session de chat, rattachée à un owner et à un contexte.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_conversations")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub owner_id: i32,
    pub agent_id: Option<i32>,
    pub context_id: i32,
    pub title: Option<String>,
    pub todo: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
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
    fn conversation_roundtrip_serializes() {
        let conv = Model {
            id: 5,
            owner_id: 42,
            agent_id: Some(3),
            context_id: 1,
            title: Some("Build session".to_string()),
            todo: Some("Fix the linker".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&conv).expect("serialize Conversation");
        let deserialized: Model = serde_json::from_str(&json).expect("deserialize Conversation");

        assert_eq!(deserialized.id, 5);
        assert_eq!(deserialized.owner_id, 42);
        assert_eq!(deserialized.agent_id, Some(3));
        assert_eq!(deserialized.context_id, 1);
        assert_eq!(deserialized.title.as_deref(), Some("Build session"));
        assert_eq!(deserialized.todo.as_deref(), Some("Fix the linker"));
    }
}
