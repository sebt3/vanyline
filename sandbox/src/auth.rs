use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet},
};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::{AppState, config::Config};

const JWKS_CACHE_TTL: Duration = Duration::from_secs(3600);

// ── OIDC / JWKS ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OidcDiscovery {
    jwks_uri: String,
}

struct JwksCache {
    jwks: JwkSet,
    jwks_uri: String,
    fetched_at: Instant,
}

pub struct AuthState {
    config: Arc<Config>,
    cache: RwLock<Option<JwksCache>>,
    http: reqwest::Client,
}

impl AuthState {
    pub fn new(config: Arc<Config>) -> Self {
        let http = Self::build_http_client(&config).expect("failed to build HTTP client for JWKS");
        Self {
            config,
            cache: RwLock::new(None),
            http,
        }
    }

    fn build_http_client(config: &Config) -> anyhow::Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder();
        if let Some(ca_path) = &config.oidc_ca_cert {
            let pem = std::fs::read(ca_path)
                .with_context(|| format!("reading OIDC CA cert: {ca_path}"))?;
            let cert = reqwest::Certificate::from_pem(&pem).context("parsing OIDC CA cert")?;
            builder = builder.add_root_certificate(cert);
        }
        Ok(builder.build()?)
    }

    async fn get_jwks_uri(&self) -> Result<String> {
        let issuer = self.config.oidc_issuer.as_ref().context(
            "oidc_issuer missing — should be unreachable: Config::validate() refuses to \
             start without --no-auth/STATIC_TOKEN unless oidc_issuer is set, and this \
             code path is only reached when neither is active",
        )?;
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let resp: OidcDiscovery = self
            .http
            .get(&url)
            .send()
            .await
            .context("OIDC discovery request failed")?
            .error_for_status()
            .context("OIDC discovery returned error status")?
            .json()
            .await
            .context("OIDC discovery parse failed")?;
        Ok(resp.jwks_uri)
    }

    async fn refresh_jwks(&self) -> Result<()> {
        let cached_uri = self.cache.read().await.as_ref().map(|c| c.jwks_uri.clone());

        let jwks_uri = match cached_uri {
            Some(uri) => uri,
            None => self.get_jwks_uri().await?,
        };

        let jwks: JwkSet = self
            .http
            .get(&jwks_uri)
            .send()
            .await
            .context("JWKS fetch failed")?
            .error_for_status()
            .context("JWKS returned error status")?
            .json()
            .await
            .context("JWKS parse failed")?;

        *self.cache.write().await = Some(JwksCache {
            jwks,
            jwks_uri,
            fetched_at: Instant::now(),
        });

        Ok(())
    }

    async fn ensure_jwks(&self) -> Result<()> {
        let needs_refresh = self
            .cache
            .read()
            .await
            .as_ref()
            .map(|c| c.fetched_at.elapsed() > JWKS_CACHE_TTL)
            .unwrap_or(true);

        if needs_refresh {
            self.refresh_jwks().await?;
        }
        Ok(())
    }

    pub async fn validate_token(&self, token: &str) -> Result<Claims, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::InvalidToken)?;
        let kid = header.kid.ok_or(AuthError::InvalidToken)?;

        self.ensure_jwks().await.map_err(|e| {
            tracing::error!("JWKS refresh failed: {e}");
            AuthError::JwksFetchFailed
        })?;

        let cache = self.cache.read().await;
        let jwks = &cache.as_ref().ok_or(AuthError::JwksFetchFailed)?.jwks;

        let jwk = jwks.find(&kid).ok_or(AuthError::KeyNotFound)?;

        let decoding_key = match &jwk.algorithm {
            AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e)
                .map_err(|_| AuthError::InvalidToken)?,
            _ => return Err(AuthError::UnsupportedAlgorithm),
        };

        let audience = self.config.oidc_audience.as_deref().unwrap_or("");
        let issuer = self.config.oidc_issuer.as_deref().unwrap_or("");

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[audience]);
        validation.set_issuer(&[issuer]);

        let data = decode::<Claims>(token, &decoding_key, &validation).map_err(|e| {
            tracing::debug!("JWT validation failed: {e}");
            AuthError::InvalidToken
        })?;

        Ok(data.claims)
    }

    pub fn check_access(&self, claims: &Claims) -> Result<AccessLevel, AuthError> {
        let groups = claims.groups.as_deref().unwrap_or(&[]);
        let admins = self.config.admin_groups();
        let readers = self.config.read_groups();

        if groups.iter().any(|g| admins.iter().any(|a| *a == g)) {
            return Ok(AccessLevel::Admin);
        }
        if groups.iter().any(|g| readers.iter().any(|r| *r == g)) {
            return Ok(AccessLevel::Read);
        }
        Err(AuthError::InsufficientPermissions)
    }
}

