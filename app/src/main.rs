#![deny(clippy::unwrap_used, clippy::expect_used)]

mod api;
mod auth;
mod config;
mod config_store;
mod db;
mod error;
mod k8s;
mod migration;
mod ws;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{Router, extract::FromRef, routing::get};
use config::Config;
use miryad_core::auth::{MiryadAuthState, OidcClient};
use miryad_core::rest::resource_router;
use sea_orm_migration::MigratorTrait;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use vanyline_lib::k8s::VnlK8sClient;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub cookie_key: cookie::Key,
    pub busy: Arc<Mutex<HashSet<i32>>>,
    pub k8s: Arc<tokio::sync::Mutex<Option<VnlK8sClient>>>,
    pub auth: MiryadAuthState,
}

impl FromRef<AppState> for MiryadAuthState {
    fn from_ref(state: &AppState) -> Self {
        state.auth.clone()
    }
}

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
    };

    let static_dir = config.static_dir.clone();
    let state = AppState {
        config,
        cookie_key,
        busy: Arc::new(Mutex::new(HashSet::new())),
        k8s: Arc::new(tokio::sync::Mutex::new(None)),
        auth: auth_state,
    };

    let app = Router::new()
        .route("/health", get(health))
        .merge(miryad_core::auth::auth_router())
        .merge(resource_router::<
            db::entities::llm_providers::Entity,
            AppState,
        >())
        .merge(resource_router::<db::entities::mcp_servers::Entity, AppState>())
        .merge(resource_router::<db::entities::skills::Entity, AppState>())
        .merge(resource_router::<db::entities::toolsets::Entity, AppState>())
        .merge(resource_router::<
            db::entities::model_profiles::Entity,
            AppState,
        >())
        .merge(resource_router::<db::entities::agents::Entity, AppState>())
        .nest("/api/v1", api::api_v1_router())
        .nest("/api", api::api_router())
        .route(
            "/api/ws/chat/{conversation_id}",
            get(ws::chat::ws_chat_handler),
        )
        .with_state(state)
        .fallback_service(
            ServeDir::new(&static_dir).fallback(ServeFile::new(format!("{static_dir}/index.html"))),
        );

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

async fn health() -> &'static str {
    "ok"
}
