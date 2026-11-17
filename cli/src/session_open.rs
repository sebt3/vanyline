//! Ouverture et reprise d'une session lymaille, partagée par `chat` et `run`.
//! Contrat : `session_open.sdd`.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use lymaille::{
    AgentDefinition, AgentRef, CancelHandle, Clock, ContextError, ContextResult,
    DEFAULT_SUMMARY_PROMPT, DefaultStrategy, Digest, EventSink, GrantedModule, Grants,
    InferenceProvider, McpClientConfig, McpConnector, McpEnv, McpRetryPolicy, ModelSnapshot,
    ProfileStore as _, Provenance, Provider, ProviderStore as _, Redactor, ReplayMode,
    ResolvedProfile, RigConfig, RigProvider, RmcpConnector, Scratchpad, ScratchpadEngine,
    ScratchpadLimits, SegmentContext, SessionId, SessionState, Summarizer, SystemClock,
    ToolsetContext, TurnConfig, TurnRunner, builtin_rhai_pad_skill, mcp_module, resolve_start,
    restore_session, rhai_pad_definition,
};
use thiserror::Error;

use crate::host::HostError;
use crate::host::audit::{AuditLog, AuditSink};
use crate::host::catalog_store::FileCatalog;
use crate::host::context_derive::{ClusterSource, DerivedContext, derive_context};
use crate::host::layers::Layers;
use crate::host::owner_auth::OwnerAuth;
use crate::host::process_lock::SessionLock;
use crate::host::secrets::{SecretValue, Secrets};
use crate::host::session_index::FileSessionIndex;
use crate::host::session_store::FileSessionStore;

/// Marqueur d'une identifiant, un profil, un agent ou un contexte inconnu.
const UNKNOWN: &str = "VNL-CLI-SES-001";
/// Marqueur d'un refus de lymaille (agent, profil, moteur, restauration).
const LYMAILLE: &str = "VNL-CLI-SES-002";
/// Marqueur d'un module MCP de la sandbox indisponible.
const SANDBOX: &str = "VNL-CLI-SES-003";
/// Marqueur du refus de résumé au débordement.
const COMPACTION: &str = "VNL-CLI-SES-004";
/// Taille maximale du rendu d'un résultat, en caractères (budget du contexte).
pub const RENDU_MAX_CARACTERES: u64 = 12_000;

/// Résumé refusé au débordement du contexte : aucun résumé n'est produit (`session_open.sdd`, `RefusingSummarizer`).
pub struct RefusingSummarizer;

impl Summarizer for RefusingSummarizer {
    fn summarize(
        &self,
        _transcript: &str,
        _digest: Option<&Digest>,
    ) -> impl Future<Output = ContextResult<String>> + Send {
        std::future::ready(Err(ContextError::Summarizer(format!(
            "{COMPACTION}: le contexte déborde, aucun résumé n'est produit (la session reste sauvegardée)"
        ))))
    }
}

/// Où `open` lit la configuration et la session, et comment il atteint le cluster et le fournisseur.
pub trait OpenEnv: Send + Sync {
    /// Source de lectures du cluster (`context_derive`).
    type Cluster: ClusterSource + Send + Sync;
    /// Fournisseur d'inférence construit depuis la référence du provider.
    type Provider: InferenceProvider + Send + Sync + 'static;
    /// Connecteur MCP vers la sandbox.
    type Connector: McpConnector + Send + Sync + 'static;

    /// Répertoire personnel de l'utilisateur.
    fn home(&self) -> PathBuf;
    /// Répertoire de travail du processus.
    fn cwd(&self) -> PathBuf;
    /// Ouvre la source du cluster, dans le namespace demandé (ou celui du kubeconfig).
    fn connect<'a>(
        &'a self,
        namespace: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Cluster, String>> + Send + 'a>>;
    /// Construit le fournisseur d'inférence de `provider`, avec sa clé résolue si elle existe.
    fn provider(&self, provider: &Provider, key: Option<&str>) -> Result<Self::Provider, String>;
    /// Construit le connecteur MCP vers la sandbox `url`, authentifié comme l'Owner `owner`.
    fn sandbox_connector(
        &self,
        cluster: &Self::Cluster,
        owner: &str,
        url: &str,
    ) -> Result<Self::Connector, String>;
}

