//! Runtime — endpoint Restate embarqué — contrat : `runtime.sdd`.
//!
//! Séquence de `start_with` (contractuelle, dans cet ordre) : valider la config (`aucun port
//! n'est ouvert avant un verdict valide`), poser la `WorkflowPolicy` `OwnerOnly` AVANT le
//! moindre trafic, lier `DagInterpreter` + `StepDispatcher` (registre des kinds) à l'`Endpoint`,
//! démarrer le serveur HTTP/2 en tâche de fond sur le port interne, puis enregistrer le
//! déploiement — un refus est loggé et réessayé en arrière-plan (backoff plafonné à 30 s),
//! jamais bloquant pour le démarrage de l'app. Les `config`/`inputs` d'un step ne sont jamais
//! journalisés ici : ce fichier ne les lit pas.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use miryad_core::resource::AccessPolicy;
use miryad_core::resource::MiryadResource;
use miryad_core::workflow::DagInterpreter;
use miryad_core::workflow::DagSteps;
use miryad_core::workflow::RunHandle;
use miryad_core::workflow::StepDispatcher;
use miryad_core::workflow::StepRegistry;
use miryad_core::workflow::WorkflowConfig;
use miryad_core::workflow::WorkflowDefinition;
use miryad_core::workflow::WorkflowError;
use miryad_core::workflow::WorkflowPolicy;
use miryad_core::workflow::configure_policy;
use miryad_core::workflow::recommended_options;
use miryad_core::workflow::register_deployment;
use miryad_core::workflow::trigger_run;
use restate_sdk::endpoint::Endpoint;
use restate_sdk::http_server::HttpServer;
use restate_sdk::service::IntoServiceDefinition as _;
use tokio::net::TcpListener;
use vanyline_lib::k8s::VnlK8sClient;

use crate::AppState;
use crate::config::Config;
use crate::error::AppError;
use crate::workflow::status::RestateAdmin;
use crate::workflow::steps::StepEnv;
use crate::workflow::steps::build_registry;

/// Timeouts d'exécution `inactivity_timeout`/`abort_timeout` relevés à 25 h sur le service
/// `StepDispatcher` (`runtime.sdd` `Handles`) : couvrent un step shell de 24 h
/// (`steps/shell.sdd`). La politique de retry bornée de miryad-core
/// (`recommended_options`, pause au 5e essai) reste à son défaut — jamais de retry permanent.
const ETAPE_LONGUE: Duration = Duration::from_hours(25);

/// Premier délai de réessai d'enregistrement en production (doublement, plafond
/// `REGISTER_RETRY_CAP`).
const REGISTER_RETRY_INIT: Duration = Duration::from_secs(1);

/// Plafond du backoff d'enregistrement (`runtime.sdd` : « backoff plafonné à 30 s »).
const REGISTER_RETRY_CAP: Duration = Duration::from_secs(30);

/// Configuration du moteur de workflow, lue depuis le `Config` de l'app (jamais de l'environnement
/// ici — `config.sdd` a la lecture unique) — contrat : `runtime.sdd`, `WorkflowConfigApp`.
#[derive(Clone, Debug)]
pub struct WorkflowConfigApp {
    /// Admin API Restate (`VNL_RESTATE_ADMIN_URL`) : enregistrement, statut des runs.
    pub admin_url: String,
    /// Ingress Restate (`VNL_RESTATE_INGRESS_URL`) : déclenchement des runs.
    pub ingress_url: String,
    /// URL sous laquelle Restate joint cet endpoint (`VNL_WORKFLOW_DEPLOYMENT_URL` ; à défaut,
    /// dérivée du nom de l'Application).
    pub deployment_url: String,
    /// Adresse d'écoute de l'endpoint (`VNL_WORKFLOW_LISTEN`, défaut `0.0.0.0:9080`).
    pub listen: String,
}

/// Présence contractuelle d'une optionnalité : absente OU vide ⇒ non posée. (Contrairement à
/// `config.rs`, qui conserve `Some("")`, le consommateur moteur tranche ici : « absentes ⇒
/// moteur non configuré » de `runtime.sdd`, et une URL vide ne peut pas être jointe.)
fn non_vide(valeur: Option<&String>) -> Option<&str> {
    valeur.map(String::as_str).filter(|v| !v.is_empty())
}

