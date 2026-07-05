use rig_core::completion::ToolDefinition;
use rig_core::tool::server::ToolServerHandle;
use rig_core::tool::ToolDyn;
use rig_core::wasm_compat::WasmBoxedFuture;
use rmcp::model::Tool;
use rmcp::service::ServerSink;

use crate::error::VnyError;
use crate::types::McpServer;

/// Create a fresh, running tool-server handle. Callers add tools to it
/// (local tools and/or MCP tools) before handing it to `run_chat_turn`.
pub fn new_tool_handle() -> ToolServerHandle {
    rig_core::tool::server::ToolServer::new().run()
}

/// Connect to MCP servers and add their prefixed tools to an existing handle.
/// Per-server failures are logged and skipped; they do not abort the whole set.
pub async fn connect_mcp_servers_prefixed(
    servers: &[McpServer],
    handle: &ToolServerHandle,
) -> Result<(), VnyError> {
    for server in servers {
        match connect_mcp_server_inner(server).await {
            Ok((tools, client)) => {
                let prefixed_tools = PrefixedMcpTool::new(tools, client, &server.name);
                for tool in prefixed_tools {
                    if let Err(e) = handle.add_tool(tool).await {
                        tracing::warn!("failed to add prefixed tool: {e}");
                    }
                }
            }
            Err(e) => {
                tracing::warn!("skipping MCP server {}: {e}", server.name);
            }
        }
    }
    Ok(())
}

/// A tool that presents a prefixed name to the LLM but calls the MCP server
/// with the original (unprefixed) tool name.
#[derive(Clone)]
pub struct PrefixedMcpTool {
    original: Tool,
    prefixed: Tool,
    client: ServerSink,
}

impl PrefixedMcpTool {
    pub fn new(tools: Vec<Tool>, client: ServerSink, prefix: &str) -> Vec<Self> {
        let prefix = format!("{prefix}/");
        tools
            .into_iter()
            .map(|original| {
                let prefixed_name = format!("{}{}", prefix, original.name);
                let mut prefixed = Tool::new(
                    prefixed_name,
                    original.description.clone().unwrap_or(std::borrow::Cow::Borrowed("")),
                    original.input_schema.clone(),
                );
                prefixed.title = original.title.clone();
                prefixed.output_schema = original.output_schema.clone();
                prefixed.annotations = original.annotations.clone();
                prefixed.execution = original.execution.clone();
                prefixed.icons = original.icons.clone();
                prefixed.meta = original.meta.clone();
                Self {
                    original,
                    prefixed,
                    client: client.clone(),
                }
            })
            .collect()
    }
}

impl ToolDyn for PrefixedMcpTool {
    fn name(&self) -> String {
        self.prefixed.name.to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: self.prefixed.name.to_string(),
                description: self
                    .prefixed
                    .description
                    .clone()
                    .unwrap_or(std::borrow::Cow::from(""))
                    .to_string(),
                parameters: serde_json::to_value(&self.prefixed.input_schema).unwrap_or_default(),
            }
        })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, rig_core::tool::ToolError>> {
        let name = self.original.name.clone();
        let client = self.client.clone();
        Box::pin(async move {
            let arguments: Option<rmcp::model::JsonObject> =
                serde_json::from_str(&args).unwrap_or_default();

            let request = arguments
                .map(|a| rmcp::model::CallToolRequestParams::new(name.clone()).with_arguments(a))
                .unwrap_or_else(|| rmcp::model::CallToolRequestParams::new(name));

            let result = client
                .call_tool(request)
                .await
                .map_err(|e| {
                    rig_core::tool::ToolError::ToolCallError(Box::new(McpToolCallError(
                        e.to_string(),
                    )))
                })?;

            if let Some(true) = result.is_error {
                let error_msg = result
                    .content
                    .into_iter()
                    .map(|x| x.raw.as_text().map(|y| y.to_owned()))
                    .map(|x| x.map(|x| x.text))
                    .collect::<Option<Vec<String>>>();

                if let Some(msg) = error_msg {
                    return Err(rig_core::tool::ToolError::ToolCallError(Box::new(
                        McpToolCallError(msg.join("\n")),
                    )));
                }
                return Err(rig_core::tool::ToolError::ToolCallError(Box::new(
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
                        return Err(rig_core::tool::ToolError::ToolCallError(Box::new(
                            McpToolCallError(
                                "MCP tool returned audio content (not supported)".to_string(),
                            ),
                        )))
                    }
                    thing => {
                        return Err(rig_core::tool::ToolError::ToolCallError(Box::new(
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

async fn connect_mcp_server_inner(
    server: &McpServer,
) -> Result<(Vec<Tool>, ServerSink), VnyError> {
    match server.server_type.as_str() {
        "http-streamable" => {
            let transport =
                rmcp::transport::StreamableHttpClientTransport::from_uri(server.url.as_str());
            let running = rmcp::serve_client((), transport).await.map_err(|e| {
                VnyError::McpConnectError(server.name.clone(), e.to_string())
            })?;
            let server_sink = running.peer().clone();
            let tools = running.list_all_tools().await.map_err(|e| {
                VnyError::McpToolsError(server.name.clone(), e.to_string())
            })?;
            Ok((tools, server_sink))
        }
        "sse" => Err(VnyError::SseNotImplemented),
        other => Err(VnyError::UnknownServerType(other.to_string())),
    }
}
