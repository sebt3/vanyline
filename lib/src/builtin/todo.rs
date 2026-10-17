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
    /// Construit le tool pour UN tour, sans validation : le handle partagé est
    /// stocké tel quel, le contenu du mutex n'est jamais inspecté à la
    /// construction (cf. `./todo.sdd`).
    #[must_use]
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

            let mut guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    /// Construit le tool pour UN tour, sans validation : le handle partagé est
    /// stocké tel quel (cf. `./todo.sdd`).
    #[must_use]
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

            let guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    fn name_vaut_todowrite() {
        let t = TodoWriteTool::new(empty_state());
        assert_eq!(t.name(), "todowrite");
    }

    // 2. name_is_todoread
    #[test]
    fn name_vaut_todoread() {
        let t = TodoReadTool::new(empty_state());
        assert_eq!(t.name(), "todoread");
    }

    // 3. todowrite_definition_schema
    #[tokio::test]
    async fn todowrite_definition_schema() {
        let t = TodoWriteTool::new(empty_state());
        let def = t.definition(String::new()).await;
        assert_eq!(def.name, "todowrite");
        assert_eq!(def.parameters["required"], serde_json::json!(["todos"]));
        assert_eq!(def.parameters["properties"]["todos"]["type"], "array");
    }

    // 4. todoread_definition_schema
    #[tokio::test]
    async fn todoread_definition_schema() {
        let t = TodoReadTool::new(empty_state());
        let def = t.definition(String::new()).await;
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

    fn stored(state: &TodoState) -> Option<String> {
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Scenario `todowrite_definition_figee`.
    #[tokio::test]
    async fn todowrite_definition_figee() {
        let tool = TodoWriteTool::new(empty_state());
        let def = tool.definition("p".into()).await;
        assert_eq!(def.name, "todowrite");
        assert_eq!(
            def.description,
            "Remplace toute la liste de taches par la liste fournie. "
        );
        assert_eq!(def.parameters["required"], serde_json::json!(["todos"]));
        assert_eq!(def.parameters["properties"]["todos"]["type"], "array");
        assert_eq!(
            def.parameters["properties"]["todos"]["items"]["required"],
            serde_json::json!(["content"])
        );
    }

    /// Scenario `todoread_definition_figee`.
    #[tokio::test]
    async fn todoread_definition_figee() {
        let tool = TodoReadTool::new(empty_state());
        let def = tool.definition("p".into()).await;
        assert_eq!(def.name, "todoread");
        assert_eq!(
            def.description,
            "Renvoie la liste de taches courante (todowrite)."
        );
        assert_eq!(
            def.parameters,
            serde_json::json!({"type": "object", "properties": {}, "required": []})
        );
    }

    /// Scenario `todowrite_remplace_tout_le_state`.
    #[tokio::test]
    async fn todowrite_remplace_tout_le_state() {
        let state = seeded_state("[{\"content\":\"ancienne\"}]");
        let tool = TodoWriteTool::new(state.clone());
        let out = tool
            .call(r#"{"todos":[{"content":"a","status":"pending"},{"content":"b"}]}"#.into())
            .await
            .unwrap();
        assert!(out.contains("2 items"));
        let value: serde_json::Value = serde_json::from_str(&stored(&state).unwrap()).unwrap();
        let items = value.as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert!(!stored(&state).unwrap().contains("ancienne"));
    }

    /// Scenario `todowrite_liste_vide_ecrit_some_vide`.
    #[tokio::test]
    async fn todowrite_liste_vide_ecrit_some_vide() {
        let state = empty_state();
        let out = TodoWriteTool::new(state.clone())
            .call(r#"{"todos": []}"#.into())
            .await
            .unwrap();
        assert_eq!(out, "todo list updated (0 items)");
        assert_eq!(stored(&state).as_deref(), Some("[]"));
    }

    /// Scenario `todowrite_ne_revient_jamais_a_none`.
    #[tokio::test]
    async fn todowrite_ne_revient_jamais_a_none() {
        let state = seeded_state("[{\"content\":\"x\"}]");
        TodoWriteTool::new(state.clone())
            .call(r#"{"todos": []}"#.into())
            .await
            .unwrap();
        assert_eq!(stored(&state).as_deref(), Some("[]"));
        let read = TodoReadTool::new(state).call("{}".into()).await.unwrap();
        assert_eq!(read, "[]");
    }

    /// Scenario `todowrite_json_invalide_rejete_sans_effet`.
    #[tokio::test]
    async fn todowrite_json_invalide_rejete_sans_effet() {
        let state = empty_state();
        let err = TodoWriteTool::new(state.clone())
            .call("not json".into())
            .await
            .unwrap_err();
        assert!(matches!(err, rig_core::tool::ToolError::ToolCallError(_)));
        assert_eq!(stored(&state), None);
    }

    /// Scenario `todowrite_todos_non_tableau_rejete`.
    #[tokio::test]
    async fn todowrite_todos_non_tableau_rejete() {
        let original = "[{\"content\":\"x\"}]";
        let state = seeded_state(original);
        let err = TodoWriteTool::new(state.clone())
            .call(r#"{"todos":"pas-un-tableau"}"#.into())
            .await;
        assert!(err.is_err());
        assert_eq!(stored(&state).as_deref(), Some(original));
    }

    /// Scenario `todowrite_elements_hors_schema_acceptes`.
    #[tokio::test]
    async fn todowrite_elements_hors_schema_acceptes() {
        let state = empty_state();
        let out = TodoWriteTool::new(state.clone())
            .call(r#"{"todos":[42,null,{"status":"done"}]}"#.into())
            .await
            .unwrap();
        assert!(out.contains("3 items"));
        let value: serde_json::Value = serde_json::from_str(&stored(&state).unwrap()).unwrap();
        assert_eq!(value, serde_json::json!([42, null, {"status": "done"}]));
    }

    /// Scenario `todoread_retourne_le_state_brut`.
    #[tokio::test]
    async fn todoread_retourne_le_state_brut() {
        let raw = "[{\"content\":\"a\"},{\"content\":\"b\"}]";
        let out = TodoReadTool::new(seeded_state(raw))
            .call("{}".into())
            .await
            .unwrap();
        assert_eq!(out, raw);
    }

    /// Scenario `todoread_sans_state_retourne_message`.
    #[tokio::test]
    async fn todoread_sans_state_retourne_message() {
        let out = TodoReadTool::new(empty_state())
            .call("{}".into())
            .await
            .unwrap();
        assert_eq!(out, "no todo list yet");
    }

    /// Scenario `todoread_args_non_json_rejete_sans_effet`.
    #[tokio::test]
    async fn todoread_args_non_json_rejete_sans_effet() {
        let original = "[{\"content\":\"a\"}]";
        let state = seeded_state(original);
        let err = TodoReadTool::new(state.clone())
            .call("not json".into())
            .await
            .unwrap_err();
        assert!(matches!(err, rig_core::tool::ToolError::ToolCallError(_)));
        assert_eq!(stored(&state).as_deref(), Some(original));
    }

    /// Scenario `todoread_accepte_nimporte_quel_json_valide`.
    #[tokio::test]
    async fn todoread_accepte_nimporte_quel_json_valide() {
        let original = "[{\"content\":\"a\"}]";
        let out = TodoReadTool::new(seeded_state(original))
            .call("42".into())
            .await
            .unwrap();
        assert_eq!(out, original);
    }

    /// Scenario `etat_partage_entre_clones_de_arc`.
    #[tokio::test]
    async fn etat_partage_entre_clones_de_arc() {
        let state = empty_state();
        let writer = TodoWriteTool::new(state.clone());
        let reader = TodoReadTool::new(state.clone());
        writer
            .call(r#"{"todos":[{"content":"visible"}]}"#.into())
            .await
            .unwrap();
        let out = reader.call("{}".into()).await.unwrap();
        assert!(out.contains("visible"));
    }

    /// Scenario `reserialisation_normalisee`.
    #[tokio::test]
    async fn reserialisation_normalisee() {
        let state = empty_state();
        TodoWriteTool::new(state.clone())
            .call(r#"{ "todos" : [ { "status": "in_progress", "content":  "x" } ] }"#.into())
            .await
            .unwrap();
        assert_eq!(
            stored(&state).as_deref(),
            Some(r#"[{"content":"x","status":"in_progress"}]"#)
        );
    }

    /// Scenario `mutex_poisonne_reste_utilisable`.
    #[tokio::test]
    async fn mutex_poisonne_reste_utilisable() {
        let state = seeded_state("[{\"content\":\"tenu\"}]");
        let poisoner = state.clone();
        let handle = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("panique volontaire pour empoisonner le mutex");
        });
        assert!(handle.join().is_err());
        assert!(state.is_poisoned());

        let read = TodoReadTool::new(state.clone())
            .call("{}".into())
            .await
            .unwrap();
        assert_eq!(read, "[{\"content\":\"tenu\"}]");

        TodoWriteTool::new(state.clone())
            .call(r#"{"todos":[{"content":"neuf"}]}"#.into())
            .await
            .unwrap();
        assert!(stored(&state).unwrap().contains("neuf"));
    }
}
