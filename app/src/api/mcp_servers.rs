use axum::{
    Json,
    extract::{Path, State},
};
use miryad_core::auth::AuthUser;
use miryad_core::rbac::can_write;
use miryad_core::users::resolve_user;
use sea_orm::{EntityTrait, IntoActiveModel};

use crate::{
    AppState,
    db::entities::mcp_servers::{Entity as McpServerEntity, Model as McpServerModel},
    error::AppError,
};
use vanyline_lib::domain::{McpServer as DomainMcpServer, McpTransport};

/// Mappe le Model `SeaORM` sur le type domaine `lib`. `server_type: "sse"` -> erreur
/// claire. `server_type: "sse"` mappe sur `McpTransport::Sse` — accepté, mais la
/// découverte de tools remonte `VNL-MCP-004` via `prefixed_mcp` (connexion SSE
/// pas encore implémentée). Pure, testable.
pub fn build_domain_server(server: &McpServerModel) -> Result<DomainMcpServer, AppError> {
    let transport = match server.server_type.as_str() {
        "http-streamable" => McpTransport::HttpStreamable,
        "sse" => McpTransport::Sse,
        other => {
            return Err(AppError::McpError(format!(
                "VNL-MCP-003: server_type must be 'sse' or 'http-streamable', got: {other}"
            )));
        }
    };
    let headers = match &server.headers {
        serde_json::Value::Object(map) => map
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
        _ => std::collections::BTreeMap::new(),
    };
    Ok(DomainMcpServer {
        name: server.name.clone(),
        transport,
        url: server.url.clone(),
        headers,
    })
}

#[derive(serde::Serialize)]
pub struct McpTestResult {
    pub tools: Vec<String>,
}