impl WorkflowConfigApp {
    /// Lit le moteur dans le `Config` — FAIL-FAST (décidé 2026-10-14, remplace warn +
    /// moteur éteint de W3) : `Ok(None)` SILENCIEUX seulement si AUCUNE des variables
    /// moteur (`VNL_RESTATE_ADMIN_URL`, `VNL_RESTATE_INGRESS_URL`,
    /// `VNL_WORKFLOW_DEPLOYMENT_URL`) n'est posée (déploiement sans workflows, voulu éteint).
    /// Config partielle (variable posée mais URL absente/vide) ou deployment URL non
    /// dérivable (`VNL_WORKFLOW_DEPLOYMENT_URL` absente/vide et nom d'Application ou
    /// namespace absents/vide, port de `VNL_WORKFLOW_LISTEN` invalide) ⇒ `Err` d'erreur de
    /// configuration miryad-core (`InvalidConfig`, marqueur `MRD-WORKFLOW-007` imbriqué,
    /// même famille que `VNL-WF-002`) — rendu `VNL-WF-001` + `exit(1)` par `main.rs`.
    pub fn from_config(config: &Config) -> Result<Option<Self>, WorkflowError> {
        let admin = config.restate_admin_url.as_ref();
        let ingress = config.restate_ingress_url.as_ref();
        let deployment = config.workflow_deployment_url.as_ref();
        if admin.is_none() && ingress.is_none() && deployment.is_none() {
            // Aucune variable posée : moteur éteint voulu, silencieux (`runtime.sdd`
            // Scenario `aucune_variable_silence_voulu`).
            return Ok(None);
        }
        let declared = || {
            format!(
                " VNL_RESTATE_ADMIN_URL posée = {}, VNL_RESTATE_INGRESS_URL posée = {}, VNL_WORKFLOW_DEPLOYMENT_URL posée = {}",
                admin.is_some(),
                ingress.is_some(),
                deployment.is_some(),
            )
        };
        let invalid = |cause: String| WorkflowError::InvalidConfig(cause);
        let admin_url = non_vide(admin).ok_or_else(|| {
            invalid(format!(
                "VNL_RESTATE_ADMIN_URL absente ou vide alors que le moteur de workflow est déclaré;{}",
                declared()
            ))
        })?;
        let ingress_url = non_vide(ingress).ok_or_else(|| {
            invalid(format!(
                "VNL_RESTATE_INGRESS_URL absente ou vide alors que le moteur de workflow est déclaré;{}",
                declared()
            ))
        })?;
        let deployment_url = match non_vide(deployment) {
            Some(url) => url.to_string(),
            None => Self::derive_deployment_url(config).ok_or_else(|| {
                invalid(
                    "VNL_WORKFLOW_DEPLOYMENT_URL absente ou vide et défaut non dérivable \
                     (nom d'Application ou namespace absents/vide, ou port de \
                     VNL_WORKFLOW_LISTEN non parseable)"
                        .to_string(),
                )
            })?,
        };
        Ok(Some(Self {
            admin_url: admin_url.to_string(),
            ingress_url: ingress_url.to_string(),
            deployment_url,
            listen: config.workflow_listen.clone(),
        }))
    }

    /// Défaut de la deployment URL, forme du déploiement existant
    /// (`deploy/web/configmap.yaml`) : `http://{application_name}.{namespace}.svc:{port}`, le
    /// port étant celui de `VNL_WORKFLOW_LISTEN` — le Service de l'app expose le port
    /// d'endpoint au Restate de l'Application. Non dérivable sans l'un de ces trois éléments.
    fn derive_deployment_url(config: &Config) -> Option<String> {
        let app = non_vide(config.application_name.as_ref())?;
        let namespace = non_vide(config.k8s_namespace.as_ref())?;
        let port = config
            .workflow_listen
            .rsplit_once(':')
            .map(|(_, port)| port)
            .filter(|port| port.parse::<u16>().is_ok())?;
        Some(format!("http://{app}.{namespace}.svc:{port}"))
    }

    /// La config miryad-core correspondante (deux clones sans validation — `validate` est
    /// appelée par les fonctions de `miryad_core::workflow::client`).
    #[must_use]
    pub(crate) fn miryad_config(&self) -> WorkflowConfig {
        WorkflowConfig {
            admin_url: self.admin_url.clone(),
            ingress_url: self.ingress_url.clone(),
            deployment_url: self.deployment_url.clone(),
        }
    }
}

/// Handle `Clone` du moteur dans `AppState` (`workflow: Option<WorkflowRuntime>`) — contrat :
/// `runtime.sdd`, `WorkflowRuntime`.
#[derive(Clone)]
pub struct WorkflowRuntime {
    app: WorkflowConfigApp,
    http: reqwest::Client,
}

impl WorkflowRuntime {
    /// Déclenche un run du DAG donné — délègue à `miryad_core::workflow::client::trigger_run`
    /// (POST ingress, jamais bloquant sur la complétion ; les `inputs` du DAG sont déjà
    /// matérialisés par `run_context`).
    ///
    /// # Errors
    ///
    /// Les erreurs de `trigger_run` (`miryad_core::workflow::WorkflowError`) : ingress
    /// injoignable (`MRD-WORKFLOW-001`), refus hors `2xx` (`MRD-WORKFLOW-002`), réponse
    /// indécodable (`MRD-WORKFLOW-003`), config invalide (`MRD-WORKFLOW-007`).
    #[allow(dead_code)] // W4 consommateurs (`api/workflows.sdd`, `mcp_domain.sdd`) — tooling.sdd
    pub async fn trigger(&self, dag: &DagSteps) -> Result<RunHandle, WorkflowError> {
        trigger_run(&self.app.miryad_config(), &self.http, dag).await
    }

    /// URL d'ingress Restate (déclenchement des runs) — `runtime.sdd` `Exposes`.
    #[allow(dead_code)] // W4 consommateurs (`api/workflows.sdd`) — tooling.sdd
    #[must_use]
    pub fn ingress_url(&self) -> &str {
        &self.app.ingress_url
    }
}

