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
fn tool_definition(name: &str) -> ToolDefinition {
    mcp::filesystem_tools()
        .into_iter()
        .chain(mcp::search_tools())
        .chain(mcp::command_tools())
        .find(|t| t["name"] == name)
        .map(|t| ToolDefinition {
            name: t["name"].as_str().unwrap().to_string(),
            description: t["description"].as_str().unwrap().to_string(),
            parameters: t["inputSchema"].clone(),
        })
        .unwrap_or_else(|| panic!("no schema found for tool '{name}' in vanyline_tools::mcp"))
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
    fn name(&self) -> String { "read_file".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("read_file") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ReadFileArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::read_file(filesystem::ReadFileOptions { path: args.path, offset: args.offset, limit: args.limit }).await
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
    fn name(&self) -> String { "write_file".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("write_file") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: WriteFileArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            let len = args.content.len();
            let path = args.path.clone();
            filesystem::write_file(filesystem::WriteFileOptions { path: args.path, content: args.content }).await
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
    fn name(&self) -> String { "edit_file".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("edit_file") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: EditFileArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
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
    fn name(&self) -> String { "delete_file".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("delete_file") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: DeleteFileArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            let path = args.path.clone();
            filesystem::delete_file(filesystem::DeleteFileOptions { path: args.path }).await
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
    fn name(&self) -> String { "list_directory".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("list_directory") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ListDirectoryArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::list_directory(filesystem::ListDirectoryOptions { path: args.path, depth: args.depth }).await
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
    fn name(&self) -> String { "find_files".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("find_files") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: FindFilesArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            search::find_files(search::FindFilesOptions { pattern: args.pattern, path: args.path }).await
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
    fn name(&self) -> String { "search".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("search") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: SearchArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            search::search(search::SearchOptions { pattern: args.pattern, path: args.path, glob: args.glob }).await
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
    fn name(&self) -> String { "execute_command".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { tool_definition("execute_command") })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ExecuteCommandArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            command::execute(command::ExecuteCommandOptions {
                command: args.command,
                timeout_secs: args.timeout_secs.unwrap_or(30),
                cwd: args.cwd,
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