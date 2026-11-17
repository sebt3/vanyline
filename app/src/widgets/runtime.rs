//! Exécution d'un script par le `ScriptRunner` de miryad-core — contrat : `./runtime.sdd`
//! (`dsd/app/widgets.md` `FR-3`, `FR-5`, `FR-7`, `DEC-2`). L'app ne réimplémente ni les droits,
//! ni le montage des toolsets, ni l'annulation : elle construit l'exécuteur une fois, pose les
//! plafonds de la configuration et traduit les erreurs typées de la crate en marqueurs
//! `VNL-APP-WDG-*`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::Utc;
use miryad_core::lymaille::host_module::HostModules;
use miryad_core::lymaille::resource_module::ResourceRegistry;
use miryad_core::lymaille::script_runner::{ScriptDeps, ScriptLimits, ScriptRunner};
use miryad_core::lymaille::{EnvSecretResolver, LymailleError, SharedHost};
use sea_orm::DatabaseConnection;
use serde_json::Value as JsonValue;
use vynil_core::cancel::CancelToken;

use super::cache::{Clock, ComputeAborted, WidgetCache};
use super::model::RunError;
use crate::config::Config;

/// Longueur maximale du message d'une erreur de script rendu au widget (`FR-7`, message borné).
const LONGUEUR_MESSAGE_SCRIPT: usize = 200;

/// La couture d'exécution (`Exposes` de `./runtime.sdd`) : rend la valeur JSON du script, ou un
/// marqueur `VNL-APP-WDG-*`. Les tests de `./run.sdd` injectent un exécuteur factice.
pub trait ScriptExecutor: Send + Sync {
    /// Exécute `source` sous le sujet `subject`, dans le contexte `context`, annulable par
    /// `cancel` (`ScriptRunner::run_cancellable`). Rend la valeur JSON brute du script.
    fn execute<'a>(
        &'a self,
        context: &'a str,
        subject: &'a str,
        source: &'a str,
        cancel: &'a CancelToken,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, RunError>> + Send + 'a>>;
}

/// Exécuteur de production : le `ScriptRunner` de miryad-core, sans registre de ressources ni
/// module d'hôte (`./runtime.sdd` `Must`), avec les plafonds de la configuration.
pub struct CrateExecutor {
    runner: ScriptRunner,
    limits: ScriptLimits,
}

impl CrateExecutor {
    /// Construit le `ScriptRunner` sur la connexion `db`, l'hôte lymaille partagé `host` et les
    /// plafonds `widget_max_*` de `config`. Sans I/O : rien ne se connecte à la construction.
    #[must_use]
    pub fn new(db: DatabaseConnection, host: Option<SharedHost>, config: &Config) -> Self {
        let mut deps = ScriptDeps::new(
            db,
            Arc::new(ResourceRegistry::new()),
            Arc::new(HostModules::new()),
            Arc::new(EnvSecretResolver),
        );
        deps.host = host;
        Self {
            runner: ScriptRunner::new(deps),
            limits: ScriptLimits {
                max_duration: config.widget_max_duration,
                max_operations: config.widget_max_operations,
                max_result_bytes: config.widget_max_result_bytes,
            },
        }
    }
}

impl ScriptExecutor for CrateExecutor {
    fn execute<'a>(
        &'a self,
        context: &'a str,
        subject: &'a str,
        source: &'a str,
        cancel: &'a CancelToken,
    ) -> Pin<Box<dyn Future<Output = Result<JsonValue, RunError>> + Send + 'a>> {
        Box::pin(async move {
            self.runner
                .run_cancellable(context, subject, source, &self.limits, None, cancel)
                .await
                .map_err(traduire)
        })
    }
}

/// Le service d'exécution de l'application (`./runtime.sdd`) : handle `Clone` construit au
/// démarrage, portant l'exécuteur et le cache des résultats.
#[derive(Clone)]
pub struct WidgetService {
    executor: Arc<dyn ScriptExecutor>,
    cache: WidgetCache,
}

