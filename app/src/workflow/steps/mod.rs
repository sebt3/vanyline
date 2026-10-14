//! Registres des kinds de step et accès communs — contrat : `mod.sdd`.
//!
//! Construit le [`StepRegistry`] du `StepDispatcher` (`../runtime.sdd`) avec les quatre kinds de
//! vanyline — `agent`, `rhai`, `shell`, `gate` — et porte ce qu'ils partagent : l'environnement
//! d'exécution ([`StepEnv`]), la lecture typée du contexte de run injecté (`config._run`,
//! `../run_context.sdd`) et l'accès authentifié à la sandbox d'un run ([`SandboxAccess`]).
//! Aucune logique de kind ici.

pub mod agent;
pub mod gate;
pub mod rhai;
pub mod shell;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use miryad_core::workflow::MiryadWorkflowStep;
use miryad_core::workflow::StepError;
use miryad_core::workflow::StepRegistry;
use serde_json::Value;
use vanyline_lib::VnyError;
use vanyline_lib::k8s::VnlK8sClient;

use super::run_context::RunContext;
use crate::config::Config;

/// Timeout de chaque appel HTTP vers la sandbox (`mod.sdd` `Exposes`).
const SANDBOX_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Tout ce dont un step a besoin, sans passer par l'état HTTP de l'app (`mod.sdd` `Exposes`).
#[derive(Clone)]
pub struct StepEnv {
    /// Connexion base de l'app (le step borne lui-même ses requêtes à l'utilisateur du run).
    pub db: sea_orm::DatabaseConnection,
    /// Client K8s typé : lecture de la CR Sandbox et jeton de l'Owner (`lib/src/k8s.sdd`).
    pub k8s: VnlK8sClient,
    /// Client HTTP partagé (le timeout de 10 s est posé par `SandboxAccess` à chaque appel).
    pub http: reqwest::Client,
    /// Configuration de l'app : champ contractuel (`mod.sdd` `Exposes`), aucun kind W2 ne le lit.
    #[allow(dead_code)] // W3+ consommateurs (`runtime.rs`, `status.rs`) — tooling.sdd
    pub config: Arc<Config>,
}

/// Lecture typée de `config._run` (le `@RunContext` de `../run_context.sdd`) : les huit champs
/// sont accessibles par [`std::ops::Deref`].
#[derive(Clone, Debug)]
pub struct RunInfo(RunContext);

impl std::ops::Deref for RunInfo {
    type Target = RunContext;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Face interne du cluster pour la sandbox d'un run : phase de la CR, jeton de l'Owner, URL
/// interne. Séam `pub(crate)` : la convention projet interdit tout mock de `kube`
/// (`lib/src/k8s.sdd` `Must not`), [`VnlK8sClient`] n'est donc pas constructible hors cluster —
/// les Scenario de `mod.sdd` sur [`SandboxAccess`] verrouillent la logique sur ce séam ; la
/// production le branche sur `VnlK8sClient` via [`K8sLocator`].
#[async_trait]
#[allow(clippy::double_must_use)] // async_trait pose `must_use` sur le BoxFuture déjà must_use — tooling.sdd (précédent `trait EventSink`)
pub(crate) trait SandboxLocator: Send + Sync {
    /// `status.phase` de la CR Sandbox de l'Owner ; erreur `retryable: false` si la CR est
    /// absente ou illisible (impossible de prouver `Running`).
    async fn phase(&self, owner: &str, sandbox: &str) -> Result<Option<String>, StepError>;
    /// Jeton courant de l'Owner — redemandé à chaque utilisation ([`mod.sdd`] `Must` : le cache
    /// interne de `owner_token` le rafraîchit avant expiration).
    async fn token(&self, owner: &str) -> Result<String, StepError>;
    /// Base de l'URL interne de la sandbox : `http://sandbox-{name}.{ns}.svc:3000`.
    async fn base_url(&self, owner: &str, sandbox: &str) -> Result<String, StepError>;
}

/// Le séam branché sur le client K8s de production.
pub(crate) struct K8sLocator {
    k8s: VnlK8sClient,
}

impl K8sLocator {
    pub(crate) fn new(k8s: VnlK8sClient) -> Self {
        Self { k8s }
    }
}

#[async_trait]
impl SandboxLocator for K8sLocator {
    async fn phase(&self, owner: &str, sandbox: &str) -> Result<Option<String>, StepError> {
        match self.k8s.get_sandbox(owner, sandbox).await {
            Ok(cr) => Ok(cr.status.and_then(|s| s.phase)),
            // `lib/src/k8s.rs` aplatit `NotFound` dans `K8sApiError` : CR absente ⇒ phase
            // impossible à prouver, non retryable (le run est périmé) ; autre erreur API ⇒
            // transitoire.
            Err(e @ VnyError::K8sApiError(_)) => {
                let msg = e.to_string();
                if msg.contains("404") || msg.to_lowercase().contains("not found") {
                    Ok(None)
                } else {
                    Err(StepError {
                        message: format!("lecture de la sandbox « {sandbox} » impossible : {e}"),
                        retryable: true,
                    })
                }
            }
            Err(e) => Err(StepError {
                message: format!("lecture de la sandbox « {sandbox} » impossible : {e}"),
                retryable: true,
            }),
        }
    }

