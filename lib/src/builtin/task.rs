use std::sync::Arc;

use rig_core::completion::ToolDefinition;
use rig_core::tool::{ToolDyn, ToolError};
use rig_core::wasm_compat::WasmBoxedFuture;

use crate::domain::{Agent, AgentMode};
use crate::event::{ChatEvent, EventSink};
use crate::session::SessionContext;

#[derive(Debug)]
struct SubagentError(String);

impl std::fmt::Display for SubagentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SubagentError {}

/// Encapsule chaque `ChatEvent` émis par un tour imbriqué en
/// `ChatEvent::SubagentEvent { id, event }` avant de le transmettre au sink
/// parent. `SubagentStart`/`SubagentEnd` ne passent PAS par ce sink — ils sont
/// émis directement sur le sink parent par `TaskTool::call` (ce sont des
/// événements À PROPOS du subagent, pas DU subagent).
struct SubagentEventSink {
    inner: Arc<dyn EventSink>,
    subagent_id: String,
}

#[async_trait::async_trait]
impl EventSink for SubagentEventSink {
    async fn emit(&self, event: ChatEvent) {
        self.inner
            .emit(ChatEvent::SubagentEvent {
                id: self.subagent_id.clone(),
                event: Box::new(event),
            })
            .await;
    }
}

/// Tool builtin `task(agent, prompt) -> réponse finale`. N'accepte que des
/// agents de mode `Subagent`/`All` (jamais `Primary`). PAS un singleton :
/// construit une fois par tour dans `run_agent_turn_at_depth` (tâche 6b/8),
/// avec la profondeur ACTUELLE (`current_depth`) et l'index des agents
/// invocables déjà résolus (`available_agents`) — même logique que `SkillTool`
/// (tâche 7).
pub struct TaskTool {
    ctx: SessionContext,
    current_depth: u8,
    available_agents: Vec<Agent>,
}

impl TaskTool {
    pub fn new(ctx: SessionContext, current_depth: u8, available_agents: Vec<Agent>) -> Self {
        Self {
            ctx,
            current_depth,
            available_agents,
        }
    }
}

#[derive(serde::Deserialize)]
struct TaskArgs {
    agent: String,
    prompt: String,
}

impl TaskTool {
    fn parameter_schema() -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"agent":{"type":"string"},"prompt":{"type":"string"}},"required":["agent","prompt"]})
    }
}

