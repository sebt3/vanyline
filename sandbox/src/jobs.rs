//! API de JOBS de la sandbox (spec : `./jobs.sdd`) : exécution durable de
//! scripts shell de longue durée dans le pod, pilotée par le moteur de
//! workflow. Contrat : démarrer (idempotent sur `job_id`), interroger, lire
//! les logs, annuler. L'état est EN MÉMOIRE seulement (un redémarrage du pod
//! tue les processus ; le worker voit alors un 404 et fait échouer le nœud).
//!
//! Exécution : `sh -c <script>` dans un groupe de processus dédié
//! (`process_group(0)`, mécanisme de la régression R8 de
//! `../../tools/src/command.rs`) ; stdout et stderr entrelacés dans UN seul
//! fichier `<tmp>/vanyline-jobs/<job_id>.log`, jamais bornés à l'écriture.
//! À l'annulation comme au timeout, c'est le GROUPE qui est tué (SIGTERM puis
//! SIGKILL après 5 s de grâce) — jamais le seul `sh`.
//!
//! Le callback d'awakeable Restate ne vise JAMAIS une URL fournie par le
//! client : seule `config.restate_ingress_url` est appelée. Le script, l'`env`
//! et les logs d'un job ne sont jamais journalisés.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path as PathExt, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::{Child, Command};

use crate::AppState;
use crate::config::Config;
use crate::tools_impl::confine_path;
use crate::venv::venv_overlay;

/// Timeout par défaut d'un job (`timeout_secs` absent) : 3 600 s (spec
/// `Handles`, plafond 24 h).
pub const DEFAULT_TIMEOUT_SECS: u64 = 3_600;

/// Plafond du `timeout_secs` fourni par l'appelant : 24 h, valeur écrasée
/// (jamais un refus — la liste `400` de la spec ne nomme que `script`/`cwd`/`job_id`).
pub const MAX_TIMEOUT_SECS: u64 = 24 * 3_600;

/// Plafond de jobs `running` simultanés : au-delà, `POST /jobs` rend 429.
pub const MAX_RUNNING_JOBS: usize = 4;

/// Nombre de jobs terminés conservés (le plus ancien est évincé avec son
/// fichier de log).
pub const MAX_FINISHED_KEPT: usize = 20;

/// Taille maximale d'une réponse de logs (défaut et plafond de `limit`).
pub const LOG_PAGE_MAX_BYTES: u64 = 64 * 1024;

/// Taille maximale du `log_tail` d'un `@JobStatus`.
pub const LOG_TAIL_MAX_BYTES: u64 = 4 * 1024;

/// Délai de grâce SIGTERM → SIGKILL du groupe (annulation et timeout).
pub const KILL_GRACE: Duration = Duration::from_secs(5);

/// Attente maximale d'un handler `DELETE` avant de rendre l'état terminal
/// (la mort du groupe est prouvée par la tâche d'attente ; au-delà de cette
/// borne — improbable depuis `KILL_GRACE` — l'état courant est rendu).
pub const CANCEL_WAIT_MAX: Duration = Duration::from_secs(8);

/// Longueur maximale acceptée pour un `job_id`.
pub const JOB_ID_MAX_LEN: usize = 128;

/// État d'un job (spec `@JobStatus.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Processus en cours d'exécution.
    Running,
    /// Terminé avec le code de sortie 0.
    Succeeded,
    /// Code de sortie ≠ 0 ou mort par signal.
    Failed,
    /// Échéance du `timeout_secs` atteinte : le groupe a été tué.
    TimedOut,
    /// Annulé (`DELETE`) : le groupe a été tué.
    Cancelled,
}

impl JobState {
    /// `true` si l'état est terminal (ni `running`, ni ré-annulable).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !matches!(self, JobState::Running)
    }
}

/// Vue publique d'un job (spec `@JobStatus`) : état + sortie bornée.
/// L'`env` du job n'y figure JAMAIS (peut porter des secrets de workflow).
#[derive(Debug, Clone, Serialize)]
pub struct JobStatus {
    /// Identifiant fourni par l'appelant (clé d'idempotence).
    pub job_id: String,
    /// État courant.
    pub state: JobState,
    /// Code de sortie (terminal ; `None` = mort par signal ou tué).
    pub exit_code: Option<i32>,
    /// Démarrage, secondes depuis l'epoch Unix.
    pub started_at: u64,
    /// Fin (terminal), secondes depuis l'epoch Unix.
    pub ended_at: Option<u64>,
    /// Taille totale courante du fichier de log (octets).
    pub log_size: u64,
    /// Derniers 4 KiB du log (UTF-8 lossy).
    pub log_tail: String,
}