// ── Claims ───────────────────────────────────────────────────────────────────

#[derive(Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub groups: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessLevel {
    Admin,
    Read,
}

/// Injected by the auth middleware into request extensions.
/// Fields are read by tool handlers in derived projects.
#[derive(Clone)]
#[allow(dead_code)]
pub struct AuthInfo {
    pub subject: String,
    pub access: AccessLevel,
}

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing or malformed Authorization header")]
    MissingToken,
    #[error("invalid or expired token")]
    InvalidToken,
    #[error("could not fetch JWKS from identity provider")]
    JwksFetchFailed,
    #[error("signing key not found in JWKS")]
    KeyNotFound,
    #[error("unsupported signing algorithm (RS256 required)")]
    UnsupportedAlgorithm,
    #[error("insufficient permissions — check group membership in Authentik")]
    InsufficientPermissions,
}

impl AuthError {
    /// Build the HTTP response, emitting an RFC 9728-conformant absolute
    /// `resource_metadata` URI in `WWW-Authenticate` for 401s. `public_url` is the
    /// operator-configured public URL (`None` → localhost default).
    fn into_response_with_metadata(self, public_url: Option<&str>) -> Response {
        let (status, error_code) = match &self {
            AuthError::InsufficientPermissions => (StatusCode::FORBIDDEN, "insufficient_scope"),
            _ => (StatusCode::UNAUTHORIZED, "invalid_token"),
        };

        let mut resp = (
            status,
            Json(serde_json::json!({
                "error": error_code,
                "error_description": self.to_string(),
            })),
        )
            .into_response();

        if status == StatusCode::UNAUTHORIZED {
            let metadata_url = crate::config::oauth_metadata_url(public_url);
            let value =
                format!(r#"Bearer error="invalid_token", resource_metadata="{metadata_url}""#);
            // Invariant VMP-130: a configured public_url yields a valid header value.
            let header = axum::http::HeaderValue::from_str(&value)
                .expect("VMP-130: resource_metadata URL must be a valid HTTP header value");
            resp.headers_mut()
                .insert(axum::http::header::WWW_AUTHENTICATE, header);
        }

        resp
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        self.into_response_with_metadata(None)
    }
}

// ── Middleware ────────────────────────────────────────────────────────────────

pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match authenticate(&state, req, next).await {
        Ok(resp) => resp,
        Err(err) => err.into_response_with_metadata(state.config.public_url.as_deref()),
    }
}

