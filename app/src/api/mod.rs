pub mod agents;
pub mod conversations;
pub mod llm_providers;
pub mod local_tools;
pub mod mcp_servers;
pub mod me;
pub mod model_profiles;
pub mod owners;
pub mod projects;
pub mod sandboxes;
pub mod skills;
pub mod toolsets;

use axum::{
    routing::{get, post, put},
    Router,
};

use crate::AppState;

pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/local-tools", get(local_tools::list_local_tools))
        .route("/me", get(me::handler_me))
        // LLM providers (admin)
        .route(
            "/llm-providers",
            get(llm_providers::list_providers).post(llm_providers::create_provider),
        )
        .route(
            "/llm-providers/{id}",
            get(llm_providers::get_provider)
                .put(llm_providers::update_provider)
                .delete(llm_providers::delete_provider),
        )
        .route(
            "/llm-providers/{id}/test",
            post(llm_providers::test_provider),
        )
        .route(
            "/llm-providers/{id}/default",
            put(llm_providers::set_default_provider),
        )
        // MCP servers (admin)
        .route(
            "/mcp-servers",
            get(mcp_servers::list_servers).post(mcp_servers::create_server),
        )
        .route(
            "/mcp-servers/{id}",
            get(mcp_servers::get_server)
                .put(mcp_servers::update_server)
                .delete(mcp_servers::delete_server),
        )
        .route(
            "/mcp-servers/{id}/test",
            post(mcp_servers::test_server),
        )
        // Model profiles (OIDC, by name)
        .route(
            "/model-profiles",
            get(model_profiles::list_model_profiles).post(model_profiles::create_model_profile),
        )
        .route(
            "/model-profiles/{name}",
            get(model_profiles::get_model_profile)
                .put(model_profiles::update_model_profile)
                .delete(model_profiles::delete_model_profile),
        )
        // Toolsets (OIDC, by name)
        .route(
            "/toolsets",
            get(toolsets::list_toolsets).post(toolsets::create_toolset),
        )
        .route(
            "/toolsets/{name}",
            get(toolsets::get_toolset)
                .put(toolsets::update_toolset)
                .delete(toolsets::delete_toolset),
        )
        // Skills (OIDC, leaf resource)
        .route(
            "/skills",
            get(skills::list_skills).post(skills::create_skill),
        )
        .route(
            "/skills/{name}",
            get(skills::get_skill)
                .put(skills::update_skill)
                .delete(skills::delete_skill),
        )
        // Agents (read: OIDC, write: admin)
        .route(
            "/agents",
            get(agents::list_agents).post(agents::create_agent),
        )
        .route(
            "/agents/{name}",
            get(agents::get_agent)
                .put(agents::update_agent)
                .delete(agents::delete_agent),
        )
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
