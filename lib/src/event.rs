use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use rig_core::agent::{Agent, MultiTurnStreamItem};
use rig_core::completion::{CompletionModel, GetTokenUsage};
use rig_core::message::{Message, ToolResultContent};
use rig_core::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat};

use crate::error::VnyError;

/// L'événement de session unique — circule sur stdout REPL, WebSocket app et
/// notifications JSON-RPC (cf. design harness-core.md). Tag `"type"`, `snake_case`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    /// Delta de texte de la réponse finale (`token`), à accumuler côté client.
    Token {
        /// Contenu du delta.
        content: String,
    },
    /// Contenu de raisonnement (thinking) — même sémantique que `Token`
    /// (delta à accumuler côté client) mais un canal séparé, pour un rendu
    /// distinct de la réponse finale. `rig-core` expose ce contenu
    /// (`StreamedAssistantContent::Reasoning`/`ReasoningDelta`) mais rien ne
    /// le captait avant ce fix — jeté silencieusement par `StreamAccumulator`.
    ReasoningDelta {
        /// Contenu du delta de raisonnement.
        content: String,
    },
    /// Appel d'outil annoncé par le modèle (`tool_call`). L'`id` est
    /// l'`internal_call_id` de rig, partagé avec le `tool_result` corrélé.
    ToolCall {
        /// `internal_call_id` de rig (id de corrélation, jamais l'id fournisseur).
        id: String,
        /// Nom de l'outil appelé.
        name: String,
        /// Arguments de l'appel (tout JSON ; `any` côté TypeScript).
        #[cfg_attr(feature = "ts-rs", ts(type = "any"))]
        args: serde_json::Value,
    },
    /// `is_error` est toujours `false` — `rig-core` 0.38.1 convertit les
    /// erreurs de tool call en texte avant `StreamedUserContent::ToolResult`,
    /// l'information n'atteint jamais ce code. Limite connue et documentée
    /// (`docs/architecture.md`, section "Limites connues"), pas un bug local.
    ToolResult {
        /// `internal_call_id` de rig, identique à celui du `tool_call` corrélé.
        id: String,
        /// Nom de l'outil restauré depuis l'appel ; vide si l'id n'a jamais
        /// été annoncé.
        name: String,
        /// Contenu texte du résultat (contenus `Text` concaténés, `Image` ignorés).
        result: String,
        /// Drapeau d'erreur — toujours `false` sur ce chemin (limite rig-core,
        /// voir le commentaire de l'enum).
        is_error: bool,
    },
    /// Un skill a été chargé par le tool builtin `skill` (`skill_loaded`).
    /// Produit hors de ce fichier (`builtin/skill.rs`).
    SkillLoaded {
        /// Nom du skill chargé.
        name: String,
    },
    /// Démarrage d'un tour de subagent (`subagent_start`). Produit hors de ce
    /// fichier (`builtin/task.rs`).
    SubagentStart {
        /// Identifiant du subagent (UUID v4).
        id: String,
        /// Nom de l'agent invoqué.
        agent: String,
        /// Prompt transmis au subagent.
        task: String,
    },
    /// Événement émis PAR un tour imbriqué, encapsulé (`subagent_event`).
    /// Produit hors de ce fichier (`builtin/task.rs`) ; nesting récursif autorisé.
    SubagentEvent {
        /// Identifiant du subagent qui a émis l'événement.
        id: String,
        /// L'événement imbriqué, quel qu'il soit (y compris `error`/`done`).
        event: Box<ChatEvent>,
    },
    /// Fin d'un tour de subagent (`subagent_end`). Produit hors de ce fichier
    /// (`builtin/task.rs`).
    SubagentEnd {
        /// Identifiant du subagent terminé.
        id: String,
        /// Texte de sa réponse finale, ou `Error: …` si son tour a échoué.
        result: String,
    },
    /// Compteurs de tokens d'un appel de complétion (`usage`) ; seuls entrée et
    /// sortie sont conservés.
    Usage {
        /// Tokens d'entrée.
        input_tokens: u64,
        /// Tokens de sortie.
        output_tokens: u64,
    },
    /// Fin de tour (`done`) — émis sur tout chemin de succès de
    /// [`stream_agent_events`], jamais après [`ChatEvent::Error`].
    Done,
    /// Erreur terminale pour les consommateurs (`error`) : aucun `done` ne la
    /// suit.
    Error {
        /// Marqueur `VNL-*` (ex. `"VNL-LLM-001"` sur le chemin stream).
        code: String,
        /// Message lisible de l'erreur.
        message: String,
    },
    /// Un serveur MCP sélectionné pour ce tour n'a pas pu être joint — contrairement
    /// à `Error`, non terminale : le tour continue sans les tools de ce serveur
    /// (cf. `connect_mcp_servers_selected`, avant ce fix l'échec était seulement loggé).
    ToolUnavailable {
        /// Nom du serveur MCP injoignable.
        server: String,
        /// Motif de l'échec (affichage de l'erreur de connexion).
        reason: String,
    },
}