/// Environnement de production : kubeconfig, inférence compatible `OpenAI` (`RigProvider`), MCP authentifié.
pub struct KubeEnv;

impl OpenEnv for KubeEnv {
    type Cluster = vanyline_lib::k8s::VnlK8sClient;
    type Provider = RigProvider;
    type Connector = RmcpConnector<OwnerAuth>;

    fn home(&self) -> PathBuf {
        dirs::home_dir().unwrap_or_default()
    }

    fn cwd(&self) -> PathBuf {
        std::env::current_dir().unwrap_or_default()
    }

    fn connect<'a>(
        &'a self,
        namespace: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Cluster, String>> + Send + 'a>> {
        Box::pin(async move {
            vanyline_lib::k8s::VnlK8sClient::discover(namespace.map(str::to_owned))
                .await
                .map_err(|erreur| format!("VNL-K8S-001: {erreur}"))
        })
    }

    fn provider(&self, provider: &Provider, key: Option<&str>) -> Result<RigProvider, String> {
        let config = RigConfig::new(provider.address().to_owned(), key.map(str::to_owned));
        Ok(RigProvider::new(&config))
    }

    fn sandbox_connector(
        &self,
        cluster: &vanyline_lib::k8s::VnlK8sClient,
        owner: &str,
        url: &str,
    ) -> Result<RmcpConnector<OwnerAuth>, String> {
        let auth = Arc::new(OwnerAuth::new(cluster.clone(), owner.to_owned()));
        Ok(RmcpConnector::new(McpClientConfig::new(url), auth))
    }
}

/// Ce que l'appelant demande pour ouvrir une session.
#[derive(Debug, Clone)]
pub struct SessionRequest {
    /// Projet du contexte de travail.
    pub context: String,
    /// Owner K8s dont le namespace porte le projet et la sandbox.
    pub owner: String,
    /// Sandbox de travail, si `--sandbox`.
    pub sandbox: Option<String>,
    /// Session neuve, dernière du contexte, ou identifiée.
    pub resume: Resume,
    /// Profil de modèle (`-m`) ; absent, celui de l'agent.
    pub profile: Option<String>,
    /// Dépôt de configuration partagé, si explicite.
    pub repo: Option<PathBuf>,
    /// Namespace d'application (kubeconfig si absent).
    pub namespace: Option<String>,
}

/// Choix de la session à ouvrir (`-c`, `--resume`, sinon neuve).
#[derive(Debug, Clone)]
pub enum Resume {
    /// Session neuve.
    New,
    /// Dernière session du contexte (`-c`).
    Last,
    /// Session identifiée (`--resume`).
    Id(SessionId),
}

/// Session lymaille prête à conduire un tour.
pub struct OpenedSession<P, N>
where
    P: InferenceProvider + 'static,
    N: EventSink + 'static,
{
    /// Runner de tours de la session, avec son état.
    pub runner: TurnRunner<P, DefaultStrategy<RefusingSummarizer>, FileSessionStore>,
    /// Handle d'annulation du tour.
    pub cancel: CancelHandle,
    /// Puits d'audit ; ses échecs d'écriture se récupèrent par `take_failures`.
    pub audit: Arc<AuditSink<N>>,
    /// Verrou de la session, gardé tant que la session est ouverte.
    pub lock: SessionLock,
    /// Contexte dérivé du cluster pour cette session.
    pub derived: DerivedContext,
    /// Messages à afficher à l'ouverture (reprise sans session, restauration).
    pub notices: Vec<String>,
}

