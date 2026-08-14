use axum::{Json, extract::State};
use serde::Serialize;

use crate::{
    AppState, api::conversations::get_or_create_user, auth::middleware::AuthUser, error::AppError,
};

#[derive(Serialize)]
pub struct MeResponse {
    pub email: String,
    pub k8s_owner_name: Option<String>,
}

pub async fn handler_me(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<MeResponse>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    Ok(Json(MeResponse {
        email: db_user.email,
        k8s_owner_name: db_user.k8s_owner_name,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::auth::MockOidcClient;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
    };
    use serde_json::json;
    use tower::ServiceExt;

    fn test_key() -> cookie::Key {
        cookie::Key::from(&[0u8; 64])
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
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
            k8s_namespace: None,
            application_name: None,
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
        };

        let state = AppState {
            config,
            oidc_client: std::sync::Arc::new(MockOidcClient),
            cookie_key,
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        };

        Router::new()
            .route("/me", get(handler_me))
            .with_state(state)
    }

    #[tokio::test]
    async fn handler_me_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder().uri("/me").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn me_response_serializes() {
        let with_none = MeResponse {
            email: "test@example.com".to_string(),
            k8s_owner_name: None,
        };
        let json = serde_json::to_value(&with_none).unwrap();
        assert_eq!(json["email"], "test@example.com");
        assert_eq!(json["k8s_owner_name"], json!(null));

        let with_some = MeResponse {
            email: "test@example.com".to_string(),
            k8s_owner_name: Some("owner-abc".to_string()),
        };
        let json = serde_json::to_value(&with_some).unwrap();
        assert_eq!(json["email"], "test@example.com");
        assert_eq!(json["k8s_owner_name"], "owner-abc");
    }
}