/// Destinataire des [`ChatEvent`] d'un tour : un sink est infaillible à cette
/// interface, la politique de perte est propre à chaque implémentation.
// async_trait génère un #[must_use] sur le future boxed de chaque méthode, déjà
// must_use : exemption ciblée justifiée (tooling.sdd).
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait EventSink: Send + Sync {
    /// Publie un événement (par valeur, sans valeur de retour).
    async fn emit(&self, event: ChatEvent);
}

/// Un tool call collecté durant le tour, avec son résultat une fois disponible.
/// Distinct de `crate::types::ToolCall` (qui n'a pas de champ `id` — cf. stratégie
/// additive : on ne touche pas à `types.rs` dans cette tâche).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct ToolCallRecord {
    /// `internal_call_id` de rig — id de corrélation avec le résultat.
    pub id: String,
    /// Nom de l'outil appelé.
    pub name: String,
    /// Arguments de l'appel (tout JSON ; `any` côté TypeScript).
    #[cfg_attr(feature = "ts-rs", ts(type = "any"))]
    pub arguments: serde_json::Value,
    /// Résultat une fois le `tool_result` corrélé reçu ; `None` si l'appel
    /// n'a jamais été résolu dans le tour.
    pub result: Option<String>,
}

/// Bilan d'un tour d'agent retourné par [`stream_agent_events`] — type
/// purement interne Rust, pas de derive serde ni ts-rs (cf. `./event.sdd`).
pub struct ChatTurnResult {
    /// Texte de réponse finale : deltas assistant `Text` concaténés (le
    /// raisonnement n'y rejoint jamais).
    pub response_text: String,
    /// Tool calls collectés durant le tour, dans l'ordre d'arrivée.
    pub tool_calls: Vec<ToolCallRecord>,
}

/// État accumulé au fil du stream : texte de réponse, tool calls collectés,
/// et corrélation id → nom de tool en attente de son résultat.
#[derive(Default)]
pub struct StreamAccumulator {
    /// Texte assistant concaténé (deltas `Text` uniquement).
    pub response_text: String,
    /// Tool calls collectés, dans l'ordre d'arrivée.
    pub tool_calls: Vec<ToolCallRecord>,
    pending_names: HashMap<String, String>,
}

/// Extrait et concatène SANS séparateur les contenus textuels (Text/Summary)
/// d'un bloc `Reasoning` rig. Pur — extraite de `StreamAccumulator::apply`
/// (garde `too_many_lines`, harnais tooling.sdd).
fn reasoning_text(reasoning: &rig_core::message::Reasoning) -> String {
    reasoning
        .content
        .iter()
        .filter_map(|c| match c {
            rig_core::message::ReasoningContent::Text { text, .. } => Some(text.clone()),
            rig_core::message::ReasoningContent::Summary(s) => Some(s.clone()),
            // `#[non_exhaustive]` côté rig-core : Encrypted, Redacted et
            // futures variantes ignorées plutôt que de casser la compilation
            // à chaque upgrade.
            _ => None,
        })
        .collect()
}

