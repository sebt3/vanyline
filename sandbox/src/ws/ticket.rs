//! WS ticket store: short-lived, single-use tickets for WebSocket authentication.
//!
//! The browser WebSocket API does not support custom headers on the handshake.
//! This module solves that by having the frontend call `POST /ws/ticket` (behind
//! `require_auth`), receive a short-lived opaque ticket, and then present it as
//! a query-string parameter on the `GET /ws/*` upgrade request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::auth::AccessLevel;
use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Middleware d'auth WS partagé par `/ws/fs` et `/ws/terminal`. Lit le ticket en
/// query string (`?ticket=`), le consomme via `redeem_from_query`, renvoie 401 en
/// cas d'erreur (MissingTicket/InvalidTicket), sinon laisse passer vers le handler.
pub async fn ws_auth_middleware(
    State(state): State<crate::AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // Extract ticket from query string
    let ticket = req.uri().query().and_then(|qs| {
        for pair in qs.split('&') {
            if let Some((key, value)) = pair.split_once('=')
                && key == "ticket"
            {
                return Some(value.to_string());
            }
        }
        None
    });

    match crate::ws::ticket::redeem_from_query(&state.tickets, ticket) {
        Ok(_) => next.run(req).await,
        Err(e) => e.into_response(),
    }
}

/// Convert a `WsAuthError` into an HTTP response (401 for both variants,
/// matching the token-auth error path).
impl IntoResponse for WsAuthError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            WsAuthError::MissingTicket => StatusCode::UNAUTHORIZED,
            WsAuthError::InvalidTicket => StatusCode::UNAUTHORIZED,
        };
        (
            status,
            Json(serde_json::json!({
                "error": self.to_string(),
            })),
        )
            .into_response()
    }
}

/// Duration of a ticket — a few seconds, long enough for the browser to
/// chain `POST /ws/ticket` then the WS handshake. Not a full session.
pub const TICKET_TTL_SECS: u64 = 30;

/// What a ticket authenticates — same shape as `AuthInfo` (auth.rs):
/// `subject` + `access`. This is the data that the ticket path must produce,
/// not a shortcut less rich than the JWT path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketClaims {
    pub subject: String,
    pub access: AccessLevel,
}

/// Error returned by the WS extractor when the ticket is missing/consumed.
/// Reused by `ws-fs` and `ws-terminal`.
#[derive(Debug, thiserror::Error)]
pub enum WsAuthError {
    #[error("VNL-SBX-WS-001: missing or malformed ticket")]
    MissingTicket,
    #[error("VNL-SBX-WS-002: invalid or expired ticket")]
    InvalidTicket,
}

/// In-memory store, single-use: each ticket is removed on the first `GET /ws/*`
/// that presents it, whether the request succeeds or not. Never reusable.
#[derive(Debug, Clone)]
pub struct TicketStore {
    tickets: Arc<Mutex<HashMap<String, (TicketClaims, Instant)>>>,
}

impl Default for TicketStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TicketStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self {
            tickets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Issues an opaque ticket for `claims`, dated now, with TTL `TICKET_TTL_SECS`.
    #[allow(clippy::expect_used)] // poison means a bug in *our* code, not an external actor
    pub fn issue(&self, claims: TicketClaims) -> String {
        use rand::{Rng, distributions::Alphanumeric};

        let ticket: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(32)
            .map(|c| c as char)
            .collect();

        let now = Instant::now();
        let ttl = Duration::from_secs(TICKET_TTL_SECS);

        let mut map = self
            .tickets
            .lock()
            .expect("ticket store mutex should not be poisoned");
        map.insert(ticket.clone(), (claims, now + ttl));
        ticket
    }

    /// Consumes the ticket. Removes the entry **no matter what** (valid or expired).
    /// Returns `Some(TicketClaims)` if the ticket is present and not expired;
    /// `None` otherwise.
    #[allow(clippy::expect_used)] // poison means a bug in *our* code, not an external actor
    pub fn redeem(&self, ticket: &str) -> Option<TicketClaims> {
        let mut map = self
            .tickets
            .lock()
            .expect("ticket store mutex should not be poisoned");
        if let Some((claims, expires_at)) = map.remove(ticket) {
            if Instant::now() < expires_at {
                Some(claims)
            } else {
                // Expired — already removed, return None
                None
            }
        } else {
            // Ticket already consumed or never existed
            None
        }
    }
}