/// L'interface de `status.rs` (lecture de statut, annulation) : URL d'admin et client HTTP
/// partagé.
impl RestateAdmin for WorkflowRuntime {
    fn admin_url(&self) -> &str {
        &self.app.admin_url
    }
    fn http(&self) -> &reqwest::Client {
        &self.http
    }
}

/// Démarre le moteur depuis l'état de l'app : construit le `StepEnv` (`db`, `http`, `config`
/// depuis l'`AppState` ; client K8s propre au moteur — la découverte lazy de l'`AppState` garde
/// son contrat « première requête, jamais au démarrage », `k8s.sdd`) et le registre des kinds,
/// puis délègue la séquence contractuelle à [`start_with`].
///
/// # Errors
///
/// Config invalide (`MRD-WORKFLOW-007` remonté), découverte K8s impossible, adresse
/// d'endpoint non parseable ou bind refusé — jamais un échec d'enregistrement (réessayé en
/// arrière-plan).
pub async fn start_runtime(
    state: &AppState,
    config: &WorkflowConfigApp,
) -> Result<WorkflowRuntime, AppError> {
    let k8s = VnlK8sClient::discover(state.config.k8s_namespace.clone())
        .await
        .map_err(|e| {
            AppError::InternalError(format!(
                "VNL-WF-006: découverte K8s du moteur de workflow: {e}"
            ))
        })?;
    let env = StepEnv {
        db: state.auth.db.clone(),
        k8s,
        http: reqwest::Client::new(),
        config: Arc::new(state.config.clone()),
    };
    start_with(
        config.clone(),
        build_registry(env),
        REGISTER_RETRY_INIT,
        REGISTER_RETRY_CAP,
    )
    .await
}

/// Cœur de la séquence contractuelle de `runtime.sdd` — séam `pub(crate)` (jurisprudence
/// `assemble_registry`/`open_with`) : hors cluster, `VnlK8sClient` n'est pas constructible
/// (convention anti-mock `lib/src/k8s.sdd`) et aucun test n'attend 30 s de backoff réels ; le
/// registre et le backoff viennent donc en paramètres. Les Scenario de `runtime.sdd` s'écrivent
/// contre cette fonction ; `start_runtime` n'ajoute que la construction du registre.
pub(crate) async fn start_with(
    cfg: WorkflowConfigApp,
    registry: StepRegistry,
    retry_initial: Duration,
    retry_cap: Duration,
) -> Result<WorkflowRuntime, AppError> {
    // 1 — valider AVANT tout (Must : une config invalide n'ouvre aucun port).
    let miryad = cfg.miryad_config();
    miryad.validate().map_err(|e| {
        AppError::InternalError(format!(
            "VNL-WF-002: configuration du moteur de workflow invalide: {e}"
        ))
    })?;
    // 2 — la politique `OwnerOnly` est posée ici, avant la moindre écoute (Must).
    poser_politique_proprietaire()?;
    // 3 — lier les deux services de miryad-core à l'endpoint. `StepDispatcher` reçoit le
    // registre des kinds et les options relevées à 25 h ; `DagInterpreter` reste aux options
    // par défaut (`runtime.sdd` `Handles` ne nomme que le dispatcher).
    let options = recommended_options()
        .inactivity_timeout(ETAPE_LONGUE)
        .abort_timeout(ETAPE_LONGUE);
    let endpoint = Endpoint::builder()
        .bind(DagInterpreter)
        .bind(
            StepDispatcher::new(registry)
                .into_service_definition()
                .options(options),
        )
        .build();
    // 4 — port interne seulement (jamais celui de l'API) : l'adresse vient de
    // `VNL_WORKFLOW_LISTEN`, le serveur tourne en tâche de fond jusqu'à l'arrêt du processus.
    let addr: SocketAddr = cfg.listen.parse().map_err(|e| {
        AppError::InternalError(format!(
            "VNL-WF-003: VNL_WORKFLOW_LISTEN « {} » non parseable: {e}",
            cfg.listen
        ))
    })?;
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        AppError::InternalError(format!(
            "VNL-WF-004: bind de l'endpoint Restate sur {addr}: {e}"
        ))
    })?;
    tracing::info!("endpoint Restate du moteur de workflow à l'écoute sur {addr} (port interne)");
    tokio::spawn(
        HttpServer::new(endpoint).serve_with_cancel(listener, std::future::pending::<()>()),
    );
    let runtime = WorkflowRuntime {
        app: cfg,
        http: reqwest::Client::new(),
    };
    // 5 — enregistrement du déploiement (`force: true`, via `register_deployment`) ; un refus
    // est loggé et réessayé en arrière-plan, le démarrage de l'app aboutit quand même (Must).
    if let Err(e) = register_deployment(&miryad, &runtime.http).await {
        tracing::error!(
            "enregistrement du déploiement workflow refusé: {e} — réessai en arrière-plan (backoff, plafond {} s)",
            retry_cap.as_secs()
        );
        tokio::spawn(retry_enregistrement(
            runtime.app.clone(),
            runtime.http.clone(),
            retry_initial,
            retry_cap,
        ));
    } else {
        tracing::info!("déploiement workflow enregistré auprès de Restate (force: true)");
    }
    Ok(runtime)
}