/// Registre en mémoire des jobs du pod (spec `@Jobs`), partagé par `Arc` dans
/// `AppState` (même forme que `CodeHealth`, décision 2026-10-10). Les verrous
/// sont des `std::sync::Mutex` à section courte — jamais tenus à travers un
/// `await`.
// Le nom `Jobs` est posé par la spec (jobs.sdd `Exposes`, `@Jobs`).
#[allow(clippy::module_name_repetitions)]
#[derive(Clone)]
pub struct Jobs {
    inner: Arc<Inner>,
}

/// Chronologie des relances du callback d'awakeable : backoff exponentiel de
/// `base`, plafonné à `max`, abandon après `window` sans 2xx (spec `Handles`).
#[derive(Debug, Clone, Copy)]
pub struct CallbackTiming {
    /// Délai avant la première relance.
    pub base: Duration,
    /// Plafond du délai de relance.
    pub max: Duration,
    /// Fenêtre totale de relance (au-delà, échec définitif journalisé).
    pub window: Duration,
}

impl Default for CallbackTiming {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(250),
            max: Duration::from_secs(30),
            window: Duration::from_secs(600),
        }
    }
}

/// Enregistrement interne d'un job (jamais exposé tel quel).
#[derive(Clone)]
struct JobRecord {
    state: JobState,
    exit_code: Option<i32>,
    started_at: SystemTime,
    ended_at: Option<SystemTime>,
    /// pgid du groupe (`process_group(0)` ⇒ pgid == pid du `sh` leader) ;
    /// `None` pendant la fenêtre entre l'insertion et le spawn.
    pgid: Option<i32>,
    /// Annulation demandée par `DELETE` (la tâche d'attente publie l'état).
    cancel_requested: bool,
    /// Échéance du timeout écoulée (le tueur a agi ou agit).
    timed_out: bool,
    /// Awakeable Restate à résoudre à l'état terminal (`callback.awakeable_id`).
    awakeable_id: Option<String>,
    /// Ordre d'insertion (file FIFO de l'éviction des terminés).
    seq: u64,
}

struct Inner {
    config: Arc<Config>,
    registry: Mutex<HashMap<String, JobRecord>>,
    next_seq: AtomicU64,
    timing: CallbackTiming,
    /// Client HTTP des callbacks : pas de suivi de redirection (la destination
    /// est l'URL configurée, rien d'autre — spec `Must not`), timeout borné.
    http: Option<reqwest::Client>,
}

impl Jobs {
    /// Constructeur de production : chronologie de callback par défaut
    /// [`CallbackTiming::default`].
    #[must_use]
    pub fn new(config: Arc<Config>) -> Self {
        let timing = CallbackTiming::default();
        Self::with_callback_timing(config, timing.base, timing.max, timing.window)
    }

    /// Constructeur à backoff injecté : mêmes sémantiques que [`Jobs::new`],
    /// chronologie de callback contractée pour les tests d'intégration
    /// (observer 3 tentatives en millisecondes sans attendre les minutes
    /// contractuelles — cf. `../tests/jobs.rs`).
    #[must_use]
    pub fn with_callback_timing(
        config: Arc<Config>,
        base: Duration,
        max: Duration,
        window: Duration,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok();
        if http.is_none() {
            tracing::warn!("VNL-SBX-JOBS-008: HTTP callback client unavailable");
        }
        Self {
            inner: Arc::new(Inner {
                config,
                registry: Mutex::new(HashMap::new()),
                next_seq: AtomicU64::new(1),
                timing: CallbackTiming { base, max, window },
                http,
            }),
        }
    }

    /// Chemin du fichier de log d'un job : `<tmp>/vanyline-jobs/<job_id>.log`
    /// (jamais dans le worktree — spec `Handles`/`Must not`).
    #[must_use]
    pub fn log_path(job_id: &str) -> PathBuf {
        log_dir().join(format!("{job_id}.log"))
    }

