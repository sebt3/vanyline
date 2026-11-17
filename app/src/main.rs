//! Binaire `vanyline-app` : SÉQUENCE de démarrage ordonnée (config → DB →
//! migrations → OIDC → routeur) et assemblage du routeur racine — contrat :
//! `main.sdd`.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::assert_is_empty
    )
)]

mod api;
mod auth;
mod config;
mod dashboards;
mod db;
mod error;
mod k8s;
mod mcp_domain;
mod migration;
mod naming;
mod sessions;
mod widgets;
mod workflow;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, extract::FromRef, routing::get};
use config::Config;
use error::AppError;
use miryad_core::auth::{MiryadAuthState, OidcClient};
use miryad_core::rest::openapi::{resource_openapi, swagger_ui_router};
use miryad_core::rest::resource_router;
use sea_orm_migration::MigratorTrait;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use vanyline_lib::k8s::VnlK8sClient;

/// État global partagé par tout le routeur (visibilité limitée au crate :
/// binaire, aucun consommateur externe) — champ à champ : `main.sdd`.
#[derive(Clone)]
pub struct AppState {
    /// Configuration lue depuis l'environnement (`config.sdd`).
    pub config: Config,
    /// Clé de scellement des cookies, dérivée de `COOKIE_SECRET` (64 premiers
    /// octets) ; seul consommateur en production : le clone injecté dans
    /// `MiryadAuthState` au démarrage.
    pub cookie_key: cookie::Key,
    /// Client K8s lazy : initialisé `None`, découverte à la première requête,
    /// jamais au démarrage (`k8s.sdd`).
    pub k8s: Arc<tokio::sync::Mutex<Option<VnlK8sClient>>>,
    /// État miryad-core construit au démarrage ; c'est LE porteur de la
    /// connexion DB (`db` y est mové, pas de champ `db` distinct).
    pub auth: MiryadAuthState,
    /// Hub de push des phases de sandbox (`ws/sandbox_state.sdd`).
    pub shared_sandbox_state: ws::sandbox_state::SharedState,
    /// Exécution des widgets scriptés (`widgets/runtime.sdd`) : `ScriptRunner` de production et
    /// cache, construits au démarrage sur l'hôte lymaille partagé (`main.sdd` étape 10ter).
    pub widgets: widgets::runtime::WidgetService,
    /// Handle du moteur de workflow (`workflow/runtime.sdd`) — `None` quand l'app n'est
    /// pas configurée pour un moteur (aucune `VNL_RESTATE_*`) : aucun port d'endpoint
    /// ouvert, les routes de workflow répondent `503` (W4, `api/workflows.sdd`).
    pub workflow: Option<workflow::WorkflowRuntime>,
}

impl FromRef<AppState> for MiryadAuthState {
    fn from_ref(state: &AppState) -> Self {
        state.auth.clone()
    }
}