impl ToolDyn for TaskTool {
    fn name(&self) -> String {
        "task".to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        let available = self.available_agents.clone();
        let description = if available.is_empty() {
            "Délègue une tâche à un subagent. Aucun subagent disponible.".to_string()
        } else {
            let mut s =
                "Délègue une tâche à un subagent nommé. Subagents disponibles :\n".to_string();
            for agent in &available {
                let desc = agent
                    .description
                    .as_deref()
                    .unwrap_or("(pas de description)");
                s.push_str(&format!("- {} : {}\n", agent.name, desc));
            }
            s
        };
        Box::pin(async move {
            ToolDefinition {
                name: "task".to_string(),
                description,
                parameters: TaskTool::parameter_schema(),
            }
        })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        let store = self.ctx.store.clone();
        let sink = self.ctx.sink.clone();
        let current_depth = self.current_depth;
        let subagent_depth_max = self.ctx.subagent_depth_max;

        Box::pin(async move {
            // 1. Désérialiser args
            let parsed: TaskArgs =
                serde_json::from_str(&args).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;

            // 2. Garde de profondeur
            if current_depth >= subagent_depth_max {
                return Err(ToolError::ToolCallError(Box::new(SubagentError(format!(
                    "subagent depth limit ({}) reached, cannot delegate further",
                    subagent_depth_max
                )))));
            }

            // 3. Résoudre l'agent cible
            let target = store
                .get_agent(&parsed.agent)
                .await
                .map_err(|e| ToolError::ToolCallError(Box::new(e)))?;

            // 4. Vérifier mode Primary
            if target.mode == AgentMode::Primary {
                return Err(ToolError::ToolCallError(Box::new(SubagentError(format!(
                    "agent '{}' is a primary agent and cannot be invoked as a subagent",
                    parsed.agent
                )))));
            }

            // 5. Générer subagent_id
            let subagent_id = uuid::Uuid::new_v4().to_string();

            // 6. Émettre SubagentStart sur le sink parent
            sink.emit(ChatEvent::SubagentStart {
                id: subagent_id.clone(),
                agent: parsed.agent.clone(),
                task: parsed.prompt.clone(),
            })
            .await;

            // 7. Construire le contexte imbriqué
            let nested_sink: Arc<dyn EventSink> = Arc::new(SubagentEventSink {
                inner: sink.clone(),
                subagent_id: subagent_id.clone(),
            });
            let nested_ctx = SessionContext {
                store: store.clone(),
                sink: nested_sink,
                local_tools: self.ctx.local_tools.clone(),
                subagent_depth_max: self.ctx.subagent_depth_max,
                extra_mcp: self.ctx.extra_mcp.clone(),
            };

            // 8. Lancer le tour imbriqué
            let result = crate::session::run_agent_turn_at_depth(
                &nested_ctx,
                &parsed.agent,
                Vec::new(),
                &parsed.prompt,
                None,
                current_depth + 1,
            )
            .await;

            // 9. Émettre SubagentEnd et retourner
            match result {
                Ok(turn) => {
                    sink.emit(ChatEvent::SubagentEnd {
                        id: subagent_id.clone(),
                        result: turn.response_text.clone(),
                    })
                    .await;
                    Ok(turn.response_text)
                }
                Err(e) => {
                    sink.emit(ChatEvent::SubagentEnd {
                        id: subagent_id.clone(),
                        result: format!("Error: {e}"),
                    })
                    .await;
                    Err(ToolError::ToolCallError(Box::new(e)))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::event::ChatEvent;
    use crate::store::{ConfigStore, InMemoryConfigStore};
    use std::sync::Mutex;

    fn sample_store() -> Arc<dyn ConfigStore> {
        Arc::new(InMemoryConfigStore::default())
    }

    fn sample_sink() -> Arc<dyn EventSink> {
        Arc::new(RecordingSink::new())
    }

    struct RecordingSink(Mutex<Vec<ChatEvent>>);

    impl RecordingSink {
        fn new() -> Self {
            Self(Mutex::new(Vec::new()))
        }
        fn events(&self) -> Vec<ChatEvent> {
            self.0.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl EventSink for RecordingSink {
        async fn emit(&self, event: ChatEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    // 1. name_is_task
    #[test]
    fn name_is_task() {
        let task_tool = TaskTool::new(
            SessionContext {
                store: sample_store(),
                sink: sample_sink(),
                local_tools: std::collections::HashMap::new(),
                subagent_depth_max: 1,
                extra_mcp: Vec::new(),
            },
            0,
            Vec::new(),
        );
        assert_eq!(task_tool.name(), "task");
    }

    // 2. definition_embeds_subagent_index
    #[tokio::test]
    async fn definition_embeds_subagent_index() {
        let available_agents = vec![
            Agent {
                name: "sub".to_string(),
                description: Some("A subagent".to_string()),
                mode: AgentMode::Subagent,
                model: "m".to_string(),
                toolsets: vec![],
                skills: Default::default(),
                system_prompt: "prompt".to_string(),
            },
            Agent {
                name: "all".to_string(),
                description: Some("An all agent".to_string()),
                mode: AgentMode::All,
                model: "m".to_string(),
                toolsets: vec![],
                skills: Default::default(),
                system_prompt: "prompt".to_string(),
            },
        ];
        let task_tool = TaskTool::new(
            SessionContext {
                store: sample_store(),
                sink: sample_sink(),
                local_tools: std::collections::HashMap::new(),
                subagent_depth_max: 1,
                extra_mcp: Vec::new(),
            },
            0,
            available_agents,
        );
        let def = task_tool.definition("".into()).await;

        assert!(def.description.contains("- sub : A subagent"));
        assert!(def.description.contains("- all : An all agent"));
        assert_eq!(def.name, "task");
        assert_eq!(
            def.parameters["required"],
            serde_json::json!(["agent", "prompt"])
        );
    }

    // 3. definition_empty_index_placeholder
    #[tokio::test]
    async fn definition_empty_index_placeholder() {
        let task_tool = TaskTool::new(
            SessionContext {
                store: sample_store(),
                sink: sample_sink(),
                local_tools: std::collections::HashMap::new(),
                subagent_depth_max: 1,
                extra_mcp: Vec::new(),
            },
            0,
            Vec::new(),
        );
        let def = task_tool.definition("".into()).await;

        assert!(def.description.contains("Aucun subagent disponible"));
        assert!(!def.description.contains("- "));
    }

    // 4. subagent_event_sink_wraps_events
    #[tokio::test]
    async fn subagent_event_sink_wraps_events() {
        let recording = Arc::new(RecordingSink::new());
        let wrapped = SubagentEventSink {
            inner: recording.clone(),
            subagent_id: "sub-1".to_string(),
        };

        wrapped
            .emit(ChatEvent::Token {
                content: "hi".to_string(),
            })
            .await;

        assert_eq!(
            recording.events(),
            vec![ChatEvent::SubagentEvent {
                id: "sub-1".to_string(),
                event: Box::new(ChatEvent::Token {
                    content: "hi".to_string()
                }),
            }]
        );
    }

    // 5. call_depth_exceeded_rejected
    #[tokio::test]
    async fn call_depth_exceeded_rejected() {
        let store = Arc::new(InMemoryConfigStore {
            agents: vec![Agent {
                name: "sub".to_string(),
                description: None,
                mode: AgentMode::Subagent,
                model: "m".to_string(),
                toolsets: vec![],
                skills: Default::default(),
                system_prompt: "prompt".to_string(),
            }],
            ..Default::default()
        });
        let sink = Arc::new(RecordingSink::new());
        let ctx = SessionContext {
            store: store.clone(),
            sink: sink.clone(),
            local_tools: std::collections::HashMap::new(),
            subagent_depth_max: 1,
            extra_mcp: Vec::new(),
        };
        let task_tool = TaskTool::new(ctx, 1, vec![]);

        let result = task_tool
            .call(serde_json::json!({"agent":"sub","prompt":"do x"}).to_string())
            .await;

        assert!(result.is_err());
        assert!(sink.events().is_empty());
    }

    // 6. call_unknown_agent_rejected
    #[tokio::test]
    async fn call_unknown_agent_rejected() {
        let store = Arc::new(InMemoryConfigStore {
            agents: vec![Agent {
                name: "other".to_string(),
                description: None,
                mode: AgentMode::Subagent,
                model: "m".to_string(),
                toolsets: vec![],
                skills: Default::default(),
                system_prompt: "prompt".to_string(),
            }],
            ..Default::default()
        });
        let sink = Arc::new(RecordingSink::new());
        let ctx = SessionContext {
            store: store.clone(),
            sink: sink.clone(),
            local_tools: std::collections::HashMap::new(),
            subagent_depth_max: 1,
            extra_mcp: Vec::new(),
        };
        let task_tool = TaskTool::new(ctx, 0, vec![]);

        let result = task_tool
            .call(serde_json::json!({"agent":"nope","prompt":"x"}).to_string())
            .await;

        assert!(result.is_err());
        assert!(sink.events().is_empty());
    }

    // 7. call_primary_mode_rejected
    #[tokio::test]
    async fn call_primary_mode_rejected() {
        let store = Arc::new(InMemoryConfigStore {
            agents: vec![Agent {
                name: "main".to_string(),
                description: None,
                mode: AgentMode::Primary,
                model: "m".to_string(),
                toolsets: vec![],
                skills: Default::default(),
                system_prompt: "prompt".to_string(),
            }],
            ..Default::default()
        });
        let sink = Arc::new(RecordingSink::new());
        let ctx = SessionContext {
            store: store.clone(),
            sink: sink.clone(),
            local_tools: std::collections::HashMap::new(),
            subagent_depth_max: 1,
            extra_mcp: Vec::new(),
        };
        let task_tool = TaskTool::new(ctx, 0, vec![]);

        let result = task_tool
            .call(serde_json::json!({"agent":"main","prompt":"x"}).to_string())
            .await;

        assert!(result.is_err());
        assert!(sink.events().is_empty());
    }
}