impl WidgetService {
    /// Service de production : `CrateExecutor` et cache à l'horloge système.
    #[must_use]
    pub fn new(db: DatabaseConnection, host: Option<SharedHost>, config: &Config) -> Self {
        let clock: Clock = Arc::new(Utc::now);
        Self {
            executor: Arc::new(CrateExecutor::new(db, host, config)),
            cache: WidgetCache::new(clock),
        }
    }

    /// Service sur un exécuteur fourni (séam des tests de `./run.sdd`).
    #[must_use]
    pub fn with_executor(executor: Arc<dyn ScriptExecutor>, cache: WidgetCache) -> Self {
        Self { executor, cache }
    }

    /// Exécute `source` sous `subject` dans `context` via l'exécuteur du service.
    ///
    /// # Errors
    ///
    /// Le marqueur `VNL-APP-WDG-*` de l'échec (`./runtime.sdd`, table de traduction).
    pub async fn execute(
        &self,
        context: &str,
        subject: &str,
        source: &str,
        cancel: &CancelToken,
    ) -> Result<JsonValue, RunError> {
        self.executor
            .execute(context, subject, source, cancel)
            .await
    }

    /// Service de test sans exécution réelle : tout script rend une panne `010`. Séam des
    /// littéraux `AppState` des tests qui ne touchent pas aux widgets.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn pour_tests() -> Self {
        struct ExecuteurNul;
        impl ScriptExecutor for ExecuteurNul {
            fn execute<'a>(
                &'a self,
                _context: &'a str,
                _subject: &'a str,
                _source: &'a str,
                _cancel: &'a CancelToken,
            ) -> Pin<Box<dyn Future<Output = Result<JsonValue, RunError>> + Send + 'a>>
            {
                Box::pin(async { Err(panne_interne()) })
            }
        }
        let clock: Clock = Arc::new(Utc::now);
        Self::with_executor(Arc::new(ExecuteurNul), WidgetCache::new(clock))
    }

    /// Le cache des résultats du service (`./cache.sdd`).
    #[must_use]
    pub fn cache(&self) -> &WidgetCache {
        &self.cache
    }
}

/// Traduction d'une erreur typée de la crate en marqueur `VNL-APP-WDG-*` (`./runtime.sdd`,
/// table « Traduction d'erreurs »). Corps de message fixe, sauf `ScriptFailed` (texte de la crate,
/// déjà sans source, ligne, jeton ni URL, borné). Une panne interne rend `010` et est journalisée.
#[must_use]
pub(crate) fn traduire(erreur: LymailleError) -> RunError {
    match erreur {
        LymailleError::ScriptTimeout => marqueur("004", "plafond de durée atteint"),
        LymailleError::ScriptOperationLimit => marqueur("005", "plafond d'opérations atteint"),
        LymailleError::ScriptResultTooLarge => marqueur("006", "résultat plus gros que le plafond"),
        LymailleError::ScriptFailed(texte) => marqueur("003", borner(&texte)),
        LymailleError::ScriptCancelled => marqueur("009", "exécution annulée"),
        LymailleError::Forbidden(_) | LymailleError::UnknownSubject => {
            marqueur("008", "droits refusés pour ce widget")
        }
        autre => {
            tracing::error!("VNL-APP-WDG-010: panne de l'exécuteur de scripts: {autre}");
            panne_interne()
        }
    }
}

/// Marqueur `VNL-APP-WDG-<numero>` avec son message.
fn marqueur(numero: &str, message: &str) -> RunError {
    RunError {
        code: format!("VNL-APP-WDG-{numero}"),
        message: message.to_string(),
    }
}

/// Panne interne (`010`) : message fixe, la cause reste dans le journal.
pub(crate) fn panne_interne() -> RunError {
    marqueur("010", "panne interne de l'exécution du widget")
}

/// Borne un message de script à `LONGUEUR_MESSAGE_SCRIPT` caractères.
fn borner(texte: &str) -> &str {
    match texte.char_indices().nth(LONGUEUR_MESSAGE_SCRIPT) {
        Some((indice, _)) => &texte[..indice],
        None => texte,
    }
}

