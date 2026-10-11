//! Kind `shell` — contrat : `shell.sdd`. Exécute un script shell DANS la sandbox du run par
//! l'API de jobs (`/sandbox/src/jobs.sdd`) : démarrage idempotent sur `job_id = {run_id}:{step_id}`
//! puis interrogation jusqu'à l'état terminal. Le callback d'awakeable n'est pas utilisé (pas
//! de step durable dans miryad-core — issue #26).

use miryad_core::workflow::MiryadWorkflowStep;
use miryad_core::workflow::RunInfo as EngineRunInfo;
use miryad_core::workflow::StepError;
use serde_json::Value;

use super::{RunInfo, SandboxAccess, StepEnv};

/// Kind de step `shell`.
pub struct ShellStep {
    env: StepEnv,
}

impl ShellStep {
    /// Le nom de kind contractuel (`shell.sdd` `Exposes`).
    pub(crate) const KIND: &'static str = "shell";

    /// Construit le kind à partir de l'environnement d'exécution.
    pub(crate) fn new(env: StepEnv) -> Self {
        Self { env }
    }
}

/// La `config` du kind `shell`, désérialisée (les clés inconnues — `_run` notamment — sont
/// ignorées ici et lues par `RunInfo::from_config`).
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ShellConfig {
    /// Script transmis tel quel à la sandbox.
    pub script: String,
    /// Sous-répertoire confiné demandé.
    pub cwd: Option<String>,
    /// Variables d'addition d'environnement — jamais exposées (secret possible).
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// Timeout transmis tel quel à la sandbox (défaut sandbox 3 600 s).
    pub timeout_secs: Option<u64>,
    /// Rend la sortie sans erreur quel que soit l'état terminal.
    #[serde(default)]
    pub allow_failure: bool,
}

/// Parse et valide la config (`script` vide ⇒ erreur non retryable).
pub(crate) fn parse_config(config: &Value) -> Result<ShellConfig, StepError> {
    let cfg: ShellConfig = serde_json::from_value(config.clone()).map_err(|e| StepError {
        message: format!("config du step `shell` invalide : {e}"),
        retryable: false,
    })?;
    if cfg.script.trim().is_empty() {
        return Err(StepError {
            message: "config du step `shell` invalide : `script` est vide".to_string(),
            retryable: false,
        });
    }
    Ok(cfg)
}

/// Corps JSON `JobStatus` (`/sandbox/src/jobs.sdd`) rendu par POST /jobs et GET /jobs/{{id}}.
fn state_of(status: &Value) -> Option<&str> {
    status.get("state").and_then(Value::as_str)
}

/// Statut HTTP signifiant que la sandbox a oublié le job (`shell.sdd` `Raises` : job perdu —
/// détection structurelle du statut, jamais par appariement du message d'erreur).
const JOB_LOST_STATUS: u16 = 404;

/// États non terminaux connus de l'API jobs : `running` est le seul que rend `JobStatus`
/// (`/sandbox/src/jobs.sdd` — pas de `pending` dans le contrat) ; tout autre état, comme un
/// statut absent, est traité comme terminal (sortie `state: "inconnu"` puis refus).
fn is_terminal(state: Option<&str>) -> bool {
    !matches!(state, Some("running"))
}

/// Rend la sortie contractuelle : `{state, exit_code, log_tail, duration_ms}` (durée en
/// secondes rendue en millisecondes, arithmetic-safe).
fn output_of(status: &Value) -> Value {
    let started = status
        .get("started_at")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let ended = status
        .get("ended_at")
        .and_then(Value::as_u64)
        .unwrap_or(started);
    serde_json::json!({
        "state": state_of(status).unwrap_or("inconnu"),
        "exit_code": status.get("exit_code").cloned().unwrap_or(Value::Null),
        "log_tail": status.get("log_tail").cloned().unwrap_or(Value::String(String::new())),
        "duration_ms": ended.saturating_sub(started).saturating_mul(1_000),
    })
}

