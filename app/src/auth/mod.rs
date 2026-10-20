#[cfg(test)]
pub(crate) mod test_support {
    use miryad_core::auth::oidc::{OidcIdentity, OidcLoginResult};
    use miryad_core::auth::{AuthError, MiryadAuthState, OidcClientTrait};
    use openidconnect::{CsrfToken, Nonce, PkceCodeVerifier};

    /// Poivre HMAC des empreintes de tokens API pour les tests de l'app — partagé entre
    /// `test_auth_state_with_db` (validation) et les `issue_token` des fixtures (ex.
    /// `bearer_for` de `../db/entities/agents.rs`) : émission et validation doivent voir le
    /// même poivre, sinon `validate_token` rejette le clair émis.
    pub(crate) const TEST_TOKEN_PEPPER: &str = "test-token-pepper";

    struct MockMiryadOidcClient;

    #[async_trait::async_trait]
    impl OidcClientTrait for MockMiryadOidcClient {
        fn authorization_url(
            &self,
        ) -> (openidconnect::url::Url, CsrfToken, Nonce, PkceCodeVerifier) {
            (
                openidconnect::url::Url::parse("https://issuer.example.com/authorize")
                    .expect("valid test url"),
                CsrfToken::new("csrf".to_string()),
                Nonce::new("nonce".to_string()),
                PkceCodeVerifier::new("test-pkce-verifier".to_string()),
            )
        }
        async fn exchange_code(
            &self,
            _code: &str,
            _expected_nonce: &Nonce,
            _pkce_verifier: &PkceCodeVerifier,
        ) -> Result<OidcLoginResult, AuthError> {
            Err(AuthError::Oidc("MRD-TEST-001: mock not used".to_string()))
        }
        fn verify_bearer(&self, _jwt: &str) -> Result<OidcIdentity, AuthError> {
            Err(AuthError::Oidc("MRD-TEST-002: mock not used".to_string()))
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
            // HTTP de test : les tours de route n'exercent pas l'attribut `Secure` des cookies.
            secure_cookies: false,
            token_pepper: TEST_TOKEN_PEPPER.to_string(),
        }
    }
}

/// Tests de verrouillage des 4 `Scenario` de `./mod.sdd` (décision Sébastien 2026-10-05) :
/// chaque test porte en commentaire doc le titre du `Scenario` dont il dérive, et n'ajoute
/// aucun comportement — il fige les valeurs de `Returns` du contrat de `test_support`.
#[cfg(test)]
mod scenario_tests {

    use miryad_core::auth::AuthError;
    use openidconnect::{Nonce, PkceCodeVerifier};
    use sea_orm::{
        ActiveValue::{NotSet, Set},
        ConnectionTrait, EntityTrait, Statement,
    };

    use crate::auth::test_support::{TEST_TOKEN_PEPPER, test_auth_state, test_auth_state_with_db};
    use crate::db::entities::llm_providers;
    use crate::db::test_support::real_db;

