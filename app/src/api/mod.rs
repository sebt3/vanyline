pub mod conversations;
pub mod llm_providers;
pub mod local_tools;
pub mod mcp_servers;
pub mod me;
pub mod owners;
pub mod projects;
pub mod sandboxes;

use axum::{
    Router,
    routing::{get, post, put},
};

use crate::AppState;

pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/local-tools", get(local_tools::list_local_tools))
        .route("/me", get(me::handler_me))
        // LLM providers (admin, resource_router handles CRUD on /api/v1/llm-providers)
        .route(
            "/llm-providers/{id}/test",
            post(llm_providers::test_provider),
        )
        .route(
            "/llm-providers/{id}/default",
            put(llm_providers::set_default_provider),
        )
        // MCP servers (admin) — CRUD handled by resource_router, custom /test route below
        .route("/mcp-servers/{id}/test", post(mcp_servers::test_server))
        // Conversations (OIDC)
        .route(
            "/conversations",
            get(conversations::list_conversations).post(conversations::create_conversation),
        )
        .route(
            "/conversations/{id}",
            get(conversations::get_conversation).delete(conversations::delete_conversation),
        )
        .route(
            "/conversations/{id}/messages",
            get(conversations::get_messages),
        )
        // Projects (OIDC)
        .route(
            "/projects",
            get(projects::list_projects).post(projects::create_project),
        )
        .route(
            "/projects/{name}",
            get(projects::get_project).delete(projects::delete_project),
        )
        // Sandboxes (OIDC)
        .route(
            "/sandboxes",
            get(sandboxes::list_sandboxes).post(sandboxes::create_sandbox),
        )
        .route(
            "/sandboxes/{name}",
            get(sandboxes::get_sandbox).delete(sandboxes::delete_sandbox),
        )
        .route(
            "/sandboxes/{name}/suspend",
            post(sandboxes::set_sandbox_suspended),
        )
        .route("/sandboxes/{name}/ws-ticket", post(sandboxes::ws_ticket))
}