/// Démarrage idempotent puis interrogation bornée (contrat : `shell.sdd` `Handles`) : POST
/// `{run_id}:{step_id}` sans callback, puis GET jusqu'à l'état terminal (2 s → 10 s).
pub(crate) async fn execute(
    access: &SandboxAccess,
    run: &RunInfo,
    engine: &EngineRunInfo,
    cfg: &ShellConfig,
) -> Result<Value, StepError> {
    let job_id = format!("{}:{}", run.run_id, engine.step_id);
    let mut body = serde_json::Map::new();
    body.insert("job_id".to_string(), Value::String(job_id.clone()));
    body.insert("script".to_string(), Value::String(cfg.script.clone()));
    if let Some(cwd) = &cfg.cwd {
        body.insert("cwd".to_string(), Value::String(cwd.clone()));
    }
    if let Some(env) = &cfg.env {
        body.insert(
            "env".to_string(),
            serde_json::to_value(env).map_err(|e| StepError {
                message: format!("config du job {job_id} non sérialisable : {e}"),
                retryable: false,
            })?,
        );
    }
    if let Some(t) = cfg.timeout_secs {
        body.insert("timeout_secs".to_string(), Value::from(t));
    }
    // 201 = lancé, 200 = job déjà connu (rejeu) : le corps est l'état dans les deux cas.
    let mut status = access
        .post_json("/jobs", &Value::Object(body))
        .await
        .map_err(|e| StepError {
            message: format!("démarrage du job {job_id} refusé : {}", e.message),
            retryable: e.retryable,
        })?;
    // Interrogation : backoff 2 s → 10 s entre les GET (le POST idempotent tient lieu d'état
    // quand il rend déjà un état terminal).
    let mut delay = tokio::time::Duration::from_secs(2);
    let max_delay = tokio::time::Duration::from_secs(10);
    while !is_terminal(state_of(&status)) {
        tokio::time::sleep(delay).await;
        if delay < max_delay {
            delay = delay.mul_f64(2.0).min(max_delay);
        }
        let (http_status, body) = access
            .get_json_status(&format!("/jobs/{job_id}"))
            .await
            .map_err(|e| StepError {
                message: format!("interrogation du job {job_id} impossible : {}", e.message),
                retryable: e.retryable,
            })?;
        if http_status == JOB_LOST_STATUS {
            // 404 pendant l'interrogation : la sandbox a oublié le job (`shell.sdd`
            // `Raises`) — match structurel du statut.
            return Err(StepError {
                message: format!("job perdu : la sandbox a oublié le job {job_id}"),
                retryable: false,
            });
        }
        if !(200..300).contains(&http_status) {
            return Err(StepError {
                message: format!("interrogation du job {job_id} : HTTP {http_status}"),
                retryable: false,
            });
        }
        status = body.unwrap_or(Value::Null);
    }
    let state = state_of(&status).unwrap_or("inconnu");
    if state != "succeeded" && !cfg.allow_failure {
        let exit = status
            .get("exit_code")
            .map_or_else(|| "aucun".to_string(), ToString::to_string);
        let tail = status
            .get("log_tail")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(StepError {
            // `env` (secrets possibles) n'est jamais cité ici.
            message: format!(
                "job {job_id} : état « {state} », code de sortie {exit}, fin du log : {tail}"
            ),
            retryable: false,
        });
    }
    Ok(output_of(&status))
}