pub async fn test_server(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<McpTestResult>, AppError> {
    let db = state.auth.db.clone();
    let principal_user = resolve_user(&db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let server = McpServerEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::McpError("VNL-MCP-002: MCP server not found".to_string()))?;
    if !can_write::<McpServerEntity>(&db, &principal_user, &server)
        .await
        .map_err(AppError::from)?
    {
        return Err(AppError::Forbidden);
    }
    let domain = build_domain_server(&server)?;
    let tools = vanyline_lib::prefixed_mcp::list_mcp_server_tools(&domain)
        .await
        .map_err(|e| AppError::McpError(e.to_string()))?;

    // Persist available_tools = tools
    let mut active = server.clone().into_active_model();
    active.available_tools = sea_orm::ActiveValue::Set(tools.clone().into());
    McpServerEntity::update(active)
        .exec(&db)
        .await
        .map_err(AppError::from)?;

    Ok(Json(McpTestResult { tools }))
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {

    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    fn test_key() -> cookie::Key {
        cookie::Key::from(&[0u8; 64])
    }

    fn make_app() -> Router {
        // make_app() ne monte que /mcp-servers/{id}/test (POST) — les routes CRUD
        // sont gérées désormais par resource_router.
        let config = crate::config::Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
            oidc_connect_timeout: std::time::Duration::from_secs(5),
            oidc_timeout: std::time::Duration::from_secs(15),
            cookie_secret: "0".repeat(64),
            database_url: "postgres://localhost/test".to_string(),
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
        };

        let state = AppState {
            config,
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        };

        Router::new()
            .route("/mcp-servers/{id}/test", axum::routing::post(test_server))
            .with_state(state)
    }

    #[test]
    fn build_domain_server_http_streamable() {
        let server = McpServerModel {
            id: 1,
            name: "test-server".to_string(),
            server_type: "http-streamable".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({"Authorization": "Bearer token"}),
            available_tools: serde_json::json!([]),
        };
        let result = build_domain_server(&server).unwrap();
        assert_eq!(result.name, "test-server");
        assert_eq!(result.transport, McpTransport::HttpStreamable);
        assert_eq!(result.url, "http://localhost:3000/mcp");
        assert_eq!(
            result.headers.get("Authorization"),
            Some(&"Bearer token".to_string())
        );
    }

    #[test]
    fn build_domain_server_sse_maps_to_sse_transport() {
        let server = McpServerModel {
            id: 2,
            name: "sse-server".to_string(),
            server_type: "sse".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
        };
        let result = build_domain_server(&server).expect("sse accepté");
        assert_eq!(result.transport, McpTransport::Sse);
    }

    #[test]
    fn build_domain_server_bogus_type_returns_error() {
        let server = McpServerModel {
            id: 3,
            name: "bogus-server".to_string(),
            server_type: "bogus".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
        };
        let err = build_domain_server(&server).unwrap_err();
        match err {
            AppError::McpError(msg) => {
                assert!(msg.contains("VNL-MCP-003"));
            }
            _ => panic!("expected McpError"),
        }
    }

    #[tokio::test]
    async fn test_server_without_cookie_returns_401() {
        let app = make_app();
        let req = Request::builder()
            .uri("/mcp-servers/1/test")
            .method("POST")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Scenario : `build_domain_server_elimine_les_valeurs_de_header_non_chainees`.
    #[test]
    fn build_domain_server_elimine_les_valeurs_de_header_non_chainees() {
        let server = McpServerModel {
            id: 1,
            name: "srv".to_string(),
            server_type: "http-streamable".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({
                "Authorization": "Bearer x",
                "X-Port": 8443,
                "X-Flag": null,
                "X-Nested": {"k": 1}
            }),
            available_tools: serde_json::json!([]),
        };
        let result = build_domain_server(&server).expect("mapping succeeds");
        assert_eq!(result.headers.len(), 1);
        assert_eq!(
            result.headers.get("Authorization"),
            Some(&"Bearer x".to_string())
        );
    }

    /// Scenario : `build_domain_server_headers_non_objet_donne_une_map_vide`.
    #[test]
    fn build_domain_server_headers_non_objet_donne_une_map_vide() {
        for headers in [
            serde_json::Value::Null,
            serde_json::json!(["a", "b"]),
            serde_json::json!(42),
        ] {
            let server = McpServerModel {
                id: 1,
                name: "srv".to_string(),
                server_type: "http-streamable".to_string(),
                url: "http://localhost:3000/mcp".to_string(),
                headers,
                available_tools: serde_json::json!([]),
            };
            let result = build_domain_server(&server).expect("never Err on odd headers shape");
            assert!(result.headers.is_empty());
        }
    }

    /// Scenario : `mcp_test_result_serialize_en_un_tableau_sous_la_cle_tools`.
    #[test]
    fn mcp_test_result_serialize_en_un_tableau_sous_la_cle_tools() {
        let result = McpTestResult {
            tools: vec!["fs.read".to_string(), "fs.write".to_string()],
        };
        let json = serde_json::to_value(&result).expect("serialize McpTestResult");
        assert_eq!(json, serde_json::json!({"tools": ["fs.read", "fs.write"]}));
    }

    /// Sécurité (2026-09-27) : `headers` est écriture seule en REST, mais `build_domain_server`
    /// doit continuer à recopier `Authorization` depuis la ligne relue en base (chemin Rust
    /// natif, sans sérialisation JSON).
    #[tokio::test]
    async fn build_domain_server_uses_headers_stored_in_db() {
        use crate::db::entities::mcp_servers::ActiveModel;
        use sea_orm::ActiveValue::{NotSet, Set};

        let db = crate::db::test_support::real_db().await;
        let id = McpServerEntity::insert(ActiveModel {
            id: NotSet,
            name: Set("authed".to_string()),
            server_type: Set("http-streamable".to_string()),
            url: Set("http://localhost:3000/mcp".to_string()),
            headers: Set(serde_json::json!({"Authorization": "Bearer stored-token"})),
            available_tools: Set(serde_json::json!([])),
        })
        .exec(&db)
        .await
        .expect("insert")
        .last_insert_id;

        let row = McpServerEntity::find_by_id(id)
            .one(&db)
            .await
            .expect("query")
            .expect("row exists");
        let domain = build_domain_server(&row).expect("maps");
        assert_eq!(
            domain.headers.get("Authorization").map(String::as_str),
            Some("Bearer stored-token")
        );
    }

    /// `AppState` admin (groupe `admin`) sur la base sqlite de test.
    async fn admin_state() -> AppState {
        let state = AppState {
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
                database_url: "postgres://localhost/test".to_string(),
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
            },
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(
                crate::db::test_support::real_db().await,
            ),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        };
        let db = &state.auth.db;
        let admin = miryad_core::users::resolve_user(db, "admin-user", None)
            .await
            .expect("admin resolves");
        miryad_core::users::sync_group_memberships(db, admin.id, &["admin".to_string()])
            .await
            .expect("sync admin group");
        state
    }

    /// Cookie de session admin (même clé que `admin_state`).
    fn admin_cookie() -> String {
        let identity = miryad_core::auth::OidcIdentity {
            id_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.signature".to_string(),
            subject: "admin-user".to_string(),
            email: None,
            preferred_username: None,
        };
        miryad_core::auth::cookie::build_set_cookie(&identity, &test_key(), false)
            .split(';')
            .next()
            .expect("cookie has a name=value pair")
            .to_string()
    }

    async fn seed_server(state: &AppState, headers: serde_json::Value) -> i32 {
        use crate::db::entities::mcp_servers::ActiveModel;
        use sea_orm::ActiveValue::{NotSet, Set};
        McpServerEntity::insert(ActiveModel {
            id: NotSet,
            name: Set("seeded".to_string()),
            server_type: Set("http-streamable".to_string()),
            url: Set("http://localhost:3000/mcp".to_string()),
            headers: Set(headers),
            available_tools: Set(serde_json::json!([])),
        })
        .exec(&state.auth.db)
        .await
        .expect("server inserts")
        .last_insert_id
    }

    /// `PUT /api/v1/mcp-servers/{id}` via le `resource_router` réel de miryad-core ; rend le
    /// statut et le corps sans rien asserter.
    async fn put_server_raw(
        state: &AppState,
        id: i32,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, String) {
        let app = axum::Router::new()
            .merge(miryad_core::rest::resource_router::<
                McpServerEntity,
                AppState,
            >())
            .with_state(state.clone());
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/mcp-servers/{id}"))
            .header("Cookie", admin_cookie())
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("valid request");
        let resp = tower::ServiceExt::oneshot(app, req)
            .await
            .expect("router does not fail");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body reads");
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// `PUT` qui doit réussir.
    async fn put_server(state: &AppState, id: i32, body: serde_json::Value) {
        let (status, text) = put_server_raw(state, id, body).await;
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "PUT must succeed: {text}"
        );
    }

    async fn stored_server_type(state: &AppState, id: i32) -> String {
        McpServerEntity::find_by_id(id)
            .one(&state.auth.db)
            .await
            .expect("query")
            .expect("row exists")
            .server_type
    }

    async fn stored_headers(state: &AppState, id: i32) -> serde_json::Value {
        McpServerEntity::find_by_id(id)
            .one(&state.auth.db)
            .await
            .expect("query")
            .expect("row exists")
            .headers
    }

    /// Décision 2026-10-18 (`mcp_servers.sdd`, Tasks « MISE À JOUR DES SECRETS ») : un PUT sans
    /// `headers` conserve les en-têtes existants (bout en bout, `resource_router`).
    #[tokio::test]
    async fn rest_put_sans_headers_conserve_les_headers() {
        let state = admin_state().await;
        let id = seed_server(&state, serde_json::json!({"Authorization": "Bearer keep"})).await;
        let body = serde_json::json!({
            "name": "seeded",
            "server_type": "http-streamable",
            "url": "http://localhost:3000/mcp"
        });
        put_server(&state, id, body).await;
        assert_eq!(
            stored_headers(&state, id).await,
            serde_json::json!({"Authorization": "Bearer keep"})
        );
    }

    /// Objet non vide remplace les en-têtes existants.
    #[tokio::test]
    async fn rest_put_headers_non_vide_remplace() {
        let state = admin_state().await;
        let id = seed_server(&state, serde_json::json!({"Authorization": "Bearer old"})).await;
        let body = serde_json::json!({
            "name": "seeded",
            "server_type": "http-streamable",
            "url": "http://localhost:3000/mcp",
            "headers": {"Authorization": "Bearer new"}
        });
        put_server(&state, id, body).await;
        assert_eq!(
            stored_headers(&state, id).await,
            serde_json::json!({"Authorization": "Bearer new"})
        );
    }

    /// `headers: null` explicite efface les en-têtes (`{}` en base).
    #[tokio::test]
    async fn rest_put_headers_null_efface() {
        let state = admin_state().await;
        let id = seed_server(&state, serde_json::json!({"Authorization": "Bearer old"})).await;
        let body = serde_json::json!({
            "name": "seeded",
            "server_type": "http-streamable",
            "url": "http://localhost:3000/mcp",
            "headers": null
        });
        put_server(&state, id, body).await;
        assert_eq!(stored_headers(&state, id).await, serde_json::json!({}));
    }

    /// Scenario `put_server_type_invalide_rejete_ligne_intacte` (`mcp_servers.sdd`) : un PUT au
    /// `server_type` inconnu est rejeté avec `VNL-MCP-003` ; la ligne (type ET en-têtes)
    /// reste intacte.
    #[tokio::test]
    async fn rest_put_server_type_invalide_rejete_ligne_intacte() {
        let state = admin_state().await;
        let id = seed_server(&state, serde_json::json!({"Authorization": "Bearer keep"})).await;
        let body = serde_json::json!({
            "name": "seeded",
            "server_type": "bogus",
            "url": "http://localhost:3000/mcp"
        });
        let (status, text) = put_server_raw(&state, id, body).await;
        assert_eq!(
            status,
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "{text}"
        );
        assert!(text.contains("VNL-MCP-003"), "code rendu : {text}");
        assert_eq!(stored_server_type(&state, id).await, "http-streamable");
        assert_eq!(
            stored_headers(&state, id).await,
            serde_json::json!({"Authorization": "Bearer keep"})
        );
    }
}
