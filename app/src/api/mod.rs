pub mod agents;
pub mod conversations;
pub mod llm_providers;
pub mod mcp_servers;
pub mod me;

use axum::{
    routing::{get, post, put},
    Router,
};

use crate::AppState;

pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/me", get(me::handler_me))
        // LLM providers (admin)
        .route("/llm-providers", get(llm_providers::list_providers).post(llm_providers::create_provider))
        .route("/llm-providers/{id}", get(llm_providers::get_provider).put(llm_providers::update_provider).delete(llm_providers::delete_provider))
        .route("/llm-providers/{id}/test", post(llm_providers::test_provider))
        .route("/llm-providers/{id}/default", put(llm_providers::set_default_provider))
        // MCP servers (admin)
        .route("/mcp-servers", get(mcp_servers::list_servers).post(mcp_servers::create_server))
        .route("/mcp-servers/{id}", get(mcp_servers::get_server).put(mcp_servers::update_server).delete(mcp_servers::delete_server))
        // Agents (read: OIDC, write: admin)
        .route("/agents", get(agents::list_agents).post(agents::create_agent))
        .route("/agents/{id}", get(agents::get_agent).put(agents::update_agent).delete(agents::delete_agent))
        // Conversations (OIDC)
        .route("/conversations", get(conversations::list_conversations).post(conversations::create_conversation))
        .route("/conversations/{id}", get(conversations::get_conversation).delete(conversations::delete_conversation))
        .route("/conversations/{id}/messages", get(conversations::get_messages))
}