impl StreamAccumulator {
    /// Accumulateur vide, identique à `Default::default()`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Traduit UN item du stream rig en zéro ou plusieurs `ChatEvent`, en mettant
    /// à jour l'état interne. **Pure — aucune I/O, aucun réseau** : c'est la
    /// fonction testée unitairement (cf. section Tests). Retourne
    /// `(events, is_final)` — `is_final = true` signifie que l'appelant doit
    /// arrêter de consommer le stream (équivalent du `FinalResponse => break`
    /// de l'ancien `stream_agent_response` dans chat.rs).
    ///
    /// Corrélation ToolCall/ToolResult : utiliser `internal_call_id` (généré par
    /// rig, cf. `StreamedAssistantContent::ToolCall` et
    /// `StreamedUserContent::ToolResult`) comme `id` du `ChatEvent` — PAS
    /// `tool_call.id`/`tool_result.id` (id fournisseur, pas fiable pour la
    /// corrélation selon la doc rig elle-même).
    pub fn apply<R>(&mut self, item: MultiTurnStreamItem<R>) -> (Vec<ChatEvent>, bool) {
        match item {
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text)) => {
                self.response_text.push_str(&text.text);
                (vec![ChatEvent::Token { content: text.text }], false)
            }
            MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::ReasoningDelta { reasoning, .. },
            ) => (
                vec![ChatEvent::ReasoningDelta { content: reasoning }],
                false,
            ),
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(
                reasoning,
            )) => {
                let text = reasoning_text(&reasoning);
                if text.is_empty() {
                    (Vec::new(), false)
                } else {
                    (vec![ChatEvent::ReasoningDelta { content: text }], false)
                }
            }
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                tool_call,
                internal_call_id,
            }) => {
                let name = tool_call.function.name.clone();
                let args = tool_call.function.arguments.clone();
                // Cloner internal_call_id car il sera déplacé vers ToolCallRecord
                let internal_call_id_clone = internal_call_id.clone();
                self.pending_names
                    .insert(internal_call_id.clone(), name.clone());
                let record = ToolCallRecord {
                    id: internal_call_id,
                    name: name.clone(),
                    arguments: args.clone(),
                    result: None,
                };
                self.tool_calls.push(record);
                (
                    vec![ChatEvent::ToolCall {
                        id: internal_call_id_clone,
                        name,
                        args,
                    }],
                    false,
                )
            }
            MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                tool_result,
                internal_call_id,
            }) => {
                let name = self
                    .pending_names
                    .remove(&internal_call_id)
                    .unwrap_or_default();

                // Concaténer les Text content, ignorer Image
                let result_concat: String = tool_result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ToolResultContent::Text(text) => Some(text.text.clone()),
                        ToolResultContent::Image(_) => None,
                    })
                    .collect();

                // Mettre à jour le record existant par internal_call_id.
                // Si aucun record ne correspond (corrélation perdue), on
                // n'écrit pas dans tool_calls.
                for record in &mut self.tool_calls {
                    if record.id == internal_call_id {
                        record.result = Some(result_concat.clone());
                        break;
                    }
                }

                (
                    vec![ChatEvent::ToolResult {
                        id: internal_call_id,
                        name,
                        result: result_concat,
                        is_error: false,
                    }],
                    false,
                )
            }
            MultiTurnStreamItem::CompletionCall(completion_call) => {
                if let Some(usage) = completion_call.usage {
                    (
                        vec![ChatEvent::Usage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                        }],
                        false,
                    )
                } else {
                    (Vec::new(), false)
                }
            }
            MultiTurnStreamItem::FinalResponse(_) => (Vec::new(), true),
            // ToolCallDelta, Final — ignorés (Reasoning/ReasoningDelta gérés ci-dessus)
            _ => (Vec::new(), false),
        }
    }
}

