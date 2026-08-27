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
        test_auth_state_with_db(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        )
    }

    /// Comme `test_auth_state()`, mais avec une vraie connexion (ex: sqlite en mémoire avec
    /// migrations appliquées, cf. `db::test_support::real_db`) — `MockDatabase` ne pose aucun
    /// vrai schéma, seulement des résultats de requête pré-programmés, insuffisant pour les
    /// tests qui vérifient un aller-retour DB réel (désérialisation + insert).
    pub(crate) fn test_auth_state_with_db(db: sea_orm::DatabaseConnection) -> MiryadAuthState {
        MiryadAuthState {
            oidc_client: std::sync::Arc::new(MockMiryadOidcClient),
            cookie_key: ::cookie::Key::from(&[0u8; 64]),
            post_login_redirect: "/".to_string(),
            post_logout_redirect: "/".to_string(),
            db,
        }
    }
}
