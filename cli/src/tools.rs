use rig_core::completion::ToolDefinition;
use rig_core::tool::ToolDyn;
use rig_core::tool::ToolError;
use rig_core::wasm_compat::WasmBoxedFuture;
use std::borrow::Cow;

pub async fn connect_mcp_servers_prefixed(
    servers: &[vanyline_lib::McpServer],
    handle: &rig_core::tool::server::ToolServerHandle,
) -> Result<(), String> {
    for server in servers {
        connect_mcp_server_prefixed(server, handle).await?;
    }
    Ok(())
}

async fn connect_mcp_server_prefixed(
    server: &vanyline_lib::McpServer,
    handle: &rig_core::tool::server::ToolServerHandle,
) -> Result<(), String> {
    let (tools, client) = match server.server_type.as_str() {
        "http-streamable" => {
            let transport =
                rmcp::transport::StreamableHttpClientTransport::from_uri(server.url.as_str());
            let running = rmcp::serve_client((), transport).await
                .map_err(|e| format!("connect {}: {}", server.name, e))?;
            let tools = running.list_all_tools().await
                .map_err(|e| format!("list tools {}: {}", server.name, e))?;
            (tools, running.peer().clone())
        }
        "sse" => return Err(format!("SSE transport not yet implemented for {}", server.name)),
        other => return Err(format!("Unknown server type {}: for {}", other, server.name)),
    };

    let prefix = format!("{}{}", server.name, "/");
    for original in tools {
        let prefixed_name = format!("{}{}", prefix, original.name);
        let mut prefixed = rmcp::model::Tool::new(
            prefixed_name,
            original.description.clone().unwrap_or(Cow::Borrowed("")),
            original.input_schema.clone(),
        );
        prefixed.title = original.title.clone();
        prefixed.output_schema = original.output_schema.clone();
        prefixed.annotations = original.annotations.clone();
        prefixed.execution = original.execution.clone();
        prefixed.icons = original.icons.clone();
        prefixed.meta = original.meta.clone();

        let name = original.name.clone();
        let client = client.clone();
        let tool = PrefixedMcpTool { original: prefixed, client, rpc_name: name };

        if let Err(e) = handle.add_tool(tool).await {
            tracing::warn!("failed to add prefixed tool {}: {e}", server.name);
        }
    }
    Ok(())
}

struct PrefixedMcpTool {
    original: rmcp::model::Tool,
    client: rmcp::service::ServerSink,
    rpc_name: Cow<'static, str>,
}

impl ToolDyn for PrefixedMcpTool {
    fn name(&self) -> String {
        self.original.name.to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: self.original.name.to_string(),
                description: self.original.description.clone().unwrap_or(Cow::Borrowed("")).to_string(),
                parameters: serde_json::to_value(&self.original.input_schema).unwrap_or_default(),
            }
        })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        let name = self.rpc_name.clone();
        let client = self.client.clone();
        Box::pin(async move {
            let arguments: Option<rmcp::model::JsonObject> =
                serde_json::from_str(&args).unwrap_or_default();

            let request = arguments
                .map(|a| rmcp::model::CallToolRequestParams::new(name.clone()).with_arguments(a))
                .unwrap_or_else(|| rmcp::model::CallToolRequestParams::new(name));

            let result = client.call_tool(request).await
                .map_err(|e| ToolError::ToolCallError(Box::new(McpToolCallError(e.to_string()))))?;

            if let Some(true) = result.is_error {
                let error_msg = result.content
                    .into_iter()
                    .map(|x| x.raw.as_text().map(|y| y.to_owned()))
                    .filter_map(|x| x.map(|x| x.text))
                    .collect::<Vec<String>>();

                if !error_msg.is_empty() {
                    return Err(ToolError::ToolCallError(Box::new(
                        McpToolCallError(error_msg.join("\n")),
                    )));
                }
                return Err(ToolError::ToolCallError(Box::new(
                    McpToolCallError("MCP tool error (no message)".to_string()),
                )));
            }

            let mut content = String::new();
            for item in result.content {
                let chunk = match item.raw {
                    rmcp::model::RawContent::Text(raw) => raw.text,
                    rmcp::model::RawContent::Image(raw) => {
                        format!("data:{};base64,{}", raw.mime_type, raw.data)
                    }
                    rmcp::model::RawContent::Resource(raw) => match raw.resource {
                        rmcp::model::ResourceContents::TextResourceContents {
                            uri,
                            mime_type,
                            text,
                            ..
                        } => {
                            format!(
                                "{mime_type}{uri}:{text}",
                                mime_type =
                                    mime_type.map(|m| format!("data:{m};")).unwrap_or_default(),
                            )
                        }
                        rmcp::model::ResourceContents::BlobResourceContents {
                            uri,
                            mime_type,
                            blob,
                            ..
                        } => format!(
                            "{mime_type}{uri}:{blob}",
                            mime_type =
                                mime_type.map(|m| format!("data:{m};")).unwrap_or_default(),
                        ),
                    },
                    rmcp::model::RawContent::Audio(_) => {
                        return Err(ToolError::ToolCallError(Box::new(
                            McpToolCallError("MCP tool returned audio content (not supported)".to_string()),
                        )))
                    }
                    thing => {
                        return Err(ToolError::ToolCallError(Box::new(
                            McpToolCallError(format!(
                                "MCP tool returned unsupported content: {thing:?}"
                            )),
                        )))
                    }
                };
                content.push_str(&chunk);
            }
            Ok(content)
        })
    }
}

#[derive(Debug)]
struct McpToolCallError(String);

impl std::fmt::Display for McpToolCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP tool call error: {}", self.0)
    }
}

impl std::error::Error for McpToolCallError {}

// ── Local tools ──────────────────────────────────────────────────────────────

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
            filesystem::read_file(filesystem::ReadFileOptions { path: args.path }).await
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
