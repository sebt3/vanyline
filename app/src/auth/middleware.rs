use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::auth::cookie::extract_token;
use crate::error::AppError;
use crate::AppState;

pub struct AuthUser {
    #[allow(dead_code)]
    pub id_token: String,
    pub email: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let cookie_header = parts
            .headers
            .get("Cookie")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let (id_token, email) =
            extract_token(cookie_header.as_deref(), &state.cookie_key).map_err(|e| {
                tracing::debug!("VNL-AUTH-001: auth rejected: {}", e);
                e
            })?;

        tracing::debug!(email = %email, "auth ok");
        Ok(AuthUser { id_token, email })
    }
}

pub struct AdminAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let expected = format!("Bearer {}", state.config.admin_secret);
        match auth_header {
            Some(h) if h == expected => Ok(AdminAuth),
            _ => {
                tracing::debug!("VNL-AUTH-004: admin auth rejected");
                Err(AppError::Forbidden)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::MockOidcClient;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        Router,
    };
    use base64::Engine;
    use tower::ServiceExt;

    fn test_key() -> cookie::Key {
        cookie::Key::from(&[0u8; 64])
    }

    async fn protected_handler(_user: AuthUser) -> impl axum::response::IntoResponse {
        axum::Json(serde_json::json!({ "authenticated": true }))
    }

    async fn admin_handler(_admin: AdminAuth) -> impl axum::response::IntoResponse {
        axum::Json(serde_json::json!({ "admin": true }))
    }

    fn make_app(cookie_key: cookie::Key) -> Router {
        let config = crate::config::Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
            cookie_secret: "0".repeat(64),
            database_url: "postgres://localhost/test".to_string(),
            admin_secret: "test-admin-secret".to_string(),
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
        };

        let state = AppState {
            config,
            oidc_client: std::sync::Arc::new(MockOidcClient),
            cookie_key,
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
        };

        Router::new()
            .route("/protected", axum::routing::get(protected_handler))
            .route("/admin", axum::routing::get(admin_handler))
            .with_state(state)
    }

    #[tokio::test]
    async fn protected_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/protected")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_with_valid_cookie_passes() {
        let key = test_key();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!(r#"{{"exp":{exp},"email":"test@example.com"}}"#));
        let jwt = format!("header.{payload}.sig");
        let set_cookie = crate::auth::cookie::build_set_cookie(&jwt, "test@example.com", &key);
        let cookie_value = set_cookie.split(';').next().unwrap().trim().to_string();

        let app = make_app(key);
        let req = Request::builder()
            .uri("/protected")
            .header("Cookie", cookie_value)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_with_correct_bearer_passes() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/admin")
            .header("Authorization", "Bearer test-admin-secret")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_with_wrong_bearer_returns_403() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/admin")
            .header("Authorization", "Bearer wrong")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
