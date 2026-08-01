//! Observability: Prometheus metrics + optional OpenTelemetry tracing.
//!
//! # Architecture
//!
//! Two independent pillars, both non-blocking:
//!
//! - **Prometheus** — always enabled. A global recorder is installed at startup;
//!   the `/metrics` HTTP endpoint renders the current state.
//! - **OpenTelemetry** — enabled only when `OTEL_EXPORTER_OTLP_ENDPOINT` (or
//!   `--otel-endpoint`) is set. Traces are exported via gRPC/OTLP to a collector
//!   (Grafana Alloy, OpenTelemetry Collector, …). If the collector is unreachable
//!   at startup the server still starts; spans are silently dropped when the send
//!   queue fills.
//!
//! # What this template does NOT implement (document for fork owners)
//!
//! - **`/metrics` authentication** — the endpoint is public on its dedicated port.
//!   Protect it at the network level (NetworkPolicy, reverse-proxy IP allowlist,
//!   or mTLS) rather than inside the app.
//! - **Custom business metrics** — add them with `metrics::counter!` /
//!   `metrics::histogram!` in your tool handlers.
//! - **Distributed trace propagation** — wire `TraceContextPropagator` and
//!   extract W3C `traceparent` headers from incoming MCP requests if you need
//!   end-to-end traces across services.
//! - **OTel resource attributes** — extend `build_resource()` below with
//!   `k8s.pod.name`, `deployment.environment`, etc. from your env.
//! - **OTel metrics export** — the template only exports traces via OTLP.
//!   Add `opentelemetry-otlp` metrics features + `SdkMeterProvider` if needed.

use std::sync::OnceLock;

use anyhow::{Context, Result};
use metrics_exporter_prometheus::PrometheusHandle;
use opentelemetry::{KeyValue, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    trace::{BatchSpanProcessor, SdkTracerProvider},
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

// ── Prometheus ────────────────────────────────────────────────────────────────

static PROMETHEUS: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the Prometheus global metrics recorder.
///
/// Safe to call multiple times (idempotent via [`OnceLock`]). Subsequent calls
/// are no-ops and return `Ok(())`.
pub fn init_prometheus() -> Result<()> {
    if PROMETHEUS.get().is_some() {
        return Ok(());
    }
    let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .context("failed to install Prometheus metrics recorder")?;
    PROMETHEUS.set(handle).ok();
    Ok(())
}

/// Render the current Prometheus metrics snapshot as text (exposition format
/// 0.0.4). Returns `None` before [`init_prometheus`] has been called.
pub fn render_metrics() -> Option<String> {
    PROMETHEUS.get().map(|h| h.render())
}

// ── OpenTelemetry ─────────────────────────────────────────────────────────────

// Store the provider so we can call .shutdown() on exit.
static OTEL_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

fn build_resource() -> Resource {
    // TODO (fork owners): add k8s.pod.name, deployment.environment, etc.
    Resource::builder()
        .with_service_name(env!("CARGO_PKG_NAME"))
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build()
}

fn try_init_otel_tracer(endpoint: &str) -> Result<opentelemetry_sdk::trace::SdkTracer> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .context("building OTLP span exporter")?;

    let processor = BatchSpanProcessor::builder(exporter).build();

    let provider = SdkTracerProvider::builder()
        .with_span_processor(processor)
        .with_resource(build_resource())
        .build();

    // `tracer()` requires `TracerProvider` trait imported above.
    let tracer = provider.tracer(env!("CARGO_PKG_NAME"));

    opentelemetry::global::set_tracer_provider(provider.clone());
    OTEL_PROVIDER.set(provider).ok();

    Ok(tracer)
}

// ── Combined init ─────────────────────────────────────────────────────────────

/// Initialize both the Prometheus recorder and the `tracing` subscriber
/// (structured logs + optional OTel traces).
///
/// Call once at startup, before [`crate::config::Config::validate`].
///
/// # OpenTelemetry
///
/// If `otel_endpoint` is `Some`, the function attempts to build an OTLP gRPC
/// exporter. Failure is **non-fatal**: a warning is printed to stderr and the
/// server starts without trace export.
#[allow(clippy::unwrap_used)] // chaine litterale connue a la compilation, toujours valide
pub fn init(otel_endpoint: Option<&str>) -> Result<()> {
    init_prometheus()?;

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "vanyline_sandbox=info,tower_http=debug".parse().unwrap());

    // Attempt OTel init; log failure but don't abort startup.
    let otel_layer = otel_endpoint.and_then(|ep| match try_init_otel_tracer(ep) {
        Ok(tracer) => Some(tracing_opentelemetry::layer().with_tracer(tracer)),
        Err(e) => {
            eprintln!("WARN  OpenTelemetry init failed (continuing without traces): {e}");
            None
        }
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
        .init();

    Ok(())
}

/// Flush pending OTel spans on graceful shutdown.
///
/// Call before the process exits when OTel is enabled; a no-op otherwise.
pub fn shutdown_otel() {
    if let Some(provider) = OTEL_PROVIDER.get()
        && let Err(e) = provider.shutdown()
    {
        eprintln!("WARN  OTel shutdown error: {e}");
    }
}
