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
    routing::{any, get, post, put},
};

use crate::AppState;

/// Endpoints métier non-CRUD des ressources `resource_router` (`llm-providers`, `mcp-servers`) —
/// montés à part sous `/api/v1`, au même préfixe que le CRUD généré de ces mêmes ressources
/// (décision Phase 1 : "conservés en handlers axum custom, à côté de resource_router"). Vivaient
/// auparavant sous `/api` non versionné, en désaccord avec cette décision (trouvé en revue
/// Phase 3, cf. `docs/features/miryad-core-integration.md`).
pub fn api_v1_router() -> Router<AppState> {
    Router::new()
        .route(
            "/llm-providers/{id}/test",
            post(llm_providers::test_provider),
        )
        .route(
            "/llm-providers/{id}/default",
            put(llm_providers::set_default_provider),
        )
        .route("/mcp-servers/{id}/test", post(mcp_servers::test_server))
}

pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/local-tools", get(local_tools::list_local_tools))
        .route("/me", get(me::handler_me))
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
        .route("/sandboxes/{name}/git/{*path}", any(sandboxes::git_proxy))
}