    /// Garde d'accès au registre : une lock empoisonnée (panic dans une
    /// section qui ne fait qu'insérer/modifier des entrées indépendantes)
    /// garde une map exploitable — on récupère l'intérieur (précédent
    /// `FsFlushRequests`, `./lib.rs`).
    fn registry(&self) -> MutexGuard<'_, HashMap<String, JobRecord>> {
        self.inner
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Instantané `@JobStatus` d'un job connu (`None` = jamais lancé ou évincé).
    #[must_use]
    pub fn status(&self, job_id: &str) -> Option<JobStatus> {
        let record = self.registry().get(job_id).cloned();
        record.map(|record| Self::snapshot_of(job_id, &record))
    }

    /// Instantané `@JobStatus` depuis un enregistrement + l'état du fichier de
    /// log (lecture hors verrou : jamais de stat/IO sous le mutex).
    fn snapshot_of(job_id: &str, record: &JobRecord) -> JobStatus {
        let path = Self::log_path(job_id);
        let (log_size, log_tail) = read_log_size_and_tail(&path);
        JobStatus {
            job_id: job_id.to_string(),
            state: record.state,
            exit_code: record.exit_code,
            started_at: epoch_secs(record.started_at),
            ended_at: record.ended_at.map(epoch_secs),
            log_size,
            log_tail,
        }
    }

    /// Attente bornée de l'état terminal d'un job (handlers `DELETE`) : rend
    /// le dernier instantané connu (`None` si le job a disparu).
    async fn await_terminal(&self, job_id: &str, wait: Duration) -> Option<JobStatus> {
        let deadline = Instant::now().checked_add(wait);
        loop {
            let status = self.status(job_id);
            let terminal = status.as_ref().is_some_and(|s| s.state.is_terminal());
            let expired = deadline.is_none_or(|d| Instant::now() >= d);
            if terminal || expired || status.is_none() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Pose le pgid après le spawn ; rend `true` si une annulation a été
    /// demandée pendant la fenêtre pré-spawn (le superviseur doit tuer).
    fn mark_spawned(&self, job_id: &str, pgid: Option<i32>) -> bool {
        let mut reg = self.registry();
        match reg.get_mut(job_id) {
            Some(record) => {
                record.pgid = pgid;
                record.cancel_requested
            }
            None => false,
        }
    }

    /// Demande d'annulation : pose `cancel_requested` et rend le pgid courant
    /// (`None` si le job est inconnu ou déjà terminal — idempotent).
    fn request_cancel(&self, job_id: &str) -> (bool, Option<i32>) {
        let mut reg = self.registry();
        match reg.get_mut(job_id) {
            None => (false, None),
            Some(record) if record.state.is_terminal() => (true, None),
            Some(record) => {
                record.cancel_requested = true;
                (true, record.pgid)
            }
        }
    }

    /// Le job est-il encore `running` ? (watchdog SIGKILL).
    fn is_running(&self, job_id: &str) -> bool {
        self.registry()
            .get(job_id)
            .is_some_and(|r| r.state == JobState::Running)
    }

    /// Cause publiée du terme (annulation/timeout demandés).
    fn cause_flags(&self, job_id: &str) -> (bool, bool) {
        let reg = self.registry();
        match reg.get(job_id) {
            Some(record) => (record.cancel_requested, record.timed_out),
            None => (false, false),
        }
    }

    /// Marque l'échéance du timeout et rend le pgid à tuer.
    fn mark_timed_out(&self, job_id: &str) -> Option<i32> {
        let mut reg = self.registry();
        match reg.get_mut(job_id) {
            Some(record) => {
                record.timed_out = true;
                record.pgid
            }
            None => None,
        }
    }

    /// Publie l'état terminal (unique écriteur de la transition), évicte au
    ///-delà de [`MAX_FINISHED_KEPT`] (le plus ancien terminé, avec son
    /// fichier de log) et rend (instantané, awakeable éventuel).
    fn finish(
        &self,
        job_id: &str,
        state: JobState,
        exit_code: Option<i32>,
    ) -> (JobStatus, Option<String>) {
        let record = {
            let mut reg = self.registry();
            let Some(record) = reg.get_mut(job_id) else {
                return (
                    JobStatus {
                        job_id: job_id.to_string(),
                        state,
                        exit_code,
                        started_at: 0,
                        ended_at: Some(epoch_secs(SystemTime::now())),
                        log_size: 0,
                        log_tail: String::new(),
                    },
                    None,
                );
            };
            record.state = state;
            record.exit_code = exit_code;
            record.ended_at = Some(SystemTime::now());
            let awakeable = record.awakeable_id.clone();
            let cloned = record.clone();
            evict_finished(&mut reg);
            (cloned, awakeable)
        };
        let status = Self::snapshot_of(job_id, &record.0);
        (status, record.1)
    }

    /// Retire un job non démarré (échec de spawn après insertion).
    fn forget(&self, job_id: &str) {
        self.registry().remove(job_id);
    }
}

/// Éviction FIFO : au plus [`MAX_FINISHED_KEPT`] terminés conservés ; les plus
/// anciens (petit `seq`) sortent, fichier de log supprimé. Les `running` ne
/// sont jamais évincés.
fn evict_finished(reg: &mut HashMap<String, JobRecord>) {
    loop {
        let mut oldest: Option<(u64, String)> = None;
        let mut finished = 0usize;
        for (id, record) in reg.iter() {
            if record.state.is_terminal() {
                finished = finished.saturating_add(1);
                if oldest.as_ref().is_none_or(|(seq, _)| record.seq < *seq) {
                    oldest = Some((record.seq, id.clone()));
                }
            }
        }
        if finished <= MAX_FINISHED_KEPT {
            return;
        }
        let Some((_seq, id)) = oldest else {
            return;
        };
        reg.remove(&id);
        let path = Jobs::log_path(&id);
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("VNL-SBX-JOBS-009: removing evicted log failed: {e}");
        }
    }
}

fn log_dir() -> PathBuf {
    std::env::temp_dir().join("vanyline-jobs")
}

fn epoch_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Taille totale du log + dernier [`LOG_TAIL_MAX_BYTES`] (UTF-8 lossy, lecture
/// par seek). Fichier absent ou illisible : (0, "").
fn read_log_size_and_tail(path: &Path) -> (u64, String) {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let size = meta.len();
            let tail = read_log_slice(
                path,
                size.saturating_sub(LOG_TAIL_MAX_BYTES),
                LOG_TAIL_MAX_BYTES,
            )
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
            (size, tail)
        }
        Err(_) => (0, String::new()),
    }
}

