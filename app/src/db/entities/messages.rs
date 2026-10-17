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

    use super::*;
    use chrono::Utc;
    use sea_orm::{ActiveValue::NotSet, Iterable};

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

    #[test]
    fn message_forme_json_de_fil() {
        let created_at = Utc::now();
        let msg = Model {
            id: 7,
            owner_id: 42,
            conversation_id: 5,
            role: "assistant".to_string(),
            payload: serde_json::json!({"text": "Hello!", "attachments": []}),
            created_at,
        };

        let json = serde_json::to_value(&msg).expect("serialize Message");
        let obj = json.as_object().expect("message serialises as an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "conversation_id",
                "created_at",
                "id",
                "owner_id",
                "payload",
                "role"
            ]
        );
        assert!(obj["payload"].is_object(), "payload nests as an object");
        let stamp = obj["created_at"].as_str().expect("created_at is a string");
        let parsed = chrono::DateTime::parse_from_rfc3339(stamp)
            .expect("created_at is RFC 3339")
            .with_timezone(&Utc);
        assert_eq!(parsed, created_at);
    }

    #[test]
    fn message_aucune_relation_ni_hook() {
        assert_eq!(Relation::iter().count(), 0);
    }

    #[tokio::test]
    async fn message_insert_relecture_reelle() {
        use crate::db::test_support::real_db;
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};

        let db = real_db().await;
        let user_id = miryad_core::users::resolve_user(&db, "alice", None)
            .await
            .expect("user resolves")
            .id;
        let context_id = crate::db::entities::chat_contexts::Entity::insert(
            crate::db::entities::chat_contexts::ActiveModel {
                id: NotSet,
                kind: Set("home".to_string()),
                data: Set(serde_json::json!({})),
                created_at: Set(Utc::now()),
            },
        )
        .exec(&db)
        .await
        .expect("context inserts")
        .last_insert_id;
        let conversation_id = crate::db::entities::conversations::Entity::insert(
            crate::db::entities::conversations::ActiveModel {
                id: NotSet,
                owner_id: Set(user_id),
                agent_id: Set(None),
                context_id: Set(context_id),
                title: Set(None),
                todo: Set(None),
                created_at: Set(Utc::now()),
                updated_at: Set(Utc::now()),
            },
        )
        .exec(&db)
        .await
        .expect("conversation inserts")
        .last_insert_id;

        let payload = serde_json::json!({
            "role": "user",
            "content": "bonjour \u{e9}",
            "tool_calls": []
        });
        let inserted = ActiveModel {
            id: NotSet,
            owner_id: Set(user_id),
            conversation_id: Set(conversation_id),
            role: Set("user".to_string()),
            payload: Set(payload.clone()),
            created_at: Set(Utc::now()),
        }
        .insert(&db)
        .await
        .expect("message inserts");

        assert_ne!(inserted.id, 0, "la base attribue un id non nul");
        let read = Entity::find_by_id(inserted.id)
            .one(&db)
            .await
            .expect("message query runs")
            .expect("message is found");
        assert_eq!(read.owner_id, user_id);
        assert_eq!(read.conversation_id, conversation_id);
        assert_eq!(read.role, "user");
        assert_eq!(read.payload, payload);
    }
}
