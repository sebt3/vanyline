mod config;
mod error;

use axum::{routing::get, Router};
use config::Config;
use std::net::SocketAddr;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "vanyline_app=debug,tower_http=debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env().unwrap_or_else(|e| {
        tracing::error!("{}", e);
        std::process::exit(1);
    });

    let listen_addr: SocketAddr = config.listen_addr.parse().unwrap_or_else(|_| {
        tracing::error!("VNL-CFG-008: Invalid LISTEN_ADDR: {}", config.listen_addr);
        std::process::exit(1);
    });

    let static_dir = config.static_dir.clone();
    let state = AppState { config };

    let app = Router::new()
        .route("/health", get(health))
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