/// tranche `[offset, offset+len[` du fichier de log (bornée par la taille
/// réelle ; fichier absent = vide).
fn read_log_slice(path: &Path, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    if offset >= size || len == 0 {
        return Ok(Vec::new());
    }
    let take = len.min(size.saturating_sub(offset));
    let mut buf = vec![0u8; usize::try_from(take).unwrap_or(usize::MAX)];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// Envoi `signal` (SIGTERM/SIGKILL) au GROUPE de processus `pgid`.
///
/// Mécanisme de la régression R8 de `../../tools/src/command.rs` :
/// `kill(2)` sur un pid négatif cible le groupe créé par `process_group(0)`
/// (pgid == pid du leader) — pas seulement le `sh`, mais tous ses descendants.
/// Best-effort : `ESRCH` (groupe déjà mort) ou toute autre erreur n'est pas
/// remontée, l'appelant attend la mort du processus de tête de toute façon.
fn signal_group(pgid: i32, signal: i32) {
    // SAFETY : kill(2) sur -pgid (groupe créé par process_group(0)) ; ne touche
    // aucun pointer. Précédent identique dans tools/src/command.rs (R8).
    #[allow(unsafe_code)]
    unsafe {
        libc::kill(pgid.wrapping_neg(), signal);
    }
}

/// Watchdog SIGTERM → SIGKILL : après [`KILL_GRACE`], si le job est encore
/// `running`, SIGKILL du groupe (le SIGTERM n'a pas suffi).
fn spawn_kill_watchdog(jobs: Arc<Jobs>, job_id: String, pgid: i32) {
    tokio::spawn(async move {
        tokio::time::sleep(KILL_GRACE).await;
        if jobs.is_running(&job_id) {
            tracing::debug!(
                job_id,
                "VNL-SBX-JOBS-010: SIGTERM insufficient, SIGKILL group"
            );
            signal_group(pgid, libc::SIGKILL);
        }
    });
}

/// pgid du leader d'après `Child::id()` (même bornage que command.rs : hors
/// plage POSIX, le kill serait sans objet).
fn child_pgid(child: &Child) -> Option<i32> {
    child.id().and_then(|pid| i32::try_from(pid).ok())
}

// ── Corps JSON des routes ────────────────────────────────────────────────────

/// Corps de `POST /jobs` (spec `Exposes`).
#[derive(Debug, Deserialize)]
pub struct CreateJobBody {
    job_id: String,
    script: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    callback: Option<CallbackBody>,
}

/// `callback` : l'awakeable est créé par l'appelant (step Restate durable) ;
/// seule l'URL configurée reçoit le resolve (spec `Must`).
#[derive(Debug, Deserialize)]
struct CallbackBody {
    awakeable_id: String,
}

/// Query de `GET /jobs/{job_id}/logs`.
#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
}

