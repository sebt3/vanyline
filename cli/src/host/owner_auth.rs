//! Authentification des appels MCP vers la sandbox par le jeton d'Owner. Contrat : `host/owner_auth.sdd`.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use lymaille::{McpAuth, McpError, McpHeaders};
use vanyline_lib::k8s::{OwnerToken, VnlK8sClient};
use zeroize::Zeroize as _;

/// Marqueur d'un jeton d'Owner refusé ou impossible à obtenir.
const TOKEN: &str = "VNL-CLI-HST-043";

/// Source du jeton d'Owner : le client K8s de la lib en production, un faux en test (seam, `owner_auth.sdd`).
pub trait OwnerTokenSource: Send + Sync {
    /// Jeton frais de l'`owner`. Le cache et la marge de renouvellement sont ceux de la lib.
    fn owner_token<'a>(
        &'a self,
        owner: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<OwnerToken, TokenRefused>> + Send + 'a>>;
}

/// Refus de la source, sans cause : rien d'un jeton ni d'une URL ne remonte jusqu'à lymaille.
#[derive(Debug)]
pub struct TokenRefused;

/// En-têtes d'authentification des appels MCP d'une session, pour un Owner.
pub struct OwnerAuth<S: OwnerTokenSource = VnlK8sClient> {
    source: S,
    owner: String,
}

impl OwnerAuth<VnlK8sClient> {
    /// Authentification de l'`owner` par le client K8s de la lib.
    #[must_use]
    pub fn new(client: VnlK8sClient, owner: String) -> Self {
        Self::with_source(client, owner)
    }
}

impl<S: OwnerTokenSource> OwnerAuth<S> {
    /// Authentification de l'`owner` par une source donnée (seam de test).
    #[must_use]
    pub fn with_source(source: S, owner: String) -> Self {
        Self { source, owner }
    }
}

impl<S: OwnerTokenSource> fmt::Debug for OwnerAuth<S> {
    // Le nom de l'Owner seul : aucun jeton ne se affiche.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerAuth")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl OwnerTokenSource for VnlK8sClient {
    fn owner_token<'a>(
        &'a self,
        owner: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<OwnerToken, TokenRefused>> + Send + 'a>> {
        Box::pin(async move {
            VnlK8sClient::owner_token(self, owner)
                .await
                .map_err(|erreur| {
                    // le message de la lib ne porte jamais le jeton (lib/src/k8s.sdd)
                    tracing::debug!("{TOKEN}: jeton de l'Owner {owner} non obtenu: {erreur}");
                    TokenRefused
                })
        })
    }
}

impl<S: OwnerTokenSource> McpAuth for OwnerAuth<S> {
    async fn headers(&self) -> Result<McpHeaders, McpError> {
        // Pas de cache ici : la lib renouvelle le jeton avant son expiration.
        let jeton = self
            .source
            .owner_token(&self.owner)
            .await
            .map_err(|TokenRefused| {
                McpError::Failed(format!(
                    "{TOKEN}: jeton de l'Owner {} indisponible",
                    self.owner
                ))
            })?;
        let mut token = jeton.token;
        let headers = McpHeaders::new().with_bearer(&token);
        // la copie locale est effacée ; celle de lymaille reste opaque (ni `Debug` ni message)
        token.zeroize();
        Ok(headers)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use lymaille::{McpAuth as _, McpError};

    use super::{OwnerAuth, OwnerTokenSource, TokenRefused};
    use vanyline_lib::k8s::OwnerToken;

    /// Faux client : une réponse par appel, `None` = refus. Compte ses appels.
    struct FauxClient {
        reponses: Mutex<VecDeque<Option<String>>>,
        appels: AtomicUsize,
    }

    impl FauxClient {
        fn avec(reponses: Vec<Option<&str>>) -> Self {
            Self {
                reponses: Mutex::new(reponses.into_iter().map(|r| r.map(str::to_owned)).collect()),
                appels: AtomicUsize::new(0),
            }
        }
    }

    impl OwnerTokenSource for FauxClient {
        fn owner_token<'a>(
            &'a self,
            _owner: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<OwnerToken, TokenRefused>> + Send + 'a>,
        > {
            Box::pin(async move {
                let _ = self.appels.fetch_add(1, Ordering::SeqCst);
                let suivante = self
                    .reponses
                    .lock()
                    .ok()
                    .and_then(|mut file| file.pop_front())
                    .flatten();
                suivante
                    .map(|token| OwnerToken {
                        token,
                        expires_at: chrono::Utc::now(),
                    })
                    .ok_or(TokenRefused)
            })
        }
    }

    /// Scenario: jeton ajouté à chaque appel
    #[tokio::test]
    async fn jeton_ajoute_a_chaque_appel() {
        let auth = OwnerAuth::with_source(
            FauxClient::avec(vec![Some("jeton-secret-1"), Some("jeton-secret-1")]),
            "alice".into(),
        );
        let premier = auth.headers().await;
        let second = auth.headers().await;
        assert!(premier.is_ok());
        assert!(second.is_ok());
        let debug = format!("{:?}", premier.ok());
        assert!(
            debug.contains("bearer: true"),
            "pas d'en-tête Bearer : {debug}"
        );
        assert!(!debug.contains("jeton-secret-1"));
        assert_eq!(auth.source.appels.load(Ordering::SeqCst), 2);
    }

    /// Scenario: renouvellement en session longue — aucun cache de l'hôte : la source est sollicitée à chaque appel.
    #[tokio::test]
    async fn renouvellement_demande_a_la_source_a_chaque_appel() {
        let auth = OwnerAuth::with_source(
            FauxClient::avec(vec![Some("jeton-expire"), Some("jeton-neuf")]),
            "alice".into(),
        );
        assert!(auth.headers().await.is_ok());
        assert!(auth.headers().await.is_ok());
        assert_eq!(auth.source.appels.load(Ordering::SeqCst), 2);
    }

    /// Scenario: échec de `TokenRequest` — erreur sans jeton ni URL.
    #[tokio::test]
    async fn echec_de_tokenrequest_sans_jeton_ni_url() {
        let auth = OwnerAuth::with_source(FauxClient::avec(vec![None]), "alice".into());
        match auth.headers().await {
            Err(McpError::Failed(message)) => {
                assert!(
                    message.contains("VNL-CLI-HST-043"),
                    "marqueur absent : {message}"
                );
                assert!(!message.contains("http"), "URL dans le message : {message}");
                assert!(
                    !message.contains("jeton-"),
                    "jeton dans le message : {message}"
                );
            }
            autre => panic!("erreur attendue, obtenu {autre:?}"),
        }
    }

    /// Le Debug de l'hôte ne montre que le nom de l'Owner, jamais un jeton.
    #[test]
    fn debug_montre_le_nom_de_l_owner_sans_jeton() {
        let auth = OwnerAuth::with_source(
            FauxClient::avec(vec![Some("jeton-secret-2")]),
            "alice".into(),
        );
        let debug = format!("{auth:?}");
        assert!(debug.contains("alice"));
        assert!(!debug.contains("jeton-secret-2"));
    }
}
