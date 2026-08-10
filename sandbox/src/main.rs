use std::sync::Arc;

use anyhow::Result;
use clap::Parser as _;
use vanyline_sandbox::{
    AppState, auth::AuthState, build_app, config::Config, spawn_metrics_server,
    ws::ticket::TicketStore,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI/env before starting tracing so we can pass otel_endpoint.
    // Full validation (OIDC checks) runs after tracing is initialised.
    let config = Config::parse();
    vanyline_sandbox::telemetry::init(config.otel_endpoint.as_deref())?;
    config.validate()?;
    let config = Arc::new(config);

    // Spawn the internal metrics server (separate port, non-fatal bind failure).
    let metrics_addr: std::net::SocketAddr = config.metrics_listen.parse()?;
    spawn_metrics_server(metrics_addr).await;

    let auth = Arc::new(AuthState::new(config.clone())?);
    let state = AppState {
        config: config.clone(),
        auth,
        tickets: TicketStore::new(),
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