/// Erreur d'ouverture de session.
#[derive(Debug, Error)]
pub enum OpenError {
    /// Erreur de l'hôte (verrou, lecture, validation, cluster).
    #[error(transparent)]
    Host(#[from] HostError),
    /// Cluster injoignable, avec sa cause (`VNL-K8S-001`).
    #[error("{cause}")]
    Cluster {
        /// Cause avec son marqueur.
        cause: String,
    },
    /// Identifiant, profil ou agent inconnu, nommé dans la cause (`VNL-CLI-SES-001`).
    #[error("{cause}")]
    Unknown {
        /// Cause avec son marqueur.
        cause: String,
    },
    /// Refus de lymaille (agent, moteur, restauration) (`VNL-CLI-SES-002`).
    #[error("{cause}")]
    Lymaille {
        /// Cause avec son marqueur.
        cause: String,
    },
    /// Module MCP de la sandbox indisponible (`VNL-CLI-SES-003`).
    #[error("{cause}")]
    Sandbox {
        /// Cause avec son marqueur.
        cause: String,
    },
}

/// Ouvre ou reprend une session selon `demande`, avec l'environnement `env`, en écrivant les événements dans `sink`.
///
/// # Errors
///
/// Returns [`OpenError`] for a cluster, host, lymaille or sandbox refusal, and for an unknown identifier or profile.
pub async fn open<E: OpenEnv, N: EventSink + 'static>(
    env: &E,
    demande: &SessionRequest,
    sink: N,
) -> Result<OpenedSession<E::Provider, N>, OpenError> {
    let layers = Layers::discover(
        &env.home(),
        &env.cwd(),
        demande.repo.as_deref(),
        Path::exists,
    );
    let cluster = env
        .connect(demande.namespace.as_deref())
        .await
        .map_err(|cause| OpenError::Cluster { cause })?;
    let derived = derive_context(
        &cluster,
        &demande.owner,
        &demande.context,
        demande.sandbox.as_deref(),
    )
    .await?;
    let contexte = contexte_nom(&derived.context)?;

    // Session : choisie puis verrouillée avant toute lecture de son état.
    let store = FileSessionStore::new(&layers);
    let (id, mut notices) = choisir_session(&layers, &store, demande, &contexte)?;
    let lock = SessionLock::acquire(&layers, &id)?;
    let existant = store.read(&id)?;
    match &existant {
        Some(etat) if etat.toolset_context() != contexte => {
            return Err(inconnu_session(&id, &contexte));
        }
        None if matches!(demande.resume, Resume::Id(_)) => {
            return Err(inconnu_session(&id, &contexte));
        }
        _ => {}
    }

    // Agent, profil, provider.
    let catalogue = FileCatalog::new(&layers);
    let agent = agent_du_contexte(&catalogue, &derived, &contexte).await?;
    let (resolu, fournisseur, cle) =
        profil_et_fournisseur(&catalogue, &layers, demande, &agent).await?;
    let inference = env
        .provider(&fournisseur, cle.as_ref().map(SecretValue::expose))
        .map_err(|cause| lymaille_texte(&cause))?;

    // Puits partagé : le puits possède son journal, lymaille le tient en `'static`.
    let audit = Arc::new(AuditSink::new(AuditLog::new(&layers), sink));
    let puits: Arc<dyn EventSink> = audit.clone();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());

    let engine = moteur(env, &cluster, &derived, demande, &puits, &clock, &id).await?;

    let cancel = CancelHandle::new();
    let (segment, config) = tour(&agent, &resolu, &clock, &puits, &cancel)?;

    // État : restauré si la session existe, neuf sinon.
    let (etat, pad, notes) = match existant {
        Some(_) => {
            let provenance = Provenance::new(agent.fingerprint(), BTreeMap::new());
            restore_session(&store, &id, Arc::clone(&engine), &provenance)
                .await
                .map_err(lymaille_erreur)?
                .into_parts()
        }
        None => (
            SessionState::new(
                id.clone(),
                AgentRef::new(agent.id(), agent.fingerprint()),
                contexte.clone(),
                BTreeMap::new(),
                clock.now(),
            ),
            Scratchpad::new(Arc::clone(&engine)),
            Vec::new(),
        ),
    };
    notices.extend(notes);

    let runner = TurnRunner::new(
        inference,
        DefaultStrategy::new(RefusingSummarizer, ReplayMode::None),
        resolu,
        segment,
        pad,
        FileSessionStore::new(&layers),
        etat,
        config,
    );
    Ok(OpenedSession {
        runner,
        cancel,
        audit,
        lock,
        derived,
        notices,
    })
}