impl From<ComputeAborted> for RunError {
    /// Calcul interrompu sans résultat (panique de l'exécution partagée, `cache.sdd`) : `010`.
    fn from(_: ComputeAborted) -> Self {
        tracing::error!("VNL-APP-WDG-010: calcul du widget interrompu sans résultat");
        panne_interne()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::real_db;
    use crate::sessions::catalog::{G_HOME, ensure_catalog};
    use miryad_core::users::resolve_user;
    use serde_json::json;

    /// Configuration de test : plafonds des widgets à leurs défauts, base jamais jointe ici.
    fn config_de_test() -> Config {
        Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
            oidc_connect_timeout: std::time::Duration::from_secs(5),
            oidc_timeout: std::time::Duration::from_secs(15),
            cookie_secret: "0".repeat(64),
            database_url: "sqlite::memory:".to_string(),
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
            k8s_namespace: None,
            application_name: None,
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
            restate_admin_url: None,
            restate_ingress_url: None,
            workflow_listen: "0.0.0.0:9080".to_string(),
            workflow_deployment_url: None,
            subagent_max_secs: 2700,
            widget_max_duration: std::time::Duration::from_secs(300),
            widget_max_operations: 0,
            widget_max_result_bytes: 1_048_576,
        }
    }

    /// Scenario: les erreurs typées de la crate deviennent des marqueurs.
    #[test]
    fn les_erreurs_typees_deviennent_des_marqueurs() {
        let cas = [
            (LymailleError::ScriptTimeout, "VNL-APP-WDG-004"),
            (LymailleError::ScriptOperationLimit, "VNL-APP-WDG-005"),
            (LymailleError::ScriptResultTooLarge, "VNL-APP-WDG-006"),
            (
                LymailleError::Forbidden("script grants unavailable".to_string()),
                "VNL-APP-WDG-008",
            ),
            (LymailleError::UnknownSubject, "VNL-APP-WDG-008"),
            (LymailleError::ScriptCancelled, "VNL-APP-WDG-009"),
        ];
        for (erreur, attendu) in cas {
            assert_eq!(traduire(erreur).code, attendu);
        }

        let echec = traduire(LymailleError::ScriptFailed("x".to_string()));
        assert_eq!(echec.code, "VNL-APP-WDG-003");
        assert_eq!(echec.message, "x");
    }

    /// Scenario: une panne interne ne fuit pas sa cause.
    #[test]
    fn une_panne_interne_ne_fuit_pas_sa_cause() {
        let url = "postgres://utilisateur:secret@base.interne.example/vanyline";
        let rendu = traduire(LymailleError::Store(format!("connexion à {url} refusée")));

        assert_eq!(rendu.code, "VNL-APP-WDG-010");
        assert!(
            !rendu.message.contains("postgres") && !rendu.message.contains(url),
            "le message rendu ne porte aucune trace de la cause : {}",
            rendu.message
        );
    }

    /// Scenario: un script s'exécute sous le sujet donné, dans le contexte donné. Le sujet
    /// inconnu est refusé par la crate (`UnknownSubject` → `008`), le sujet connu obtient son
    /// résultat : l'identité du spectateur est celle du sujet passé (`AC-2`).
    #[tokio::test]
    async fn un_script_s_execute_sous_le_sujet_donne() {
        let db = real_db().await;
        ensure_catalog(&db).await.expect("catalogue publié");
        resolve_user(&db, "ana", None)
            .await
            .expect("compte de test");
        let service = WidgetService::new(db.clone(), None, &config_de_test());
        let cancel = CancelToken::new();
        let source = "40 + 2";

        let ana = service.execute(G_HOME, "ana", source, &cancel).await;
        assert_eq!(ana.expect("ana a un compte et des droits"), json!(42));

        let ben = service.execute(G_HOME, "ben", source, &cancel).await;
        assert_eq!(
            ben.expect_err("ben n'a pas de compte local").code,
            "VNL-APP-WDG-008"
        );
    }
}
