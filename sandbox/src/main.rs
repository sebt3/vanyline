//! Binaire `vanyline-sandbox` : amorçage du serveur du pod de travail —
//! parsing CLI/env, telemétrie, serveur d'exploitation, TLS et bind (spec :
//! `./main.sdd` ; le routage et ses contrats vivent dans `./lib.rs`).

use std::sync::Arc;

use anyhow::Result;
use clap::Parser as _;
use vanyline_sandbox::{
    AppState, LspManager, auth::AuthState, build_app, config::Config, fs_push_channels,
    spawn_metrics_server, ws::ticket::TicketStore,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI/env before starting tracing so we can pass otel_endpoint.
    // Full validation (owner identity, JWKS source) runs after tracing is initialised.
    let config = Config::parse();
    vanyline_sandbox::telemetry::init(config.otel_endpoint.as_deref())?;
    config.validate()?;
    let config = Arc::new(config);

    // Spawn the ops server (separate port, non-fatal bind failure): /health and
    // /metrics, no auth, never published.
    let ops_addr: std::net::SocketAddr = config.ops_listen.parse()?;
    spawn_metrics_server(ops_addr).await;

    let auth = Arc::new(AuthState::new(config.clone())?);
    let lsp = Arc::new(LspManager::from_env(config.sandbox_root.clone())?);
    let (fs_events, fs_flush) = fs_push_channels();
    let state = AppState {
        config: config.clone(),
        auth,
        tickets: TicketStore::new(),
        lsp,
        fs_events,
        fs_flush,
    };
    let app = build_app(state);

    let addr: std::net::SocketAddr = config.listen.parse()?;
    tracing::info!("listening on {addr}");

    match (&config.tls_cert, &config.tls_key) {
        (Some(cert), Some(key)) => {
            tracing::info!("TLS enabled");
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            axum_server::bind_rustls(addr, tls)
                .serve(app.into_make_service())
                .await?;
        }
        (None, None) => {
            let listener = tokio::net::TcpListener::bind(addr).await?;
            axum::serve(listener, app).await?;
        }
        _ => {
            anyhow::bail!("Both --tls-cert and --tls-key must be provided together");
        }
    }

    vanyline_sandbox::telemetry::shutdown_otel();
    Ok(())
}