/// Pose la `WorkflowPolicy` `OwnerOnly` en lecture et en écriture — chaque utilisateur ne voit que
/// ses définitions (le contrôle fin portée projet/sandbox vit dans les entités `db`).
///
/// La cellule de configuration de miryad-core est une `OnceLock` process-globale : dans ce
/// binaire, `runtime.rs` est le seul poseur, donc une collision signifie re-démarrage du
/// moteur ou Scenario parallèle — la première politique posée est la nôtre, le re-call est un
/// no-op vérifié. Une politique effectivement différente est une erreur de démarrage.
fn poser_politique_proprietaire() -> Result<(), AppError> {
    let politique = WorkflowPolicy {
        read: AccessPolicy::OwnerOnly,
        write: AccessPolicy::OwnerOnly,
    };
    match configure_policy(politique) {
        Ok(()) => Ok(()),
        Err(WorkflowError::PolicyAlreadySet) => {
            if <WorkflowDefinition as MiryadResource>::read_policy() == AccessPolicy::OwnerOnly
                && <WorkflowDefinition as MiryadResource>::write_policy() == AccessPolicy::OwnerOnly
            {
                tracing::debug!("politique workflow déjà OwnerOnly — re-call sans effet");
                Ok(())
            } else {
                Err(AppError::InternalError(
                    "VNL-WF-005: une politique d'accès workflow autre que OwnerOnly est déjà posée"
                        .to_string(),
                ))
            }
        }
        Err(e) => Err(AppError::InternalError(format!(
            "VNL-WF-005: pose de la politique workflow impossible: {e}"
        ))),
    }
}

/// Réessai d'enregistrement en arrière-plan : délais croissants (doublement) plafonnés à
/// `retry_cap` (`runtime.sdd` : 30 s en production), jusqu'au succès. Le message journalise
/// l'erreur miryad-core (code `MRD-WORKFLOW-*` inclus) — jamais la config des steps.
async fn retry_enregistrement(
    cfg: WorkflowConfigApp,
    http: reqwest::Client,
    initial: Duration,
    cap: Duration,
) {
    let miryad = cfg.miryad_config();
    let mut delai = initial;
    let mut tentatives: u64 = 0;
    loop {
        tokio::time::sleep(delai).await;
        tentatives = tentatives.saturating_add(1);
        match register_deployment(&miryad, &http).await {
            Ok(()) => {
                tracing::info!(
                    tentatives,
                    "déploiement workflow enregistré après {tentatives} réessai(s) en arrière-plan"
                );
                return;
            }
            Err(e) => {
                tracing::warn!(tentatives, "réessai d'enregistrement refusé: {e}");
                delai = delai.saturating_mul(2).min(cap);
            }
        }
    }
}
/// Fenêtre de rétention des runs terminés/échoués (`main.sdd`, purge quotidienne).
pub(crate) const RETENTION_RUNS_JOURS: i64 = 30;

/// Période de la purge des runs : une fois par jour.
pub(crate) const PERIODE_PURGE_RUNS: Duration = Duration::from_hours(24);

/// Démarre la purge quotidienne des runs (`workflow_runs::purge_older_than`) : une seule tâche
/// de fond, appelée par `main` uniquement quand le moteur est configuré. Le premier passage
/// a lieu IMMÉDIATEMENT au démarrage, puis un par période (décision 2026-10-18).
pub(crate) fn demarrer_purge_quotidienne(
    db: sea_orm::DatabaseConnection,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(boucle_purge(
        || async {
            tokio::time::sleep(PERIODE_PURGE_RUNS).await;
            true
        },
        move || {
            let db = db.clone();
            async move {
                crate::db::entities::workflow_runs::purge_older_than(&db, RETENTION_RUNS_JOURS)
                    .await
            }
        },
    ))
}

