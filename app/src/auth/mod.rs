pub mod cookie;
pub mod middleware;
pub mod oidc;

pub use oidc::OidcClientTrait;

#[cfg(test)]
pub use oidc::MockOidcClient;

#[cfg(test)]
pub(crate) mod test_support {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use miryad_core::auth::oidc::OidcLoginResult;
    use miryad_core::auth::{AuthError, MiryadAuthState, OidcClientTrait};
    use openidconnect::{CsrfToken, Nonce};

    struct MockMiryadOidcClient;

    #[async_trait::async_trait]
    impl OidcClientTrait for MockMiryadOidcClient {
        fn authorization_url(&self) -> (openidconnect::url::Url, CsrfToken, Nonce) {
            (
                openidconnect::url::Url::parse("https://issuer.example.com/authorize")
                    .expect("valid test url"),
                CsrfToken::new("csrf".to_string()),
                Nonce::new("nonce".to_string()),
            )
        }
        async fn exchange_code(
            &self,
            _code: &str,
            _expected_nonce: &Nonce,
        ) -> Result<OidcLoginResult, AuthError> {
            Err(AuthError::Oidc("MRD-TEST-001: mock not used".to_string()))
        }
    }

    pub(crate) fn test_auth_state() -> MiryadAuthState {
        MiryadAuthState {
            oidc_client: std::sync::Arc::new(MockMiryadOidcClient),
            cookie_key: ::cookie::Key::from(&[0u8; 64]),
            post_login_redirect: "/".to_string(),
            post_logout_redirect: "/".to_string(),
            db: sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        }
    }
}
