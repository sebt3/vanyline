use rmcp::{serve_client, transport::StreamableHttpClientTransport};

use crate::error::VnyError;
use crate::types::McpServer;

pub async fn connect_mcp_server(
    server: &McpServer,
) -> Result<(Vec<rmcp::model::Tool>, rmcp::service::ServerSink), VnyError> {
    match server.server_type.as_str() {
        "http-streamable" => {
            let transport = StreamableHttpClientTransport::from_uri(server.url.as_str());
            let running = serve_client((), transport).await.map_err(|e| {
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