/// Query de `DELETE /jobs?prefix=`.
#[derive(Debug, Deserialize)]
pub struct PrefixQuery {
    #[serde(default)]
    prefix: Option<String>,
}

/// Réponse d'erreur JSON uniforme, message porteur d'un identifiant unique
/// (règle git du repo ; jamais le script, l'env ni les logs).
fn job_error(status: StatusCode, code: &str, detail: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": format!("{code}: {detail}") })),
    )
        .into_response()
}

/// `job_id` exploitable comme nom de fichier et clé de registre : non vide,
/// borné, sans séparateur de chemin, sans `..`, sans caractère de contrôle —
/// charset `[A-Za-z0-9._:-]` (les identifiants de run `<run_id>:<step>` de la
/// spec passent).
fn valid_job_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.len() <= JOB_ID_MAX_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
}

// ── Handlers (routeur protégé de ./lib.rs) ──────────────────────────────────

/// Validation d'amorçage d'un `POST /jobs` (aucune écriture en registre) :
/// `job_id` exploitable, `script` non vide, callback uniquement si l'URL
/// d'ingress Restate est configurée, `cwd` confiné et répertoire existant.
/// Rend le cwd résolu, ou la réponse 400 correspondante (spec `Exposes`).
fn validate_creation(state: &AppState, body: &CreateJobBody) -> Result<PathBuf, Box<Response>> {
    if !valid_job_id(&body.job_id) {
        return Err(Box::new(job_error(
            StatusCode::BAD_REQUEST,
            "VNL-SBX-JOBS-001",
            "invalid job_id (empty, too long, or outside [A-Za-z0-9._:-])",
        )));
    }
    if body.script.is_empty() {
        return Err(Box::new(job_error(
            StatusCode::BAD_REQUEST,
            "VNL-SBX-JOBS-002",
            "script must not be empty",
        )));
    }
    if body.callback.is_some() && state.config.restate_ingress_url.is_none() {
        return Err(Box::new(job_error(
            StatusCode::BAD_REQUEST,
            "VNL-SBX-JOBS-003",
            "callback requested but no restate ingress url is configured",
        )));
    }
    resolve_cwd(&state.config, body.cwd.as_deref()).map_err(|e| {
        Box::new(job_error(
            StatusCode::BAD_REQUEST,
            "VNL-SBX-JOBS-004",
            &format!("cwd refused: {e}"),
        ))
    })
}

