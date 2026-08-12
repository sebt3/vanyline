use std::sync::Arc;

use rig_core::completion::ToolDefinition;
use rig_core::tool::ToolDyn;
use rig_core::wasm_compat::WasmBoxedFuture;

/// etat todo partage : `Arc<std::sync::Mutex<Option<String>>>` ou la String est
/// la serialisation JSON de la liste de taches (`[{"content":..., "status":...}]`),
/// `None` = aucun etat pose. Meme handle que `SessionContext.todo_state` (clone de
/// l'Arc), fourni par l'hote (CLI seme depuis `Conversation.todo`) et lu par le CLI
/// apres le tour pour persister.
type TodoState = Arc<std::sync::Mutex<Option<String>>>;

/// Tool builtin `todowrite(todos: [...]) -> confirmation`. REMPLACE tout l'etat
/// todo par la liste fournie (comportement opencode). Ecrit dans le handle partage.
pub struct TodoWriteTool {
    state: TodoState,
}

impl TodoWriteTool {
    pub fn new(state: TodoState) -> Self {
        Self { state }
    }
}

#[derive(serde::Deserialize)]
struct TodoWriteArgs {
    todos: Vec<serde_json::Value>,
}

impl TodoWriteTool {
    fn parameter_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "status": { "type": "string" }
                        },
                        "required": ["content"]
                    }
                }
            },
            "required": ["todos"]
        })
    }
}

impl ToolDyn for TodoWriteTool {
    fn name(&self) -> String {
        "todowrite".to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        let description = "Remplace toute la liste de taches par la liste fournie. ".to_string();
        Box::pin(async move {
            ToolDefinition {
                name: "todowrite".to_string(),
                description,
                parameters: TodoWriteTool::parameter_schema(),
            }
        })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, rig_core::tool::ToolError>> {
        let state = self.state.clone();
        Box::pin(async move {
            let parsed: TodoWriteArgs = serde_json::from_str(&args)
                .map_err(|e| rig_core::tool::ToolError::ToolCallError(Box::new(e)))?;

            let json = serde_json::to_string(&parsed.todos)
                .map_err(|e| rig_core::tool::ToolError::ToolCallError(Box::new(e)))?;

            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(json);

            Ok(format!("todo list updated ({} items)", parsed.todos.len()))
        })
    }
}

/// Tool builtin `todoread() -> etat todo courant`. Lit le handle partage et
/// renvoie la liste de taches (JSON), ou un message si aucun etat n'est pose.
pub struct TodoReadTool {
    state: TodoState,
}

impl TodoReadTool {
    pub fn new(state: TodoState) -> Self {
        Self { state }
    }
}

impl TodoReadTool {
    fn parameter_schema() -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }
}

impl ToolDyn for TodoReadTool {
    fn name(&self) -> String {
        "todoread".to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        let description = "Renvoie la liste de taches courante (todowrite).".to_string();
        Box::pin(async move {
            ToolDefinition {
                name: "todoread".to_string(),
                description,
                parameters: TodoReadTool::parameter_schema(),
            }
        })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, rig_core::tool::ToolError>> {
        let state = self.state.clone();
        Box::pin(async move {
            // todoread n'a pas de parametres utiles ; une entree mal formee est une erreur.
            let _parsed: serde_json::Value = serde_json::from_str(&args)
                .map_err(|e| rig_core::tool::ToolError::ToolCallError(Box::new(e)))?;

            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_deref() {
                Some(json) => Ok(json.to_string()),
                None => Ok("no todo list yet".to_string()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn empty_state() -> TodoState {
        Arc::new(std::sync::Mutex::new(None))
    }

    fn seeded_state(json: &str) -> TodoState {
        Arc::new(std::sync::Mutex::new(Some(json.to_string())))
    }

    // 1. name_is_todowrite
    #[test]
    fn name_is_todowrite() {
        let t = TodoWriteTool::new(empty_state());
        assert_eq!(t.name(), "todowrite");
    }

    // 2. name_is_todoread
    #[test]
    fn name_is_todoread() {
        let t = TodoReadTool::new(empty_state());
        assert_eq!(t.name(), "todoread");
    }

    // 3. todowrite_definition_schema
    #[tokio::test]
    async fn todowrite_definition_schema() {
        let t = TodoWriteTool::new(empty_state());
        let def = t.definition("".into()).await;
        assert_eq!(def.name, "todowrite");
        assert_eq!(def.parameters["required"], serde_json::json!(["todos"]));
        assert_eq!(def.parameters["properties"]["todos"]["type"], "array");
    }

    // 4. todoread_definition_schema
    #[tokio::test]
    async fn todoread_definition_schema() {
        let t = TodoReadTool::new(empty_state());
        let def = t.definition("".into()).await;
        assert_eq!(def.name, "todoread");
        assert_eq!(def.parameters["required"], serde_json::json!([]));
    }

    // 5. todowrite_replaces_state
    #[tokio::test]
    async fn todowrite_replaces_state() {
        let state = empty_state();
        let t = TodoWriteTool::new(state.clone());
        let args = serde_json::json!({
            "todos": [
                {"content": "ecrire les tests", "status": "pending"},
                {"content": "committer"}
            ]
        })
        .to_string();
        let result = t.call(args).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("2 items"));
        let guard = state.lock().unwrap();
        let stored = guard.as_deref().unwrap();
        let arr: serde_json::Value = serde_json::from_str(stored).unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 2);
        assert_eq!(arr[0]["content"], "ecrire les tests");
    }

    // 6. todoread_returns_state
    #[tokio::test]
    async fn todoread_returns_state() {
        let state = seeded_state("[{\"content\":\"a\"},{\"content\":\"b\"}]");
        let t = TodoReadTool::new(state.clone());
        let result = t.call("{}".to_string()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "[{\"content\":\"a\"},{\"content\":\"b\"}]");
    }

    // 7. todoread_empty_state
    #[tokio::test]
    async fn todoread_empty_state() {
        let t = TodoReadTool::new(empty_state());
        let result = t.call("{}".to_string()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "no todo list yet");
    }

    // 8. todowrite_invalid_json_errors
    #[tokio::test]
    async fn todowrite_invalid_json_errors() {
        let state = empty_state();
        let t = TodoWriteTool::new(state.clone());
        let result = t.call("not json".to_string()).await;
        assert!(result.is_err());
        let guard = state.lock().unwrap();
        assert!(guard.is_none());
    }

    // 9. todoread_invalid_json_errors
    #[tokio::test]
    async fn todoread_invalid_json_errors() {
        let state = seeded_state("[{\"content\":\"a\"}]");
        let t = TodoReadTool::new(state.clone());
        let result = t.call("not json".to_string()).await;
        assert!(result.is_err());
    }
}