/// Identifiant de la session à ouvrir, et les notices de son choix. `Last` sans session ouvre une neuve, avec notice.
fn choisir_session(
    layers: &Layers,
    store: &FileSessionStore,
    demande: &SessionRequest,
    contexte: &str,
) -> Result<(SessionId, Vec<String>), OpenError> {
    match &demande.resume {
        Resume::New => Ok((nouvel_identifiant()?, Vec::new())),
        Resume::Last => match FileSessionIndex::new(store, &AuditLog::new(layers))
            .latest_for_context(contexte)
        {
            Some(id) => Ok((id, Vec::new())),
            None => Ok((
                nouvel_identifiant()?,
                vec![format!(
                    "aucune session pour le contexte `{contexte}` : une session neuve est ouverte"
                )],
            )),
        },
        Resume::Id(id) => Ok((id.clone(), Vec::new())),
    }
}

/// Agent par défaut du contexte, lu dans le catalogue (ou livré avec la CLI).
async fn agent_du_contexte(
    catalogue: &FileCatalog,
    derived: &DerivedContext,
    contexte: &str,
) -> Result<AgentDefinition, OpenError> {
    let cible = resolve_start(&derived.context_type, catalogue)
        .await
        .map_err(lymaille_erreur)?
        .ok_or_else(|| {
            lymaille_texte(&format!(
                "aucun agent par défaut pour le contexte `{contexte}`"
            ))
        })?;
    Ok(cible.agent().clone())
}

/// Profil de modèle résolu, provider du catalogue et clé résolue du provider (absente sans référence).
async fn profil_et_fournisseur(
    catalogue: &FileCatalog,
    layers: &Layers,
    demande: &SessionRequest,
    agent: &AgentDefinition,
) -> Result<(ResolvedProfile, Provider, Option<SecretValue>), OpenError> {
    let nom_profil = match (&demande.profile, agent.profile()) {
        (Some(nom), _) => nom.clone(),
        (None, Some(nom)) => nom.to_owned(),
        (None, None) => {
            return Err(inconnu_texte(&format!(
                "aucun profil pour l'agent `{}` : passer -m ou définir un profil à l'agent",
                agent.id()
            )));
        }
    };
    let profil = catalogue
        .get_profile(&nom_profil)
        .await
        .map_err(lymaille_erreur)?
        .ok_or_else(|| inconnu_texte(&format!("profil `{nom_profil}` absent du catalogue")))?;
    let resolu = profil.resolve(None).map_err(lymaille_erreur)?;
    let fournisseur = catalogue
        .get_provider(resolu.provider())
        .await
        .map_err(lymaille_erreur)?
        .ok_or_else(|| {
            inconnu_texte(&format!(
                "provider `{}` absent du catalogue",
                resolu.provider()
            ))
        })?;
    let cle = match fournisseur.auth() {
        Some(nom) => Some(Secrets::new(layers).resolve(nom)?),
        None => None,
    };
    Ok((resolu, fournisseur, cle))
}