/// `POST /jobs` — démarre un job (idempotent sur `job_id`) ou rejoue son état.
/// 201 + `@JobStatus` (nouveau), 200 + `@JobStatus` (rejeu — JAMAIS un second
/// processus), 400 (script vide, `job_id` invalide, `cwd` hors confinement,
/// callback sans `restate_ingress_url`), 429 (plafond de jobs simultanés).
pub async fn handle_create(
    State(state): State<AppState>,
    Json(body): Json<CreateJobBody>,
) -> Response {
    let jobs = Arc::clone(&state.jobs);
    let cwd = match validate_creation(&state, &body) {
        Ok(cwd) => cwd,
        Err(response) => return *response,
    };

    // Idempotence AVANT le plafond : un rejeu du même job_id passe toujours,
    // sans jamais lancer un second processus.
    let seq = {
        let mut reg = jobs.registry();
        if let Some(record) = reg.get(&body.job_id).cloned() {
            drop(reg);
            let status = Jobs::snapshot_of(&body.job_id, &record);
            return (StatusCode::OK, Json(status)).into_response();
        }
        if reg
            .values()
            .filter(|r| r.state == JobState::Running)
            .count()
            >= MAX_RUNNING_JOBS
        {
            return job_error(
                StatusCode::TOO_MANY_REQUESTS,
                "VNL-SBX-JOBS-005",
                "concurrent job limit reached",
            );
        }
        let seq = jobs.inner.next_seq.fetch_add(1, Ordering::Relaxed);
        reg.insert(
            body.job_id.clone(),
            JobRecord {
                state: JobState::Running,
                exit_code: None,
                started_at: SystemTime::now(),
                ended_at: None,
                pgid: None,
                cancel_requested: false,
                timed_out: false,
                awakeable_id: body.callback.as_ref().map(|c| c.awakeable_id.clone()),
                seq,
            },
        );
        seq
    };

    match spawn_job(&jobs, &body, &cwd) {
        Ok(child) => {
            let pgid = child_pgid(&child);
            // Fenêtre annulation-près-spawn : si DELETE est passé entre
            // l'insertion et ici, tuer maintenant (le superviseur attendrait
            // sinon un mort déjà réapré par le wait).
            let cancel_raced = jobs.mark_spawned(&body.job_id, pgid);
            if cancel_raced && let Some(pgid) = pgid {
                signal_group(pgid, libc::SIGTERM);
                spawn_kill_watchdog(Arc::clone(&jobs), body.job_id.clone(), pgid);
            }
            let timeout = Duration::from_secs(
                body.timeout_secs
                    .unwrap_or(DEFAULT_TIMEOUT_SECS)
                    .min(MAX_TIMEOUT_SECS),
            );
            tokio::spawn(supervise(
                Arc::clone(&jobs),
                body.job_id.clone(),
                child,
                timeout,
            ));
            let status = jobs
                .status(&body.job_id)
                .unwrap_or_else(|| placeholder_status(&body.job_id));
            (StatusCode::CREATED, Json(status)).into_response()
        }
        Err(e) => {
            jobs.forget(&body.job_id);
            let path = Jobs::log_path(&body.job_id);
            if let Err(remove_err) = std::fs::remove_file(&path)
                && remove_err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("VNL-SBX-JOBS-007: removing failed-job log failed: {remove_err}");
            }
            tracing::warn!(job_id = %body.job_id, seq, "VNL-SBX-JOBS-006: job spawn failed: {e}");
            job_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "VNL-SBX-JOBS-006",
                "job could not be started",
            )
        }
    }
}

/// cwd par défaut = racine de confinement ; sous-répertoire demandé confiné
/// par `confine_path` (`./tools_impl.sdd`) — jamais de chemin absolu hors
/// confinement ni d'évasion `..`. Le répertoire doit exister.
fn resolve_cwd(config: &Config, raw: Option<&str>) -> Result<PathBuf, String> {
    let resolved =
        confine_path(&config.sandbox_root, raw.unwrap_or("")).map_err(|e| e.to_string())?;
    if resolved.is_dir() {
        Ok(resolved)
    } else {
        Err(format!(
            "resolved path is not a directory: {}",
            resolved.display()
        ))
    }
}

/// Prépare le log (un fichier unique, stdout + stderr entrelacés via deux
/// `Stdio` sur la MÊME description de fichier), l'env (hérité, puis overlay
/// `.venv`, puis `env` du corps — additif, dernière valeur gagnante) et lance
/// `sh -c` dans un groupe de processus dédié.
fn spawn_job(jobs: &Jobs, body: &CreateJobBody, cwd: &Path) -> Result<Child, String> {
    let path = Jobs::log_path(&body.job_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating log dir: {e}"))?;
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| format!("opening log file: {e}"))?;
    // try_clone partage la DESCRIPTION du fichier : les deux fd partagent
    // l'offset → stdout et stderr s'écrivent entrelacés dans l'ordre réel.
    let stderr_file = log_file
        .try_clone()
        .map_err(|e| format!("cloning log file: {e}"))?;

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&body.script)
        .current_dir(cwd)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr_file))
        .kill_on_drop(true)
        .process_group(0);
    for (key, value) in venv_overlay(&jobs.inner.config.sandbox_root) {
        cmd.env(key, value);
    }
    if let Some(env) = &body.env {
        for (key, value) in env {
            cmd.env(key, value);
        }
    }
    cmd.spawn().map_err(|e| format!("spawning sh: {e}"))
}