// tooling.sdd D5 — main.sdd §Séquence de démarrage : l'ordre contractuel des étapes
// (rustls → tracing → config → cookie → DB → migrations → OIDC → routeur → bind/serve)
// est LE contrat de cette fonction ; toute extraction en sous-fonctions fragmenterait
// la séquence vérifiable telle quelle.
#[allow(clippy::too_many_lines)] // main.sdd : séquence de démarrage ordonnée contractée en une fonction
#[tokio::main]
async fn main() {
    // reqwest (rustls-tls) et kube (rustls-tls) tirent chacun un backend crypto
    // rustls différent (ring / aws-lc-rs) — les deux finissent compilés dans le
    // même binaire (unification des features Cargo). Sans provider explicite,
    // rustls panique au premier client TLS construit sans provider déjà
    // installé (observé en pratique sur le client K8s, jamais sur le client
    // OIDC/reqwest qui résout son provider en interne sans passer par le
    // helper global ambigu).
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vanyline_app=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env().unwrap_or_else(|e| {
        tracing::error!("{}", e);
        std::process::exit(1);
    });

    let listen_addr: SocketAddr = config.listen_addr.parse().unwrap_or_else(|_| {
        tracing::error!("VNL-CFG-008: invalid LISTEN_ADDR: {}", config.listen_addr);
        std::process::exit(1);
    });

    let cookie_key = {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&config.cookie_secret)
            .unwrap_or_else(|_| config.cookie_secret.as_bytes().to_vec());
        if bytes.len() < 64 {
            tracing::error!("VNL-CFG-005: COOKIE_SECRET must be at least 64 bytes");
            std::process::exit(1);
        }
        cookie::Key::from(&bytes[..64])
    };

    let db = sea_orm::Database::connect(&config.database_url)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("VNL-DB-003: cannot connect to database (sea-orm): {e}");
            std::process::exit(1);
        });

    miryad_core::migration::Migrator::up(&db, None)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("VNL-DB-004: miryad-core migrations failed: {e}");
            std::process::exit(1);
        });

    migration::Migrator::up(&db, None)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("VNL-DB-005: app migrations failed: {e}");
            std::process::exit(1);
        });

    // B9 sessions lymaille (`sessions/mod.sdd`, `main.sdd` amendé) : le catalogue est publié
    // AVANT le bind (idempotent, `sessions/catalog.sdd`) ; panne = refus fermé, l'app ne
    // démarre pas — identifiant `VNL-APP-SES-109`, suite des identifiants du montage.
    sessions::catalog::ensure_catalog(&db)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("VNL-APP-SES-109: catalogue lymaille non publié: {e}");
            std::process::exit(1);
        });

    let miryad_oidc_client = OidcClient::new(&config.oidc_config())
        .await
        .unwrap_or_else(|e| {
            tracing::error!("VNL-AUTH-004: cannot build miryad-core OIDC client: {e}");
            std::process::exit(1);
        });

    let auth_state = MiryadAuthState {
        oidc_client: std::sync::Arc::new(miryad_oidc_client),
        cookie_key: cookie_key.clone(),
        post_login_redirect: "/#/".to_string(),
        post_logout_redirect: "/#/".to_string(),
        db,
        // L'app n'est exposée que derrière l'Ingress HTTPS de l'Application : cookies scellés
        // `Secure` en dur. Arbitrage Sébastien 2026-10-05, cf. `main.sdd` étape 10.
        secure_cookies: true,
        // Poivre HMAC des empreintes de tokens API : dérivé du secret existant `COOKIE_SECRET`
        // (longueur ≥ 64 contrôlée plus haut), aucune nouvelle variable d'env. Effet accepté :
        // une rotation de `COOKIE_SECRET` réémet les tokens API (décision Sébastien 2026-10-05).
        token_pepper: config.cookie_secret.clone(),
    };

    let static_dir = config.static_dir.clone();
    let k8s: Arc<tokio::sync::Mutex<Option<vanyline_lib::k8s::VnlK8sClient>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    // L'hôte lymaille est construit avant l'état (il ne le lit pas) et partagé avec le
    // `WidgetService`, comme avec les sessions (`main.sdd` étape 10ter).
    let hote: miryad_core::lymaille::SharedHost = sessions::lymaille_shared_host_des(
        auth_state.db.clone(),
        Arc::clone(&k8s),
        config.k8s_namespace.clone(),
    );
    let widgets = widgets::runtime::WidgetService::new(auth_state.db.clone(), Some(hote), &config);
    let mut state = AppState {
        config,
        cookie_key,
        k8s,
        auth: auth_state,
        shared_sandbox_state: ws::sandbox_state::SharedState::new(),
        widgets,
        workflow: None,
    };

    // Moteur de workflow (workflow/runtime.sdd, FAIL-FAST revu 2026-10-14) —
    // `from_config` : Ok(None) silencieux seulement si AUCUNE variable `VNL_RESTATE_*`
    // n'est posée (moteur éteint voulu, aucun port ouvert, aucune requête Restate) ;
    // Ok(Some) ⇒ start_runtime (l'échec d'ENREGISTREMENT du déploiement ne remonte
    // jamais ici : réessayé en arrière-plan par runtime.sdd) ; Err (config partielle ou
    // deployment non dérivable) ⇒ fail-fast exit(1) comme l'échec de start_runtime.
    match workflow::WorkflowConfigApp::from_config(&state.config) {
        Ok(None) => {}
        Ok(Some(wf_config)) => {
            state.workflow = Some(
                workflow::runtime::start_runtime(&state, &wf_config)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::error!(
                            "VNL-WF-001: moteur de workflow incapable de démarrer: {e}"
                        );
                        std::process::exit(1);
                    }),
            );
            // Purge quotidienne des runs (une seule tâche, moteur configuré seulement) ;
            // le handle est détaché : la tâche vit avec le processus.
            let _purge_runs = workflow::runtime::demarrer_purge_quotidienne(state.auth.db.clone());
        }
        Err(e) => {
            tracing::error!("VNL-WF-001: moteur de workflow incapable de démarrer: {e}");
            std::process::exit(1);
        }
    }

    let app = build_router(state, &static_dir).unwrap_or_else(|e| {
        tracing::error!("VNL-APP-SES-109: assemblage du routeur refusé: {e}");
        std::process::exit(1);
    });

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("VNL-NET-001: failed to bind {}: {}", listen_addr, e);
            std::process::exit(1);
        });
    tracing::info!("listening on {}", listen_addr);
    axum::serve(listener, app).await.unwrap_or_else(|e| {
        tracing::error!("VNL-NET-002: server error: {}", e);
        std::process::exit(1);
    });
}

