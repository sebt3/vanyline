pub mod llm_providers;
pub mod mcp_servers;
pub mod me;
pub mod owners;
pub mod projects;
pub mod sandboxes;
pub mod workflows;

use axum::{
    Router,
    routing::{any, get, post, put},
};

use crate::AppState;

/// Endpoints métier non-CRUD des ressources `resource_router` (`llm-providers`, `mcp-servers`) —
/// montés à part sous `/api/v1`, au même préfixe que le CRUD généré de ces mêmes ressources
/// (décision Phase 1 : "conservés en handlers axum custom, à côté de `resource_router`). Vivaient
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
        .route("/me", get(me::handler_me))
        // Projects (OIDC)
        .route(
            "/projects",
            get(projects::list_projects).post(projects::create_project),
        )
        .route(
            "/projects/{name}",
            get(projects::get_project).delete(projects::delete_project),
        )
        // Workflows (OIDC) — mêmes chemins non versionnés sous `/api` que projects/sandboxes
        // (api/workflows.sdd ; montée ratifiée W4 2026-10-10).
        .route(
            "/projects/{project}/workflows",
            get(workflows::list_workflows).post(workflows::create_workflow),
        )
        .route(
            "/projects/{project}/workflows/{id}",
            get(workflows::get_workflow)
                .put(workflows::update_workflow)
                .delete(workflows::delete_workflow),
        )
        // Runs de workflows (api/workflows.sdd, lot W4b — montées seules).
        .route(
            "/projects/{project}/workflows/{id}/runs",
            post(workflows::create_run),
        )
        .route(
            "/projects/{project}/runs",
            get(workflows::list_project_runs),
        )
        .route("/runs/{run_id}", get(workflows::get_run))
        .route("/runs/{run_id}/cancel", post(workflows::cancel_run))
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
        .route("/sandboxes/{name}/code-health", get(sandboxes::code_health))
        .route(
            "/sandboxes/{name}/code-health/refresh",
            post(sandboxes::code_health_refresh),
        )
        .route(
            "/sandboxes/{sandbox}/runs",
            get(workflows::list_sandbox_runs),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// `AppState` sans I/O : les Scenarios n'exercent que la table de routage, l'extracteur
    /// `AuthUser` refusant avant tout accès DB (précédent : `test_state` de
    /// `./llm_providers.rs`, ici avec `test_auth_state` mocké de `../auth/mod.rs`).
    fn test_state() -> AppState {
        AppState {
            config: crate::config::Config {
                oidc_issuer_url: "https://issuer.example.com".to_string(),
                oidc_client_id: "client-id".to_string(),
                oidc_client_secret: "client-secret".to_string(),
                oidc_redirect_url: "https://app.example.com/callback".to_string(),
                oidc_scopes: vec![],
                oidc_ca_cert: None,
                oidc_connect_timeout: std::time::Duration::from_secs(5),
                oidc_timeout: std::time::Duration::from_secs(15),
                cookie_secret: "0".repeat(64),
                database_url: "sqlite::memory:".to_string(),
                listen_addr: "0.0.0.0:8080".to_string(),
                static_dir: "./static".to_string(),
                k8s_namespace: None,
                application_name: None,
                default_home_storage_class: None,
                default_home_access_mode: None,
                default_project_storage_class: None,
                default_project_access_mode: None,
                restate_admin_url: None,
                restate_ingress_url: None,
                workflow_listen: "0.0.0.0:9080".to_string(),
                workflow_deployment_url: None,
                subagent_max_secs: 2700,
            },
            cookie_key: cookie::Key::from(&[0u8; 64]),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    /// Montage `nest("/api", …)` reproduisant `../main.rs`, puis une requête non authentifiée
    /// dont on ne retient que le statut.
    async fn ask(method: &str, uri: &str) -> StatusCode {
        let app = Router::new()
            .nest("/api", api_router())
            .with_state(test_state());
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("requête valide"),
        )
        .await
        .expect("le routeur répond")
        .status()
    }

    /// Scenario: `les routes du chat maison ne sont plus montées` — aucune route du chat
    /// maison retiré en C2 du chantier lymaille ne subsiste sur CE router.
    #[tokio::test]
    async fn routes_chat_maison_plus_montees() {
        for (method, uri) in [
            ("GET", "/api/conversations"),
            ("POST", "/api/conversations"),
            ("GET", "/api/conversations/7"),
            ("DELETE", "/api/conversations/7"),
            ("GET", "/api/local-tools"),
        ] {
            assert_eq!(
                ask(method, uri).await,
                StatusCode::NOT_FOUND,
                "{method} {uri} : aucune route du chat maison ne doit être montée"
            );
        }
    }

    /// Scenario: `api_router expose la route d'identité me` — route branchée (le refus
    /// d'authentification en aval est le contrat de `./me.rs`, pas de CE fichier).
    #[tokio::test]
    async fn api_router_expose_la_route_didentite_me() {
        assert_ne!(
            ask("GET", "/api/me").await,
            StatusCode::NOT_FOUND,
            "GET /api/me doit être monté"
        );
    }
}
