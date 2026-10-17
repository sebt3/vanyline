use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Tool call résolu, forme de PERSISTANCE des conversations (cf. `./types.sdd`).
/// Volontairement sans champ `id` : la corrélation call/result n'a de sens que
/// pendant le tour ; le type de transport avec `id` est
/// [`crate::event::ToolCallRecord`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Nom du tool.
    pub name: String,
    /// Arguments du call — tout JSON accepté.
    pub arguments: serde_json::Value,
    /// Résultat du call ; `None` se sérialise `"result": null` (clé toujours
    /// présente — asymétrie assumée avec [`Message::tool_calls`]).
    pub result: Option<String>,
}

/// Message persisté d'une conversation (forme de stockage, cf. `./types.sdd`).
/// Le vocabulaire d'usage des rôles est `"user"` / `"assistant"`, imposé par
/// les consommateurs, jamais par le type (une chaîne hors vocabulaire est
/// acceptée et persistée).
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    /// Rôle du message (`"user"` / `"assistant"` d'usage, chaîne libre en droit).
    pub role: String,
    /// Contenu texte du message.
    pub content: String,
    /// Tool calls portés ; omis du JSON produit quand `None`
    /// (`skip_serializing_if`), absent OU `null` rendent `None` en lecture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Désérialisation manuelle, strictement équivalente à un derive (struct
/// intermédiaire `MessageInner` aux mêmes champs, sans attribut) — vestige
/// tracé en `[?]` de `./types.sdd`.
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