/// Boucle de purge (`main.sdd`, Scenario `purge_au_demarrage_puis_quotidienne`) : une purge
/// IMMÉDIATE, puis `attendre` rend `true` pour continuer, `false` pour arrêter ; une purge en
/// erreur est journalisée (`tracing::warn!`, `VNL-WF-007`) sans arrêter la tâche, un succès
/// l'est en `tracing::info!` (`VNL-WF-008`). Le pas de temps est injecté
/// (`attendre`) pour rester testable sans `sleep` réel.
pub(crate) async fn boucle_purge<Attente, AttenteFut, Purge, PurgeFut>(
    mut attendre: Attente,
    mut purger: Purge,
) where
    Attente: FnMut() -> AttenteFut,
    AttenteFut: std::future::Future<Output = bool>,
    Purge: FnMut() -> PurgeFut,
    PurgeFut: std::future::Future<Output = Result<u64, sea_orm::DbErr>>,
{
    loop {
        match purger().await {
            Ok(supprimes) => {
                tracing::info!(
                    "VNL-WF-008: purge quotidienne des runs terminée, {supprimes} run(s) supprimé(s)"
                );
            }
            Err(e) => {
                tracing::warn!("VNL-WF-007: purge quotidienne des runs échouée: {e}");
            }
        }
        if !attendre().await {
            break;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use miryad_core::resource::AccessPolicy;
    use miryad_core::resource::MiryadResource;
    use miryad_core::workflow::DagSteps;
    use miryad_core::workflow::MiryadWorkflowStep;
    use miryad_core::workflow::StepDefinition;
    use miryad_core::workflow::StepError;
    use miryad_core::workflow::WorkflowDefinition;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    // ── Faux Restate (admin + ingress) — motif des faux serveurs du crate (port 0 + axum
    // en tâche, cf. `status.rs`) : `POST /deployments` capture le corps et refuse les
    // `refuse_first` premières tentatives (503) ; `POST /DagInterpreter/{clé}/run/send`
    // rend le `202` observé de l'ingress.
    #[derive(Clone)]
    struct FakeRestate {
        refusals: Arc<AtomicUsize>,
        regs: Arc<Mutex<Vec<String>>>,
        accepted: Arc<AtomicBool>,
        sends: Arc<Mutex<Vec<String>>>,
    }

    async fn spawn_fake_restate(refuse_first: usize) -> (String, FakeRestate) {
        let fake = FakeRestate {
            refusals: Arc::new(AtomicUsize::new(refuse_first)),
            regs: Arc::new(Mutex::new(Vec::new())),
            accepted: Arc::new(AtomicBool::new(false)),
            sends: Arc::new(Mutex::new(Vec::new())),
        };
        let (regs, refusals, accepted, sends) = (
            fake.regs.clone(),
            fake.refusals.clone(),
            fake.accepted.clone(),
            fake.sends.clone(),
        );
        let app = axum::Router::new()
            .route(
                "/deployments",
                axum::routing::post(move |body: String| {
                    let (regs, refusals, accepted) =
                        (regs.clone(), refusals.clone(), accepted.clone());
                    async move {
                        regs.lock().unwrap().push(body);
                        let left = refusals
                            .try_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                                Some(v.saturating_sub(1))
                            })
                            .unwrap();
                        if left > 0 {
                            return (
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                r#"{"message":"restate not ready"}"#.to_string(),
                            );
                        }
                        accepted.store(true, Ordering::SeqCst);
                        (
                            axum::http::StatusCode::OK,
                            r#"{"deploymentId":"dp_fake"}"#.to_string(),
                        )
                    }
                }),
            )
            .route(
                "/DagInterpreter/{key}/run/send",
                axum::routing::post(move |uri: axum::extract::OriginalUri| {
                    let sends = sends.clone();
                    async move {
                        sends.lock().unwrap().push(uri.path().to_string());
                        (
                            axum::http::StatusCode::ACCEPTED,
                            r#"{"invocationId":"inv_fake_1","status":"Accepted"}"#.to_string(),
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), fake)
    }

    /// Port libre : lié puis relâché (motif `status.rs`/miryad-core) — ni écouteurs avant
    /// le démarrage, ni faux positif ensuite.
    fn port_libre() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    /// Connexion TCP réelle : `true` si quelqu'un écoute sur le port.
    fn port_ouvert(port: u16) -> bool {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(500),
        )
        .is_ok()
    }

    /// Config de démarrage pointant admin et ingress sur le faux Restate, endpoint sur
    /// `127.0.0.1:{port}`.
    fn cfg_demarrage(base: &str, port: u16) -> WorkflowConfigApp {
        WorkflowConfigApp {
            admin_url: base.to_string(),
            ingress_url: base.to_string(),
            deployment_url: "http://vanyline.media-station.svc:9410".to_string(),
            listen: format!("127.0.0.1:{port}"),
        }
    }

    /// Backoff de test : millisecondes, aucun test n'attend les 30 s de production.
    const BACKOFF_TEST: (Duration, Duration) =
        (Duration::from_millis(20), Duration::from_millis(50));

    /// Sonde de registre : un kind factice — `build_registry` exige un `StepEnv` avec un
    /// `VnlK8sClient` non constructible hors cluster (convention anti-mock `lib/src/k8s.sdd`,
    /// même jurisprudence que `assemble_registry` de `steps/mod.rs`).
    struct Sonde(&'static str);
    #[async_trait]
    impl MiryadWorkflowStep for Sonde {
        fn kind(&self) -> &'static str {
            self.0
        }
        async fn run(
            &self,
            _run: &miryad_core::workflow::RunInfo,
            _config: Value,
            _inputs: HashMap<String, Value>,
        ) -> Result<Value, StepError> {
            Ok(Value::Null)
        }
    }

    fn registre_sonde() -> StepRegistry {
        let mut reg = StepRegistry::new();
        reg.register(Sonde("sonde")).unwrap();
        reg
    }

    /// `Config` complet pour les Scenario `from_config` (mêmes conventions que
    /// `full_config` de `config.rs`), moteur éteint par défaut.
    fn config_base() -> Config {
        Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/auth/callback".to_string(),
            oidc_scopes: vec!["openid".to_string()],
            oidc_ca_cert: None,
            oidc_connect_timeout: Duration::from_secs(5),
            oidc_timeout: Duration::from_secs(15),
            cookie_secret: "0".repeat(64),
            database_url: "postgres://localhost/test".to_string(),
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
            k8s_namespace: Some("media-station".to_string()),
            application_name: Some("vanyline".to_string()),
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
            restate_admin_url: None,
            restate_ingress_url: None,
            workflow_listen: "0.0.0.0:9080".to_string(),
            workflow_deployment_url: None,
        }
    }

    fn config_moteur() -> Config {
        let mut c = config_base();
        c.restate_admin_url = Some("http://restate-admin:9070".to_string());
        c.restate_ingress_url = Some("http://restate-ingress:8080".to_string());
        c
    }

    // ── Scenario: demarrage_complet ───────────────────────────────────────────
    // Config valide + faux Restate : l'endpoint écoute sur `VNL_WORKFLOW_LISTEN`
    // (connexion TCP réelle), la politique est posée, le déploiement est enregistré avec
    // `force: true` (première tentative attendue au retour de `start_with`).
    #[tokio::test]
    async fn demarrage_complet() {
        let (base, fake) = spawn_fake_restate(0).await;
        let port = port_libre();
        let runtime = start_with(
            cfg_demarrage(&base, port),
            registre_sonde(),
            BACKOFF_TEST.0,
            BACKOFF_TEST.1,
        )
        .await
        .expect("une config valide doit démarrer le moteur");

        assert!(
            port_ouvert(port),
            "l'endpoint doit écouter sur l'adresse demandée 127.0.0.1:{port}"
        );
        let regs = fake.regs.lock().unwrap();
        assert_eq!(
            regs.len(),
            1,
            "la première tentative d'enregistrement est attendue au démarrage"
        );
        assert!(
            regs[0].contains(r#""force":true"#)
                && regs[0].contains("http://vanyline.media-station.svc:9410"),
            "le POST /deployments porte l'uri et force:true : {}",
            regs[0]
        );
        drop(regs);
        assert_eq!(
            <WorkflowDefinition as MiryadResource>::read_policy(),
            AccessPolicy::OwnerOnly,
            "la politique de lecture est posée avant le moindre trafic"
        );
        assert_eq!(
            <WorkflowDefinition as MiryadResource>::write_policy(),
            AccessPolicy::OwnerOnly,
            "la politique d'écriture est posée avant le moindre trafic"
        );
        assert_eq!(
            runtime.admin_url(),
            base,
            "l'URL d'admin est exposée (status.sdd)"
        );
        assert_eq!(runtime.ingress_url(), base, "l'URL d'ingress est exposée");
    }

    // ── Scenario: config_invalide ─────────────────────────────────────────────
    // `VNL_RESTATE_ADMIN_URL` non absolue : erreur de configuration, AUCUN port ouvert
    // (prouvé par l'absence d'écoute sur le port d'endpoint demandé).
    #[tokio::test]
    async fn config_invalide() {
        let port = port_libre();
        let cfg = WorkflowConfigApp {
            admin_url: "pas-une-url".to_string(),
            ingress_url: "http://restate-ingress:8080".to_string(),
            deployment_url: "http://vanyline.media-station.svc:9410".to_string(),
            listen: format!("127.0.0.1:{port}"),
        };
        let res = start_with(cfg, registre_sonde(), BACKOFF_TEST.0, BACKOFF_TEST.1).await;
        let Err(err) = res else {
            panic!("une config invalide ne doit jamais démarrer le moteur");
        };
        let rendu = err.to_string();
        assert!(
            rendu.contains("MRD-WORKFLOW-007") && rendu.contains("admin_url"),
            "l'erreur nomme le champ fautif et son code : {rendu}"
        );
        assert!(
            !port_ouvert(port),
            "aucun port ouvert sur l'adresse demandée {port}"
        );
    }

    // ── Scenario: enregistrement_reessaye ─────────────────────────────────────
    // Un Restate qui refuse l'enregistrement puis l'accepte : l'app est démarrée
    // (start rend Ok, l'endpoint écoute), l'enregistrement est réessayé en arrière-plan
    // jusqu'au succès (backoff injecté, pas de 30 s réels).
    #[tokio::test]
    async fn enregistrement_reessaye() {
        let (base, fake) = spawn_fake_restate(2).await;
        let port = port_libre();
        let _runtime = tokio::time::timeout(
            Duration::from_secs(5),
            start_with(
                cfg_demarrage(&base, port),
                registre_sonde(),
                BACKOFF_TEST.0,
                BACKOFF_TEST.1,
            ),
        )
        .await
        .expect("le démarrage n'attend pas le succès de l'enregistrement")
        .expect("un refus d'enregistrement ne fait pas échouer le démarrage");

        assert!(
            port_ouvert(port),
            "l'app est démarrée : l'endpoint écoute malgré le refus"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if fake.accepted.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("l'enregistrement est réessayé en arrière-plan jusqu'au succès");
        let regs = fake.regs.lock().unwrap();
        assert_eq!(
            regs.len(),
            3,
            "deux tentatives refusées puis la troisième acceptée"
        );
        assert!(
            regs.iter().all(|corps| corps.contains(r#""force":true"#)),
            "chaque tentative réenregistre avec force:true : {regs:?}"
        );
    }

    // ── Scenario: politique_proprietaire_seul ─────────────────────────────────
    // Tranche au niveau politique (le contrôle fin des deux utilisateurs exige la base
    /// `miryad_workflow_definitions` derrière `rest::core` — verrouillé par les tests
    // miryad-core `tests/workflow_policy.rs`, signalé au rapport) : après démarrage, la
    // politique effective de la ressource est `OwnerOnly` en lecture et en écriture et
    // l'owner_column est déclarée — c'est ce couple qui rend une définition invisible au
    // second utilisateur (liste et lecture).
    #[tokio::test]
    async fn politique_proprietaire_seul() {
        let (base, _fake) = spawn_fake_restate(0).await;
        let port = port_libre();
        start_with(
            cfg_demarrage(&base, port),
            registre_sonde(),
            BACKOFF_TEST.0,
            BACKOFF_TEST.1,
        )
        .await
        .expect("démarrage avec la politique posée");
        assert_eq!(
            <WorkflowDefinition as MiryadResource>::read_policy(),
            AccessPolicy::OwnerOnly
        );
        assert_eq!(
            <WorkflowDefinition as MiryadResource>::write_policy(),
            AccessPolicy::OwnerOnly
        );
        assert!(
            <WorkflowDefinition as MiryadResource>::owner_column().is_some(),
            "OwnerOnly sans owner_column est une déclaration invalide (miryad-core resource.rs)"
        );
    }

    // ── Scenario: moteur_non_configure (mod.sdd, volet W3) ───────────────────
    // REVU 2026-10-14 (FAIL-FAST) : l'extinction silencieuse `Ok(None)` ne couvre plus
    // que l'absence totale de variables ; les cas partiel/vide sont des `Err` portés par
    // `config_partielle_fail_fast`. `main.rs` n'appelle alors jamais `start_runtime`,
    // aucun port d'endpoint Restate n'est ouvert.
    #[test]
    fn moteur_non_configure() {
        assert!(
            matches!(WorkflowConfigApp::from_config(&config_base()), Ok(None)),
            "aucune variable moteur ⇒ Ok(None) silencieux"
        );
    }

    // ── Scenario: aucune_variable_silence_voulu ───────────────────────────────
    // Décidé 2026-10-14 : AUCUNE variable `VNL_RESTATE_*` posée ⇒ `Ok(None)` SILENCIEUX —
    // déploiement sans moteur de workflow, voulu tel quel (plus de warn).
    #[test]
    fn aucune_variable_silence_voulu() {
        assert!(
            matches!(WorkflowConfigApp::from_config(&config_base()), Ok(None)),
            "aucune variable posée ⇒ Ok(None)"
        );
    }

    // ── Scenario: config_partielle_fail_fast ─────────────────────────────────
    // Décidé 2026-10-14 (FAIL-FAST remplace warn+moteur éteint de W3) : une ou deux des
    // URL posées (ou posées vides) ⇒ `Err` (rendu `VNL-WF-001` + exit(1) par `main.rs`),
    // jamais un `Ok(None)` silencieux. L'erreur porte le code miryad-core `MRD-WORKFLOW-007`
    // (variante `InvalidConfig`, même famille que `VNL-WF-002`).
    #[test]
    fn config_partielle_fail_fast() {
        let mut admin_seule = config_base();
        admin_seule.restate_admin_url = Some("http://restate-admin:9070".to_string());
        let err = WorkflowConfigApp::from_config(&admin_seule)
            .expect_err("admin seule ⇒ fail-fast, jamais Ok(None)");
        assert!(
            err.to_string().contains("MRD-WORKFLOW-007"),
            "la cause MRD-WORKFLOW-007 est imbriquée : {err}"
        );

        let mut ingress_seule = config_base();
        ingress_seule.restate_ingress_url = Some("http://restate-ingress:8080".to_string());
        assert!(
            WorkflowConfigApp::from_config(&ingress_seule).is_err(),
            "ingress seule ⇒ fail-fast"
        );

        let mut vide = config_base();
        vide.restate_admin_url = Some(String::new());
        vide.restate_ingress_url = Some("http://restate-ingress:8080".to_string());
        assert!(
            WorkflowConfigApp::from_config(&vide).is_err(),
            "URL posée vide avec les autres posées ⇒ fail-fast (jamais de silence)"
        );
    }

    // ── Scenario: deployment_non_derable_fail_fast ───────────────────────────
    // URL posées mais nom d'Application (ou namespace) absent/vide ⇒ deployment URL non
    // dérivable : fail-fast aussi (2026-10-14, plus de warn + moteur éteint).
    #[test]
    fn deployment_non_derable_fail_fast() {
        let mut nom_vide = config_moteur();
        nom_vide.application_name = Some(String::new());
        let err = WorkflowConfigApp::from_config(&nom_vide)
            .expect_err("nom d'application vide ⇒ deployment non dérivable ⇒ Err");
        assert!(
            err.to_string().contains("MRD-WORKFLOW-007"),
            "la cause MRD-WORKFLOW-007 est imbriquée : {err}"
        );

        let mut sans_namespace = config_moteur();
        sans_namespace.k8s_namespace = None;
        assert!(
            WorkflowConfigApp::from_config(&sans_namespace).is_err(),
            "namespace absent ⇒ deployment non dérivable ⇒ Err"
        );
    }

    // Appoint `Exposes` : la deployment URL défaut est dérivée du nom de l'Application
    // (forme du déploiement existant, deploy/web/configmap.yaml :
    // `http://{app}.{ns}.svc:{port}`), l'explicite prime, l'in dérivable est un fail-fast
    // (revu 2026-10-14 : plus de None).
    #[test]
    fn deploiement_derivee_du_nom_application() {
        let cfg = WorkflowConfigApp::from_config(&config_moteur())
            .expect("config valide")
            .expect("moteur configuré");
        assert_eq!(
            cfg.deployment_url, "http://vanyline.media-station.svc:9080",
            "dérivée : http://{{application_name}}.{{namespace}}.svc:{{port de VNL_WORKFLOW_LISTEN}}"
        );
        assert_eq!(cfg.listen, "0.0.0.0:9080");
        assert_eq!(cfg.admin_url, "http://restate-admin:9070");
        assert_eq!(cfg.ingress_url, "http://restate-ingress:8080");

        let mut explicite = config_moteur();
        explicite.workflow_deployment_url = Some("http://app.interne:9410".to_string());
        assert_eq!(
            WorkflowConfigApp::from_config(&explicite)
                .expect("config valide")
                .expect("moteur configuré")
                .deployment_url,
            "http://app.interne:9410",
            "VNL_WORKFLOW_DEPLOYMENT_URL posée prime le défaut"
        );

        let mut sans_nom = config_moteur();
        sans_nom.application_name = None;
        assert!(
            WorkflowConfigApp::from_config(&sans_nom).is_err(),
            "sans nom d'Application, défaut impossible à dériver ⇒ fail-fast (2026-10-14)"
        );
    }

    // Appoint `Exposes` @WorkflowRuntime : `trigger(dag)` délègue à `trigger_run` —
    // POST /DagInterpreter/{run_key}/run/send sur l'ingress, `invocationId` lu dans le corps.
    #[tokio::test]
    async fn trigger_delegue_trigger_run() {
        let (base, fake) = spawn_fake_restate(0).await;
        let port = port_libre();
        let runtime = start_with(
            cfg_demarrage(&base, port),
            registre_sonde(),
            BACKOFF_TEST.0,
            BACKOFF_TEST.1,
        )
        .await
        .expect("moteur démarré");
        let dag = DagSteps(vec![StepDefinition {
            id: "a".to_string(),
            depends_on: vec![],
            kind: "sonde".to_string(),
            config: serde_json::json!({}),
        }]);

        let anse = runtime
            .trigger(&dag)
            .await
            .expect("le 202 rend un RunHandle");
        assert_eq!(
            anse.invocation_id, "inv_fake_1",
            "invocationId lu dans le corps"
        );
        let sends = fake.sends.lock().unwrap();
        assert_eq!(
            sends.as_slice(),
            [format!("/DagInterpreter/{}/run/send", anse.run_key)],
            "la cible est l'ingress, suffixe /send, clé de run dans l'URI"
        );
    }

    /// Tampon d'écriture partagé : capture la sortie du subscriber de test.
    #[derive(Clone, Default)]
    struct Tampon(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Tampon {
        fn write(&mut self, octets: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("tampon non empoisonné")
                .extend_from_slice(octets);
            Ok(octets.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Scenario: `purge_au_demarrage_puis_quotidienne` (`main.sdd`) — la première purge
    /// précède TOUTE attente (passage immédiat au démarrage), puis une purge par attente
    /// acquittée ; un échec est journalisé `VNL-WF-007` sans arrêter la tâche, un succès
    /// `VNL-WF-008` avec le nombre de lignes supprimées ; la tâche s'arrête quand l'attente
    /// rend `false`. Aucune attente réelle (pas de `sleep`).
    #[tokio::test]
    async fn purge_au_demarrage_puis_quotidienne() {
        let tampon = Tampon::default();
        let sortie = tampon.clone();
        let abonne = tracing_subscriber::fmt()
            .with_writer(move || sortie.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        let _garde = tracing::subscriber::set_default(abonne);

        let journal = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let attentes = Arc::new(AtomicUsize::new(0));
        let purges = Arc::new(AtomicUsize::new(0));
        let (j_attente, a) = (journal.clone(), attentes.clone());
        let (j_purge, p) = (journal.clone(), purges.clone());
        boucle_purge(
            move || {
                let (j, a) = (j_attente.clone(), a.clone());
                async move {
                    j.lock().expect("journal").push("attente");
                    a.fetch_add(1, Ordering::SeqCst) < 1
                }
            },
            move || {
                let (j, n) = (j_purge.clone(), p.fetch_add(1, Ordering::SeqCst));
                async move {
                    j.lock().expect("journal").push("purge");
                    if n == 0 {
                        Err(sea_orm::DbErr::Custom("base indisponible".to_string()))
                    } else {
                        Ok(3)
                    }
                }
            },
        )
        .await;

        assert_eq!(
            journal.lock().expect("journal").as_slice(),
            ["purge", "attente", "purge", "attente"],
            "purge immédiate au démarrage, puis une purge par attente acquittée"
        );
        assert_eq!(
            purges.load(Ordering::SeqCst),
            2,
            "l'erreur de la première purge n'arrête pas la tâche"
        );
        let logs = String::from_utf8(tampon.0.lock().expect("tampon").clone()).expect("logs UTF-8");
        assert!(
            logs.contains("VNL-WF-007"),
            "échec journalisé avec VNL-WF-007 : {logs}"
        );
        assert!(
            logs.contains("VNL-WF-008") && logs.contains("3 run(s) supprimé(s)"),
            "succès journalisé avec VNL-WF-008 et le nombre de lignes : {logs}"
        );
    }
}
