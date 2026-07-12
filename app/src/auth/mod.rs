pub mod cookie;
pub mod middleware;
pub mod oidc;

pub use oidc::OidcClientTrait;

#[cfg(test)]
pub use oidc::MockOidcClient;

use axum::{
    extract::{Query, State},
    http::{header::SET_COOKIE, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use serde::Deserialize;

use crate::{error::AppError, AppState};

use ::cookie::{Cookie, CookieJar};

use crate::auth::cookie::{build_set_cookie, clear_cookie};

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/login", axum::routing::get(handler_login))
        .route("/callback", axum::routing::get(handler_callback))
        .route("/logout", axum::routing::get(handler_logout))
}

#[derive(Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
}

async fn handler_login(State(state): State<AppState>) -> impl IntoResponse {
    let (url, csrf_token, nonce) = state.oidc_client.authorization_url();
    tracing::info!(url = %url, "OIDC login → redirect");

    let pending_value = format!("{}:{}", csrf_token.secret(), nonce.secret());

    let mut jar = CookieJar::new();
    let mut private_jar = jar.private_mut(&state.cookie_key);
    private_jar.add(Cookie::new("oidc_pending", pending_value));

    let encrypted = jar.get("oidc_pending");
    let encrypted_value = encrypted.map(|c: &Cookie| c.value()).unwrap_or("");

    let set_cookie_pending = format!(
        "oidc_pending={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=300",
        encrypted_value
    );

    Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", url.as_str())
        .header(SET_COOKIE, set_cookie_pending)
        .body(axum::body::Body::empty())
        .unwrap()
        .into_response()
}

async fn handler_callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let cookie_header = headers
        .get("Cookie")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let pending_value = cookie_header
        .as_ref()
        .and_then(|h| {
            h.split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .find_map(|c| {
                    c.split_once('=').and_then(|(name, value)| {
                        if name == "oidc_pending" {
                            Some(value.to_string())
                        } else {
                            None
                        }
                    })
                })
        })
        .ok_or(AppError::OidcError("VNL-AUTH-003: missing oidc_pending cookie".to_string()))?;

    let jar = CookieJar::new();
    let private_jar = jar.private(&state.cookie_key);
    let raw_cookie = Cookie::new("oidc_pending", pending_value);
    let decrypted = private_jar
        .decrypt(raw_cookie)
        .ok_or(AppError::OidcError("VNL-AUTH-003: invalid oidc_pending cookie".to_string()))?;

    let value = decrypted.value();
    let parts: Vec<&str> = value.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(AppError::OidcError("VNL-AUTH-003: malformed oidc_pending value".to_string()));
    }

    let expected_csrf = parts[0];
    let nonce = openidconnect::Nonce::new(parts[1].to_string());

    if params.state != expected_csrf {
        tracing::warn!("VNL-AUTH-003: CSRF state mismatch");
        return Err(AppError::OidcError("VNL-AUTH-003: invalid CSRF state".to_string()));
    }

    let (id_token_str, email) = state
        .oidc_client
        .exchange_code(&params.code, &nonce)
        .await?;

    let set_cookie_main = build_set_cookie(&id_token_str, &email, &state.cookie_key);
    let set_cookie_clear_pending = "oidc_pending=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0";

    tracing::info!(email = %email, "OIDC authentication successful");

    Ok(Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", "/#/")
        .header(SET_COOKIE, set_cookie_main)
        .header(SET_COOKIE, set_cookie_clear_pending)
        .body(axum::body::Body::empty())
        .unwrap()
        .into_response())
}

async fn handler_logout() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", "/#/")
        .header(SET_COOKIE, clear_cookie())
        .body(axum::body::Body::empty())
        .unwrap()
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    fn test_key() -> ::cookie::Key {
        ::cookie::Key::from(&[0u8; 64])
    }

    fn make_app() -> axum::Router {
        let config = crate::config::Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
            cookie_secret: "0".repeat(64),
            database_url: "postgres://localhost/test".to_string(),
            
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
        };

        let state = AppState {
            config,
            oidc_client: std::sync::Arc::new(MockOidcClient),
            cookie_key: test_key(),
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
        };

        auth_router().with_state(state)
    }

    #[tokio::test]
    async fn logout_clears_cookie() {
        let app = make_app();
        let req = Request::builder()
            .uri("/logout")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(resp.headers().get("Location").unwrap(), "/#/");
        let set_cookie = resp.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
        assert!(set_cookie.contains("Max-Age=0"));
        assert!(set_cookie.contains(crate::auth::cookie::COOKIE_NAME));
    }

    #[tokio::test]
    async fn callback_without_pending_cookie_returns_error() {
        let app = make_app();
        let req = Request::builder()
            .uri("/callback?code=test&state=wrong")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::UNAUTHORIZED
                || resp.status().is_client_error()
                || resp.status().is_server_error()
        );
    }
}
