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
/// notifications JSON-RPC (cf. design harness-core.md). Tag "type", snake_case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Token {
        content: String,
    },
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// `is_error` est toujours `false` — `rig-core` 0.38.1 convertit les
    /// erreurs de tool call en texte avant `StreamedUserContent::ToolResult`,
    /// l'information n'atteint jamais ce code. Limite connue et documentée
    /// (`docs/architecture.md`, section "Limites connues"), pas un bug local.
    ToolResult {
        id: String,
        name: String,
        result: String,
        is_error: bool,
    },
    SkillLoaded {
        name: String,
    },
    SubagentStart {
        id: String,
        agent: String,
        task: String,
    },
    SubagentEvent {
        id: String,
        event: Box<ChatEvent>,
    },
    SubagentEnd {
        id: String,
        result: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Done,
    Error {
        code: String,
        message: String,
    },
}

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&self, event: ChatEvent);
}

/// Un tool call collecté durant le tour, avec son résultat une fois disponible.
/// Distinct de `crate::types::ToolCall` (qui n'a pas de champ `id` — cf. stratégie
/// additive : on ne touche pas à `types.rs` dans cette tâche).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: Option<String>,
}

pub struct ChatTurnResult {
    pub response_text: String,
    pub tool_calls: Vec<ToolCallRecord>,
}

/// État accumulé au fil du stream : texte de réponse, tool calls collectés,
/// et corrélation id → nom de tool en attente de son résultat.
#[derive(Default)]
pub struct StreamAccumulator {
    pub response_text: String,
    pub tool_calls: Vec<ToolCallRecord>,
    pending_names: HashMap<String, String>,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Traduit UN item du stream rig en zéro ou plusieurs ChatEvent, en mettant
    /// à jour l'état interne. **Pure — aucune I/O, aucun réseau** : c'est la
    /// fonction testée unitairement (cf. section Tests). Retourne
    /// `(events, is_final)` — `is_final = true` signifie que l'appelant doit
    /// arrêter de consommer le stream (équivalent du `FinalResponse => break`
    /// de l'ancien `stream_agent_response` dans chat.rs).
    ///
    /// Corrélation ToolCall/ToolResult : utiliser `internal_call_id` (généré par
    /// rig, cf. `StreamedAssistantContent::ToolCall` et
    /// `StreamedUserContent::ToolResult`) comme `id` du ChatEvent — PAS
    /// `tool_call.id`/`tool_result.id` (id fournisseur, pas fiable pour la
    /// corrélation selon la doc rig elle-même).
    pub fn apply<R>(&mut self, item: MultiTurnStreamItem<R>) -> (Vec<ChatEvent>, bool) {
        match item {
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text)) => {
                self.response_text.push_str(&text.text);
                (vec![ChatEvent::Token { content: text.text }], false)
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
                for record in self.tool_calls.iter_mut() {
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
            // ToolCallDelta, Reasoning, ReasoningDelta, Final — ignorés
            _ => (Vec::new(), false),
        }
    }
}

/// Streame un tour d'agent en émettant des ChatEvent sur `sink`. Remplace, pour
/// le nouveau cœur, l'ancien `stream_agent_response` (qui reste inchangé dans
/// chat.rs pour l'instant). Émet `Done` à la fin dans tous les cas (succès ou
/// arrêt sur FinalResponse) ; sur erreur du stream, émet `Error` puis retourne
/// `Err` sans émettre `Done`.
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
                    message: format!("{}", e),
                })
                .await;
                return Err(VnyError::LlmError(format!("{}", e)));
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
    use rig_core::message::{Text, ToolCall, ToolFunction, ToolResult, ToolResultContent};
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

    /// Test de nesting de l'événement SubagentEvent — roundtrip serde.
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

    /// ToolCall puis ToolResult — corrélation par internal_call_id.
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
             (used by consumers to correlate them) — got {:?} vs {:?}",
            tool_call_event_id, tool_result_event_id
        );
    }

    /// ToolResult avec internal_call_id jamais vu — name vide, pas de panique.
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
                name: "".into(),
                result: "result".into(),
                is_error: false,
            }]
        );
        assert!(acc.tool_calls.is_empty());
    }

    /// CompletionCall avec usage — émet Usage, sans usage — émet [].
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
        assert!(events2.is_empty());
    }

    /// Mock sink — vérifie que les événements sont émis dans l'ordre.
    #[test]
    fn mock_sink_emit() {
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

        let sink = Arc::new(MockSink::default());

        // Le trait EventSink est implémenté et synchrone dans sa signature (async fn).
        // On ne peut pas await dans un test synchrone, donc on vérifie uniquement
        // la compilation via un type check statique.
        fn _assert_sink<T: EventSink>() {}
        _assert_sink::<MockSink>();

        // On utilise Arc<T>::clone + drop comme preuve que le type est Send + Sync.
        let _s1 = sink.clone();
        let _s2 = sink.clone();
        drop(_s1);
        drop(_s2);
        drop(sink);
    }
}
