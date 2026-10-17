use std::collections::HashMap;
use std::sync::Arc;

use rig_core::completion::ToolDefinition;
use rig_core::tool::ToolDyn;
use rig_core::tool::ToolError;
use rig_core::wasm_compat::WasmBoxedFuture;

use vanyline_tools::command;
use vanyline_tools::filesystem;
use vanyline_tools::mcp;
use vanyline_tools::search;

/// Construit la `ToolDefinition` d'un outil à partir de `vanyline_tools::mcp` —
/// source unique des schémas, partagée avec la sandbox. Panique si `name` ne
/// correspond à aucun schéma : erreur de programmation (nom mal orthographié),
/// pas un cas d'exécution normal.
// tooling.sdd D6a — panic + `as_str().unwrap()` DOCUMENTÉS par tools.sdd
// §Raises : « erreurs de programmation (nom mal orthographié), jamais atteints
// avec les 8 noms actuels » (Scenario `tool_definition_inconnu_panique`).
// Conversion en erreur ou exemption assumée : en attente du [!] tools.sdd
// Tasks l.267 — arbitrage Sébastien, pas de décision prise ici.
#[allow(clippy::panic, clippy::unwrap_used)]
fn tool_definition(name: &str) -> ToolDefinition {
    mcp::filesystem_tools()
        .into_iter()
        .chain(mcp::search_tools())
        .chain(mcp::command_tools())
        .find(|t| t["name"] == name)
        .map_or_else(
            || panic!("no schema found for tool '{name}' in vanyline_tools::mcp"),
            |t| ToolDefinition {
                name: t["name"].as_str().unwrap().to_string(),
                description: t["description"].as_str().unwrap().to_string(),
                parameters: t["inputSchema"].clone(),
            },
        )
}

#[derive(Debug, serde::Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: usize,
}

pub struct ReadFileTool;

