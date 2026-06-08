mod api;
mod auth;
mod config;
mod error;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::get, Router};
use config::Config;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub oidc_client: Arc<dyn auth::OidcClientTrait>,
    pub cookie_key: cookie::Key,
}

#[tokio::main]
async fn main() {
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

    let oidc_client = Arc::new(auth::oidc::OidcClient::new(&config).await);

    let static_dir = config.static_dir.clone();
    let state = AppState {
        config,
        oidc_client,
        cookie_key,
    };

    let app = Router::new()
        .route("/health", get(health))
        .nest("/auth", auth::auth_router())
        .nest("/api", api::api_router())
        .with_state(state)
        .fallback_service(
            ServeDir::new(&static_dir)
                .fallback(ServeFile::new(format!("{}/index.html", static_dir))),
        );

    let listener = tokio::net::TcpListener::bind(listen_addr).await.unwrap();
    tracing::info!("listening on {}", listen_addr);
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> &'static str {
    "ok"
}
