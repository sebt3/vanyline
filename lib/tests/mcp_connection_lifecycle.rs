//! Fix lib MCP (R3 + R12) — test d'intégration bout-en-bout, avec un vrai
//! serveur MCP HTTP local (pas de mock) et le code réel de `vanyline-lib`.

use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use vanyline_lib::domain::{McpSelection, McpServer as DomainMcpServer, McpTransport};
use vanyline_lib::prefixed_mcp::{connect_mcp_servers_selected, new_tool_handle};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EchoRequest {
    text: String,
}

#[derive(Debug, Clone)]
struct EchoServer {
    tool_router: ToolRouter<Self>,
}

impl EchoServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl EchoServer {
    #[tool(description = "Echo back the given text")]
    fn echo(&self, Parameters(EchoRequest { text }): Parameters<EchoRequest>) -> String {
        text
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

type CapturedHeaders = Arc<Mutex<Option<BTreeMap<String, String>>>>;

/// Monte un serveur MCP HTTP réel sur 127.0.0.1:0. Retourne l'URL du endpoint
/// `/mcp`, le `CancellationToken` pour l'arrêter proprement, et un état
/// partagé contenant les derniers headers HTTP vus par le serveur.
async fn spawn_echo_server() -> (String, CancellationToken, CapturedHeaders) {
    let ct = CancellationToken::new();

    let service: StreamableHttpService<EchoServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(EchoServer::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(ct.child_token()),
        );

    let captured_headers: CapturedHeaders = Arc::new(Mutex::new(None));
    let captured_for_mw = captured_headers.clone();
    let capture_mw = axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let captured = captured_for_mw.clone();
            async move {
                let mut map = BTreeMap::new();
                for (k, v) in req.headers() {
                    if let Ok(vs) = v.to_str() {
                        map.insert(k.to_string(), vs.to_string());
                    }
                }
                *captured.lock().await = Some(map);
                next.run(req).await
            }
        },
    );

    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(capture_mw);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_ct = ct.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { server_ct.cancelled_owned().await })
            .await;
    });

    (format!("http://{addr}/mcp"), ct, captured_headers)
}

fn test_server(url: String, headers: BTreeMap<String, String>) -> DomainMcpServer {
    DomainMcpServer {
        name: "test-mcp".to_string(),
        transport: McpTransport::HttpStreamable,
        url,
        headers,
    }
}

fn test_selection() -> Vec<McpSelection> {
    vec![McpSelection {
        server: "test-mcp".to_string(),
        tools: vec![],
    }]
}

/// R3 : les headers custom configurés sur `McpServer.headers` doivent
/// atteindre le serveur MCP.
#[tokio::test]
async fn test_mcp_custom_headers_reach_server() {
    let (url, ct, captured_headers) = spawn_echo_server().await;

    let mut headers = BTreeMap::new();
    headers.insert("x-test-auth".to_string(), "hello-from-client".to_string());
    let server = test_server(url, headers);

    let handle = new_tool_handle();
    let connections = connect_mcp_servers_selected(&test_selection(), &[server], &handle)
        .await
        .expect("connect_mcp_servers_selected should succeed");

    let captured = captured_headers.lock().await;
    let map = captured
        .as_ref()
        .expect("server should have captured headers");
    assert_eq!(
        map.get("x-test-auth").map(String::as_str),
        Some("hello-from-client"),
        "x-test-auth header should have reached the server"
    );
    drop(captured);

    for conn in connections {
        let _ = conn.cancel().await;
    }
    ct.cancel();
}

/// R12 : deux appels de tool séquentiels sur la même connexion doivent tous
/// les deux réussir. Avant le fix, le second échoue (ou hang) car le
/// `RunningService` est droppé — et sa tâche de fond annulée — juste après
/// `list_all_tools()`, avant tout appel de tool réel dans le tour.
#[tokio::test]
async fn test_mcp_tool_survives_multiple_calls_in_same_connection() {
    let (url, ct, _headers) = spawn_echo_server().await;
    let server = test_server(url, BTreeMap::new());

    let handle = new_tool_handle();
    let connections = connect_mcp_servers_selected(&test_selection(), &[server], &handle)
        .await
        .expect("connect_mcp_servers_selected should succeed");

    // Les connexions et le handle restent en vie pendant les deux appels —
    // c'est exactement le contrat que `session.rs` doit respecter après le
    // fix (garder les McpRunningService en vie jusqu'à la fin du tour).

    let result1 = handle
        .call_tool("test-mcp/echo", "{\"text\":\"salut\"}")
        .await;
    assert_eq!(
        result1.expect("call #1 should succeed"),
        "salut",
        "first call should echo back the input"
    );

    let result2 = handle
        .call_tool("test-mcp/echo", "{\"text\":\"au revoir\"}")
        .await;
    assert_eq!(
        result2.expect("call #2 should succeed"),
        "au revoir",
        "second call should also succeed — this is the call that was broken by R12"
    );

    for conn in connections {
        let _ = conn.cancel().await;
    }
    ct.cancel();
}
