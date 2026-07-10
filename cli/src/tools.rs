use rig_core::completion::ToolDefinition;
use rig_core::tool::ToolDyn;
use rig_core::tool::ToolError;
use rig_core::wasm_compat::WasmBoxedFuture;

use vanyline_tools::command;
use vanyline_tools::filesystem;

#[derive(Debug, serde::Deserialize)]
struct ReadFileArgs {
    path: String,
}

pub struct ReadFileTool;

impl ToolDyn for ReadFileTool {
    fn name(&self) -> String { "read_file".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: "read_file".into(),
                description: "Read the contents of a file".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string", "description": "Path to the file"}},
                    "required": ["path"]
                }),
            }
        })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ReadFileArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::read_file(filesystem::ReadFileOptions { path: args.path, ..Default::default() }).await
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
        Box::pin(async move {
            ToolDefinition {
                name: "write_file".into(),
                description: "Write content to a file, creating it if necessary".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Path to the file"},
                        "content": {"type": "string", "description": "File content"}
                    },
                    "required": ["path", "content"]
                }),
            }
        })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: WriteFileArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::write_file(filesystem::WriteFileOptions { path: args.path, content: args.content }).await
                .map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            Ok("".into())
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
        Box::pin(async move {
            ToolDefinition {
                name: "delete_file".into(),
                description: "Delete a file".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string", "description": "Path to the file"}},
                    "required": ["path"]
                }),
            }
        })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: DeleteFileArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::delete_file(filesystem::DeleteFileOptions { path: args.path }).await
                .map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            Ok("".into())
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct CreateDirectoryArgs {
    path: String,
}

pub struct CreateDirectoryTool;

impl ToolDyn for CreateDirectoryTool {
    fn name(&self) -> String { "create_directory".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: "create_directory".into(),
                description: "Create a directory, including parents if needed".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string", "description": "Path to the directory"}},
                    "required": ["path"]
                }),
            }
        })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: CreateDirectoryArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::create_directory(filesystem::CreateDirectoryOptions { path: args.path }).await
                .map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            Ok("".into())
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct ListDirectoryArgs {
    path: String,
}

pub struct ListDirectoryTool;

impl ToolDyn for ListDirectoryTool {
    fn name(&self) -> String { "list_directory".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: "list_directory".into(),
                description: "List the contents of a directory".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string", "description": "Path to the directory"}},
                    "required": ["path"]
                }),
            }
        })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ListDirectoryArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            filesystem::list_directory(filesystem::ListDirectoryOptions { path: args.path }).await
                .map_err(|e| ToolError::ToolCallError(Box::new(e)))
                .map(|v| serde_json::to_string(&v).unwrap_or_default())
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct ExecuteCommandArgs {
    command: String,
    timeout_secs: Option<u64>,
}

pub struct ExecuteCommandTool;

impl ToolDyn for ExecuteCommandTool {
    fn name(&self) -> String { "execute_command".into() }
    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: "execute_command".into(),
                description: "Execute a shell command and capture output".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "Shell command to execute"},
                        "timeout_secs": {"type": "integer", "description": "Timeout in seconds (default: 30)"}
                    },
                    "required": ["command"]
                }),
            }
        })
    }
    fn call(&self, params: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let args: ExecuteCommandArgs = serde_json::from_str(&params).map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            let result = command::execute(command::ExecuteCommandOptions {
                command: args.command,
                timeout_secs: args.timeout_secs.unwrap_or(30),
            }).await.map_err(|e| ToolError::ToolCallError(Box::new(e)))?;
            serde_json::to_string(&result).map_err(|e| ToolError::ToolCallError(Box::new(e)))
        })
    }
}

/// Create the full set of local tools available to the LLM.
pub fn local_tools() -> (
    ReadFileTool,
    WriteFileTool,
    DeleteFileTool,
    CreateDirectoryTool,
    ListDirectoryTool,
    ExecuteCommandTool,
) {
    (
        ReadFileTool,
        WriteFileTool,
        DeleteFileTool,
        CreateDirectoryTool,
        ListDirectoryTool,
        ExecuteCommandTool,
    )
}
