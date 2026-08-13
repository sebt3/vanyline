use rig_core::completion::ToolDefinition;
use rig_core::tool::server::ToolServerHandle;
use rig_core::tool::ToolDyn;
use rig_core::wasm_compat::WasmBoxedFuture;
use rmcp::model::Tool;
use rmcp::service::ServerSink;

use globset::Glob;

use crate::domain::{McpSelection, McpServer as DomainMcpServer, McpTransport};
use crate::error::VnyError;

/// McpRunningService générique pour un client MCP `()` (pas de handler côté
/// client — vanyline ne reçoit pas de requêtes serveur->client). Alias pour
/// lisibilité : c'est le type exact retourné par `rmcp::serve_client((), _)`.
pub type McpRunningService = rmcp::service::RunningService<rmcp::RoleClient, ()>;

/// Create a fresh, running tool-server handle. Callers add tools to it
/// (local tools and/or MCP tools) before handing it to `run_chat_turn`.
pub fn new_tool_handle() -> ToolServerHandle {
    rig_core::tool::server::ToolServer::new().run()
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
                    original
                        .description
                        .clone()
                        .unwrap_or(std::borrow::Cow::Borrowed("")),
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

            let result = client.call_tool(request).await.map_err(|e| {
                rig_core::tool::ToolError::ToolCallError(Box::new(McpToolCallError(e.to_string())))
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
                            mime_type = mime_type.map(|m| format!("data:{m};")).unwrap_or_default(),
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

// ---------------------------------------------------------------------------
// Toolset resolution — task-05: glob filtering & locale selection
// ---------------------------------------------------------------------------

/// Un pattern glob vide dans `McpSelection.tools` signifie "tous les outils"
/// (`"*"` implicite). Pattern invalide (glob mal formé) -> ne matche rien
/// (fail-safe : un outil non exposé plutôt qu'une erreur qui bloquerait toute
/// la connexion au serveur).
fn tool_matches(patterns: &[String], name: &str) -> bool {
    if patterns.is_empty() {
        return true;
    }
    patterns.iter().any(|pattern| {
        Glob::new(pattern)
            .ok()
            .map(|g| g.compile_matcher().is_match(name))
            .unwrap_or(false)
    })
}

/// Résout, pour un ensemble de `McpSelection`, les paires (sélection, serveur)
/// effectivement à contacter — UNE paire par sélection dont le `server` a une
/// correspondance dans `all_servers` ; une sélection sans correspondance est
/// omise (cf. "Choix de scope explicite" plus haut). Pure — aucune I/O. C'est ce
/// qui rend vérifiable, sans réseau, que "les serveurs non sélectionnés ne sont
/// jamais contactés" : `connect_mcp_servers_selected` ne boucle QUE sur le
/// résultat de cette fonction, jamais sur `all_servers` en entier.
fn selected_servers<'a>(
    selections: &'a [McpSelection],
    all_servers: &'a [DomainMcpServer],
) -> Vec<(&'a McpSelection, &'a DomainMcpServer)> {
    let mut result = Vec::with_capacity(selections.len());
    for selection in selections {
        if let Some(server) = all_servers.iter().find(|s| s.name == selection.server) {
            result.push((selection, server));
        }
    }
    result
}

/// Variante de `connect_mcp_server_inner` (existant, inchangé) pour le type
/// `domain::McpServer` (transport = enum `McpTransport`, plus de match sur une
/// String magique). `McpTransport` n'a aujourd'hui qu'une variante
/// (`HttpStreamable`) — un `match` exhaustif à un bras est volontaire : le jour
/// où une variante `Sse` est ajoutée à l'enum, le compilateur forcera à traiter
/// ce cas ici plutôt que de l'oublier silencieusement.
async fn connect_domain_mcp_server_inner(
    server: &DomainMcpServer,
) -> Result<(Vec<Tool>, ServerSink, McpRunningService), VnyError> {
    match server.transport {
        McpTransport::HttpStreamable => {
            let mut headers = std::collections::HashMap::new();
            for (name, value) in &server.headers {
                match (
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                    reqwest::header::HeaderValue::from_str(value),
                ) {
                    (Ok(name), Ok(value)) => {
                        headers.insert(name, value);
                    }
                    _ => tracing::warn!("invalid MCP header for server {}: {name}", server.name),
                }
            }
            let config = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                server.url.as_str(),
            )
            .custom_headers(headers);
            let transport = rmcp::transport::StreamableHttpClientTransport::from_config(config);
            let running = rmcp::serve_client((), transport)
                .await
                .map_err(|e| VnyError::McpConnectError(server.name.clone(), e.to_string()))?;
            let server_sink = running.peer().clone();
            let tools = running
                .list_all_tools()
                .await
                .map_err(|e| VnyError::McpToolsError(server.name.clone(), e.to_string()))?;
            Ok((tools, server_sink, running))
        }
    }
}

/// Liste les noms des tools exposés par un serveur MCP, sans les ajouter à un
/// handle. Réutilise `connect_domain_mcp_server_inner` (même logique de
/// connexion/listing) — alimente `POST /api/mcp-servers/{id}/test` (app).
pub async fn list_mcp_server_tools(
    server: &DomainMcpServer,
) -> Result<Vec<String>, VnyError> {
    let (tools, _client, _running) = connect_domain_mcp_server_inner(server).await?;
    Ok(tools.into_iter().map(|t| t.name.to_string()).collect())
}

/// Connecte UNIQUEMENT les serveurs référencés par `selections` (jamais
/// `all_servers` en entier) et n'ajoute au handle que les tools du serveur dont
/// le nom matche `tool_matches(&selection.tools, ..)`. Échec de connexion à un
/// serveur sélectionné : log + skip, comme `connect_mcp_servers_prefixed`
/// (existant) — une panne MCP n'abat pas les autres sélections.
/// Retourne les `McpRunningService` connectés — l'appelant est responsable de
/// les garder en vie (pour le fix R12) et de les annuler à la fin du tour.
pub async fn connect_mcp_servers_selected(
    selections: &[McpSelection],
    all_servers: &[DomainMcpServer],
    handle: &ToolServerHandle,
) -> Result<Vec<McpRunningService>, VnyError> {
    let mut connections = Vec::new();
    for (selection, server) in selected_servers(selections, all_servers) {
        match connect_domain_mcp_server_inner(server).await {
            Ok((tools, client, running)) => {
                let matching_tools: Vec<_> = tools
                    .into_iter()
                    .filter(|tool| tool_matches(&selection.tools, &tool.name))
                    .collect();
                let prefixed_tools = PrefixedMcpTool::new(matching_tools, client, &server.name);
                for tool in prefixed_tools {
                    if let Err(e) = handle.add_tool(tool).await {
                        tracing::warn!("failed to add prefixed tool: {e}");
                    }
                }
                connections.push(running);
            }
            Err(e) => {
                tracing::warn!("skipping selected MCP server {}: {e}", server.name);
            }
        }
    }
    Ok(connections)
}

/// Sélectionne, parmi les local tools fournis par l'hôte (`available` —
/// `SessionContext::local_tools` à la tâche 6, ici générique sur `V` pour rester
/// testable sans instancier de vrais `ToolDyn`), ceux demandés par
/// `Toolset.local_tools`. Retourne `(trouvés, absents)` — les absents sont à
/// logger par l'appelant (pas une erreur bloquante, même philosophie que les
/// échecs MCP par serveur).
pub fn select_local_tools<'a, V>(
    requested: &'a [String],
    available: &'a std::collections::HashMap<String, V>,
) -> (Vec<&'a str>, Vec<String>) {
    let mut found = Vec::with_capacity(requested.len());
    let mut missing = Vec::new();
    for name in requested {
        if available.contains_key(name) {
            found.push(name.as_str());
        } else {
            missing.push(name.clone());
        }
    }
    (found, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::McpServer as DomainMcpServer;
    use crate::domain::McpTransport;
    use std::collections::HashMap;

    // ---- tool_matches ----

    #[test]
    fn tool_matches_empty_patterns_matches_all() {
        assert!(tool_matches(&[], "anything"));
    }

    #[test]
    fn tool_matches_exact() {
        assert!(tool_matches(&["read_file".to_string()], "read_file"));
        assert!(!tool_matches(&["read_file".to_string()], "write_file"));
    }

    #[test]
    fn tool_matches_wildcard() {
        assert!(tool_matches(&["read_*".to_string()], "read_file"));
        assert!(!tool_matches(&["read_*".to_string()], "write_file"));
    }

    #[test]
    fn tool_matches_any_of_multiple_patterns() {
        assert!(tool_matches(
            &["foo".to_string(), "read_*".to_string()],
            "read_file"
        ));
    }

    #[test]
    fn tool_matches_invalid_pattern_matches_nothing() {
        assert!(!tool_matches(&["[".to_string()], "x"));
    }

    // ---- selected_servers ----

    #[test]
    fn selected_servers_only_referenced() {
        let all_servers = vec![
            DomainMcpServer {
                name: "a".to_string(),
                transport: McpTransport::HttpStreamable,
                url: "http://a".to_string(),
                headers: Default::default(),
            },
            DomainMcpServer {
                name: "b".to_string(),
                transport: McpTransport::HttpStreamable,
                url: "http://b".to_string(),
                headers: Default::default(),
            },
            DomainMcpServer {
                name: "c".to_string(),
                transport: McpTransport::HttpStreamable,
                url: "http://c".to_string(),
                headers: Default::default(),
            },
        ];
        let selections = vec![McpSelection {
            server: "b".to_string(),
            tools: vec![],
        }];
        let result = selected_servers(&selections, &all_servers);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.server, "b");
        assert_eq!(result[0].1.name, "b");
    }

    #[test]
    fn selected_servers_unknown_reference_omitted() {
        let all_servers = vec![DomainMcpServer {
            name: "a".to_string(),
            transport: McpTransport::HttpStreamable,
            url: "http://a".to_string(),
            headers: Default::default(),
        }];
        let selections = vec![McpSelection {
            server: "z".to_string(),
            tools: vec![],
        }];
        let result = selected_servers(&selections, &all_servers);
        assert!(result.is_empty());
    }

    // ---- select_local_tools ----

    #[test]
    fn select_local_tools_found_and_missing() {
        let mut available: HashMap<String, ()> = HashMap::new();
        available.insert("read_file".to_string(), ());
        available.insert("write_file".to_string(), ());
        let requested = vec!["read_file".to_string(), "delete_file".to_string()];
        let (found, missing) = select_local_tools(&requested, &available);
        assert_eq!(found, vec!["read_file"]);
        assert_eq!(missing, vec!["delete_file".to_string()]);
    }

    #[test]
    fn select_local_tools_empty_requested() {
        let available: HashMap<String, ()> = HashMap::new();
        let requested: Vec<String> = vec![];
        let (found, missing) = select_local_tools(&requested, &available);
        assert!(found.is_empty());
        assert!(missing.is_empty());
    }
}