/// Tâche d'attente d'un job : attend la sortie (bornée par le timeout, tueur
/// de groupe à l'échéance), publie l'état terminal, résout l'awakeable Restate
/// éventuel en tâche de fond.
async fn supervise(jobs: Arc<Jobs>, job_id: String, mut child: Child, timeout: Duration) {
    let outcome = tokio::time::timeout(timeout, child.wait()).await;
    let (state, exit_code) = match outcome {
        Ok(Ok(status)) => {
            let code = status.code();
            let (cancel, timed_out) = jobs.cause_flags(&job_id);
            if cancel {
                (JobState::Cancelled, code)
            } else if timed_out {
                (JobState::TimedOut, code)
            } else {
                match code {
                    Some(0) => (JobState::Succeeded, code),
                    other => (JobState::Failed, other),
                }
            }
        }
        Ok(Err(e)) => {
            tracing::warn!("VNL-SBX-JOBS-011: waiting on job failed: {e}");
            (JobState::Failed, None)
        }
        Err(_elapsed) => {
            // Échéance : tuer le groupe (SIGTERM puis SIGKILL de grâce) puis
            // ré-apprêter le leader.
            if let Some(pgid) = jobs.mark_timed_out(&job_id) {
                signal_group(pgid, libc::SIGTERM);
                spawn_kill_watchdog(Arc::clone(&jobs), job_id.clone(), pgid);
            }
            let code = child.wait().await.ok().and_then(|s| s.code());
            (JobState::TimedOut, code)
        }
    };
    let (status, awakeable) = jobs.finish(&job_id, state, exit_code);
    tracing::info!(job_id, state = ?status.state, "job finished");
    if let Some(awakeable_id) = awakeable {
        tokio::spawn(resolve_awakeable(Arc::clone(&jobs), awakeable_id, status));
    }
}

/// Callback d'awakeable (spec `Handles`) : `POST {restate_ingress_url}
/// /restate/awakeables/{id}/resolve` avec le `@JobStatus`, relances à backoff
/// exponentiel plafonné jusqu'à un 2xx ou l'épuisement de la fenêtre. La
/// destination n'est JAMAIS l'entrée de l'appelant.
async fn resolve_awakeable(jobs: Arc<Jobs>, awakeable_id: String, status: JobStatus) {
    let Some(base) = jobs.inner.config.restate_ingress_url.clone() else {
        return; // validé au POST : ne doit pas arriver ; refus silencieux sûr.
    };
    let Some(client) = jobs.inner.http.clone() else {
        tracing::warn!("VNL-SBX-JOBS-008: no HTTP client, awakeable callback abandoned");
        return;
    };
    let url = format!(
        "{}/restate/awakeables/{}/resolve",
        base.trim_end_matches('/'),
        encode_url_component(&awakeable_id)
    );
    let timing = jobs.inner.timing;
    let deadline = Instant::now().checked_add(timing.window);
    let mut delay = timing.base;
    loop {
        match client.post(&url).json(&status).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(job_id = %status.job_id, "awakeable resolved");
                return;
            }
            Ok(resp) => tracing::warn!(
                job_id = %status.job_id,
                status = resp.status().as_u16(),
                "VNL-SBX-JOBS-012: awakeable resolve failed, retrying"
            ),
            Err(e) => tracing::warn!(
                job_id = %status.job_id,
                "VNL-SBX-JOBS-012: awakeable resolve request failed: {e}"
            ),
        }
        let exhausted =
            deadline.is_none_or(|d| Instant::now().checked_add(delay).is_none_or(|n| n > d));
        if exhausted {
            tracing::warn!(
                job_id = %status.job_id,
                "VNL-SBX-JOBS-013: awakeable resolve window exhausted, worker falls back to polling"
            );
            return;
        }
        tokio::time::sleep(delay).await;
        delay = delay.saturating_mul(2).min(timing.max);
    }
}

/// Encodage minimal d'un composant d'URL (l'awakeable id vient de l'appelant
/// mais ne quitte jamais la destination configurée ; on encode quand même tout
/// caractère hors jeu sûr).
fn encode_url_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                out.push(char::from(byte));
            }
            other => {
                // write! en base 16 (fmt::Write : pas de String intermédiaire).
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

/// `GET /jobs/{job_id}` — 200 + `@JobStatus` ; 404 si inconnu (jamais lancé,
/// évincé, ou perdu à un redémarrage du pod).
pub async fn handle_get(
    State(state): State<AppState>,
    PathExt(job_id): PathExt<String>,
) -> Response {
    match state.jobs.status(&job_id) {
        Some(status) => (StatusCode::OK, Json(status)).into_response(),
        None => job_error(StatusCode::NOT_FOUND, "VNL-SBX-JOBS-014", "job not found"),
    }
}

/// `GET /jobs/{job_id}/logs?offset=<octets>&limit=<octets>` — 200, texte
/// (stdout + stderr entrelacés) à partir de `offset`, au plus `limit` octets
/// (défaut et plafond 64 KiB) ; l'en-tête `X-Log-Size` donne la taille totale
/// pour la lecture incrémentale.
pub async fn handle_logs(
    State(state): State<AppState>,
    PathExt(job_id): PathExt<String>,
    Query(query): Query<LogsQuery>,
) -> Response {
    if state.jobs.status(&job_id).is_none() {
        return job_error(StatusCode::NOT_FOUND, "VNL-SBX-JOBS-014", "job not found");
    }
    let offset = query.offset.unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(LOG_PAGE_MAX_BYTES)
        .min(LOG_PAGE_MAX_BYTES);
    let path = Jobs::log_path(&job_id);
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    let body = match read_log_slice(&path, offset, limit) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::warn!("VNL-SBX-JOBS-015: reading job log failed: {e}");
            return job_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "VNL-SBX-JOBS-015",
                "reading job log failed",
            );
        }
    };
    (
        StatusCode::OK,
        [
            ("content-type", "text/plain; charset=utf-8"),
            ("x-log-size", size.to_string().as_str()),
        ],
        body,
    )
        .into_response()
}

