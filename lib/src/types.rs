use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct MessageInner {
            role: String,
            content: String,
            tool_calls: Option<Vec<ToolCall>>,
        }
        let inner = MessageInner::deserialize(deserializer)?;
        Ok(Message {
            role: inner.role,
            content: inner.content,
            tool_calls: inner.tool_calls,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub messages: Vec<Message>,
    /// Etat todo (todowrite/todoread) porte par la conversation : serialisation
    /// JSON de la liste de taches. Persiste (resume via `-c/--continue`) — c'est
    /// la seule forme d'etat resumable en une-passe. `None` quand aucun etat
    /// todo n'a encore ete pose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<String>,
}
