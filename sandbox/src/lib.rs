pub mod auth;
pub mod config;
pub mod mcp;
pub mod telemetry;
pub mod tools_impl;

use std::sync::Arc;
use std::time::Instant;

use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tower_http::trace::TraceLayer;

use auth::AuthState;
use config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub auth: Arc<AuthState>,
}

/// Build the main MCP application router (MCP + public routes).
///
/// The metrics server runs on a separate port — see [`spawn_metrics_server`].
pub fn build_app(state: AppState) -> Router {
    let protected =
        Router::new()
            .route("/mcp", post(mcp::handle))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth::require_auth,
            ));

    let public = Router::new().route("/health", get(health)).route(
        "/.well-known/oauth-protected-resource",
        get(mcp::oauth_metadata),
    );

    Router::new()
        .merge(protected)
        .merge(public)
        .layer(middleware::from_fn(track_metrics))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Build the internal metrics router (`GET /metrics`).
///
/// Bind on `config.metrics_listen` (default `0.0.0.0:9090`). Never expose this
/// port externally — protect it at the network/infra layer (NetworkPolicy, etc.).
pub fn build_metrics_app() -> Router {
    Router::new().route("/metrics", get(metrics_handler))
}

/// Spawn the metrics HTTP server as a background task.
///
/// A bind failure is logged as a warning but does **not** abort startup —
/// the main MCP server continues without an exposed `/metrics` endpoint.
pub async fn spawn_metrics_server(addr: std::net::SocketAddr) {
    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            tracing::info!("metrics server listening on {addr}");
            tokio::spawn(async move {
                if let Err(e) = axum::serve(listener, build_metrics_app()).await {
                    tracing::warn!("metrics server error: {e}");
                }
            });
        }
        Err(e) => {
            tracing::warn!("could not bind metrics server on {addr}: {e} — /metrics unavailable");
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

async fn metrics_handler() -> impl IntoResponse {
    match telemetry::render_metrics() {
        Some(body) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

// ── Metrics middleware ────────────────────────────────────────────────────────

async fn track_metrics(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    // Use the path as-is; routes are static in this template so cardinality is low.
    // When you add :param routes in a fork, normalise to the route template instead.
    let path = req.uri().path().to_string();
    let start = Instant::now();

    let response = next.run(req).await;

    let status = response.status().as_u16().to_string();
    let duration = start.elapsed().as_secs_f64();

    metrics::counter!("http_requests_total",
        "method" => method.clone(),
        "path"   => path.clone(),
        "status" => status
    )
    .increment(1);

    metrics::histogram!("http_request_duration_seconds",
        "method" => method,
        "path"   => path
    )
    .record(duration);

    response
}