/// Conversation persistée (fichier JSON CLI `conversations/{id}.json`, payload
/// des messages de l'`app`) — cf. `./types.sdd`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    /// Identifiant UUID v4 ; sérialisé en chaîne canonique (tirets, minuscules)
    /// — forme des noms de fichiers CLI et des résolutions par préfixe.
    pub id: Uuid,
    /// Agent de la conversation ; sérialisé `"agent": null` quand `None`.
    pub agent: Option<String>,
    /// Titre ; mêmes règles serde que [`Conversation::agent`].
    pub title: Option<String>,
    /// Messages — clé TOUJOURS requise en lecture ; un vecteur vide est légal.
    pub messages: Vec<Message>,
    /// Etat todo (todowrite/todoread) porte par la conversation : serialisation
    /// JSON de la liste de taches. Persiste (resume via `-c/--continue`) — c'est
    /// la seule forme d'etat resumable en une-passe. `None` quand aucun etat
    /// todo n'a encore ete pose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn conversation_json(todo: Option<&str>) -> Value {
        let mut v = json!({
            "id": "aaaaaaaa-0000-0000-0000-000000000001",
            "agent": null,
            "title": null,
            "messages": []
        });
        if let Some(t) = todo {
            v["todo"] = json!(t);
        }
        v
    }

    /// Scenario `tool_call_se_serialize_avec_result_null_et_sans_id`.
    #[test]
    fn tool_call_se_serialize_avec_result_null_et_sans_id() {
        let call = ToolCall {
            name: "read_file".into(),
            arguments: json!({"path": "a"}),
            result: None,
        };
        let v = serde_json::to_value(&call).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["arguments", "name", "result"]);
        assert_eq!(v["result"], Value::Null);
    }

    /// Scenario `tool_call_roundtrip_json`.
    #[test]
    fn tool_call_roundtrip_json() {
        let call = ToolCall {
            name: "search".into(),
            arguments: json!({"q": "x"}),
            result: Some("42".into()),
        };
        let back: ToolCall = serde_json::from_str(&serde_json::to_string(&call).unwrap()).unwrap();
        assert_eq!(back.name, "search");
        assert_eq!(back.arguments, json!({"q": "x"}));
        assert_eq!(back.result.as_deref(), Some("42"));
    }

    /// Scenario `message_omit_tool_calls_quand_none`.
    #[test]
    fn message_omit_tool_calls_quand_none() {
        let msg = Message {
            role: "user".into(),
            content: "bonjour".into(),
            tool_calls: None,
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v, json!({"role": "user", "content": "bonjour"}));
    }

    /// Scenario `message_accepte_tool_calls_absent_ou_null`.
    #[test]
    fn message_accepte_tool_calls_absent_ou_null() {
        let absent: Message = serde_json::from_str(r#"{"role":"user","content":"x"}"#).unwrap();
        assert!(absent.tool_calls.is_none());
        let null: Message =
            serde_json::from_str(r#"{"role":"user","content":"x","tool_calls":null}"#).unwrap();
        assert!(null.tool_calls.is_none());
    }

    /// Scenario `message_ignore_champs_inconnus`.
    #[test]
    fn message_ignore_champs_inconnus() {
        let msg: Message = serde_json::from_str(
            r#"{"role":"user","content":"x","tool_calls":null,"future":"champ"}"#,
        )
        .unwrap();
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "x");
    }

    /// Scenario `message_roundtrip_avec_tool_calls_resolus`.
    #[test]
    fn message_roundtrip_avec_tool_calls_resolus() {
        let msg = Message {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(vec![ToolCall {
                name: "t".into(),
                arguments: json!({}),
                result: Some("ok".into()),
            }]),
        };
        let back: Message = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        let calls = back.tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "t");
        assert_eq!(calls[0].arguments, json!({}));
        assert_eq!(calls[0].result.as_deref(), Some("ok"));
    }

    /// Scenario `message_accepte_role_hors_vocabulaire`.
    #[test]
    fn message_accepte_role_hors_vocabulaire() {
        let msg: Message = serde_json::from_str(r#"{"role":"system-e","content":"y"}"#).unwrap();
        assert_eq!(msg.role, "system-e");
    }

    /// Scenario `conversation_roundtrip_json_complet`.
    #[test]
    fn conversation_roundtrip_json_complet() {
        let id = Uuid::new_v4();
        let conv = Conversation {
            id,
            agent: Some("general".into()),
            title: None,
            messages: vec![Message {
                role: "user".into(),
                content: "salut".into(),
                tool_calls: None,
            }],
            todo: Some("[{\"content\":\"t1\"}]".into()),
        };
        let back: Conversation =
            serde_json::from_str(&serde_json::to_string(&conv).unwrap()).unwrap();
        assert_eq!(back.id, id);
        assert_eq!(back.agent.as_deref(), Some("general"));
        assert!(back.title.is_none());
        assert_eq!(back.messages.len(), 1);
        assert_eq!(back.messages[0].content, "salut");
        assert_eq!(back.todo.as_deref(), Some("[{\"content\":\"t1\"}]"));
    }

    /// Scenario `conversation_todo_absent_vaut_none_par_defaut`.
    #[test]
    fn conversation_todo_absent_vaut_none_par_defaut() {
        let conv: Conversation = serde_json::from_value(conversation_json(None)).unwrap();
        assert!(conv.todo.is_none());
    }

    /// Scenario `conversation_omit_todo_et_accepte_todo_null`.
    #[test]
    fn conversation_omit_todo_et_accepte_todo_null() {
        let conv: Conversation = serde_json::from_value(conversation_json(None)).unwrap();
        let v = serde_json::to_value(&conv).unwrap();
        assert!(v.get("todo").is_none());
        let back: Conversation = serde_json::from_value(v).unwrap();
        assert!(back.todo.is_none());

        let mut with_null = conversation_json(None);
        with_null["todo"] = Value::Null;
        let null_back: Conversation = serde_json::from_value(with_null).unwrap();
        assert!(null_back.todo.is_none());
    }

    /// Scenario `conversation_id_en_uuid_canonique_hyphen`.
    #[test]
    fn conversation_id_en_uuid_canonique_hyphen() {
        let conv: Conversation = serde_json::from_value(conversation_json(None)).unwrap();
        let v = serde_json::to_value(&conv).unwrap();
        assert_eq!(v["id"], "aaaaaaaa-0000-0000-0000-000000000001");
        assert_eq!(conv.id.to_string(), "aaaaaaaa-0000-0000-0000-000000000001");
    }

    /// Scenario `conversation_refuse_un_json_sans_messages`.
    #[test]
    fn conversation_refuse_un_json_sans_messages() {
        let json = r#"{"id":"aaaaaaaa-0000-0000-0000-000000000001","agent":null,"title":null}"#;
        assert!(serde_json::from_str::<Conversation>(json).is_err());
    }

    /// Scenario `conversation_messages_vide_autorisee`.
    #[test]
    fn conversation_messages_vide_autorisee() {
        let conv = Conversation {
            id: Uuid::new_v4(),
            agent: None,
            title: None,
            messages: Vec::new(),
            todo: None,
        };
        let back: Conversation =
            serde_json::from_str(&serde_json::to_string(&conv).unwrap()).unwrap();
        assert!(back.messages.is_empty());
        assert!(back.agent.is_none());
        assert!(back.title.is_none());
    }
}