/// `DELETE /jobs/{job_id}` — annule : le GROUPE est tué (SIGTERM puis SIGKILL
/// après 5 s) ; 200 + `@JobStatus` `cancelled` ; idempotent sur un job déjà
/// terminé ; 404 si inconnu.
pub async fn handle_cancel(
    State(state): State<AppState>,
    PathExt(job_id): PathExt<String>,
) -> Response {
    let jobs = Arc::clone(&state.jobs);
    let (known, pgid) = jobs.request_cancel(&job_id);
    if !known {
        return job_error(StatusCode::NOT_FOUND, "VNL-SBX-JOBS-014", "job not found");
    }
    if let Some(pgid) = pgid {
        signal_group(pgid, libc::SIGTERM);
        spawn_kill_watchdog(Arc::clone(&jobs), job_id.clone(), pgid);
    }
    match jobs.await_terminal(&job_id, CANCEL_WAIT_MAX).await {
        Some(status) => (StatusCode::OK, Json(status)).into_response(),
        None => job_error(StatusCode::NOT_FOUND, "VNL-SBX-JOBS-014", "job not found"),
    }
}

/// `DELETE /jobs?prefix=<préfixe>` — annule tous les jobs `running` dont
/// l'identifiant commence par le préfixe (annulation d'un run : `<run_id>:`) ;
/// 200 + la liste des `@JobStatus` annulés ; préfixe vide ou absent : 400.
pub async fn handle_cancel_prefix(
    State(state): State<AppState>,
    Query(query): Query<PrefixQuery>,
) -> Response {
    let jobs = Arc::clone(&state.jobs);
    let prefix = query.prefix.unwrap_or_default();
    if prefix.is_empty() {
        return job_error(
            StatusCode::BAD_REQUEST,
            "VNL-SBX-JOBS-016",
            "prefix must not be empty",
        );
    }
    let targets: Vec<(String, Option<i32>)> = {
        let mut reg = jobs.registry();
        let mut out = Vec::new();
        let ids: Vec<String> = reg
            .keys()
            .filter(|id| id.starts_with(&prefix))
            .cloned()
            .collect();
        for id in ids {
            if let Some(record) = reg.get_mut(&id)
                && !record.state.is_terminal()
            {
                record.cancel_requested = true;
                out.push((id, record.pgid));
            }
        }
        out
    };
    for (id, pgid) in &targets {
        if let Some(pgid) = pgid {
            signal_group(*pgid, libc::SIGTERM);
            spawn_kill_watchdog(Arc::clone(&jobs), id.clone(), *pgid);
        }
    }
    let mut statuses = Vec::with_capacity(targets.len());
    for (id, _pgid) in &targets {
        if let Some(status) = jobs.await_terminal(id, CANCEL_WAIT_MAX).await {
            statuses.push(status);
        }
    }
    (StatusCode::OK, Json(statuses)).into_response()
}

/// `@JobStatus` de repli (le job vient d'être inséré : `status` doit le trouver ;
/// ce repli n'est atteint qu'en cas de retrait concurrent).
fn placeholder_status(job_id: &str) -> JobStatus {
    JobStatus {
        job_id: job_id.to_string(),
        state: JobState::Running,
        exit_code: None,
        started_at: epoch_secs(SystemTime::now()),
        ended_at: None,
        log_size: 0,
        log_tail: String::new(),
    }
}