/// Contexte du tour (prompt de l'agent, skill et définition du pad) et configuration du tour (budget, puits, annulation).
fn tour(
    agent: &AgentDefinition,
    resolu: &ResolvedProfile,
    clock: &Arc<dyn Clock>,
    puits: &Arc<dyn EventSink>,
    cancel: &CancelHandle,
) -> Result<(SegmentContext, TurnConfig), OpenError> {
    let skill = builtin_rhai_pad_skill(&ScratchpadLimits::new()).map_err(lymaille_erreur)?;
    let systeme = format!("{}\n\n{}", agent.prompt(), skill.body());
    let segment = SegmentContext::new(
        systeme,
        vec![rhai_pad_definition()],
        ModelSnapshot::new(resolu.model(), ReplayMode::None),
    );
    let invite_resume = u64::try_from(DEFAULT_SUMMARY_PROMPT.chars().count()).unwrap_or(u64::MAX);
    let budget = resolu.budget(invite_resume, RENDU_MAX_CARACTERES);
    let config = TurnConfig::new(
        Arc::clone(clock),
        Redactor::new(),
        Arc::clone(puits),
        cancel.clone(),
        budget,
    )
    .with_agent(agent.id().to_owned());
    Ok((segment, config))
}

/// Moteur de script de la session : le module `sandbox` n'est accordé que si la sandbox est demandée.
async fn moteur<E: OpenEnv>(
    env: &E,
    cluster: &E::Cluster,
    derived: &DerivedContext,
    demande: &SessionRequest,
    puits: &Arc<dyn EventSink>,
    clock: &Arc<dyn Clock>,
    id: &SessionId,
) -> Result<Arc<ScratchpadEngine>, OpenError> {
    let mut grants = Grants::new();
    if let Some(url) = &derived.mcp_url {
        let connecteur = env
            .sandbox_connector(cluster, &demande.owner, url)
            .map_err(sandbox_erreur)?;
        let mcp_env = McpEnv::new(
            Arc::new(Redactor::new()),
            Arc::clone(puits),
            Arc::clone(clock),
            id.clone(),
            McpRetryPolicy::default(),
        );
        let module = mcp_module("sandbox", Arc::new(connecteur), mcp_env)
            .await
            .map_err(sandbox_erreur)?;
        let accorde = GrantedModule::new("sandbox", rhai::Shared::new(module))
            .ok_or_else(|| sandbox_texte("module `sandbox` refusé par lymaille"))?;
        grants = grants
            .with_module(accorde)
            .ok_or_else(|| sandbox_texte("module `sandbox` déjà accordé"))?;
    }
    let engine = ScratchpadEngine::builder(ScratchpadLimits::new(), grants)
        .build()
        .map_err(lymaille_erreur)?;
    Ok(Arc::new(engine))
}

/// Erreur d'une session absente, ou d'une session d'un autre contexte.
fn inconnu_session(id: &SessionId, contexte: &str) -> OpenError {
    inconnu_texte(&format!(
        "session `{}` inconnue du contexte `{contexte}`",
        id.as_str()
    ))
}

fn inconnu_texte(cause: &str) -> OpenError {
    OpenError::Unknown {
        cause: format!("{UNKNOWN}: {cause}"),
    }
}

fn lymaille_texte(cause: &str) -> OpenError {
    OpenError::Lymaille {
        cause: format!("{LYMAILLE}: {cause}"),
    }
}

fn lymaille_erreur(erreur: impl Display) -> OpenError {
    lymaille_texte(&erreur.to_string())
}

fn sandbox_texte(cause: &str) -> OpenError {
    OpenError::Sandbox {
        cause: format!("{SANDBOX}: {cause}"),
    }
}

fn sandbox_erreur(erreur: impl Display) -> OpenError {
    sandbox_texte(&erreur.to_string())
}

/// Identifiant neuf d'une session (UUID v4).
fn nouvel_identifiant() -> Result<SessionId, OpenError> {
    SessionId::new(uuid::Uuid::new_v4().to_string())
        .ok_or_else(|| lymaille_texte("identifiant de session vide"))
}

