//! Binaire `vanyline-app` : SÉQUENCE de démarrage ordonnée (config → DB →
//! migrations → OIDC → routeur) et assemblage du routeur racine — contrat :
//! `main.sdd`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

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
    /// Verrou de tour de chat par `conversation_id` (i32 = PK des
    /// conversations) — consommateurs : `ws/chat.rs`.
    pub busy: Arc<Mutex<HashSet<i32>>>,
    /// Client K8s lazy : initialisé `None`, découverte à la première requête,
    /// jamais au démarrage (`k8s.sdd`).
    pub k8s: Arc<tokio::sync::Mutex<Option<VnlK8sClient>>>,
    /// État miryad-core construit au démarrage ; c'est LE porteur de la
    /// connexion DB (`db` y est mové, pas de champ `db` distinct).
    pub auth: MiryadAuthState,
    /// Hub de push des phases de sandbox (`ws/sandbox_state.sdd`).
    pub shared_sandbox_state: ws::sandbox_state::SharedState,
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
    let state = AppState {
        config,
        cookie_key,
        busy: Arc::new(Mutex::new(HashSet::new())),
        k8s: Arc::new(tokio::sync::Mutex::new(None)),
        auth: auth_state,
        shared_sandbox_state: ws::sandbox_state::SharedState::new(),
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
        .merge(swagger_ui_router::<AppState>(openapi_spec()))
        .nest("/api/v1", api::api_v1_router())
        .nest("/api", api::api_router())
        .route(
            "/api/ws/chat/{conversation_id}",
            get(ws::chat::ws_chat_handler),
        )
        .route(
            "/api/ws/sandbox-state",
            get(ws::sandbox_state::ws_sandbox_state_handler),
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

/// Document `OpenAPI` combiné des 6 ressources `resource_router` — un fragment par entité
/// (`resource_openapi`), fusionnés (`OpenApi::merge`, dédoublonne le schéma Bearer commun),
/// `info` renseigné une fois sur le document final (les fragments ne le fixent pas). Sert
/// `GET /api/swagger-ui` (feature Cargo `swagger-ui` de miryad-core) et `/api/openapi.json`.
fn openapi_spec() -> utoipa::openapi::OpenApi {
    let mut spec = resource_openapi::<db::entities::llm_providers::Entity>();
    spec.merge(resource_openapi::<db::entities::mcp_servers::Entity>());
    spec.merge(resource_openapi::<db::entities::model_profiles::Entity>());
    spec.merge(resource_openapi::<db::entities::toolsets::Entity>());
    spec.merge(resource_openapi::<db::entities::skills::Entity>());
    spec.merge(resource_openapi::<db::entities::agents::Entity>());
    spec.info = utoipa::openapi::Info::new("vanyline", env!("CARGO_PKG_VERSION"));
    spec
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn openapi_spec_exposes_all_six_resources_without_duplicate_security_scheme() {
        let spec = openapi_spec();

        for resource in [
            "llm-providers",
            "mcp-servers",
            "model-profiles",
            "toolsets",
            "skills",
            "agents",
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

        assert_eq!(spec.info.title, "vanyline");
        assert_eq!(spec.info.version, env!("CARGO_PKG_VERSION"));

        // OpenApi::merge dédoublonne security_schemes/security par nom/égalité — les 6
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
}