/// Assemblage COMPLET du routeur racine (`main.sdd`, Scenario
/// `routeur_de_production_assemble_sans_panique`) : extrait de `main` pour être testé tel
/// qu'en production. Un conflit de chemins entre sous-routeurs fait paniquer axum au montage.
/// Le seul `Err` possible ici est le refus de montage des sessions lymaille — sans hôte,
/// aucune route de conversation n'est servie (`sessions/mod.sdd`, `VNL-APP-SES-109`) ; l'état
/// de l'app fournit toujours son hôte (`sessions::lymaille_shared_host`), le chemin `None`
/// est l'oracle du refus, verrouillé par le test `montage_refuse_sans_hote`.
fn build_router(state: AppState, static_dir: &str) -> Result<Router, AppError> {
    let hote: miryad_core::lymaille::SharedHost = sessions::lymaille_shared_host(&state);
    let lymaille = sessions::lymaille_router(Some(hote), state.auth.db.clone())?;
    Ok(Router::new()
        .route("/health", get(health))
        .merge(miryad_core::auth::auth_router())
        .merge(resources_router())
        .merge(miryad_core::rest::me::me_router::<AppState>())
        .merge(swagger_ui_router::<AppState>(openapi_spec()))
        .merge(mcp_config_router())
        .merge(mcp_domain::mcp_domain_router())
        // Routes de conversation miryad-core (chemins COMPLETS `/api/v1/lymaille/…`, préfixe
        // figé par la crate) mergées À LA RACINE et déclarées AVANT le `nest` `/api/v1` :
        // les segments littéraux priment sur le wildcard du nest (jurisprudence matchit,
        // `main.sdd`, `sessions/mod.sdd`) — hors de ce préfixe, AUCUNE route de conversation.
        .merge(lymaille)
        .nest("/api", api::api_router())
        .route(
            "/api/ws/sandbox-state",
            get(ws::sandbox_state::ws_sandbox_state_handler),
        )
        .with_state(state)
        .fallback_service(
            ServeDir::new(static_dir).fallback(ServeFile::new(format!("{static_dir}/index.html"))),
        ))
}

/// Les ressources CRUD `resource_router` de miryad-core, montées en chemins complets
/// `/api/v1/{resource}` (`main.sdd`, Exposes : ordre de merge — les dix définitions lymaille
/// de miryad-core puis `app-settings`, depuis le retrait C3 des six entités de config maison).
fn resources_router() -> Router<AppState> {
    use miryad_core::lymaille::definitions;
    Router::new()
        .merge(resource_router::<definitions::agent::Entity, AppState>())
        .merge(resource_router::<definitions::toolset::Entity, AppState>())
        .merge(resource_router::<definitions::profile::Entity, AppState>())
        .merge(resource_router::<definitions::provider::Entity, AppState>())
        .merge(resource_router::<
            definitions::private_agent::Entity,
            AppState,
        >())
        .merge(resource_router::<
            definitions::private_toolset::Entity,
            AppState,
        >())
        .merge(resource_router::<definitions::context_type::Entity, AppState>())
        .merge(resource_router::<
            definitions::catalog_context::Entity,
            AppState,
        >())
        .merge(resource_router::<definitions::user_context::Entity, AppState>())
        .merge(resource_router::<definitions::mcp_server::Entity, AppState>())
        .merge(resource_router::<
            db::entities::app_settings::Entity,
            AppState,
        >())
        .merge(resource_router::<db::entities::dashboards::Entity, AppState>())
}

