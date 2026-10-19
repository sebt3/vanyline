use axum::{
    Json,
    extract::{Path, State},
};
use miryad_core::auth::AuthUser;
use miryad_core::rbac::can_write;
use miryad_core::users::resolve_user;
use sea_orm::{ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, sea_query::Expr};

use crate::{
    AppState, db::entities::llm_providers::Column,
    db::entities::llm_providers::Entity as LlmProviderEntity,
    db::entities::llm_providers::Model as LlmProviderModel, error::AppError,
};

#[derive(serde::Serialize)]
pub struct TestResult {
    pub models: Vec<String>,
}

pub async fn set_default_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<LlmProviderModel>, AppError> {
    let db = state.auth.db.clone();
    let principal_user = resolve_user(&db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let provider = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(AppError::from)?
        .ok_or(AppError::LlmProviderNotFound)?;
    if !can_write::<LlmProviderEntity>(&db, &principal_user, &provider)
        .await
        .map_err(AppError::from)?
    {
        return Err(AppError::Forbidden);
    }
    // Deux `UPDATE` bulk (pas un fetch-all + une boucle par ligne + deux re-fetches
    // redondants du même provider, cf. revue Phase 3 miryad-core-integration) : is_default =
    // FALSE partout, puis TRUE sur `id`. Le provider déjà en main (RBAC ci-dessus) est réutilisé
    // pour la réponse plutôt que refetché.
    LlmProviderEntity::update_many()
        .col_expr(Column::IsDefault, Expr::value(false))
        .exec(&db)
        .await
        .map_err(AppError::from)?;
    LlmProviderEntity::update_many()
        .col_expr(Column::IsDefault, Expr::value(true))
        .filter(Column::Id.eq(id))
        .exec(&db)
        .await
        .map_err(AppError::from)?;

    let mut result = provider;
    result.is_default = true;
    Ok(Json(result))
}

pub async fn test_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<TestResult>, AppError> {
    let db = state.auth.db.clone();
    let principal_user = resolve_user(&db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let provider = LlmProviderEntity::find_by_id(id)
        .one(&db)
        .await
        .map_err(AppError::from)?
        .ok_or(AppError::LlmProviderNotFound)?;
    if !can_write::<LlmProviderEntity>(&db, &principal_user, &provider)
        .await
        .map_err(AppError::from)?
    {
        return Err(AppError::Forbidden);
    }
    let models = discover_models(&provider).await?;

    // Persiste `available_models`, sans re-fetch mort après coup (la réponse ne porte que
    // `models`, cf. revue Phase 3 miryad-core-integration).
    let mut active = provider.into_active_model();
    active.available_models = sea_orm::ActiveValue::Set(models.clone().into());
    LlmProviderEntity::update(active)
        .exec(&db)
        .await
        .map_err(AppError::from)?;

    Ok(Json(TestResult { models }))
}

/// URL de découverte des modèles selon le type (`llm_providers.sdd`, 2026-10-19) : `endpoint` est
/// la RACINE du serveur ; la concaténation suit un `/` final retiré, sans validation de schéma.
pub fn models_url(provider_type: &str, endpoint: &str) -> Result<String, AppError> {
    let root = endpoint.trim_end_matches('/');
    match provider_type {
        "ollama" => Ok(format!("{root}/api/tags")),
        "openai-compatible" => Ok(format!("{root}/v1/models")),
        other => Err(AppError::LlmError(format!(
            "VNL-LLM-005: unknown provider type: {other}"
        ))),
    }
}

/// Indice rendu avec le diagnostic HTTP quand `endpoint` se termine par `/v1` (le chemin `/v1`
/// est ajouté par la lib). Texte sans le séparateur ` — `, ajouté par l'appelant.
pub fn endpoint_hint(endpoint: &str) -> Option<String> {
    endpoint.trim_end_matches('/').ends_with("/v1").then(|| {
        "l'endpoint ne doit pas se terminer par /v1 : le chemin /v1 est ajouté automatiquement"
            .to_string()
    })
}

async fn discover_models(provider: &LlmProviderModel) -> Result<Vec<String>, AppError> {
    let url = models_url(&provider.provider_type, &provider.endpoint)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::LlmError(format!("VNL-LLM-003: {e}")))?;

    let mut req = client.get(&url);
    // La clé stockée part en `Authorization: Bearer` pour tous les types (ollama inclus).
    if let Some(ref key) = provider.api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AppError::LlmError(format!("VNL-LLM-004: {e}")))?;

    // Réponse non 2xx refusée AVANT tout décodage (diagnostic lisible, pas « error decoding »).
    let status = resp.status();
    if !status.is_success() {
        let hint = endpoint_hint(&provider.endpoint)
            .map(|h| format!(" — {h}"))
            .unwrap_or_default();
        return Err(AppError::LlmError(format!(
            "VNL-LLM-009: HTTP {} sur {url}{hint}",
            status.as_u16()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AppError::LlmError(format!("VNL-LLM-004: {e}")))?;

    let models = if provider.provider_type == "ollama" {
        body["models"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| m["name"].as_str().map(std::string::ToString::to_string))
            .collect()
    } else {
        body["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| m["id"].as_str().map(std::string::ToString::to_string))
            .collect()
    };
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::llm_providers::ActiveModel;
    use miryad_core::users::sync_group_memberships;
    use sea_orm::ActiveValue::{NotSet, Set};

    fn test_config() -> crate::config::Config {
        crate::config::Config {
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
        }
    }

    async fn test_state() -> AppState {
        let db = crate::db::test_support::real_db().await;
        AppState {
            config: test_config(),
            cookie_key: cookie::Key::from(&[0u8; 64]),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    fn admin_user() -> AuthUser {
        AuthUser {
            subject: "admin-user".to_string(),
            email: None,
            id_token: "test-id-token".to_string(),
        }
    }

    /// Régression Phase 3 (miryad-core-integration) : `set_default_provider` doit désactiver
    /// l'ancien défaut et activer le nouveau via deux `UPDATE` bulk, sans dépendre d'une boucle
    /// par ligne ni de re-fetches redondants.
    #[tokio::test]
    async fn set_default_provider_switches_the_flag() {
        let state = test_state().await;
        let db = &state.auth.db;
        let admin = resolve_user(db, "admin-user", None)
            .await
            .expect("admin resolves");
        sync_group_memberships(db, admin.id, &["admin".to_string()])
            .await
            .expect("sync admin group");

        let provider_a = LlmProviderEntity::insert(ActiveModel {
            id: NotSet,
            name: Set("a".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(true),
        })
        .exec(db)
        .await
        .expect("provider a inserts")
        .last_insert_id;

        let provider_b = LlmProviderEntity::insert(ActiveModel {
            id: NotSet,
            name: Set("b".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(false),
        })
        .exec(db)
        .await
        .expect("provider b inserts")
        .last_insert_id;

        let Json(result) =
            set_default_provider(State(state.clone()), admin_user(), Path(provider_b))
                .await
                .expect("set_default_provider succeeds");
        assert!(result.is_default);

        let a = LlmProviderEntity::find_by_id(provider_a)
            .one(db)
            .await
            .expect("query a")
            .expect("a exists");
        assert!(!a.is_default, "previous default must be unset");
    }

    /// Sécurité (2026-09-27) : `api_key` est écriture seule en REST, mais `discover_models`
    /// doit toujours envoyer la clé stockée en `Authorization: Bearer` (lecture interne de la
    /// ligne relue en base). Serveur HTTP factice local qui capture la requête.
    #[tokio::test]
    async fn discover_models_sends_api_key_stored_in_db() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let state = test_state().await;
        let db = &state.auth.db;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds");
        let port = listener.local_addr().expect("local addr").port();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accepts");
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut chunk).await.expect("reads");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
            }
            let body = r#"{"data":[{"id":"m1"}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.expect("writes");
            String::from_utf8_lossy(&buf).to_string()
        });

        let id = LlmProviderEntity::insert(ActiveModel {
            id: NotSet,
            name: Set("authed-provider".to_string()),
            provider_type: Set("openai-compatible".to_string()),
            endpoint: Set(format!("http://127.0.0.1:{port}")),
            api_key: Set(Some("sk-stored".to_string())),
            available_models: Set(serde_json::json!([])),
            is_default: Set(false),
        })
        .exec(db)
        .await
        .expect("provider inserts")
        .last_insert_id;
        let row = LlmProviderEntity::find_by_id(id)
            .one(db)
            .await
            .expect("query")
            .expect("row exists");

        let models = discover_models(&row).await.expect("discovery succeeds");
        let request = server.await.expect("server task joins");

        assert_eq!(models, vec!["m1".to_string()]);
        assert!(
            request.contains("authorization: Bearer sk-stored")
                || request.contains("Authorization: Bearer sk-stored"),
            "stored api_key must be sent as Bearer, request was: {request}"
        );
    }

    /// `AppState` admin (groupe `admin`) sur la base sqlite de test.
    async fn admin_state() -> AppState {
        let state = test_state().await;
        let db = &state.auth.db;
        let admin = resolve_user(db, "admin-user", None)
            .await
            .expect("admin resolves");
        sync_group_memberships(db, admin.id, &["admin".to_string()])
            .await
            .expect("sync admin group");
        state
    }

    /// Cookie de session admin (même clé que `test_state`).
    fn admin_cookie() -> String {
        let identity = miryad_core::auth::OidcIdentity {
            id_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.signature".to_string(),
            subject: "admin-user".to_string(),
            email: None,
            preferred_username: None,
        };
        miryad_core::auth::cookie::build_set_cookie(
            &identity,
            &cookie::Key::from(&[0u8; 64]),
            false,
        )
        .split(';')
        .next()
        .expect("cookie has a name=value pair")
        .to_string()
    }

    async fn seed_provider(state: &AppState, api_key: Option<&str>) -> i32 {
        LlmProviderEntity::insert(ActiveModel {
            id: NotSet,
            name: Set("seeded".to_string()),
            provider_type: Set("openai-compatible".to_string()),
            endpoint: Set("https://llm.example.com".to_string()),
            api_key: Set(api_key.map(str::to_string)),
            available_models: Set(serde_json::json!([])),
            is_default: Set(false),
        })
        .exec(&state.auth.db)
        .await
        .expect("provider inserts")
        .last_insert_id
    }

    /// `PUT /api/v1/llm-providers/{id}` via le `resource_router` réel de miryad-core ; rend le
    /// statut et le corps sans rien asserter.
    async fn put_provider_raw(
        state: &AppState,
        id: i32,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, String) {
        let app = axum::Router::new()
            .merge(miryad_core::rest::resource_router::<
                LlmProviderEntity,
                AppState,
            >())
            .with_state(state.clone());
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/llm-providers/{id}"))
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

    /// `PUT` qui doit réussir ; rend le corps.
    async fn put_provider(state: &AppState, id: i32, body: serde_json::Value) -> String {
        let (status, text) = put_provider_raw(state, id, body).await;
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "PUT must succeed: {text}"
        );
        text
    }

    async fn stored_api_key(state: &AppState, id: i32) -> Option<String> {
        LlmProviderEntity::find_by_id(id)
            .one(&state.auth.db)
            .await
            .expect("query")
            .expect("row exists")
            .api_key
    }

    /// Décision 2026-10-18 (`llm_providers.sdd`, Tasks « MISE À JOUR DES SECRETS ») : un PUT
    /// sans `api_key` conserve la clé existante (bout en bout, `resource_router`).
    #[tokio::test]
    async fn rest_put_sans_api_key_conserve_la_cle() {
        let state = admin_state().await;
        let id = seed_provider(&state, Some("sk-secret-123")).await;
        let body = serde_json::json!({
            "name": "seeded",
            "provider_type": "openai-compatible",
            "endpoint": "https://llm.example.com"
        });
        put_provider(&state, id, body).await;
        assert_eq!(
            stored_api_key(&state, id).await.as_deref(),
            Some("sk-secret-123")
        );
    }

    /// `api_key: ""` efface la clé.
    #[tokio::test]
    async fn rest_put_api_key_vide_efface_la_cle() {
        let state = admin_state().await;
        let id = seed_provider(&state, Some("sk-secret-123")).await;
        let body = serde_json::json!({
            "name": "seeded",
            "provider_type": "openai-compatible",
            "endpoint": "https://llm.example.com",
            "api_key": ""
        });
        put_provider(&state, id, body).await;
        assert_eq!(stored_api_key(&state, id).await, None);
    }

    /// `api_key` non vide remplace la clé existante.
    #[tokio::test]
    async fn rest_put_api_key_non_vide_remplace_la_cle() {
        let state = admin_state().await;
        let id = seed_provider(&state, Some("sk-secret-123")).await;
        let body = serde_json::json!({
            "name": "seeded",
            "provider_type": "openai-compatible",
            "endpoint": "https://llm.example.com",
            "api_key": "sk-new"
        });
        put_provider(&state, id, body).await;
        assert_eq!(stored_api_key(&state, id).await.as_deref(), Some("sk-new"));
    }

    /// Scenario `put_provider_type_invalide_rejete_ligne_intacte` (`llm_providers.sdd`) : un PUT
    /// au `provider_type` inconnu est rejeté avec `VNL-LLM-005` ; la ligne (type ET clé) reste
    /// intacte.
    #[tokio::test]
    async fn rest_put_provider_type_invalide_rejete_ligne_intacte() {
        let state = admin_state().await;
        let id = seed_provider(&state, Some("sk-secret-123")).await;
        let body = serde_json::json!({
            "name": "seeded",
            "provider_type": "bogus",
            "endpoint": "https://llm.example.com"
        });
        let (status, text) = put_provider_raw(&state, id, body).await;
        assert_eq!(
            status,
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "{text}"
        );
        assert!(text.contains("VNL-LLM-005"), "code rendu : {text}");
        let row = LlmProviderEntity::find_by_id(id)
            .one(&state.auth.db)
            .await
            .expect("query")
            .expect("row exists");
        assert_eq!(row.provider_type, "openai-compatible");
        assert_eq!(row.api_key.as_deref(), Some("sk-secret-123"));
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    fn provider(provider_type: &str, endpoint: &str, api_key: Option<&str>) -> LlmProviderModel {
        LlmProviderModel {
            id: 1,
            name: "p".to_string(),
            provider_type: provider_type.to_string(),
            endpoint: endpoint.to_string(),
            api_key: api_key.map(str::to_string),
            available_models: serde_json::json!([]),
            is_default: false,
        }
    }

    /// Serveur HTTP loopback jetable : accepte une connexion, capture la requête (en-têtes
    /// inclus) et répond `status_line` + `body`. Rend le port et la requête capturée.
    async fn serve_once(
        status_line: &'static str,
        body: &'static str,
    ) -> (u16, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds");
        let port = listener.local_addr().expect("local addr").port();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accepts");
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut chunk).await.expect("reads");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
            }
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.expect("writes");
            String::from_utf8_lossy(&buf).to_string()
        });
        (port, handle)
    }

    // Scenario: models_url_construit_la_requete_selon_le_type
    #[test]
    fn models_url_construit_la_requete_selon_le_type() {
        assert_eq!(
            models_url("ollama", "http://h:11434/").expect("ollama"),
            "http://h:11434/api/tags"
        );
        assert_eq!(
            models_url("openai-compatible", "http://h:11434/").expect("openai"),
            "http://h:11434/v1/models"
        );
        let err = models_url("inconnu", "http://h:11434/").expect_err("type inconnu");
        assert!(
            err.to_string().contains("VNL-LLM-005"),
            "code rendu : {err}"
        );
    }

    // Scenario: endpoint_hint_signale_un_suffixe_v1
    #[test]
    fn endpoint_hint_signale_un_suffixe_v1() {
        assert!(endpoint_hint("http://h:11434/v1").is_some());
        assert!(endpoint_hint("http://h:11434/v1/").is_some());
        assert!(endpoint_hint("http://h:11434").is_none());
    }

    // Scenario: reponse_non_2xx_donne_un_diagnostic_lisible
    #[tokio::test]
    async fn reponse_non_2xx_donne_un_diagnostic_lisible() {
        let (port, server) = serve_once("404 Not Found", "").await;
        let p = provider(
            "openai-compatible",
            &format!("http://127.0.0.1:{port}/v1"),
            None,
        );
        let err = discover_models(&p)
            .await
            .expect_err("un 404 doit échouer avant décodage");
        server.await.expect("server task joins");
        let msg = err.to_string();
        assert!(msg.contains("VNL-LLM-009"), "code rendu : {msg}");
        assert!(msg.contains("404"), "statut absent : {msg}");
        assert!(
            msg.contains(&format!("http://127.0.0.1:{port}/v1/v1/models")),
            "URL absente : {msg}"
        );
        assert!(
            msg.contains("ne doit pas se terminer par /v1"),
            "indice /v1 absent : {msg}"
        );
        assert!(!msg.contains("error decoding response body"), "{msg}");
    }

    // Scenario: la_cle_api_est_envoyee_aux_deux_types
    #[tokio::test]
    async fn la_cle_api_est_envoyee_aux_deux_types() {
        let (port, server) = serve_once("200 OK", r#"{"models":[{"name":"m1"}]}"#).await;
        let p = provider(
            "ollama",
            &format!("http://127.0.0.1:{port}"),
            Some("sk-ollama"),
        );
        let models = discover_models(&p).await.expect("découverte réussie");
        let request = server.await.expect("server task joins");
        assert_eq!(models, vec!["m1".to_string()]);
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer sk-ollama"),
            "en-tête Bearer absent : {request}"
        );
    }
}