impl ToolDyn for ReadFileTool {
    fn name(&self) -> String {
        "read_file".into()
    }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("read_file") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ReadFileArgs =
                serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::read_file(filesystem::ReadFileOptions {
                path: args.path,
                offset: args.offset,
                limit: args.limit,
                raw: false,
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

pub struct WriteFileTool;

impl ToolDyn for WriteFileTool {
    fn name(&self) -> String {
        "write_file".into()
    }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("write_file") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: WriteFileArgs =
                serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            let len = args.content.len();
            let path = args.path.clone();
            filesystem::write_file(filesystem::WriteFileOptions {
                path: args.path,
                content: args.content,
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            Ok(format!("wrote {len} bytes to {path}"))
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

pub struct EditFileTool;

impl ToolDyn for EditFileTool {
    fn name(&self) -> String {
        "edit_file".into()
    }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("edit_file") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: EditFileArgs =
                serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::edit_file(filesystem::EditFileOptions {
                path: args.path,
                old_string: args.old_string,
                new_string: args.new_string,
                replace_all: args.replace_all,
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct DeleteFileArgs {
    path: String,
}

pub struct DeleteFileTool;

impl ToolDyn for DeleteFileTool {
    fn name(&self) -> String {
        "delete_file".into()
    }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("delete_file") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: DeleteFileArgs =
                serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            let path = args.path.clone();
            filesystem::delete_file(filesystem::DeleteFileOptions { path: args.path })
                .await
                .map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            Ok(format!("deleted {path}"))
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct ListDirectoryArgs {
    path: String,
    #[serde(default)]
    depth: usize,
}

pub struct ListDirectoryTool;

impl ToolDyn for ListDirectoryTool {
    fn name(&self) -> String {
        "list_directory".into()
    }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("list_directory") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ListDirectoryArgs =
                serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::list_directory(filesystem::ListDirectoryOptions {
                path: args.path,
                depth: args.depth,
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FindFilesArgs {
    pattern: String,
    #[serde(default)]
    path: String,
}

pub struct FindFilesTool;

impl ToolDyn for FindFilesTool {
    fn name(&self) -> String {
        "find_files".into()
    }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("find_files") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: FindFilesArgs =
                serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            search::find_files(search::FindFilesOptions {
                pattern: args.pattern,
                path: args.path,
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct SearchArgs {
    pattern: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    glob: String,
}

pub struct SearchTool;

impl ToolDyn for SearchTool {
    fn name(&self) -> String {
        "search".into()
    }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("search") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: SearchArgs =
                serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            search::search(search::SearchOptions {
                pattern: args.pattern,
                path: args.path,
                glob: args.glob,
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct ExecuteCommandArgs {
    command: String,
    timeout_secs: Option<u64>,
    #[serde(default)]
    cwd: String,
}

pub struct ExecuteCommandTool;

impl ToolDyn for ExecuteCommandTool {
    fn name(&self) -> String {
        "execute_command".into()
    }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("execute_command") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ExecuteCommandArgs =
                serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            command::execute(command::ExecuteCommandOptions {
                command: args.command,
                timeout_secs: args.timeout_secs.unwrap_or(30),
                cwd: args.cwd,
                // L'activation .venv est propre au sandbox ; le harness cli reste vierge.
                envs: Vec::new(),
            })
            .await
            .map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

/// Expose les 8 outils locaux CLI dans une hashmap indexée par nom.
pub fn local_tools_map() -> HashMap<String, Arc<dyn rig_core::tool::ToolDyn>> {
    let mut map: HashMap<String, Arc<dyn rig_core::tool::ToolDyn>> = HashMap::new();
    map.insert("read_file".to_string(), Arc::new(ReadFileTool));
    map.insert("write_file".to_string(), Arc::new(WriteFileTool));
    map.insert("edit_file".to_string(), Arc::new(EditFileTool));
    map.insert("delete_file".to_string(), Arc::new(DeleteFileTool));
    map.insert("list_directory".to_string(), Arc::new(ListDirectoryTool));
    map.insert("find_files".to_string(), Arc::new(FindFilesTool));
    map.insert("search".to_string(), Arc::new(SearchTool));
    map.insert("execute_command".to_string(), Arc::new(ExecuteCommandTool));
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::tool::ToolDyn;

    // Scenario: local_tools_map_expose_exactement_huit_tools
    #[test]
    fn local_tools_map_expose_exactement_huit_tools() {
        let map = local_tools_map();
        let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "delete_file",
                "edit_file",
                "execute_command",
                "find_files",
                "list_directory",
                "read_file",
                "search",
                "write_file",
            ]
        );
        for (key, tool) in &map {
            assert_eq!(key, &tool.name());
        }
    }

    // Scenario: definition_ignorer_le_prompt_et_rendre_le_schema_mcp
    #[tokio::test]
    async fn definition_ignorer_le_prompt_et_rendre_le_schema_mcp() {
        let tool = ReadFileTool;
        let a = tool.definition("prompt A".to_string()).await;
        let b = tool.definition("prompt B".to_string()).await;
        assert_eq!(a.name, "read_file");
        assert_eq!(a.name, b.name);
        assert_eq!(a.description, b.description);
        assert_eq!(a.parameters, b.parameters);
        let schema = mcp::filesystem_tools()
            .into_iter()
            .find(|t| t["name"] == "read_file")
            .map(|t| t["inputSchema"].clone());
        assert_eq!(Some(a.parameters), schema);
    }

    // Scenario: tool_definition_inconnu_panique
    #[test]
    #[should_panic(expected = "no schema found for tool")]
    fn tool_definition_inconnu_panique() {
        let _ = tool_definition("create_directory");
    }

    // Scenario: lecture_de_fichier_inexistant_enrobe_file_not_found
    #[tokio::test]
    async fn lecture_de_fichier_inexistant_enrobe_file_not_found() {
        let r = ReadFileTool
            .call(r#"{"path": "/defier/inexistant.txt"}"#.to_string())
            .await;
        match r {
            Err(ToolError::ToolCallError(e)) => {
                assert!(e.to_string().contains("/defier/inexistant.txt"), "{e}");
            }
            other => panic!("attendu ToolCallError, reçu {other:?}"),
        }
    }

    // Scenario: json_malforme_en_retourne_tool_call_error
    #[tokio::test]
    async fn json_malforme_en_retourne_tool_call_error() {
        let r = ReadFileTool.call("pas du json".to_string()).await;
        assert!(matches!(r, Err(ToolError::ToolCallError(_))));
    }

    // Scenario: requis_manquant_en_retourne_tool_call_error
    #[tokio::test]
    async fn requis_manquant_en_retourne_tool_call_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.txt");
        let params = format!(r#"{{"path": "{}"}}"#, path.display());
        let r = WriteFileTool.call(params).await;
        match r {
            Err(ToolError::ToolCallError(e)) => {
                assert!(e.to_string().contains("missing field `content`"), "{e}");
            }
            other => panic!("attendu ToolCallError, reçu {other:?}"),
        }
        assert!(!path.exists());
    }

    // Scenario: offset_negatif_en_retourne_tool_call_error
    #[tokio::test]
    async fn offset_negatif_en_retourne_tool_call_error() {
        let r = ReadFileTool
            .call(r#"{"path": "f.txt", "offset": -1}"#.to_string())
            .await;
        assert!(matches!(r, Err(ToolError::ToolCallError(_))));
    }

    // Scenario: write_file_confirme_en_octets
    #[tokio::test]
    async fn write_file_confirme_en_octets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("five.txt");
        let path_str = path.display().to_string();
        let params = format!(r#"{{"path": "{path_str}", "content": "hello"}}"#);
        let out = WriteFileTool.call(params).await.unwrap();
        assert_eq!(out, format!("wrote 5 bytes to {path_str}"));
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    // Scenario: delete_file_confirme_le_chemin_reçu
    #[tokio::test]
    async fn delete_file_confirme_le_chemin_recu() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.txt");
        std::fs::write(&path, "x").unwrap();
        let path_str = path.display().to_string();
        let out = DeleteFileTool
            .call(format!(r#"{{"path": "{path_str}"}}"#))
            .await
            .unwrap();
        assert_eq!(out, format!("deleted {path_str}"));
        assert!(!path.exists());
    }

    // Scenario: aucun_chemin_est_confine_au_repertoire_courant
    #[tokio::test]
    async fn aucun_chemin_est_confine_au_repertoire_courant() {
        let out = ListDirectoryTool
            .call(r#"{"path": "/tmp"}"#.to_string())
            .await;
        assert!(out.is_ok(), "{out:?}");
    }

    // Scenario: champs_inconnus_silencieusement_ignores
    #[tokio::test]
    async fn champs_inconnus_silencieusement_ignores() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup.txt");
        std::fs::write(&path, "a a").unwrap();
        let params = serde_json::json!({
            "path": path.display().to_string(),
            "old_string": "a",
            "new_string": "b",
            "replaceAll": true,
        })
        .to_string();
        let r = EditFileTool.call(params).await;
        assert!(r.is_err(), "replaceAll ne doit pas être pris en compte");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a a");
    }
}