async fn authenticate(
    state: &AppState,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    if state.config.no_auth {
        req.extensions_mut().insert(AuthInfo {
            subject: "dev-no-auth".into(),
            access: AccessLevel::Admin,
        });
        return Ok(next.run(req).await);
    }

    // Static token auth (dev/demo bypass — not for production)
    if let Some(ref expected) = state.config.static_token {
        let token = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| {
                metrics::counter!("mcp_auth_errors_total", "reason" => "missing_token")
                    .increment(1);
                AuthError::MissingToken
            })?;
        if token == expected.as_str() {
            req.extensions_mut().insert(AuthInfo {
                subject: "static-token".into(),
                access: AccessLevel::Admin,
            });
            return Ok(next.run(req).await);
        }
        metrics::counter!("mcp_auth_errors_total", "reason" => "invalid_token").increment(1);
        return Err(AuthError::InvalidToken);
    }

    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            metrics::counter!("mcp_auth_errors_total", "reason" => "missing_token").increment(1);
            AuthError::MissingToken
        })?;

    let claims = state.auth.validate_token(token).await.inspect_err(|e| {
        let reason = match e {
            AuthError::InvalidToken => "invalid_token",
            AuthError::JwksFetchFailed => "jwks_fetch_failed",
            AuthError::KeyNotFound => "key_not_found",
            AuthError::UnsupportedAlgorithm => "unsupported_algorithm",
            _ => "unknown",
        };
        metrics::counter!("mcp_auth_errors_total", "reason" => reason).increment(1);
    })?;

    let access = state.auth.check_access(&claims).inspect_err(|_e| {
        metrics::counter!("mcp_auth_errors_total", "reason" => "insufficient_permissions")
            .increment(1);
    })?;

    req.extensions_mut().insert(AuthInfo {
        subject: claims.sub,
        access,
    });

    Ok(next.run(req).await)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn make_auth(admin: &str, read: &str) -> AuthState {
        let config = Arc::new(Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            oidc_issuer: None,
            oidc_audience: None,
            auth_groups_admin: admin.into(),
            auth_groups_read: read.into(),
            no_auth: false,
            static_token: None,
            public_url: None,
            oidc_ca_cert: None,
            metrics_listen: "0.0.0.0:9090".into(),
            otel_endpoint: None,
            sandbox_root: std::path::Path::new("/workspace").into(),
        });
        AuthState::new(config)
    }

    fn claims(groups: &[&str]) -> Claims {
        Claims {
            sub: "user@example.com".into(),
            groups: Some(groups.iter().map(|s| s.to_string()).collect()),
        }
    }

    #[test]
    fn admin_group_grants_admin_access() {
        let auth = make_auth("kubernetes-admin", "kubernetes-view");
        assert_eq!(
            auth.check_access(&claims(&["kubernetes-admin"])).unwrap(),
            AccessLevel::Admin
        );
    }

    #[test]
    fn read_group_grants_read_access() {
        let auth = make_auth("kubernetes-admin", "kubernetes-view");
        assert_eq!(
            auth.check_access(&claims(&["kubernetes-view"])).unwrap(),
            AccessLevel::Read
        );
    }

    #[test]
    fn admin_takes_precedence_over_read() {
        let auth = make_auth("kubernetes-admin", "kubernetes-view");
        assert_eq!(
            auth.check_access(&claims(&["kubernetes-admin", "kubernetes-view"]))
                .unwrap(),
            AccessLevel::Admin
        );
    }

    #[test]
    fn unknown_group_is_denied() {
        let auth = make_auth("kubernetes-admin", "kubernetes-view");
        assert!(matches!(
            auth.check_access(&claims(&["some-other-group"])),
            Err(AuthError::InsufficientPermissions)
        ));
    }

    #[test]
    fn no_groups_claim_is_denied() {
        let auth = make_auth("kubernetes-admin", "kubernetes-view");
        let c = Claims {
            sub: "user".into(),
            groups: None,
        };
        assert!(matches!(
            auth.check_access(&c),
            Err(AuthError::InsufficientPermissions)
        ));
    }

    #[test]
    fn missing_token_returns_401_with_www_authenticate() {
        let resp = AuthError::MissingToken.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE)
        );
    }

    #[test]
    fn invalid_token_returns_401_with_www_authenticate() {
        let resp = AuthError::InvalidToken.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE)
        );
    }

    #[test]
    fn insufficient_permissions_returns_403_without_www_authenticate() {
        let resp = AuthError::InsufficientPermissions.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(
            !resp
                .headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE)
        );
    }

    #[test]
    fn multi_group_config_all_read_groups_accepted() {
        let auth = make_auth("kubernetes-admin", "kubernetes-view,kubernetes-edit");
        assert_eq!(
            auth.check_access(&claims(&["kubernetes-edit"])).unwrap(),
            AccessLevel::Read
        );
        assert_eq!(
            auth.check_access(&claims(&["kubernetes-view"])).unwrap(),
            AccessLevel::Read
        );
    }

    #[test]
    fn www_authenticate_uses_absolute_resource_metadata_uri() {
        let resp =
            AuthError::InvalidToken.into_response_with_metadata(Some("https://mcp.example.com"));
        let header = resp
            .headers()
            .get(axum::http::header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(
            header,
            r#"Bearer error="invalid_token", resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource""#
        );
    }

    #[test]
    fn www_authenticate_defaults_to_localhost_absolute_uri() {
        let resp = AuthError::MissingToken.into_response_with_metadata(None);
        let header = resp
            .headers()
            .get(axum::http::header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(header.contains(
            "resource_metadata=\"http://localhost:3000/.well-known/oauth-protected-resource\""
        ));
    }

    #[tokio::test]
    async fn get_jwks_uri_without_issuer_returns_error_not_panic() {
        let auth = make_auth("kubernetes-admin", "kubernetes-view");
        let result = auth.get_jwks_uri().await;
        assert!(result.is_err(), "should return an error, not panic, when oidc_issuer is None");
    }
}