/// Consomme `ticket` (retiré quoi qu'il arrive) pour produire `TicketClaims`.
/// `None` de query → `MissingTicket` ; ticket absent/expiré → `InvalidTicket`.
pub fn redeem_from_query(
    store: &TicketStore,
    ticket: Option<String>,
) -> Result<TicketClaims, WsAuthError> {
    match ticket {
        Some(t) => store.redeem(&t).ok_or(WsAuthError::InvalidTicket),
        None => Err(WsAuthError::MissingTicket),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::{AppState, AuthState, build_app, config::Config};
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header::AUTHORIZATION};
    use std::sync::Arc;
    use tower::ServiceExt;

    // Test 1: ticket_redeem_returns_claims
    #[test]
    fn ticket_redeem_returns_claims() {
        let store = TicketStore::new();
        let claims = TicketClaims {
            subject: "user-u".into(),
            access: AccessLevel::Admin,
        };
        let ticket = store.issue(claims.clone());
        let redeemed = store.redeem(&ticket);
        assert!(redeemed.is_some());
        let r = redeemed.unwrap();
        assert_eq!(r.subject, "user-u");
        assert_eq!(r.access, AccessLevel::Admin);
    }

    // Test 2: ticket_is_single_use
    #[test]
    fn ticket_is_single_use() {
        let store = TicketStore::new();
        let claims = TicketClaims {
            subject: "u".into(),
            access: AccessLevel::Admin,
        };
        let ticket = store.issue(claims);
        // First redeem succeeds
        assert!(store.redeem(&ticket).is_some());
        // Second redeem fails (ticket consumed)
        assert!(store.redeem(&ticket).is_none());
    }

    // Test 3: expired_ticket_redeem_none_and_removed
    #[test]
    fn expired_ticket_redeem_none_and_removed() {
        let store = TicketStore::new();
        // Inject an already-expired ticket directly into the map.
        // mod tests is inside the same module, so it has access to the private field.
        let claims = TicketClaims {
            subject: "u".into(),
            access: AccessLevel::Admin,
        };
        // Overwrite the Arc with a new map containing an expired entry
        let expired_instant = Instant::now() - Duration::from_secs(60);
        store
            .tickets
            .lock()
            .unwrap()
            .insert("expired-ticket".to_string(), (claims, expired_instant));

        // Redeem returns None (expired)
        assert!(store.redeem("expired-ticket").is_none());
        // Second redeem also returns None (entry already removed)
        assert!(store.redeem("expired-ticket").is_none());
    }

    // Test 4: issuance_without_auth_returns_401
    // Route-level test: POST /ws/ticket without Authorization → 401
    #[tokio::test]
    async fn issuance_without_auth_returns_401() {
        let config = make_config("admin", "reader");
        let auth = AuthState::new(config.clone()).unwrap();
        let app = build_app(AppState {
            config,
            auth: Arc::new(auth),
            tickets: TicketStore::new(),
            lsp: Arc::new(crate::lsp::LspManager::default()),
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/ws/ticket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // Test 5: issuance_with_static_token_returns_ticket
    // Route-level test: POST /ws/ticket with static_token auth → 200 + ticket,
    // and the issued ticket redeems to the same subject/access it was minted for.
    #[tokio::test]
    async fn issuance_with_static_token_returns_ticket() {
        let config = make_config_with_static_token("s3cret");
        let auth = AuthState::new(config.clone()).unwrap();
        // Keep a clone of the store outside the app. Since TicketStore internally
        // uses Arc<Mutex<...>>, the clone shares the same underlying map — we can
        // verify redemptions after the request completes.
        let external_store = TicketStore::new();
        let app = build_app(AppState {
            config,
            auth: Arc::new(auth),
            tickets: external_store.clone(),
            lsp: Arc::new(crate::lsp::LspManager::default()),
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/ws/ticket")
                    .header(AUTHORIZATION, "Bearer s3cret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        // Read the body to recover the issued opaque ticket.
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ticket = json["ticket"]
            .as_str()
            .expect("response must carry a ticket");

        // Redeem on the (shared) external store clone and verify the claims.
        let claims = external_store
            .redeem(ticket)
            .expect("the ticket issued by the handler must be redeemable");
        assert_eq!(claims.subject, "static-token");
        assert_eq!(claims.access, AccessLevel::Admin);
        // The ticket is single-use: a second redeem must fail.
        assert!(external_store.redeem(ticket).is_none());
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_config(admin: &str, read: &str) -> Arc<Config> {
        Arc::new(Config {
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
            sandbox_root: std::path::PathBuf::from("/workspace"),
        })
    }

    fn make_config_with_static_token(token: &str) -> Arc<Config> {
        Arc::new(Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            oidc_issuer: None,
            oidc_audience: None,
            auth_groups_admin: "kubernetes-admin".into(),
            auth_groups_read: "kubernetes-view,kubernetes-edit".into(),
            no_auth: false,
            static_token: Some(token.into()),
            public_url: None,
            oidc_ca_cert: None,
            metrics_listen: "0.0.0.0:9090".into(),
            otel_endpoint: None,
            sandbox_root: std::path::PathBuf::from("/workspace"),
        })
    }
}