    /// Scenario: `etat_de_test_avec_base_mock`
    #[tokio::test]
    async fn etat_de_test_avec_base_mock() {
        let state = test_auth_state();

        assert_eq!(state.post_login_redirect, "/");
        assert_eq!(state.post_logout_redirect, "/");
        assert!(!state.secure_cookies);
        assert_eq!(state.token_pepper, TEST_TOKEN_PEPPER);

        // `cookie_key` dérivé de `&[0u8; 64]` : en cookie 0.18, `Key::from(&[u8])` stocke les
        // octets bruts et `master()` les relit — l'égalité des matériaux avec une clé de
        // référence construite sur le même littéral prouve la dérivation demandée.
        let reference = ::cookie::Key::from(&[0u8; 64]);
        assert_eq!(state.cookie_key.master(), reference.master());

        // Base mock backend Sqlite : le backend rapporté est `Sqlite` et une requête sans
        // résultat pré-programmé rend l'erreur propre à `MockDatabase` (une connexion réelle
        // exécuterait le SELECT), ce qui prouve qu'il s'agit bien d'une base scriptée.
        assert_eq!(
            state.db.get_database_backend(),
            sea_orm::DatabaseBackend::Sqlite
        );
        let err = state
            .db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT 1",
            ))
            .await
            .expect_err("empty mock queue rejects the query");
        assert!(
            err.to_string().contains("`query_results` buffer is empty"),
            "expected sea-orm MockDatabase error, got: {err}"
        );

        // « acceptant des résultats de requête pré-programmés » : un état construit par le
        // même chemin (`test_auth_state_with_db`) sur une `MockDatabase` alimentée rend bien
        // les lignes scriptées.
        let mut row = std::collections::BTreeMap::new();
        row.insert("ok".to_string(), sea_orm::Value::Bool(Some(true)));
        let scripted = sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite)
            .append_query_results([vec![row]]);
        let scripted_state = test_auth_state_with_db(scripted.into_connection());
        let rows = scripted_state
            .db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT 1",
            ))
            .await
            .expect("pre-programmed result is served");
        assert_eq!(rows.len(), 1);
    }

    /// Scenario: `etat_de_test_branche_sur_une_connexion_reelle`
    #[tokio::test]
    async fn etat_de_test_branche_sur_une_connexion_reelle() {
        let db = real_db().await;
        let state = test_auth_state_with_db(db);

        // Configuration figée identique à `test_auth_state` (mêmes valeurs que Returns).
        assert_eq!(state.post_login_redirect, "/");
        assert_eq!(state.post_logout_redirect, "/");
        assert!(!state.secure_cookies);
        assert_eq!(state.token_pepper, TEST_TOKEN_PEPPER);

        // Aller-retour DB réel impossible avec MockDatabase : insert puis relecture par
        // l'état lui-même (désérialisation réelle, cf. pattern de db/entities/agents.rs).
        let provider = llm_providers::ActiveModel {
            id: NotSet,
            name: Set("roundtrip".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(false),
        };
        let inserted = llm_providers::Entity::insert(provider)
            .exec(&state.db)
            .await
            .expect("provider inserts");
        let read_back = llm_providers::Entity::find_by_id(inserted.last_insert_id)
            .one(&state.db)
            .await
            .expect("provider reads back")
            .expect("row exists");
        assert_eq!(read_back.name, "roundtrip");
        assert_eq!(read_back.provider_type, "ollama");
    }

    /// Scenario: `echange_de_code_toujours_refuse`
    #[tokio::test]
    async fn echange_de_code_toujours_refuse() {
        let state = test_auth_state();

        let result = state
            .oidc_client
            .exchange_code(
                "any-code",
                &Nonce::new("any-nonce".to_string()),
                &PkceCodeVerifier::new("any-verifier".to_string()),
            )
            .await;

        let err = result.expect_err("the mock never serves a real exchange");
        assert!(
            matches!(&err, AuthError::Oidc(message) if message.contains("MRD-TEST-001")),
            "expected MRD-TEST-001 in: {err}"
        );
    }

    /// Scenario: `validation_de_bearer_toujours_refusee`
    #[test]
    fn validation_de_bearer_toujours_refusee() {
        let state = test_auth_state();

        let err = state
            .oidc_client
            .verify_bearer("any.jwt.value")
            .expect_err("the mock never validates a Bearer");
        assert!(
            matches!(&err, AuthError::Oidc(message) if message.contains("MRD-TEST-002")),
            "expected MRD-TEST-002 in: {err}"
        );
    }

    /// Scenario: `url_d_autorisation_deterministe`
    #[test]
    fn url_d_autorisation_deterministe() {
        let state = test_auth_state();

        let first = state.oidc_client.authorization_url();
        let second = state.oidc_client.authorization_url();

        assert_eq!(first.0.as_str(), "https://issuer.example.com/authorize");
        assert_eq!(first.0, second.0);
        assert_eq!(first.1.secret(), "csrf");
        assert_eq!(first.1.secret(), second.1.secret());
        assert_eq!(first.2.secret(), "nonce");
        assert_eq!(first.2.secret(), second.2.secret());
        assert_eq!(first.3.secret(), "test-pkce-verifier");
        assert_eq!(first.3.secret(), second.3.secret());
    }
}