#[async_trait::async_trait]
impl MiryadWorkflowStep for ShellStep {
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    async fn run(
        &self,
        run: &EngineRunInfo,
        config: Value,
        _inputs: std::collections::HashMap<String, Value>,
    ) -> Result<Value, StepError> {
        let cfg = parse_config(&config)?;
        let info = RunInfo::from_config(&config)?;
        let access = SandboxAccess::open(&self.env, &info).await?;
        execute(&access, &info, run, &cfg).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::workflow::steps::tests::{config_with_run, run_ctx};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::State;
    use axum::response::IntoResponse;
    use std::collections::VecDeque;

    /// Réponse programmée d'une route du faux serveur de jobs : (statut, corps).
    type Reply = (u16, Value);

    #[derive(Clone)]
    struct JobsFake {
        post_reply: Arc<Mutex<VecDeque<Reply>>>,
        get_reply: Arc<Mutex<VecDeque<Reply>>>,
        post_bodies: Arc<Mutex<Vec<Value>>>,
        posts: Arc<AtomicUsize>,
        gets: Arc<AtomicUsize>,
        /// `POST` ayant rendu `201` — chaque 201 est un processus lancé par la sandbox.
        launches: Arc<AtomicUsize>,
    }

    /// Faux serveur d'API jobs (`/sandbox/src/jobs.sdd`) : sequences de réponses consommées,
    /// la dernière se répète.
    async fn spawn_jobs_fake(post_seq: Vec<Reply>, get_seq: Vec<Reply>) -> (String, JobsFake) {
        let fake = JobsFake {
            post_reply: Arc::new(Mutex::new(post_seq.into())),
            get_reply: Arc::new(Mutex::new(get_seq.into())),
            post_bodies: Arc::new(Mutex::new(Vec::new())),
            posts: Arc::new(AtomicUsize::new(0)),
            gets: Arc::new(AtomicUsize::new(0)),
            launches: Arc::new(AtomicUsize::new(0)),
        };
        let app = axum::Router::new()
            .route(
                "/jobs",
                axum::routing::post({
                    move |State(fake): State<JobsFake>, body: String| async move {
                        let body: Value =
                            serde_json::from_str(&body).unwrap_or(Value::String(body));
                        fake.posts.fetch_add(1, Ordering::SeqCst);
                        fake.post_bodies.lock().unwrap().push(body);
                        let (status, reply) = next(&fake.post_reply);
                        if status == 201 {
                            fake.launches.fetch_add(1, Ordering::SeqCst);
                        }
                        (
                            axum::http::StatusCode::from_u16(status).unwrap(),
                            axum::Json(reply),
                        )
                            .into_response()
                    }
                }),
            )
            .route(
                "/jobs/{*id}",
                axum::routing::get({
                    move |State(fake): State<JobsFake>| async move {
                        fake.gets.fetch_add(1, Ordering::SeqCst);
                        let (status, reply) = next(&fake.get_reply);
                        (
                            axum::http::StatusCode::from_u16(status).unwrap(),
                            axum::Json(reply),
                        )
                            .into_response()
                    }
                }),
            )
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), fake)
    }

    fn next(slot: &Arc<Mutex<VecDeque<Reply>>>) -> Reply {
        let mut q = slot.lock().unwrap();
        if q.len() > 1 {
            q.pop_front().unwrap()
        } else {
            q.front().cloned().unwrap_or((500, json!({})))
        }
    }

    fn status(
        state: &str,
        exit: Option<i64>,
        started: u64,
        ended: Option<u64>,
        tail: &str,
    ) -> Value {
        json!({
            "job_id": "ignored",
            "state": state,
            "exit_code": exit,
            "started_at": started,
            "ended_at": ended,
            "log_size": tail.len(),
            "log_tail": tail,
        })
    }

    fn engine(step_id: &str) -> EngineRunInfo {
        EngineRunInfo {
            run_key: "rk".to_string(),
            step_id: step_id.to_string(),
            depth: 0,
        }
    }

    async fn access_to(base: &str, run: &RunInfo) -> SandboxAccess {
        let locator = crate::workflow::steps::tests::FakeLocator::running("tok-1", base);
        SandboxAccess::open_with(locator, reqwest::Client::new(), run)
            .await
            .unwrap()
    }

    fn run_of(sandbox: Option<&str>) -> RunInfo {
        let cfg = config_with_run(&run_ctx(sandbox), serde_json::Map::new());
        RunInfo::from_config(&cfg).unwrap()
    }

    // ── Scenario: succes ────────────────────────────────────────────────────
    // `echo hi` : POST 201 running puis GET terminal succeeded — sortie `state`, `exit_code`,
    // `log_tail` contenant `hi`, `duration_ms`. Appoints `Handles` : `job_id` déterministe
    // `{run_id}:{step_id}`, aucun `callback` transmis, `timeout_secs` transmis tel quel.
    #[tokio::test]
    async fn succes() {
        let started = 1_700_000_000_u64;
        let (base, fake) = spawn_jobs_fake(
            vec![(201, status("running", None, started, None, ""))],
            vec![(
                200,
                status("succeeded", Some(0), started, Some(started + 3), "hi\n"),
            )],
        )
        .await;
        let run = run_of(Some("sbx-1"));
        let access = access_to(&base, &run).await;
        let cfg = ShellConfig {
            script: "echo hi".to_string(),
            cwd: Some("sub".to_string()),
            env: None,
            timeout_secs: Some(7200),
            allow_failure: false,
        };

        let out = execute(&access, &run, &engine("build"), &cfg)
            .await
            .expect("un job succeeded rend une sortie");

        assert_eq!(out["state"], "succeeded");
        assert_eq!(out["exit_code"], 0);
        assert!(
            out["log_tail"].as_str().unwrap().contains("hi"),
            "log_tail : {out}"
        );
        assert_eq!(out["duration_ms"], 3000, "ended-started en millisecondes");

        // `Handles` : corps du POST contractuel — job_id `{run_id}:{step_id}`, script, cwd,
        // timeout tel quel, JAMAIS de callback.
        let posts = fake.post_bodies.lock().unwrap();
        let body = &posts[0];
        assert_eq!(
            body["job_id"],
            format!("{}:build", run.run_id),
            "job_id déterministe"
        );
        assert_eq!(body["script"], "echo hi");
        assert_eq!(body["cwd"], "sub");
        assert_eq!(body["timeout_secs"], 7200, "timeout transmis tel quel");
        assert!(
            body.get("callback").is_none(),
            "aucune URL de callback en v1 : {body}"
        );
    }

    // Appoint `Raises` : `timeout_secs` absent ⇒ la clé est absente (défaut 3 600 côté sandbox).
    #[tokio::test]
    async fn timeout_absent_non_transmis() {
        let started = 1_700_000_000_u64;
        let (base, fake) = spawn_jobs_fake(
            vec![(
                201,
                status("succeeded", Some(0), started, Some(started), ""),
            )],
            vec![],
        )
        .await;
        let run = run_of(Some("sbx-1"));
        let access = access_to(&base, &run).await;
        let cfg = ShellConfig {
            script: "true".to_string(),
            cwd: None,
            env: None,
            timeout_secs: None,
            allow_failure: false,
        };
        execute(&access, &run, &engine("a"), &cfg)
            .await
            .expect("terminal dès le POST");
        // terminal au POST ⇒ pas de polling (le POST idempotent tient lieu d'état).
        assert_eq!(fake.gets.load(Ordering::SeqCst), 0);
        let body = &fake.post_bodies.lock().unwrap()[0];
        assert!(
            body.get("timeout_secs").is_none(),
            "défaut côté sandbox : {body}"
        );
        assert!(body.get("cwd").is_none());
        assert!(body.get("env").is_none());
    }

    // ── Scenario: echec_est_une_erreur ──────────────────────────────────────
    // `exit 2`, `allow_failure` absent ⇒ erreur non retryable portant l'état, le code 2 et la
    // fin du log. `Must not` : `env` jamais exposé.
    #[tokio::test]
    async fn echec_est_une_erreur() {
        let started = 1_700_000_000_u64;
        let (base, _fake) = spawn_jobs_fake(
            vec![(201, status("running", None, started, None, ""))],
            vec![(
                200,
                status("failed", Some(2), started, Some(started + 1), "bye"),
            )],
        )
        .await;
        let run = run_of(Some("sbx-1"));
        let access = access_to(&base, &run).await;
        let mut secrets = std::collections::BTreeMap::new();
        secrets.insert("TOKEN".to_string(), "s3cr3t-de-workflow".to_string());
        let cfg = ShellConfig {
            script: "exit 2".to_string(),
            cwd: None,
            env: Some(secrets),
            timeout_secs: None,
            allow_failure: false,
        };

        let err = execute(&access, &run, &engine("build"), &cfg)
            .await
            .expect_err("un échec sans allow_failure est une erreur");
        assert!(
            !err.retryable,
            "état terminal non succeeded ⇒ non retryable : {err}"
        );
        assert!(err.message.contains("failed"), "l'état : {err}");
        assert!(err.message.contains('2'), "le code de sortie : {err}");
        assert!(err.message.contains("bye"), "la fin du log : {err}");
        assert!(
            !err.message.contains("s3cr3t-de-workflow"),
            "env jamais exposé : {err}"
        );
    }

    // ── Scenario: echec_tolere ──────────────────────────────────────────────
    #[tokio::test]
    async fn echec_tolere() {
        let started = 1_700_000_000_u64;
        let (base, _fake) = spawn_jobs_fake(
            vec![(201, status("running", None, started, None, ""))],
            vec![(
                200,
                status("failed", Some(2), started, Some(started + 1), "bye"),
            )],
        )
        .await;
        let run = run_of(Some("sbx-1"));
        let access = access_to(&base, &run).await;
        let cfg = ShellConfig {
            script: "exit 2".to_string(),
            cwd: None,
            env: None,
            timeout_secs: None,
            allow_failure: true,
        };

        let out = execute(&access, &run, &engine("build"), &cfg)
            .await
            .expect("allow_failure rend la sortie sans erreur");
        assert_eq!(out["state"], "failed");
        assert_eq!(out["exit_code"], 2);
    }

    // ── Scenario: rejeu_sans_double_execution ───────────────────────────────
    // Job connu de la sandbox : `POST /jobs` rend `200` (jamais un second 201 — jamais un
    // second processus), et le step ne relance aucun polling si l'état rendu est déjà terminal.
    // Deux exécutions (rejeu) avec le même `job_id` : un seul lancement.
    #[tokio::test]
    async fn rejeu_sans_double_execution() {
        let started = 1_700_000_000_u64;
        let terminal = status("succeeded", Some(0), started, Some(started + 1), "hi");
        let (base, fake) = spawn_jobs_fake(
            // POST : 1re exécution ⇒ 201 (lancement), rejeu ⇒ 200 (job connu).
            vec![
                (201, status("running", None, started, None, "")),
                (200, terminal.clone()),
            ],
            vec![(200, terminal.clone())],
        )
        .await;
        let run = run_of(Some("sbx-1"));
        let access = access_to(&base, &run).await;
        let cfg = ShellConfig {
            script: "echo hi".to_string(),
            cwd: None,
            env: None,
            timeout_secs: None,
            allow_failure: false,
        };

        let first = execute(&access, &run, &engine("build"), &cfg)
            .await
            .expect("première exécution");
        let gets_after_first = fake.gets.load(Ordering::SeqCst);
        let second = execute(&access, &run, &engine("build"), &cfg)
            .await
            .expect("le rejeu réussit");

        assert_eq!(first["state"], "succeeded");
        assert_eq!(second, first, "le rejeu rend la même sortie");
        assert_eq!(
            fake.launches.load(Ordering::SeqCst),
            1,
            "aucun second processus lancé"
        );
        assert_eq!(
            fake.posts.load(Ordering::SeqCst),
            2,
            "un POST par exécution"
        );
        assert_eq!(
            fake.gets.load(Ordering::SeqCst),
            gets_after_first,
            "état terminal dès le POST idempotent : aucun polling de rejeu"
        );
        let bodies = fake.post_bodies.lock().unwrap();
        assert_eq!(
            bodies[0]["job_id"], bodies[1]["job_id"],
            "job_id stable au rejeu"
        );
    }

    // ── Scenario: job_perdu ─────────────────────────────────────────────────
    // 404 pendant l'interrogation ⇒ la sandbox a perdu le job : erreur non retryable.
    #[tokio::test]
    async fn job_perdu() {
        let started = 1_700_000_000_u64;
        let (base, _fake) = spawn_jobs_fake(
            vec![(201, status("running", None, started, None, ""))],
            vec![(404, json!({"error": "not found"}))],
        )
        .await;
        let run = run_of(Some("sbx-1"));
        let access = access_to(&base, &run).await;
        let cfg = ShellConfig {
            script: "sleep 60".to_string(),
            cwd: None,
            env: None,
            timeout_secs: None,
            allow_failure: false,
        };

        let err = execute(&access, &run, &engine("build"), &cfg)
            .await
            .expect_err("404 en interrogation = job perdu");
        assert!(
            !err.retryable,
            "un job perdu ne revient pas en rejouant : {err}"
        );
        assert!(
            err.message.contains("job perdu"),
            "message contractuel : {err}"
        );
    }

    // Appoint `Raises` : job refusé par la sandbox (400 au démarrage) ⇒ non retryable.
    #[tokio::test]
    async fn job_refuse_400() {
        let (base, _fake) =
            spawn_jobs_fake(vec![(400, json!({"error": "job_id invalide"}))], vec![]).await;
        let run = run_of(Some("sbx-1"));
        let access = access_to(&base, &run).await;
        let cfg = ShellConfig {
            script: "echo hi".to_string(),
            cwd: None,
            env: None,
            timeout_secs: None,
            allow_failure: false,
        };
        let err = execute(&access, &run, &engine("a b"), &cfg)
            .await
            .expect_err("400 = refus");
        assert!(
            !err.retryable,
            "refus sandbox (400) ⇒ non retryable : {err}"
        );
    }

    // Appoint `Raises` : `script` vide ⇒ erreur de config non retryable, aucun appel.
    #[test]
    fn script_vide_refuse() {
        for script in ["", "   "] {
            let err =
                parse_config(&json!({"script": script})).expect_err("un script vide est refusé");
            assert!(!err.retryable, "config invalide : {err}");
        }
        let cfg = parse_config(&json!({"script": "make", "_run": "peu-importe"}))
            .expect("une config valide se parse");
        assert_eq!(cfg.script, "make");
        assert!(!cfg.allow_failure);
        assert_eq!(cfg.timeout_secs, None);
        // une config sans `script` est mal formée ⇒ non retryable.
        let err = parse_config(&json!({"cmd": "make"})).expect_err("script requis");
        assert!(!err.retryable);
    }

    // ── Scenario: refuse_sans_sandbox ───────────────────────────────────────
    // La garde vit dans `SandboxAccess::open` (le pas n'ouvre aucun accès sans `_run.sandbox`)
    // — verrouillée par `mod::tests::refus_sans_sandbox_dans_le_run` ; la version pas-à-pas du
    // kind : config + contexte valides, l'ouverture refuse et execute n'est jamais atteint.
    #[tokio::test]
    async fn refuse_sans_sandbox() {
        let cfg = config_with_run(
            &run_ctx(None),
            serde_json::map::Map::from_iter([("script".to_string(), json!("echo hi"))]),
        );
        // Le garde-fou observable hors cluster : `RunInfo` lit le contexte, et l'accès refuse.
        let info = RunInfo::from_config(&cfg).unwrap();
        assert!(info.sandbox.is_none());
        let locator =
            crate::workflow::steps::tests::FakeLocator::running("tok", "http://127.0.0.1:1");
        let err = SandboxAccess::open_with(locator, reqwest::Client::new(), &info)
            .await
            .expect_err("aucun accès sans sandbox dans le run");
        assert!(!err.retryable);
        assert!(
            err.message.contains("sandbox"),
            "message nommant la cause : {err}"
        );
    }
}
