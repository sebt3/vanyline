use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Parser;

/// Public URL used when `--public-url` / `MCP_PUBLIC_URL` is not configured.
/// Kept in sync with `mcp::oauth_metadata`'s `resource` default.
pub const DEFAULT_PUBLIC_URL: &str = "http://localhost:3000";

/// Absolute URL of the RFC 9728 protected-resource metadata document, derived
/// from the configured public URL (or the localhost default when unset).
/// Any trailing slash on `public_url` is trimmed so the result is well-formed.
pub fn oauth_metadata_url(public_url: Option<&str>) -> String {
    let base = public_url
        .unwrap_or(DEFAULT_PUBLIC_URL)
        .trim_end_matches('/');
    format!("{base}/.well-known/oauth-protected-resource")
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "vanyline-sandbox",
    about = "MCP HTTP Streamable server — OAuth2/OIDC secured"
)]
pub struct Config {
    /// Listen address
    #[arg(long, default_value = "0.0.0.0:3000", env = "MCP_LISTEN")]
    pub listen: String,

    /// TLS certificate file (PEM) — omit for plain HTTP (use a reverse proxy in prod)
    #[arg(long, env = "MCP_TLS_CERT")]
    pub tls_cert: Option<String>,

    /// TLS key file (PEM)
    #[arg(long, env = "MCP_TLS_KEY")]
    pub tls_key: Option<String>,

    /// OIDC issuer URL (e.g. https://authentik.kydah.fr/application/o/my-mcp/)
    #[arg(long, env = "OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,

    /// OIDC audience — client_id registered in Authentik
    #[arg(long, env = "OIDC_AUDIENCE")]
    pub oidc_audience: Option<String>,

    /// Groups with full access (comma-separated)
    #[arg(long, default_value = "kubernetes-admin", env = "AUTH_GROUPS_ADMIN")]
    pub auth_groups_admin: String,

    /// Groups with read-only access (comma-separated)
    #[arg(
        long,
        default_value = "kubernetes-view,kubernetes-edit",
        env = "AUTH_GROUPS_READ"
    )]
    pub auth_groups_read: String,

    /// Disable authentication — development only, refuses to start without explicit flag
    #[arg(long, env = "NO_AUTH")]
    pub no_auth: bool,

    /// Static bearer token for demo/testing — bypasses OIDC entirely.
    /// Grants Admin access. NOT for production use.
    #[arg(long, env = "STATIC_TOKEN")]
    pub static_token: Option<String>,

    /// Public URL of this server (used in OAuth protected resource metadata)
    #[arg(long, env = "MCP_PUBLIC_URL")]
    pub public_url: Option<String>,

    /// Path to a PEM CA certificate to trust for OIDC/JWKS requests (self-signed or private CA)
    #[arg(long, env = "OIDC_CA_CERT")]
    pub oidc_ca_cert: Option<String>,

    /// Address for the internal metrics server exposing GET /metrics (Prometheus format).
    /// Intentionally separate from the MCP port so it is never reachable from the internet.
    /// Omit to disable the metrics server entirely.
    #[arg(long, default_value = "0.0.0.0:9090", env = "METRICS_LISTEN")]
    pub metrics_listen: String,

    /// OTLP gRPC endpoint for OpenTelemetry trace export (e.g. http://alloy:4317).
    /// When unset, OTel tracing is disabled. Failures to reach the collector at runtime
    /// are non-fatal — spans are silently dropped until the collector recovers.
    #[arg(long, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    pub otel_endpoint: Option<String>,

    /// Root directory for sandbox workspace files
    #[arg(long, default_value = "/workspace", env = "VNL_SANDBOX_ROOT")]
    pub sandbox_root: PathBuf,
}

impl Config {
    pub fn parse_and_validate() -> Result<Self> {
        let config = Self::parse();
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.no_auth {
            tracing::warn!("Authentication disabled (--no-auth). Development use only!");
            return Ok(());
        }
        if self.static_token.is_some() {
            tracing::warn!("STATIC_TOKEN is set — bypasses OIDC. Development/demo only!");
            return Ok(());
        }
        if self.oidc_issuer.is_none() {
            bail!(
                "--oidc-issuer (or OIDC_ISSUER) is required. Use --no-auth or STATIC_TOKEN for local dev."
            );
        }
        if self.oidc_audience.is_none() {
            bail!(
                "--oidc-audience (or OIDC_AUDIENCE) is required. Use --no-auth or STATIC_TOKEN for local dev."
            );
        }
        Ok(())
    }

    pub fn admin_groups(&self) -> Vec<&str> {
        self.auth_groups_admin.split(',').map(str::trim).collect()
    }

    pub fn read_groups(&self) -> Vec<&str> {
        self.auth_groups_read.split(',').map(str::trim).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn base() -> Config {
        Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            oidc_issuer: None,
            oidc_audience: None,
            auth_groups_admin: "kubernetes-admin".into(),
            auth_groups_read: "kubernetes-view,kubernetes-edit".into(),
            no_auth: false,
            static_token: None,
            public_url: None,
            oidc_ca_cert: None,
            metrics_listen: "0.0.0.0:9090".into(),
            otel_endpoint: None,
            sandbox_root: Path::new("/workspace").into(),
        }
    }

    #[test]
    fn admin_groups_single() {
        assert_eq!(base().admin_groups(), vec!["kubernetes-admin"]);
    }

    #[test]
    fn read_groups_multiple() {
        assert_eq!(
            base().read_groups(),
            vec!["kubernetes-view", "kubernetes-edit"]
        );
    }

    #[test]
    fn groups_trimmed() {
        let mut c = base();
        c.auth_groups_read = " group-a , group-b ".into();
        assert_eq!(c.read_groups(), vec!["group-a", "group-b"]);
    }

    #[test]
    fn validate_no_auth_skips_oidc_check() {
        let mut c = base();
        c.no_auth = true;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn validate_requires_issuer() {
        let mut c = base();
        c.oidc_audience = Some("aud".into());
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_requires_audience() {
        let mut c = base();
        c.oidc_issuer = Some("https://example.com".into());
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_static_token_skips_oidc_check() {
        let mut c = base();
        c.static_token = Some("demo-token".into());
        assert!(c.validate().is_ok());
    }

    #[test]
    fn validate_passes_with_both_oidc_params() {
        let mut c = base();
        c.oidc_issuer = Some("https://example.com".into());
        c.oidc_audience = Some("my-client".into());
        assert!(c.validate().is_ok());
    }

    #[test]
    fn oauth_metadata_url_uses_default_when_none() {
        assert_eq!(
            oauth_metadata_url(None),
            "http://localhost:3000/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn oauth_metadata_url_uses_public_url() {
        assert_eq!(
            oauth_metadata_url(Some("https://mcp.example.com")),
            "https://mcp.example.com/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn oauth_metadata_url_trims_trailing_slash() {
        assert_eq!(
            oauth_metadata_url(Some("https://mcp.example.com/")),
            "https://mcp.example.com/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn sandbox_root_defaults_to_workspace() {
        assert_eq!(base().sandbox_root, Path::new("/workspace"));
    }
}