    async fn token(&self, owner: &str) -> Result<String, StepError> {
        self.k8s
            .owner_token(owner)
            .await
            .map(|t| t.token)
            .map_err(|e| StepError {
                // Le message de `VnyError` ne porte jamais le jeton lui-même.
                message: format!("jeton de l'Owner « {owner} » indisponible : {e}"),
                retryable: true,
            })
    }

    async fn base_url(&self, owner: &str, sandbox: &str) -> Result<String, StepError> {
        // `sandbox_mcp_url` rend `{base}/mcp` ; les relais (jobs, code-health) sont sur la même
        // base — amputer le suffixe plutôt que de reconstruire l'URL.
        self.k8s
            .sandbox_mcp_url(owner, sandbox)
            .await
            .map_err(|e| StepError {
                message: format!("URL interne de la sandbox « {sandbox} » indisponible : {e}"),
                retryable: true,
            })
            .map(|url| url.strip_suffix("/mcp").map(str::to_string).unwrap_or(url))
    }
}

/// Accès authentifié à la sandbox d'un run (`mod.sdd` `Exposes`) : URL interne de la sandbox,
/// Bearer jeton de l'Owner redemandé à chaque appel, timeout 10 s ; le jeton n'apparaît dans
/// aucune erreur.
pub struct SandboxAccess {
    http: reqwest::Client,
    owner: String,
    base: String,
    locator: Arc<dyn SandboxLocator>,
}

/// Affichage de diagnostic : identifiants seulement — ni le client HTTP, ni le séam, et de
//  toute façon jamais de jeton (`mod.sdd` `Must`).
impl std::fmt::Debug for SandboxAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxAccess")
            .field("owner", &self.owner)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl SandboxAccess {
    /// Vérifie `sandbox` présent, CR existante en phase `Running`, jeton disponible, puis forme
    /// l'URL interne. W2 stub — délègue à [`Self::open_with`] branché sur [`K8sLocator`].
    pub async fn open(env: &StepEnv, run: &RunInfo) -> Result<Self, StepError> {
        Self::open_with(
            Arc::new(K8sLocator::new(env.k8s.clone())),
            env.http.clone(),
            run,
        )
        .await
    }

    /// Logique contractuelle d'ouverture sur le séam (voir [`SandboxLocator`] : pourquoi le
    /// séam). Vérifie phase `Running` AVANT toute demande de jeton ou appel HTTP, puis exige un
    /// jeton.
    pub(crate) async fn open_with(
        locator: Arc<dyn SandboxLocator>,
        http: reqwest::Client,
        run: &RunInfo,
    ) -> Result<Self, StepError> {
        let sandbox = run.sandbox.as_deref().ok_or_else(|| StepError {
            message: format!(
                "sandbox non démarrée : le run « {} » est de portée projet, aucune sandbox",
                run.run_id
            ),
            retryable: false,
        })?;
        let phase = locator.phase(&run.owner, sandbox).await?;
        if phase.as_deref() != Some("Running") {
            return Err(StepError {
                message: format!(
                    "sandbox non démarrée : « {sandbox} » en phase « {} »",
                    phase.as_deref().unwrap_or("inconnue")
                ),
                retryable: false,
            });
        }
        // Le jeton est exigé dès l'ouverture (puis redemandé à chaque appel).
        let _token = locator.token(&run.owner).await?;
        let base = locator.base_url(&run.owner, sandbox).await?;
        Ok(Self {
            http,
            owner: run.owner.clone(),
            base,
            locator,
        })
    }

    /// GET JSON vers `{base}{chemin}` — Bearer jeton, timeout 10 s.
    pub async fn get_json(&self, path: &str) -> Result<Value, StepError> {
        self.call(axum::http::Method::GET, path, None).await
    }

    /// POST JSON vers `{base}{chemin}` (corps `body`) — Bearer jeton, timeout 10 s.
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value, StepError> {
        self.call(axum::http::Method::POST, path, Some(body)).await
    }

    /// DELETE `{base}{chemin}` (le chemin peut porter une query, ex. `/jobs?prefix=…`) — Bearer
    /// jeton, timeout 10 s. Rend le corps JSON (`Value::Null` si réponse vide).
    pub async fn delete(&self, path: &str) -> Result<Value, StepError> {
        self.call(axum::http::Method::DELETE, path, None).await
    }

    /// GET JSON rendant le **statut HTTP visible** : `(statut, corps JSON si body
    /// exploitable)` pour TOUT statut, 4xx/5xx compris — l'appelant peut matcher le statut
    /// structurellement (ex. 404 = job perdu, `shell.sdd`). Seules erreurs remontées : panne
    /// de transport (retryable) et 2xx au corps JSON illisible (non retryable) ; le jeton
    /// n'apparaît dans aucune erreur.
    pub(crate) async fn get_json_status(
        &self,
        path: &str,
    ) -> Result<(u16, Option<Value>), StepError> {
        let url = format!("{}{}", self.base, path);
        let (status, resp) = self.send(axum::http::Method::GET, path, None).await?;
        let text = resp.text().await.map_err(|e| StepError {
            message: format!("lecture de la réponse de {url} interrompue : {e}"),
            retryable: true,
        })?;
        let code = status.as_u16();
        if text.is_empty() {
            return Ok((code, None));
        }
        if status.is_success() {
            let value = serde_json::from_str(&text).map_err(|e| StepError {
                message: format!("réponse JSON invalide de {url} (HTTP {status}) : {e}"),
                retryable: false,
            })?;
            return Ok((code, Some(value)));
        }
        // Réponse d'erreur au corps non JSON : le statut reste exploitable, le corps n'est
        // pas imposé.
        Ok((code, serde_json::from_str(&text).ok()))
    }

    /// La part commune des appels : jeton redemandé (`mod.sdd` `Must`), Bearer, timeout 10 s.
    /// Rend le statut et la réponse bruts — la politique de décodage et d'erreur appartient à
    /// l'appelant ([`Self::call`], [`Self::get_json_status`]). Les erreurs de transport sont
    /// retryables et ne portent jamais l'en-tête d'authentification.
    async fn send(
        &self,
        method: axum::http::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(axum::http::StatusCode, reqwest::Response), StepError> {
        let token = self.locator.token(&self.owner).await?;
        let url = format!("{}{}", self.base, path);
        let req = self
            .http
            .request(method, &url)
            .bearer_auth(token)
            .timeout(SANDBOX_HTTP_TIMEOUT);
        let req = match body {
            Some(b) => req.json(b),
            None => req,
        };
        req.send()
            .await
            .map_err(|e| StepError {
                // `reqwest` ne met jamais l'en-tête d'authentification dans ses erreurs.
                message: format!("sandbox « {} » injoignable sur {url} : {e}", self.owner),
                retryable: true,
            })
            .map(|resp| (resp.status(), resp))
    }

    /// Un appel : jeton redemandé (`mod.sdd` `Must`), Bearer, timeout 10 s ; statut 2xx rendu
    /// en JSON ; 4xx/5xx en erreur non retryable portant le statut sans le jeton ; panne de
    /// transport retryable.
    async fn call(
        &self,
        method: axum::http::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, StepError> {
        let url = format!("{}{}", self.base, path);
        let (status, resp) = self.send(method, path, body).await?;
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let head: String = body.chars().take(200).collect();
            return Err(StepError {
                message: format!(
                    "sandbox « {} » : HTTP {url} → {status} : {head}",
                    self.owner
                ),
                retryable: false,
            });
        }
        let text = resp.text().await.map_err(|e| StepError {
            message: format!("lecture de la réponse de {url} interrompue : {e}"),
            retryable: true,
        })?;
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| StepError {
            message: format!("réponse JSON invalide de {url} (HTTP {status}) : {e}"),
            retryable: false,
        })
    }
}

impl RunInfo {
    /// Lit `config._run` (`retryable: false` si absent ou mal formé : un step lancé sans
    /// contexte est une erreur de fabrication du DAG).
    pub fn from_config(config: &Value) -> Result<Self, StepError> {
        let raw = config.get("_run").ok_or_else(|| StepError {
            message:
                "contexte d'exécution absent : `config._run` injecté par `materialize` est requis"
                    .to_string(),
            retryable: false,
        })?;
        let ctx: RunContext = serde_json::from_value(raw.clone()).map_err(|e| StepError {
            message: format!("contexte d'exécution mal formé (`config._run`) : {e}"),
            retryable: false,
        })?;
        Ok(Self(ctx))
    }
}

/// Assemble le registre depuis les quatre kinds déjà construits — séam de [`build_registry`]
/// testable sans cluster (`VnlK8sClient` n'est pas constructible hors cluster, convention
/// anti-mock `lib/src/k8s.sdd` ; les Scenario verrouillent l'assemblage par sondes ici et les
/// `kind()` exacts dans les fichiers de kinds).
pub(crate) fn assemble_registry(
    agent: Box<dyn MiryadWorkflowStep>,
    rhai: Box<dyn MiryadWorkflowStep>,
    shell: Box<dyn MiryadWorkflowStep>,
    gate: Box<dyn MiryadWorkflowStep>,
) -> StepRegistry {
    let mut reg = StepRegistry::new();
    // Une collision de kind est un bug d'assemblage (deux `build_registry` mélangés) : elle ne
    // peut pas être propagée ici (`build_registry` rend le registre), elle est journalisée.
    for step in [agent, rhai, shell, gate] {
        if let Err(e) = reg.register(DynStep(step)) {
            tracing::error!(error = %e, "assemblage du registre des steps : kind en double");
        }
    }
    reg
}

/// Enveloppe un `Box<dyn MiryadWorkflowStep>` (forme des kinds construits par
/// [`build_registry`]) pour le registre, dont `register` exige un type concret.
struct DynStep(Box<dyn MiryadWorkflowStep>);

#[async_trait]
impl MiryadWorkflowStep for DynStep {
    fn kind(&self) -> &'static str {
        self.0.kind()
    }
    async fn run(
        &self,
        run: &miryad_core::workflow::RunInfo,
        config: Value,
        inputs: std::collections::HashMap<String, Value>,
    ) -> Result<Value, StepError> {
        self.0.run(run, config, inputs).await
    }
}