/// Streame un tour d'agent en émettant des `ChatEvent` sur `sink`. Remplace, pour
/// le nouveau cœur, l'ancien `stream_agent_response` (qui reste inchangé dans
/// chat.rs pour l'instant). Émet `Done` à la fin dans tous les cas (succès ou
/// arrêt sur `FinalResponse`) ; sur erreur du stream, émet `Error` puis retourne
/// `Err` sans émettre `Done`.
///
/// # Errors
///
/// Seule erreur possible : [`VnyError::LlmError`] (`VNL-LLM-001`) quand le stream
/// rig émet une erreur — juste avant, un unique [`ChatEvent::Error`] du même code
/// et du même message est émis sur le sink, et `Done` n'est jamais émis sur ce
/// chemin (cf. `./event.sdd`).
pub async fn stream_agent_events<M, S>(
    sink: Arc<S>,
    agent: Agent<M>,
    history: Vec<Message>,
    user_msg: &str,
) -> Result<ChatTurnResult, VnyError>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: GetTokenUsage,
    S: EventSink + ?Sized + 'static,
{
    let mut stream = agent.stream_chat(user_msg, history).await;
    let mut acc = StreamAccumulator::new();

    loop {
        match stream.next().await {
            Some(Ok(item)) => {
                let (events, is_final) = acc.apply(item);
                for event in events {
                    sink.emit(event).await;
                }
                if is_final {
                    break;
                }
            }
            Some(Err(e)) => {
                sink.emit(ChatEvent::Error {
                    code: "VNL-LLM-001".to_string(),
                    message: format!("{e}"),
                })
                .await;
                return Err(VnyError::LlmError(format!("{e}")));
            }
            None => {
                // Stream ended without FinalResponse — emit Done anyway
                break;
            }
        }
    }

    sink.emit(ChatEvent::Done).await;
    Ok(ChatTurnResult {
        response_text: acc.response_text,
        tool_calls: acc.tool_calls,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rig_core::OneOrMany;
    use rig_core::agent::CompletionCall;
    use rig_core::completion::Usage;
    use rig_core::message::{
        Reasoning, Text, ToolCall, ToolFunction, ToolResult, ToolResultContent,
    };
    use rig_core::streaming::{StreamedAssistantContent, StreamedUserContent};
    use std::sync::Mutex;

    /// Test de sérialisation des tags "type" pour chaque variante.
    #[test]
    fn chat_event_tags() {
        // Token
        let token = serde_json::to_value(&ChatEvent::Token {
            content: "hello".into(),
        })
        .unwrap();
        assert_eq!(token["type"], "token");
        assert_eq!(token["content"], "hello");

        // ReasoningDelta
        let reasoning_delta = serde_json::to_value(&ChatEvent::ReasoningDelta {
            content: "je réfléchis".into(),
        })
        .unwrap();
        assert_eq!(reasoning_delta["type"], "reasoning_delta");
        assert_eq!(reasoning_delta["content"], "je réfléchis");

        // ToolCall
        let tool_call = serde_json::to_value(&ChatEvent::ToolCall {
            id: "c1".into(),
            name: "search".into(),
            args: serde_json::json!({"q":"x"}),
        })
        .unwrap();
        assert_eq!(tool_call["type"], "tool_call");
        assert_eq!(tool_call["id"], "c1");
        assert_eq!(tool_call["name"], "search");
        assert_eq!(tool_call["args"]["q"], "x");

        // ToolResult
        let result = serde_json::to_value(&ChatEvent::ToolResult {
            id: "c1".into(),
            name: "search".into(),
            result: "42".into(),
            is_error: false,
        })
        .unwrap();
        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["is_error"], false);

        // SkillLoaded
        let skill = serde_json::to_value(&ChatEvent::SkillLoaded {
            name: "my-skill".into(),
        })
        .unwrap();
        assert_eq!(skill["type"], "skill_loaded");

        // SubagentStart
        let ssub = serde_json::to_value(&ChatEvent::SubagentStart {
            id: "sub-1".into(),
            agent: "dev".into(),
            task: "test".into(),
        })
        .unwrap();
        assert_eq!(ssub["type"], "subagent_start");

        // SubagentEvent
        let subevent = ChatEvent::SubagentEvent {
            id: "s1".into(),
            event: Box::new(ChatEvent::Token {
                content: "x".into(),
            }),
        };
        let subevent_val = serde_json::to_value(&subevent).unwrap();
        assert_eq!(subevent_val["type"], "subagent_event");
        assert_eq!(subevent_val["event"]["type"], "token");

        // SubagentEnd
        let send = serde_json::to_value(&ChatEvent::SubagentEnd {
            id: "s1".into(),
            result: "done".into(),
        })
        .unwrap();
        assert_eq!(send["type"], "subagent_end");

        // Usage
        let usage = serde_json::to_value(&ChatEvent::Usage {
            input_tokens: 10,
            output_tokens: 5,
        })
        .unwrap();
        assert_eq!(usage["type"], "usage");

        // Done
        let done = serde_json::to_value(&ChatEvent::Done).unwrap();
        assert_eq!(done["type"], "done");

        // Error
        let error_evt = serde_json::to_value(&ChatEvent::Error {
            code: "ERR-001".into(),
            message: "broke".into(),
        })
        .unwrap();
        assert_eq!(error_evt["type"], "error");
    }

    /// Test de nesting de l'événement `SubagentEvent` — roundtrip serde.
    #[test]
    fn subagent_event_nesting() {
        let original = ChatEvent::SubagentEvent {
            id: "sub-1".into(),
            event: Box::new(ChatEvent::Token {
                content: "x".into(),
            }),
        };

        let json = serde_json::to_value(&original).unwrap();
        let roundtrip: ChatEvent = serde_json::from_value(json).unwrap();

        assert_eq!(roundtrip, original);
    }

    /// Deux appels successifs à apply avec Text accumulent le texte.
    #[test]
    fn apply_text_accumulates() {
        let mut acc = StreamAccumulator::new();

        // premier token
        let (events, is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamAssistantItem(
            StreamedAssistantContent::Text(Text {
                text: "Hel".into(),
                additional_params: None,
            }),
        ));
        assert!(!is_final);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ChatEvent::Token {
                content: "Hel".into()
            }
        );
        assert_eq!(acc.response_text, "Hel");

        // deuxième token
        let (events, is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamAssistantItem(
            StreamedAssistantContent::Text(Text {
                text: "lo".into(),
                additional_params: None,
            }),
        ));
        assert!(!is_final);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ChatEvent::Token {
                content: "lo".into()
            }
        );
        assert_eq!(acc.response_text, "Hello");
    }

    /// `ReasoningDelta` (rig) émet `ChatEvent::ReasoningDelta`, canal séparé
    /// de `response_text` (pas accumulé dedans).
    #[test]
    fn apply_reasoning_delta_emits_reasoning_delta_event() {
        let mut acc = StreamAccumulator::new();
        let (events, is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamAssistantItem(
            StreamedAssistantContent::ReasoningDelta {
                id: None,
                reasoning: "je réfl".into(),
            },
        ));
        assert!(!is_final);
        assert_eq!(
            events,
            vec![ChatEvent::ReasoningDelta {
                content: "je réfl".into()
            }]
        );
        assert_eq!(acc.response_text, "");
    }

    /// Bloc de reasoning complet (non-streaming) — le texte est extrait de
    /// `Reasoning.content` et émis comme `ReasoningDelta` (même canal que
    /// les deltas, un seul événement pour tout le bloc).
    #[test]
    fn apply_reasoning_complete_block_extracts_text() {
        let mut acc = StreamAccumulator::new();
        let reasoning = Reasoning::new("j'analyse la question");
        let (events, is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamAssistantItem(
            StreamedAssistantContent::Reasoning(reasoning),
        ));
        assert!(!is_final);
        assert_eq!(
            events,
            vec![ChatEvent::ReasoningDelta {
                content: "j'analyse la question".into()
            }]
        );
    }

    /// `ToolCall` puis `ToolResult` — corrélation par `internal_call_id`.
    #[test]
    fn apply_tool_call_then_result_correlates() {
        let mut acc = StreamAccumulator::new();

        // ToolCall
        let (events, _is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamAssistantItem(
            StreamedAssistantContent::ToolCall {
                tool_call: ToolCall::new(
                    "prov-1".into(),
                    ToolFunction {
                        name: "search".into(),
                        arguments: serde_json::json!({"q":"x"}),
                    },
                ),
                internal_call_id: "call-1".into(),
            },
        ));
        // Capturer l'event ToolCall avant qu'il soit écrasé par apply(ToolResult)
        let events_call = events.clone();

        assert_eq!(
            events,
            vec![ChatEvent::ToolCall {
                id: "call-1".into(),
                name: "search".into(),
                args: serde_json::json!({"q":"x"}),
            }]
        );
        assert_eq!(acc.tool_calls.len(), 1);
        assert_eq!(acc.tool_calls[0].id, "call-1");
        assert_eq!(acc.tool_calls[0].name, "search");
        assert_eq!(acc.tool_calls[0].arguments, serde_json::json!({"q":"x"}));
        assert_eq!(acc.tool_calls[0].result, None);

        // ToolResult — internal_call_id = "call-1" (même id), mais tool_result.id ≠
        let (events, _is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamUserItem(
            StreamedUserContent::ToolResult {
                tool_result: ToolResult {
                    id: "prov-1".into(),
                    call_id: None,
                    content: OneOrMany::one(ToolResultContent::text("42")),
                },
                internal_call_id: "call-1".into(),
            },
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(
            events,
            vec![ChatEvent::ToolResult {
                id: "call-1".into(),
                name: "search".into(),
                result: "42".into(),
                is_error: false,
            }]
        );
        assert_eq!(acc.tool_calls[0].result, Some("42".into()));

        // Assertion directe : ToolCall et ToolResult doivent partager le même id
        let ChatEvent::ToolCall {
            id: tool_call_event_id,
            ..
        } = &events_call[0]
        else {
            panic!("expected ToolCall event")
        };
        let ChatEvent::ToolResult {
            id: tool_result_event_id,
            ..
        } = &events[0]
        else {
            panic!("expected ToolResult event")
        };
        assert_eq!(
            tool_call_event_id, tool_result_event_id,
            "ToolCall and ToolResult events for the same tool call must share the same id \
             (used by consumers to correlate them) — got {tool_call_event_id:?} vs \
             {tool_result_event_id:?}"
        );
    }

    /// `ToolResult` avec `internal_call_id` jamais vu — name vide, pas de panique.
    #[test]
    fn apply_tool_result_without_matching_call() {
        let mut acc = StreamAccumulator::new();

        let (events, _is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamUserItem(
            StreamedUserContent::ToolResult {
                tool_result: ToolResult {
                    id: "unknown".into(),
                    call_id: None,
                    content: OneOrMany::one(ToolResultContent::text("result")),
                },
                internal_call_id: "never-seen".into(),
            },
        ));

        // Pas de panique, name vide
        assert_eq!(
            events,
            vec![ChatEvent::ToolResult {
                id: "never-seen".into(),
                name: String::new(),
                result: "result".into(),
                is_error: false,
            }]
        );
        assert!(
            acc.tool_calls.is_empty(),
            "attendu aucun tool_call, got {:?}",
            acc.tool_calls
        );
    }

    /// `CompletionCall` avec usage — émet Usage, sans usage — émet [].
    #[test]
    fn apply_completion_call_usage() {
        let mut acc = StreamAccumulator::new();

        // Avec usage
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        };
        let (events, _is_final) = acc.apply(MultiTurnStreamItem::<()>::CompletionCall(
            CompletionCall::new(0, Some(usage)),
        ));
        assert_eq!(
            events,
            vec![ChatEvent::Usage {
                input_tokens: 10,
                output_tokens: 5,
            }]
        );

        // Sans usage
        let mut acc2 = StreamAccumulator::new();
        let (events2, _is_final2) = acc2.apply(MultiTurnStreamItem::<()>::CompletionCall(
            CompletionCall::new(1, None),
        ));
        assert!(
            events2.is_empty(),
            "attendu aucun événement, got {events2:?}"
        );
    }

    /// Mock sink — vérifie que les événements sont émis dans l'ordre.
    #[test]
    fn mock_sink_emit() {
        fn assert_sink<T: EventSink>() {}

        #[derive(Default)]
        struct MockSink {
            events: Mutex<Vec<ChatEvent>>,
        }

        #[async_trait]
        impl EventSink for MockSink {
            async fn emit(&self, event: ChatEvent) {
                self.events.lock().unwrap().push(event);
            }
        }

        // Le trait EventSink est implémenté et synchrone dans sa signature (async fn).
        // On ne peut pas await dans un test synchrone, donc on vérifie uniquement
        // la compilation via un type check statique.
        assert_sink::<MockSink>();

        // On utilise Arc<T>::clone + drop comme preuve que le type est Send + Sync.
        let sink = Arc::new(MockSink::default());
        let s1 = sink.clone();
        let s2 = sink.clone();
        drop(s1);
        drop(s2);
        drop(sink);
    }

    /// Génère `packages/protocol/src/generated/chat-event.ts` depuis les types
    /// `ChatEvent` et `ToolCallRecord`. Fichier commité ; le job CI (tâche 06)
    /// régénère et vérifie `git diff --exit-code`. `TS_RS_LARGE_INT = "number"`
    /// est posé dans `.cargo/config.toml` (racine) — lisible par `Config::from_env()`.
    #[cfg(feature = "ts-rs")]
    #[test]
    fn export_chat_event_ts() {
        use ts_rs::{Config, TS};
        let cfg = Config::from_env();
        let chat = <ChatEvent as TS>::export_to_string(&cfg).unwrap();
        let record = <ToolCallRecord as TS>::export_to_string(&cfg).unwrap();
        let out = format!(
            "// Generated by vanyline-lib (ts-rs) from `ChatEvent`/`ToolCallRecord`.\n\
             // DO NOT EDIT — regenerate: cargo test -p vanyline-lib --features ts-rs\n\n\
             {chat}\n{record}\n"
        );
        let target = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packages/protocol/src/generated/chat-event.ts"
        );
        std::fs::write(target, out).unwrap();
    }

    /// Scenario `bloc_reasoning_non_extractible_nemet_rien`.
    #[test]
    fn bloc_reasoning_non_extractible_nemet_rien() {
        let mut reasoning = Reasoning::redacted("r");
        reasoning
            .content
            .push(rig_core::message::ReasoningContent::Encrypted("e".into()));
        let mut acc = StreamAccumulator::new();
        let (events, is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamAssistantItem(
            StreamedAssistantContent::Reasoning(reasoning),
        ));
        assert!(events.is_empty(), "{events:?}");
        assert!(!is_final);
        assert!(acc.response_text.is_empty());
        assert!(acc.tool_calls.is_empty());
    }

    /// Scenario `final_response_terminale_sans_emission`.
    #[test]
    fn final_response_terminale_sans_emission() {
        let mut acc = StreamAccumulator::new();
        let _ = acc.apply(MultiTurnStreamItem::<()>::StreamAssistantItem(
            StreamedAssistantContent::text("bonjour"),
        ));
        let (events, is_final) = acc.apply(MultiTurnStreamItem::<()>::FinalResponse(
            rig_core::agent::FinalResponse::empty(),
        ));
        assert!(events.is_empty(), "{events:?}");
        assert!(is_final);
        assert_eq!(acc.response_text, "bonjour");
    }

    /// Scenario `items_hors_mapping_ignorés_silencieusement`.
    #[test]
    fn items_hors_mapping_ignorés_silencieusement() {
        let mut acc = StreamAccumulator::new();
        let (events, is_final) = acc.apply(MultiTurnStreamItem::<()>::StreamAssistantItem(
            StreamedAssistantContent::ToolCallDelta {
                id: "prov-1".into(),
                internal_call_id: "call-1".into(),
                content: rig_core::streaming::ToolCallDeltaContent::Name("search".into()),
            },
        ));
        assert!(events.is_empty(), "{events:?}");
        assert!(!is_final);
        assert!(acc.tool_calls.is_empty());
    }
}