/// `POST /mcp` : CRUD de la configuration (les onze entités du `resource_router`) par le
/// serveur MCP générique de miryad-core, avec l'identité de l'appelant (`main.sdd`).
pub(crate) fn mcp_config_router() -> Router<AppState> {
    use miryad_core::lymaille::definitions;
    use miryad_core::mcp::{McpToolRegistry, OutputFormat, mcp_router};
    let mut registry = McpToolRegistry::new(OutputFormat::Json);
    registry
        .register::<definitions::agent::Entity>()
        .register::<definitions::toolset::Entity>()
        .register::<definitions::profile::Entity>()
        .register::<definitions::provider::Entity>()
        .register::<definitions::private_agent::Entity>()
        .register::<definitions::private_toolset::Entity>()
        .register::<definitions::context_type::Entity>()
        .register::<definitions::catalog_context::Entity>()
        .register::<definitions::user_context::Entity>()
        .register::<definitions::mcp_server::Entity>()
        .register::<db::entities::app_settings::Entity>()
        .register::<db::entities::dashboards::Entity>();
    mcp_router(registry)
}

async fn health() -> &'static str {
    "ok"
}

/// Document `OpenAPI` combiné des 12 ressources `resource_router` — fusion par
/// `OpenApi::merge` de DOUZE fragments `resource_openapi` dans l'ordre d'Exposes (dix lymaille,
/// puis `app-settings` et `dashboards`), `info` renseigné une seule fois sur le document final
/// (les fragments n'en portent pas). Sert `GET /api/swagger-ui` (feature Cargo `swagger-ui` de
/// miryad-core) et `/api/openapi.json`.
fn openapi_spec() -> utoipa::openapi::OpenApi {
    use miryad_core::lymaille::definitions as d;

    let mut spec = resource_openapi::<d::agent::Entity>();
    spec.merge(resource_openapi::<d::toolset::Entity>());
    spec.merge(resource_openapi::<d::profile::Entity>());
    spec.merge(resource_openapi::<d::provider::Entity>());
    spec.merge(resource_openapi::<d::private_agent::Entity>());
    spec.merge(resource_openapi::<d::private_toolset::Entity>());
    spec.merge(resource_openapi::<d::context_type::Entity>());
    spec.merge(resource_openapi::<d::catalog_context::Entity>());
    spec.merge(resource_openapi::<d::user_context::Entity>());
    spec.merge(resource_openapi::<d::mcp_server::Entity>());
    spec.merge(resource_openapi::<db::entities::app_settings::Entity>());
    spec.merge(resource_openapi::<db::entities::dashboards::Entity>());
    spec.info = utoipa::openapi::Info::new("vanyline", env!("CARGO_PKG_VERSION"));
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: `openapi_spec fusionne douze ressources avec un seul schéma de sécurité`
    /// (`main.sdd`, douze ressources depuis la vague G : les dix définitions lymaille de
    /// miryad-core + `app-settings` + `dashboards`, aucun chemin de la config maison).
    #[test]
    fn openapi_spec_exposes_all_twelve_resources_without_duplicate_security_scheme() {
        let spec = openapi_spec();

        for resource in [
            "lymaille_catalog_agents",
            "lymaille_catalog_toolsets",
            "lymaille_profiles",
            "lymaille_providers",
            "lymaille_agents",
            "lymaille_toolsets",
            "lymaille_catalog_context_types",
            "lymaille_catalog_contexts",
            "lymaille_contexts",
            "lymaille_mcp_servers",
            "app-settings",
            "dashboards",
        ] {
            assert!(
                spec.paths
                    .get_path_item(format!("/api/v1/{resource}"))
                    .is_some(),
                "missing collection path for {resource}"
            );
            assert!(
                spec.paths
                    .get_path_item(format!("/api/v1/{resource}/{{id}}"))
                    .is_some(),
                "missing item path for {resource}"
            );
        }

        // Retrait C3 du chantier lymaille : aucun chemin nu de la config maison ne subsiste.
        for withdrawn in [
            "llm-providers",
            "mcp-servers",
            "model-profiles",
            "toolsets",
            "skills",
            "agents",
        ] {
            assert!(
                spec.paths
                    .get_path_item(format!("/api/v1/{withdrawn}"))
                    .is_none(),
                "le chemin {withdrawn} retiré en C3 ne doit plus apparaître"
            );
            assert!(
                spec.paths
                    .get_path_item(format!("/api/v1/{withdrawn}/{{id}}"))
                    .is_none(),
                "le chemin item {withdrawn}/{{id}} retiré en C3 ne doit plus apparaître"
            );
        }

        assert_eq!(spec.info.title, "vanyline");
        assert_eq!(spec.info.version, env!("CARGO_PKG_VERSION"));

        // And du Scenario : les `schemas` du document sont l'union des fragments
        // `resource_openapi` des douze entités (lymaille comme app).
        let mut expected_schemas: Vec<String> = Vec::new();
        macro_rules! collect {
            ($($entity:ty),+ $(,)?) => {$(
                expected_schemas.extend(
                    resource_openapi::<$entity>()
                        .components
                        .iter()
                        .flat_map(|components| components.schemas.keys().cloned()),
                );
            )+};
        }
        {
            use miryad_core::lymaille::definitions as d;
            collect!(
                d::agent::Entity,
                d::toolset::Entity,
                d::profile::Entity,
                d::provider::Entity,
                d::private_agent::Entity,
                d::private_toolset::Entity,
                d::context_type::Entity,
                d::catalog_context::Entity,
                d::user_context::Entity,
                d::mcp_server::Entity,
                db::entities::app_settings::Entity,
                db::entities::dashboards::Entity,
            );
        }
        let mut doc_schemas: Vec<String> = spec
            .components
            .iter()
            .flat_map(|components| components.schemas.keys().cloned())
            .collect();
        doc_schemas.sort_unstable();
        expected_schemas.sort_unstable();
        expected_schemas.dedup();
        assert_eq!(doc_schemas, expected_schemas);
        // Les dix `Model` lymaille se télescopent sous le nom `Model` (voir `main.sdd`).
        assert!(doc_schemas.iter().any(|name| name == "Model"));

        // OpenApi::merge dédoublonne security_schemes/security par nom/égalité — les 12
        // fragments déclarent le même schéma Bearer, il ne doit apparaître qu'une fois.
        let components = spec.components.expect("components present");
        assert_eq!(
            components.security_schemes.len(),
            1,
            "expected a single deduplicated security scheme, got {:?}",
            components.security_schemes.keys().collect::<Vec<_>>()
        );
        assert_eq!(spec.security.expect("global security present").len(), 1);
    }

    /// `AppState` de test sur la base sqlite mémoire (même forme que `api/projects.rs`).
    async fn test_state() -> AppState {
        let db = crate::db::test_support::real_db().await;
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
                widget_max_duration: std::time::Duration::from_secs(300),
                widget_max_operations: 0,
                widget_max_result_bytes: 1_048_576,
            },
            cookie_key: cookie::Key::from(&[0u8; 64]),
            k8s: Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: ws::sandbox_state::SharedState::new(),
            workflow: None,
            widgets: crate::widgets::runtime::WidgetService::pour_tests(),
        }
    }

    /// Crée l'utilisateur `subject` (et l'ajoute au groupe `admin` si demandé) puis rend
    /// son cookie de session (même clé que `test_state`).
    async fn session_cookie(state: &AppState, subject: &str, admin: bool) -> String {
        use miryad_core::users::{resolve_user, sync_group_memberships};
        let user = resolve_user(&state.auth.db, subject, None)
            .await
            .expect("user resolves");
        if admin {
            sync_group_memberships(&state.auth.db, user.id, &["admin".to_string()])
                .await
                .expect("admin group synced");
        }
        let identity = miryad_core::auth::OidcIdentity {
            id_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.signature".to_string(),
            subject: subject.to_string(),
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

    /// Scenario: `app_settings_monte_ecriture_admin_seule` — `/api/v1/app-settings` est monté
    /// par `resources_router` : un utilisateur ordinaire authentifié lit (200) et ne peut pas
    /// écrire (403) ; un administrateur écrit.
    #[tokio::test]
    async fn app_settings_monte_ecriture_admin_seule() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = test_state().await;
        let user_cookie = session_cookie(&state, "alice", false).await;
        let admin_cookie = session_cookie(&state, "admin-user", true).await;
        let body = serde_json::json!({
            "key": "default_profile",
            "value": {"profile": "qwen"}
        })
        .to_string();
        let app = resources_router().with_state(state);

        let read = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/app-settings")
                    .header("Cookie", user_cookie.as_str())
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert_eq!(
            read.status(),
            StatusCode::OK,
            "lecture ouverte aux authentifiés"
        );

        let user_write = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/app-settings")
                    .header("Cookie", user_cookie.as_str())
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.clone()))
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert_eq!(
            user_write.status(),
            StatusCode::FORBIDDEN,
            "écriture réservée aux administrateurs"
        );

        let admin_write = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/app-settings")
                    .header("Cookie", admin_cookie.as_str())
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert!(
            admin_write.status().is_success(),
            "un administrateur peut écrire, got {}",
            admin_write.status()
        );
    }

    /// Scenario: `routeur_de_production_assemble_sans_panique` — le routeur racine complet
    /// (`build_router`) s'assemble sans conflit de chemins et sert les routes critiques
    /// (jamais 404 ; 401 sans authentification).
    #[tokio::test]
    async fn routeur_de_production_assemble_sans_panique() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = test_state().await;
        let app = build_router(state, "./static").expect("le routeur complet s'assemble");

        for (method, uri) in [
            ("GET", "/api/v1/me"),
            ("GET", "/api/v1/app-settings"),
            ("POST", "/mcp"),
            ("POST", "/mcp/domain"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .expect("valid request"),
                )
                .await
                .expect("router does not fail");
            assert_ne!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{method} {uri} doit être monté"
            );
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} sans authentification"
            );
        }
    }

    /// Scenario: `me_v1_monte_et_rend_les_groupes` — `GET /api/v1/me` (miryad-core `me_router`)
    /// rend les groupes de l'appelant : `admin` pour un administrateur, pas pour un utilisateur
    /// ordinaire ; sans session, 401.
    #[tokio::test]
    async fn me_v1_monte_et_rend_les_groupes() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = test_state().await;
        let user_cookie = session_cookie(&state, "alice", false).await;
        let admin_cookie = session_cookie(&state, "admin-user", true).await;
        let app = build_router(state, "./static").expect("le routeur complet s'assemble");

        for (cookie, expect_admin) in [(user_cookie.as_str(), false), (admin_cookie.as_str(), true)]
        {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/me")
                        .header("Cookie", cookie)
                        .body(Body::empty())
                        .expect("valid request"),
                )
                .await
                .expect("router does not fail");
            assert_eq!(resp.status(), StatusCode::OK, "GET /api/v1/me authentifié");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body readable");
            let json: serde_json::Value = serde_json::from_slice(&bytes).expect("corps JSON");
            let groups = json["groups"].as_array().expect("groups est un tableau");
            assert_eq!(
                groups.iter().any(|g| g == "admin"),
                expect_admin,
                "groupe admin attendu={expect_admin}, groupes={groups:?}"
            );
        }

        let anonyme = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/me")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert_eq!(
            anonyme.status(),
            StatusCode::UNAUTHORIZED,
            "sans session ⇒ 401"
        );
    }

    /// Scenario: `routes sous lymaille et pas ailleurs` (`sessions/mod.sdd`) — le routeur
    /// complet sert les routes de conversation SOUS `/api/v1/lymaille` (401 sans
    /// authentification, jamais 404 ; la liste répond 200 une fois authentifiée) et aucune
    /// route de conversation n'existe hors de ce préfixe — ces mêmes chemins ailleurs ne
    /// rendent jamais le 401 d'une route de conversation authentifiée.
    #[tokio::test]
    async fn routes_sous_lymaille_et_pas_ailleurs() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = test_state().await;
        let cookie = session_cookie(&state, "alice", false).await;
        let app = build_router(state, "./static").expect("le routeur complet s'assemble");

        for (methode, uri) in [
            ("GET", "/api/v1/lymaille/conversations"),
            ("POST", "/api/v1/lymaille/conversations"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(methode)
                        .uri(uri)
                        .body(Body::empty())
                        .expect("requête valide"),
                )
                .await
                .expect("le routeur répond");
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{methode} {uri} : monté sous le préfixe, 401 attendu sans authentification"
            );
        }

        let liste = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/lymaille/conversations")
                    .header("Cookie", cookie.as_str())
                    .body(Body::empty())
                    .expect("requête valide"),
            )
            .await
            .expect("le routeur répond");
        assert_eq!(
            liste.status(),
            StatusCode::OK,
            "authentifiée, GET /api/v1/lymaille/conversations répond"
        );

        // Segments propres au `conversations_router` de miryad-core (`history`, `events`,
        // `toolsets/…/draft`) : hors du préfixe, ils ne peuvent être servis que par lui.
        // (`/api/conversations/1/history` : la route de l'ancien chat maison a été retirée
        // en C2 du chantier lymaille — elle répond désormais 404, jamais 401.)
        for (methode, uri) in [
            ("GET", "/api/v1/conversations"),
            ("GET", "/api/conversations/1/history"),
            ("GET", "/conversations/1/events"),
            ("DELETE", "/toolsets/monojet/draft"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(methode)
                        .uri(uri)
                        .body(Body::empty())
                        .expect("requête valide"),
                )
                .await
                .expect("le routeur répond");
            assert_ne!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{methode} {uri} : aucune route de conversation lymaille ne doit être servie hors de /api/v1/lymaille"
            );
        }
    }
}