/// Enregistre exactement `agent`, `rhai`, `shell`, `gate` — jamais un autre kind, jamais deux
/// fois le même (`mod.sdd` `Exposes`).
pub fn build_registry(env: StepEnv) -> StepRegistry {
    assemble_registry(
        Box::new(agent::AgentStep::new(env.clone())),
        Box::new(rhai::rhai_step(env.clone())),
        Box::new(shell::ShellStep::new(env.clone())),
        Box::new(gate::GateStep::new(env)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::response::IntoResponse;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(super) fn run_ctx(sandbox: Option<&str>) -> RunContext {
        RunContext {
            run_id: "11111111-2222-4333-8444-555555555555".to_string(),
            user: "alice".to_string(),
            user_id: 7,
            owner: "alice".to_string(),
            project: "demo".to_string(),
            sandbox: sandbox.map(str::to_string),
            triggered_by: "ui".to_string(),
            definition: "deploy".to_string(),
        }
    }

    /// Config de step portant `_run` (+ champs additionnels fusionnés).
    pub(super) fn config_with_run(
        ctx: &RunContext,
        extra: serde_json::Map<String, Value>,
    ) -> Value {
        let mut map = extra;
        map.insert("_run".to_string(), serde_json::to_value(ctx).unwrap());
        Value::Object(map)
    }

    /// Faux séam cluster : phase, jeton et base contrôlés ; compte les demandes de jeton et de
    /// phase (assertions « aucun appel HTTP », « jeton redemandé »).
    pub(super) struct FakeLocator {
        pub(super) phase: Option<String>,
        pub(super) token: String,
        pub(super) base: String,
        pub(super) token_calls: AtomicUsize,
        pub(super) phase_calls: AtomicUsize,
    }

    impl FakeLocator {
        pub(super) fn running(token: &str, base: &str) -> Arc<Self> {
            Arc::new(Self {
                phase: Some("Running".to_string()),
                token: token.to_string(),
                base: base.to_string(),
                token_calls: AtomicUsize::new(0),
                phase_calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl SandboxLocator for FakeLocator {
        async fn phase(&self, _owner: &str, _sandbox: &str) -> Result<Option<String>, StepError> {
            self.phase_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.phase.clone())
        }
        async fn token(&self, _owner: &str) -> Result<String, StepError> {
            self.token_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.token.clone())
        }
        async fn base_url(&self, _owner: &str, _sandbox: &str) -> Result<String, StepError> {
            Ok(self.base.clone())
        }
    }

    /// Séam « CR absente » : `phase` rend une erreur non retryable (impossible de prouver
    /// `Running`) —Scenario `sandbox_non_demarree`, appointment CR absente.
    struct Absent;
    #[async_trait]
    impl SandboxLocator for Absent {
        async fn phase(&self, _o: &str, _s: &str) -> Result<Option<String>, StepError> {
            Err(StepError {
                message: "sandbox non démarrée : lecture de la Sandbox impossible (absente)"
                    .to_string(),
                retryable: false,
            })
        }
        async fn token(&self, _o: &str) -> Result<String, StepError> {
            Ok("tok".to_string())
        }
        async fn base_url(&self, _o: &str, _s: &str) -> Result<String, StepError> {
            Ok("http://x".to_string())
        }
    }

    /// Faux serveur sandbox : répond `status` + `body` à toute requête, enregistre
    /// `« MÉTHODE chemin »` et le dernier en-tête `Authorization`.
    pub(super) async fn spawn_fake_sandbox(
        status: u16,
        body: &'static str,
    ) -> (
        String,
        Arc<AtomicUsize>,
        Arc<Mutex<Vec<String>>>,
        Arc<Mutex<String>>,
    ) {
        let hits = Arc::new(AtomicUsize::new(0));
        let reqs = Arc::new(Mutex::new(Vec::new()));
        let auth = Arc::new(Mutex::new(String::new()));
        let (hits2, reqs2, auth2) = (hits.clone(), reqs.clone(), auth.clone());
        let app = axum::Router::new().route(
            "/{*rest}",
            axum::routing::any(move |req: axum::extract::Request| async move {
                hits2.fetch_add(1, Ordering::SeqCst);
                reqs2
                    .lock()
                    .unwrap()
                    .push(format!("{} {}", req.method(), req.uri()));
                if let Some(v) = req.headers().get(axum::http::header::AUTHORIZATION) {
                    *auth2.lock().unwrap() = v.to_str().unwrap_or_default().to_string();
                }
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    axum::body::Body::from(body),
                )
                    .into_response()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), hits, reqs, auth)
    }

    fn http_client() -> reqwest::Client {
        reqwest::Client::new()
    }

    // ── Scenario: contexte_absent ───────────────────────────────────────────
    // `_run` absent ou mal formé ⇒ erreur non retryable (erreur de fabrication du DAG).
    #[test]
    fn contexte_absent() {
        let err = RunInfo::from_config(&serde_json::json!({"script": "echo hi"}))
            .expect_err("une config sans _run est une erreur non retryable");
        assert!(!err.retryable, "absent ⇒ jamais retryable : {err}");
        let err = RunInfo::from_config(&serde_json::json!({"_run": 42}))
            .expect_err("un _run non-objet est mal formé");
        assert!(!err.retryable, "mal formé ⇒ jamais retryable : {err}");
        let err = RunInfo::from_config(&serde_json::json!({"_run": {"owner": "alice"}}))
            .expect_err("un _run sans run_id est mal formé");
        assert!(!err.retryable);
        // appoint `Exposes` : un _run complet se lit intégralement.
        let cfg = config_with_run(&run_ctx(Some("sbx-1")), serde_json::Map::new());
        let run = RunInfo::from_config(&cfg).expect("un contexte complet se lit");
        assert_eq!(run.run_id, run_ctx(Some("sbx-1")).run_id);
        assert_eq!(run.user_id, 7);
        assert_eq!(run.sandbox.as_deref(), Some("sbx-1"));
    }

    // ── Scenario: sandbox_non_demarree ──────────────────────────────────────
    // Phase `Provisioning` ⇒ « sandbox non démarrée » non retryable, AUCUN appel HTTP, aucun
    // jeton demandé. Appoint : CR absente (erreur du séam) ⇒ même refus.
    #[tokio::test]
    async fn sandbox_non_demarree() {
        let (base, hits, _reqs, _auth) = spawn_fake_sandbox(200, "{}").await;
        let locator = Arc::new(FakeLocator {
            phase: Some("Provisioning".to_string()),
            token: "tok".to_string(),
            base: base.clone(),
            token_calls: AtomicUsize::new(0),
            phase_calls: AtomicUsize::new(0),
        });
        let ctx = run_ctx(Some("sbx-1"));
        let cfg = config_with_run(&ctx, serde_json::Map::new());
        let run = RunInfo::from_config(&cfg).expect("contexte construit par le test");

        let err = SandboxAccess::open_with(locator.clone(), http_client(), &run)
            .await
            .expect_err("Provisioning n'est pas Running");
        assert!(
            err.message.contains("sandbox non démarrée"),
            "message contractuel attendu : {err}"
        );
        assert!(!err.retryable, "phase non-Running ⇒ non retryable : {err}");
        assert_eq!(
            locator.token_calls.load(Ordering::SeqCst),
            0,
            "aucun jeton demandé"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 0, "aucun appel HTTP");

        // (le séam `Absent` de CR absente est déclaré en tête du module de tests)
        let err = SandboxAccess::open_with(Arc::new(Absent), http_client(), &run)
            .await
            .expect_err("une CR absente n'est pas une sandbox démarrée");
        assert!(!err.retryable);
    }

    // Appoint `Must` : le refus d'ouverture porte « sandbox non démarrée » aussi quand la
    // phase est absente du status (None).
    #[tokio::test]
    async fn phase_absente_refus_non_retryable() {
        let (base, _hits, _reqs, _auth) = spawn_fake_sandbox(200, "{}").await;
        let locator = Arc::new(FakeLocator {
            phase: None,
            token: "tok".to_string(),
            base,
            token_calls: AtomicUsize::new(0),
            phase_calls: AtomicUsize::new(0),
        });
        let ctx = run_ctx(Some("sbx-1"));
        let cfg = config_with_run(&ctx, serde_json::Map::new());
        let run = RunInfo::from_config(&cfg).unwrap();
        let err = SandboxAccess::open_with(locator, http_client(), &run)
            .await
            .expect_err("phase None ⇒ pas Running");
        assert!(!err.retryable);
        assert!(err.message.contains("sandbox non démarrée"));
    }

    // ── Scenario: jeton_jamais_dans_les_erreurs ─────────────────────────────
    // Sandbox qui répond 401 ⇒ le message d'erreur ne contient pas le jeton.
    #[tokio::test]
    async fn jeton_jamais_dans_les_erreurs() {
        let (base, _hits, _reqs, _auth) = spawn_fake_sandbox(401, "unauthorized").await;
        let locator = FakeLocator::running("JWT-SECRET-BANNANE-9f2c", &base);
        let ctx = run_ctx(Some("sbx-1"));
        let cfg = config_with_run(&ctx, serde_json::Map::new());
        let run = RunInfo::from_config(&cfg).expect("contexte construit par le test");
        let access = SandboxAccess::open_with(locator, http_client(), &run)
            .await
            .expect("sandbox Running, le pas s'ouvre");

        let err = access
            .get_json("/code-health")
            .await
            .expect_err("401 est une erreur");
        assert!(
            !err.message.contains("JWT-SECRET-BANNANE-9f2c"),
            "le message ne doit jamais porter le jeton : {err}"
        );
        assert!(
            err.message.contains("401"),
            "le message porte le statut : {err}"
        );
        assert!(
            !err.retryable,
            "401 n'est pas une panne de transport : {err}"
        );
    }

    // Appoint `Exposes` : Bearer + base∧chemin, et `Must` « redemander le jeton à chaque
    // utilisation ».
    #[tokio::test]
    async fn bearer_et_jeton_redemande() {
        let (base, _hits, reqs, auth) = spawn_fake_sandbox(200, r#"{"ok":true}"#).await;
        let locator = FakeLocator::running("tok-1", &base);
        let ctx = run_ctx(Some("sbx-1"));
        let cfg = config_with_run(&ctx, serde_json::Map::new());
        let run = RunInfo::from_config(&cfg).expect("contexte construit par le test");
        let access = SandboxAccess::open_with(locator.clone(), http_client(), &run)
            .await
            .expect("sandbox Running");

        let out = access.get_json("/jobs/x").await.expect("200 → JSON");
        assert_eq!(out["ok"], true);
        assert!(
            reqs.lock().unwrap().iter().any(|r| r.ends_with("/jobs/x")),
            "chemin transmis sous la base : {:?}",
            reqs.lock().unwrap()
        );
        assert_eq!(auth.lock().unwrap().as_str(), "Bearer tok-1");

        access
            .post_json("/jobs", &serde_json::json!({"a": 1}))
            .await
            .expect("200");
        access.delete("/jobs?prefix=r1").await.expect("200");
        // 1 jeton à l'ouverture + 1 par appel : redemandé à chaque utilisation.
        assert_eq!(
            locator.token_calls.load(Ordering::SeqCst),
            4,
            "le jeton est redemandé à chaque utilisation"
        );
    }

    // Appoint transport : panne de connexion ⇒ erreur retryable (seule classe retryable).
    #[tokio::test]
    async fn transport_transitoire_est_retryable() {
        // Port refermé : rien n'écoute.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let locator = FakeLocator::running("tok-1", &format!("http://{addr}"));
        let ctx = run_ctx(Some("sbx-1"));
        let cfg = config_with_run(&ctx, serde_json::Map::new());
        let run = RunInfo::from_config(&cfg).unwrap();
        let access = SandboxAccess::open_with(locator, http_client(), &run)
            .await
            .expect("ouverture sans appel sandbox");
        let err = access
            .get_json("/jobs/j1")
            .await
            .expect_err("port fermé = panne de transport");
        assert!(err.retryable, "transport transitoire ⇒ retryable : {err}");
    }

    // Appoint `Exposes` : un run sans `_run.sandbox` ne peut pas ouvrir d'accès (base des
    // Scenario `refuse_sans_sandbox` de `shell.sdd` et `gate.sdd`).
    #[tokio::test]
    async fn refus_sans_sandbox_dans_le_run() {
        let (base, _hits, _reqs, _auth) = spawn_fake_sandbox(200, "{}").await;
        let locator = FakeLocator::running("tok-1", &base);
        let ctx = run_ctx(None);
        let cfg = config_with_run(&ctx, serde_json::Map::new());
        let run = RunInfo::from_config(&cfg).unwrap();
        let err = SandboxAccess::open_with(locator, http_client(), &run)
            .await
            .expect_err("un run de portée projet n'a pas de sandbox");
        assert!(
            !err.retryable,
            "absence de sandbox = erreur de contexte : {err}"
        );
    }

    // ── Scenario: registre_exact ────────────────────────────────────────────
    // Énumération par sonde : `StepRegistry` n'expose aucune liste de kinds ; un `kind()` déjà
    // pris rend `DuplicateStepKind` — la sonde échoue ⇒ le kind est enregistré. Un kind
    // étranger s'enregistre ⇒ `build_registry` n'enregistre QUE les quatre.
    struct Probe(&'static str);
    #[async_trait]
    impl MiryadWorkflowStep for Probe {
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

    #[test]
    fn registre_exact() {
        let registry = assemble_registry(
            Box::new(Probe("agent")),
            Box::new(Probe("rhai")),
            Box::new(Probe("shell")),
            Box::new(Probe("gate")),
        );
        let mut probe = registry;
        for kind in ["agent", "rhai", "shell", "gate"] {
            // La sonde doit être refusée : le kind est déjà dans le registre.
            let err = probe
                .register(Probe(kind))
                .err()
                .expect("{{kind}} doit être enregistré — la sonde doit refuser");
            assert!(
                matches!(&err, miryad_core::workflow::WorkflowError::DuplicateStepKind(k) if k == kind),
                "sonde {kind} : {err:?}"
            );
        }
        // Et rien d'autre : un cinquième kind s'enregistre sans collision.
        assert!(
            probe.register(Probe("subworkflow")).is_ok(),
            "aucun autre kind n'est enregistré par build_registry"
        );
    }

    // Appoint `Exposes` @build_registry : les kinds de production portent les quatre noms
    // exacts via leur constante `KIND` (l'assemblage des kinds réels dans `build_registry`
    // n'est pas instanciable hors cluster — `VnlK8sClient` non constructible, convention
    // anti-mock kube ; verrouillé par `assemble_registry` + constantes, signalé au rapport).
    #[test]
    fn kinds_de_production() {
        assert_eq!(agent::AgentStep::KIND, "agent");
        assert_eq!(gate::GateStep::KIND, "gate");
        assert_eq!(shell::ShellStep::KIND, "shell");
        // le kind `rhai` est celui de `RhaiStep` de miryad-core (`kind() == "rhai"`,
        // `rhai_step.rs`) — verrouillé par exécution dans les tests de `rhai.rs`.
    }
}