/// Nom du contexte dérivé, tel que lymaille le range dans l'état d'une session. Lu par sérialisation : le nom
/// n'a pas d'accesseur public.
fn contexte_nom(contexte: &ToolsetContext) -> Result<String, OpenError> {
    serde_json::to_value(contexte)
        .ok()
        .and_then(|valeur| {
            valeur
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| lymaille_texte("nom du contexte illisible"))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::time::Duration;

    use lymaille::{
        AgentRef, ContextError, Digest, ModelProfile, Provider, ProviderLevel, RecordingSink,
        ScriptedConnector, ScriptedProvider, SessionId, SessionState, SessionStore as _,
        Summarizer as _, Timestamp,
    };
    use serde_json::json;
    use vanyline_crds::{Project, ProjectSpec, Sandbox, SandboxSpec, SandboxStatus};

    use super::{OpenEnv, OpenError, RefusingSummarizer, Resume, SessionRequest, open};
    use crate::host::catalog_store::FileCatalog;
    use crate::host::context_derive::{ClusterRefusal, ClusterSource, derive_context};
    use crate::host::layers::{Layers, Level};
    use crate::host::process_lock::SessionLock;
    use crate::host::session_store::FileSessionStore;

    /// Faux cluster : le projet `demo` et la sandbox `demo-dev` (Running). Absent = `NotFound`.
    struct FauxCluster {
        sandboxes: HashMap<String, Sandbox>,
    }

    impl ClusterSource for FauxCluster {
        fn project<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Project, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move {
                if name == "demo" {
                    Ok(projet())
                } else {
                    Err(ClusterRefusal::NotFound)
                }
            })
        }

        fn sandbox<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Sandbox, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move {
                self.sandboxes
                    .get(name)
                    .cloned()
                    .ok_or(ClusterRefusal::NotFound)
            })
        }

        fn sandbox_mcp_url<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move { Ok(format!("http://{name}.vanyline-app-alice.svc:3000/mcp")) })
        }
    }

    fn projet() -> Project {
        let spec: ProjectSpec = serde_json::from_value(json!({
            "owner": "alice",
            "repoUrl": "https://example.invalid/demo.git",
            "defaultBranch": "main"
        }))
        .expect("spec de projet valide");
        let mut projet = Project::new("demo", spec);
        projet.metadata.namespace = Some("vanyline-app-alice".to_owned());
        projet
    }

    fn sandbox_running() -> Sandbox {
        let spec: SandboxSpec =
            serde_json::from_value(json!({"project": "demo", "branch": "main"}))
                .expect("spec de sandbox valide");
        let mut sandbox = Sandbox::new("demo-dev", spec);
        sandbox.metadata.namespace = Some("vanyline-app-alice".to_owned());
        sandbox.status = Some(SandboxStatus {
            phase: Some("Running".to_owned()),
            ..SandboxStatus::default()
        });
        sandbox
    }

    /// Environnement de test : cluster simulé (joignable ou non), fournisseur et connecteur scriptés, répertoires temporaires.
    struct FakeEnv {
        home: PathBuf,
        joignable: bool,
    }

    impl OpenEnv for FakeEnv {
        type Cluster = FauxCluster;
        type Provider = ScriptedProvider;
        type Connector = ScriptedConnector;

        fn home(&self) -> PathBuf {
            self.home.clone()
        }

        fn cwd(&self) -> PathBuf {
            self.home.clone()
        }

        fn connect<'a>(
            &'a self,
            _namespace: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = Result<FauxCluster, String>> + Send + 'a>> {
            Box::pin(async move {
                if self.joignable {
                    Ok(FauxCluster {
                        sandboxes: HashMap::from([("demo-dev".to_owned(), sandbox_running())]),
                    })
                } else {
                    Err("VNL-K8S-001: kubeconfig injoignable".to_owned())
                }
            })
        }

        fn provider(
            &self,
            _provider: &Provider,
            _key: Option<&str>,
        ) -> Result<ScriptedProvider, String> {
            Ok(ScriptedProvider::new(Vec::new()))
        }

        fn sandbox_connector(
            &self,
            _cluster: &FauxCluster,
            _owner: &str,
            _url: &str,
        ) -> Result<ScriptedConnector, String> {
            Ok(ScriptedConnector::new(Vec::new()))
        }
    }

    /// Couches du test : le répertoire personnel sert aussi de répertoire de travail, sans dépôt.
    fn couches(home: &Path) -> Layers {
        Layers::discover(home, home, None, Path::exists)
    }

    /// Un profil `test` et un provider `fournisseur` au niveau utilisateur, sans secret.
    fn catalogue_de_test(home: &Path) {
        let catalogue = FileCatalog::new(&couches(home));
        let profil = ModelProfile::new(
            "test".to_owned(),
            "fournisseur".to_owned(),
            "modele".to_owned(),
        )
        .with_context_size(131_072)
        .with_max_response(8_192);
        catalogue
            .put_profile_at(Level::User, &profil)
            .expect("le profil de test doit s'écrire");
        let fournisseur = Provider::new(
            "fournisseur".to_owned(),
            "openai".to_owned(),
            "http://llm.invalid/v1".to_owned(),
            None,
            ProviderLevel::User,
        )
        .expect("le provider de test doit être valide");
        catalogue
            .put_provider_at(Level::User, &fournisseur)
            .expect("le provider de test doit s'écrire");
    }

    fn demande(resume: Resume) -> SessionRequest {
        SessionRequest {
            context: "demo".to_owned(),
            owner: "alice".to_owned(),
            sandbox: None,
            resume,
            profile: Some("test".to_owned()),
            repo: None,
            namespace: None,
        }
    }

    /// Nom du contexte dérivé pour `demo` sans sandbox, tel que `open` le calcule.
    async fn nom_du_contexte(home: &Path) -> String {
        let source = FauxCluster {
            sandboxes: HashMap::new(),
        };
        let derive = derive_context(&source, "alice", "demo", None)
            .await
            .expect("le contexte de test doit se dériver");
        serde_json::to_value(&derive.context).expect("sérialisable")["name"]
            .as_str()
            .map_or_else(
                || panic!("nom de contexte attendu sous {}", home.display()),
                str::to_owned,
            )
    }

    /// Enregistre une session `id` du contexte `contexte`, sauvegardée dans le répertoire de test.
    async fn sauver(home: &Path, id: &str, contexte: &str) {
        let state = SessionState::new(
            SessionId::new(id).expect("identifiant non vide"),
            AgentRef::new("vanyline-cli", "fp-test"),
            contexte,
            BTreeMap::new(),
            Timestamp::from_millis(1_000),
        );
        FileSessionStore::new(&couches(home))
            .save(&state)
            .await
            .expect("la session de test doit se sauvegarder");
    }

    #[tokio::test]
    async fn session_neuve_prend_le_verrou_et_l_agent_du_contexte() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = FakeEnv {
            home: home.path().to_path_buf(),
            joignable: true,
        };

        let ouverte = open(&env, &demande(Resume::New), RecordingSink::new())
            .await
            .expect("une session neuve doit s'ouvrir");

        assert_eq!(ouverte.runner.state().agent().name(), "vanyline-cli");
        let verrou = SessionLock::acquire(&couches(home.path()), ouverte.runner.state().id());
        assert!(
            verrou.is_err(),
            "le verrou de la session doit être tenu pendant l'ouverture"
        );
    }

    #[tokio::test]
    async fn reprise_de_la_derniere_session_du_contexte() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let contexte = nom_du_contexte(home.path()).await;
        sauver(home.path(), "ancienne", &contexte).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        sauver(home.path(), "recente", &contexte).await;
        let env = FakeEnv {
            home: home.path().to_path_buf(),
            joignable: true,
        };

        let ouverte = open(&env, &demande(Resume::Last), RecordingSink::new())
            .await
            .expect("la dernière session doit se reprendre");

        assert_eq!(ouverte.runner.state().id().as_str(), "recente");
    }

    #[tokio::test]
    async fn last_sans_session_ouvre_une_neuve_avec_notice() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = FakeEnv {
            home: home.path().to_path_buf(),
            joignable: true,
        };

        let ouverte = open(&env, &demande(Resume::Last), RecordingSink::new())
            .await
            .expect("sans session, `Last` doit ouvrir une neuve");

        assert!(
            !ouverte.notices.is_empty(),
            "une notice doit dire qu'aucune session n'existait"
        );
    }

    #[tokio::test]
    async fn session_deja_ouverte_est_refusee_sans_chargement() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let contexte = nom_du_contexte(home.path()).await;
        sauver(home.path(), "tenue", &contexte).await;
        let autre = SessionLock::acquire(
            &couches(home.path()),
            &SessionId::new("tenue").expect("identifiant"),
        )
        .expect("le verrou de test doit se prendre");
        let env = FakeEnv {
            home: home.path().to_path_buf(),
            joignable: true,
        };

        let refus = open(
            &env,
            &demande(Resume::Id(SessionId::new("tenue").expect("identifiant"))),
            RecordingSink::new(),
        )
        .await;

        assert!(
            matches!(
                refus,
                Err(OpenError::Host(crate::host::HostError::Locked { .. }))
            ),
            "une session tenue par un autre processus doit être refusée"
        );
        drop(autre);
    }

    #[tokio::test]
    async fn identifiant_d_un_autre_contexte_est_inconnu() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        sauver(home.path(), "etrangere", "autre-contexte").await;
        let env = FakeEnv {
            home: home.path().to_path_buf(),
            joignable: true,
        };

        let refus = open(
            &env,
            &demande(Resume::Id(
                SessionId::new("etrangere").expect("identifiant"),
            )),
            RecordingSink::new(),
        )
        .await;

        assert!(
            matches!(refus, Err(OpenError::Unknown { .. })),
            "une session d'un autre contexte doit être inconnue ici"
        );
    }

    #[tokio::test]
    async fn cluster_injoignable_sans_verrou_conserve() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let contexte = nom_du_contexte(home.path()).await;
        sauver(home.path(), "hors-ligne", &contexte).await;
        let env = FakeEnv {
            home: home.path().to_path_buf(),
            joignable: false,
        };

        let refus = open(
            &env,
            &demande(Resume::Id(
                SessionId::new("hors-ligne").expect("identifiant"),
            )),
            RecordingSink::new(),
        )
        .await;

        match refus {
            Err(OpenError::Cluster { cause }) => assert!(cause.contains("VNL-K8S-001"), "{cause}"),
            autre => panic!(
                "une erreur de cluster est attendue, obtenu {:?}",
                autre.err()
            ),
        }
        assert!(
            SessionLock::acquire(
                &couches(home.path()),
                &SessionId::new("hors-ligne").expect("identifiant")
            )
            .is_ok(),
            "aucun verrou ne doit rester après un cluster injoignable"
        );
    }

    #[tokio::test]
    async fn profil_inconnu_est_refuse() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = FakeEnv {
            home: home.path().to_path_buf(),
            joignable: true,
        };
        let mut requete = demande(Resume::New);
        requete.profile = Some("inconnu".to_owned());

        let refus = open(&env, &requete, RecordingSink::new()).await;

        assert!(
            matches!(refus, Err(OpenError::Unknown { .. })),
            "un profil absent du catalogue doit être inconnu"
        );
    }

    #[tokio::test]
    async fn sandbox_branche_le_module_mcp() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = FakeEnv {
            home: home.path().to_path_buf(),
            joignable: true,
        };
        let mut requete = demande(Resume::New);
        requete.sandbox = Some("dev".to_owned());

        let ouverte = open(&env, &requete, RecordingSink::new())
            .await
            .expect("une session sur une sandbox Running doit s'ouvrir");

        assert!(
            ouverte.derived.mcp_url.is_some(),
            "l'URL MCP de la sandbox doit être dérivée"
        );
    }

    #[tokio::test]
    async fn summarizer_refuse_au_debordement() {
        let refus = RefusingSummarizer
            .summarize("user: bonjour", None::<&Digest>)
            .await;

        match refus {
            Err(ContextError::Summarizer(message)) => {
                assert!(
                    message.contains("déborde"),
                    "message explicite attendu : {message}"
                );
            }
            autre => panic!("un refus de résumé est attendu, obtenu {autre:?}"),
        }
    }
}
