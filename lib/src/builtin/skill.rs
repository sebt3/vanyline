use std::sync::Arc;

use rig_core::completion::ToolDefinition;
use rig_core::tool::ToolDyn;
use rig_core::wasm_compat::WasmBoxedFuture;

use crate::domain::SkillMeta;
use crate::event::{ChatEvent, EventSink};
use crate::store::ConfigStore;

/// Tool builtin `skill(name) -> corps du SKILL.md`, via `ConfigStore::load_skill`.
/// PAS un singleton : construit une fois par tour dans `run_agent_turn` (tâche
/// 6b, amendée par cette tâche), car sa description embarque l'index des
/// skills disponibles pour CET agent — dépend de sa `SkillSelection`, résolue
/// en amont (`resolve_turn_context` / `resolve_skill_index`).
pub struct SkillTool {
    store: Arc<dyn ConfigStore>,
    sink: Arc<dyn EventSink>,
    available: Vec<SkillMeta>,
}

impl SkillTool {
    pub fn new(
        store: Arc<dyn ConfigStore>,
        sink: Arc<dyn EventSink>,
        available: Vec<SkillMeta>,
    ) -> Self {
        Self {
            store,
            sink,
            available,
        }
    }
}

#[derive(serde::Deserialize)]
struct SkillArgs {
    name: String,
}

fn parameter_schema() -> serde_json::Value {
    serde_json::json!(
        {"type":"object","properties":{"name":{"type":"string"}}, "required":["name"]}
    )
}

impl ToolDyn for SkillTool {
    fn name(&self) -> String {
        "skill".to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        let available = self.available.clone();
        let description = if available.is_empty() {
            "Charge le corps complet d'un skill par son nom. Aucun skill disponible.".to_string()
        } else {
            let mut s = "Charge le corps complet d'un skill par son nom. Skills disponibles :\n"
                .to_string();
            for skill in &available {
                s.push_str(&format!("- {} : {}\n", skill.name, skill.description));
            }
            s
        };
        Box::pin(async move {
            ToolDefinition {
                name: "skill".to_string(),
                description,
                parameters: parameter_schema(),
            }
        })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, rig_core::tool::ToolError>> {
        let store = self.store.clone();
        let sink = self.sink.clone();
        let available = self.available.clone();
        Box::pin(async move {
            let parsed: SkillArgs = serde_json::from_str(&args)
                .map_err(|e| rig_core::tool::ToolError::ToolCallError(Box::new(e)))?;

            if !available.iter().any(|s| s.name == parsed.name) {
                return Err(rig_core::tool::ToolError::ToolCallError(Box::new(
                    crate::error::VnyError::UnknownReference("skill", parsed.name.clone()),
                )));
            }

            let body = store
                .load_skill(&parsed.name)
                .await
                .map_err(|e| rig_core::tool::ToolError::ToolCallError(Box::new(e)))?;

            sink.emit(ChatEvent::SkillLoaded {
                name: parsed.name.clone(),
            })
            .await;

            Ok(body)
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::event::ChatEvent;
    use crate::store::InMemoryConfigStore;
    use std::sync::Mutex;

    fn noop_store() -> Arc<dyn ConfigStore> {
        Arc::new(InMemoryConfigStore::default())
    }

    fn noop_sink() -> Arc<dyn EventSink> {
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

    // 1. name_is_skill
    #[test]
    fn name_is_skill() {
        let skill_tool = SkillTool::new(noop_store(), noop_sink(), Vec::new());
        assert_eq!(skill_tool.name(), "skill");
    }

    // 2. definition_embeds_skill_index
    #[tokio::test]
    async fn definition_embeds_skill_index() {
        let available = vec![
            SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            },
            SkillMeta {
                name: "web".to_string(),
                description: "Web search".to_string(),
            },
        ];
        let skill_tool = SkillTool::new(noop_store(), noop_sink(), available);
        let def = skill_tool.definition("".into()).await;

        assert!(def.description.contains("- pdf : PDF processing"));
        assert!(def.description.contains("- web : Web search"));
        assert_eq!(def.name, "skill");
        assert_eq!(def.parameters["properties"]["name"]["type"], "string");
        assert_eq!(def.parameters["required"], serde_json::json!(["name"]));
    }

    // 3. definition_empty_index_placeholder
    #[tokio::test]
    async fn definition_empty_index_placeholder() {
        let skill_tool = SkillTool::new(noop_store(), noop_sink(), Vec::new());
        let def = skill_tool.definition("".into()).await;

        assert!(def.description.contains("Aucun skill disponible"));
        assert!(!def.description.contains("- "));
    }

    // 4. call_success_returns_body_and_emits_skill_loaded
    #[tokio::test]
    async fn call_success_returns_body_and_emits_skill_loaded() {
        let store = Arc::new(InMemoryConfigStore {
            skill_bodies: {
                let mut m = std::collections::HashMap::new();
                m.insert("pdf".to_string(), "# PDF skill\ncontent".to_string());
                m
            },
            ..Default::default()
        });
        let sink = Arc::new(RecordingSink::new());
        let skill_tool = SkillTool::new(
            store,
            sink.clone(),
            vec![SkillMeta {
                name: "pdf".to_string(),
                description: "".to_string(),
            }],
        );

        let result = skill_tool
            .call(serde_json::json!({"name": "pdf"}).to_string())
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "# PDF skill\ncontent".to_string());
        assert_eq!(
            sink.events(),
            vec![ChatEvent::SkillLoaded {
                name: "pdf".to_string()
            }]
        );
    }

    // 7. call_skill_absent_from_selection_returns_error_no_emit
    #[tokio::test]
    async fn call_skill_absent_from_selection_returns_error_no_emit() {
        let store = Arc::new(InMemoryConfigStore {
            skill_bodies: {
                let mut m = std::collections::HashMap::new();
                m.insert("pdf".to_string(), "# PDF skill\ncontent".to_string());
                m
            },
            ..Default::default()
        });
        let sink = Arc::new(RecordingSink::new());
        let skill_tool = SkillTool::new(store, sink.clone(), Vec::new());

        let result = skill_tool
            .call(serde_json::json!({"name": "pdf"}).to_string())
            .await;

        assert!(result.is_err());
        assert!(sink.events().is_empty());
    }

    // 5. call_unknown_skill_returns_error_no_emit
    #[tokio::test]
    async fn call_unknown_skill_returns_error_no_emit() {
        let store = Arc::new(InMemoryConfigStore {
            skill_bodies: {
                let mut m = std::collections::HashMap::new();
                m.insert("pdf".to_string(), "# PDF skill\ncontent".to_string());
                m
            },
            ..Default::default()
        });
        let sink = Arc::new(RecordingSink::new());
        let skill_tool = SkillTool::new(store, sink.clone(), Vec::new());

        let result = skill_tool
            .call(serde_json::json!({"name": "absent"}).to_string())
            .await;

        assert!(result.is_err());
        assert!(sink.events().is_empty());
    }

    // 6. call_invalid_json_returns_error
    #[tokio::test]
    async fn call_invalid_json_returns_error() {
        let store = Arc::new(InMemoryConfigStore::default());
        let sink = Arc::new(RecordingSink::new());
        let skill_tool = SkillTool::new(store, sink.clone(), Vec::new());

        let result = skill_tool.call("not json".to_string()).await;

        assert!(result.is_err());
        assert!(sink.events().is_empty());
    }
}
