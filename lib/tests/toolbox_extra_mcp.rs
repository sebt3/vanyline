//! Test dintegration : SessionContext.extra_mcp connecte bien un serveur
//! MCP reel pendant un tour, independamment des toolsets de l'agent -- c'est
//! le mecanisme sur lequel repose la toolbox (--toolbox, tache 05b).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ServerHandler,
};
use tokio_util::sync::CancellationToken;

use vanyline_lib::domain::{
    Agent, AgentMode, McpSelection, McpServer, McpTransport, ModelProfile, Provider,
    ProviderType, SkillSelection,
};
use vanyline_lib::event::{ChatEvent, EventSink};
use vanyline_lib::session::{run_agent_turn, SessionContext};
use vanyline_lib::store::InMemoryConfigStore;

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

/// Monte un serveur MCP HTTP reel sur 127.0.0.1:0 -- meme recette que
/// `mcp_connection_lifecycle.rs`. `contacted` passe a `true` des que le
/// serveur recoit une requete HTTP : seul moyen fiable de verifier depuis
/// l'exterieur que `run_agent_turn` a bien tente la connexion (le tour
/// echoue plus tard, a l'etape LLM -- voir `broken_llm_store`).
async fn spawn_echo_server() -> (String, CancellationToken, Arc<Mutex<bool>>) {
    let ct = CancellationToken::new();
    let contacted = Arc::new(Mutex::new(false));

    let service: StreamableHttpService<EchoServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(EchoServer::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(ct.child_token()),
        );

    let contacted_for_mw = contacted.clone();
    let mark_contacted = axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let contacted = contacted_for_mw.clone();
            async move {
                *contacted.lock().unwrap() = true;
                next.run(req).await
            }
        },
    );

    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(mark_contacted);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_ct = ct.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { server_ct.cancelled_owned().await })
            .await;
    });

    (format!("http://{addr}/mcp"), ct, contacted)
}

struct NoopSink;

#[async_trait::async_trait]
impl EventSink for NoopSink {
    async fn emit(&self, _event: ChatEvent) {}
}

/// Store minimal : un agent SANS toolset -- la connexion testee doit venir
/// UNIQUEMENT de `ctx.extra_mcp`, pas d'un toolset. Le provider pointe sur
/// un port ferme (127.0.0.1:1) pour un echec rapide et deterministe a
/// l'etape LLM, APRES que le cablage MCP (y compris extra_mcp) ait eu lieu.
fn broken_llm_store() -> InMemoryConfigStore {
    InMemoryConfigStore {
        providers: vec![Provider {
            name: "broken".to_string(),
            provider_type: ProviderType::OpenaiCompatible,
            endpoint: "http://127.0.0.1:1".to_string(),
            api_key: None,
        }],
        models: vec![ModelProfile {
            name: "broken-model".to_string(),
            provider: "broken".to_string(),
            model: "irrelevant".to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::Map::new(),
        }],
        agents: vec![Agent {
            name: "test-agent".to_string(),
            description: None,
            mode: AgentMode::Primary,
            model: "broken-model".to_string(),
            toolsets: vec![],
            skills: SkillSelection::None,
            system_prompt: "test".to_string(),
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn extra_mcp_server_is_contacted_during_turn() {
    let (url, ct, contacted) = spawn_echo_server().await;

    let ctx = SessionContext {
        store: Arc::new(broken_llm_store()),
        sink: Arc::new(NoopSink),
        local_tools: std::collections::HashMap::new(),
        subagent_depth_max: 1,
        extra_mcp: vec![(
            McpServer {
                name: "toolbox".to_string(),
                transport: McpTransport::HttpStreamable,
                url,
                headers: BTreeMap::new(),
            },
            McpSelection {
                server: "toolbox".to_string(),
                tools: vec![],
            },
        )],
        model_override: None,
    };

    // Le tour echoue forcement (LLM injoignable) -- ce n'est pas ce qu'on
    // verifie ici.
    let _ = run_agent_turn(&ctx, "test-agent", Vec::new(), "hello", None).await;

    assert!(
        *contacted.lock().unwrap(),
        "the extra_mcp server should have been contacted during the turn, \
         even though the turn itself fails later at the LLM step"
    );

    ct.cancel();
}