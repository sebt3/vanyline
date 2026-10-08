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
