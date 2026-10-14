//! État de santé du code du pod sandbox, TOUJOURS disponible (spec :
//! `./health.sdd`).
//!
//! Ce fichier porte le cycle de vie de l'état — démarrage (premier calcul dès
//! que le worktree existe), déclencheurs (`notify_modified`), délai de calme
//! (debounce `health_cooldown_secs`), empreinte du worktree (HEAD + `git diff
//! HEAD` + non suivis), sondage périodique (`health_poll_secs`), un-seul-calcul
//! (jamais deux calculs concurrents, jamais d'annulation), refresh forcé et
//! base sticky — initialisée depuis `config.health_base_ref` (branche par
//! défaut du Project fournie par le controller, task sandbox.sdd) quand elle
//! est non vide, puis écrasée par `POST /code-health/refresh {"base": …}` —
//! et ses deux routes HTTP (`GET /code-health`,
//! `POST /code-health/refresh`), montées par `./lib.rs` sur le routeur protégé
//! par le jeton de l'Owner.
//!
//! Aucune logique de vérification ici : presets, parsing, code neuf et sorties
//! bornées viennent de `vanyline_health::runner` (les presets des langages
//! détectés sont ceux du crate ; `check_timeout`/`tail_bytes`/`max_parallel`
//! sont les valeurs usuelles contractées par `./health.sdd` `Handles`).
//!
//! Outil MCP `code_health` : déclaration et dispatch dans `./tools_impl.rs`
//! (branchés par `./mcp.rs`, lecture pure du snapshot — jamais de refresh).
//! Le câblage de `notify_modified` est fait dans les dispatchs d'écriture
//! `./tools_impl.rs`, les ops d'écriture `./ws/fs.rs` et les handlers git
//! (`checkout`, `merge`, `merge/abort`, succès seulement). Les diagnostics
//! LSP sont collectés AVANT les commandes et transmis au runner comme
//! `preliminary_issues` (lot H3a) : le collecteur est une fermeture
//! `PreliminarySource` injectée — `with_lsp` branche la collecte réelle
//! `lsp_preliminary_source` (surface `LspManager::all_cached_diagnostics`
//! de `./lsp.rs`), tandis que `new` et `with_timing` gardent un collecteur
//! vide `empty_preliminary` (les constructeurs existants des `AppState`
//! de test gardent leur sens).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command as TokioCommand;
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use vanyline_health::checks::Check;
use vanyline_health::report::{HealthReport, Issue, Severity};
use vanyline_health::runner::{
    DEFAULT_OUTPUT_CAP_BYTES, DEFAULT_TAIL_BYTES, RunOptions, run_checks,
};

use crate::AppState;
use crate::LspManager;
use crate::config::Config;
use crate::maint::languages_from_paths;
use crate::venv::venv_overlay;

/// Délai maximal d'une commande de vérification pendant un calcul de la
/// sandbox (outillage de compilation complet : 10 min, valeur figée — la
/// dérive de toolchain se voit par la vérification `errored`, pas par un
/// calcul infini qui affamerait le cycle).
const CHECK_TIMEOUT: Duration = Duration::from_secs(600);

/// Plafond de vérifications simultanées d'un calcul (défaut usuel du runner).
const MAX_PARALLEL: usize = 2;

/// Niveau de priorité des commandes (`nice -n 10`, décision 2026-10-04).
const NICENESS: i32 = 10;

/// Source des vérifications pour une sélection de langages. La production
/// passe `vanyline_health::checks::checks_for_languages` ; les tests
/// d'intégration injectent des presets factices (`/bin/echo`) sans toolchain.
pub type CheckSource = Arc<dyn Fn(&[String]) -> Vec<Check> + Send + Sync>;

/// Collecteur des diagnostics LSP déjà connus du pod, convertis en
/// [`Issue`] préliminaires (spec : `./health.sdd`, `Handles` — « les
/// diagnostics déjà connus des serveurs LSP du pod sont collectés AVANT les
/// commandes et transmis comme `preliminary_issues` », lot H3a). Fermure
/// synchrone sans `await` (jamais de verrou tenue à travers un point de
/// suspension), injectée comme [`CheckSource`] pour rester testable sans
/// serveur LSP réel. La production branche
/// [`lsp_preliminary_source`] (constructeur [`CodeHealth::with_lsp`]) ;
/// les tests et les constructeurs historiques gardent
/// [`empty_preliminary`] (comportement « cache vide » inchangé).
pub type PreliminarySource = Arc<dyn Fn() -> Vec<Issue> + Send + Sync>;

/// Phase de la machine d'état (spec : `./health.sdd`, `@HealthState`.phase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Un calcul est en cours ; `report` est le dernier terminé, `None` pendant
    /// le tout premier.
    Computing,
    /// Aucune modification depuis le dernier calcul terminé.
    Fresh,
    /// Modifications survenues depuis ; nouveau calcul planifié ou en cours.
    Stale,
}

/// État observable de la santé du code, rendu par `GET /code-health`
/// (spec : `./health.sdd`, `@HealthState`). Le `@HealthReport` est rendu tel que
/// produit par le crate, sans réinterprétation.
#[derive(Debug, Clone, Serialize)]
pub struct HealthState {
    /// Phase courante.
    pub phase: Phase,
    /// Dernier rapport terminé, `None` pendant le tout premier calcul.
    pub report: Option<HealthReport>,
    /// Millisecondes unix de la publication du dernier rapport.
    pub computed_at: Option<u64>,
    /// Millisecondes unix du passage à `stale` (aucune horodatage répété tant
    /// que l'état reste stale).
    pub stale_since: Option<u64>,
    /// Base sticky des recalculs (vue « code neuf »), `None` si inconnue.
    pub base: Option<String>,
}

/// Erreurs du handle `@CodeHealth` (spec : `./health.sdd`, `@HealthError`).
#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    /// Référence git de base invalide — refusée par la garde syntaxique (vide,
    /// drapeau, espace, NUL, retour ligne : jamais interpolée dans un shell) ou
    /// par `git rev-parse --verify`. Identifiant unique `VNL-SBX-CH-001`.
    #[error("VNL-SBX-CH-001: invalid git base ref: {value}")]
    InvalidRef {
        /// Valeur refusée.
        value: String,
    },
    /// `git rev-parse` non lançable : la base n'a pas pu être vérifiée.
    /// Identifiant unique `VNL-SBX-CH-002`.
    #[error("VNL-SBX-CH-002: cannot validate base ref: {reason}")]
    RefCheckUnavailable {
        /// Motif (erreur de spawn du binaire git).
        reason: String,
    },
}

/// Déclencheurs internes de la tâche de fond.
#[derive(Debug, Clone)]
enum Event {
    /// Modification notifiée (délai de calme puis calcul si l'empreinte bouge).
    Modified,
    /// Refresh forcé : recalcul immédiat, empreinte ignorée.
    Refresh,
}

/// Ce qui réveille la boucle au repos : un événement ou le tick de sondage.
enum Trigger {
    Event(Event),
    Poll,
}

/// Champs de l'état mutable (lock std, jamais tenue à travers un `await`).
#[derive(Debug)]
struct Fields {
    phase: Phase,
    report: Option<HealthReport>,
    computed_at: Option<u64>,
    stale_since: Option<u64>,
    /// Base sticky des recalculs, amorcée par `config.health_base_ref`
    /// (branche par défaut du Project fournie par le controller) si non vide.
    base: Option<String>,
    /// Sélection de langages fournie à `/refresh` (`None` = détection).
    languages_override: Option<Vec<String>>,
    /// Empreinte du worktree vue par le dernier calcul TERMINÉ (ou le dernier
    /// refus de recalcul) ; `None` = aucun cycle terminé.
    last_fingerprint: Option<String>,
    /// Un calcul est en cours.
    computing: bool,
    /// Une modification est survenue pendant le calcul en cours (le résultat
    /// sera publié puis un nouveau cycle suivra ; jamais d'annulation).
    dirty: bool,
    /// Au moins un cycle de calcul a été tenté (le premier calcul du démarrage
    /// n'a pas à repasser par le délai de calme).
    attempted: bool,
}

/// Cœur partagé de `Arc<CodeHealth>` : poignées immuables + état mutable +
/// channel des déclencheurs.
struct Inner {
    config: Arc<Config>,
    source: CheckSource,
    /// Collecteur des diagnostics LSP préliminaires (spec : `Handles`, lot
    /// H3a) — appelé AVANT `run_checks` à chaque calcul.
    preliminary: PreliminarySource,
    /// Délai de calme (`health_cooldown_secs`) — `ZERO` = recalcul immédiat.
    cooldown: Duration,
    /// Période de sondage (`health_poll_secs`) — `ZERO` = sondage désactivé.
    poll: Duration,
    tx: mpsc::UnboundedSender<Event>,
    rx: AsyncMutex<mpsc::UnboundedReceiver<Event>>,
    fields: std::sync::Mutex<Fields>,
    /// Compteur des répertoires d'out (`<tmp>/vanyline-health/<n>`).
    out_seq: AtomicU64,
    /// La tâche de fond n'est lancée qu'une fois par `CodeHealth` (plusieurs
    /// `build_app` sur le même état partagé, décision SOLDÉ health.sdd).
    spawned: AtomicBool,
}

/// État partagé de la santé du code du pod (spec : `./health.sdd`, `@CodeHealth`).
/// `Clone` = poignée bon marché sur le MÊME état (derrière `Arc`), jamais une
/// seconde machine d'état.
#[derive(Clone)]
pub struct CodeHealth {
    inner: Arc<Inner>,
}

impl CodeHealth {
    /// Poignée de production historique : source = presets réels du crate,
    /// délais pris dans `config` (`health_cooldown_secs`, `health_poll_secs`),
    /// et collecteur de diagnostics LSP VIDE — n'ayant pas d'`LspManager`, ce
    /// constructeur ne peut pas brancher la collecte réelle ; le constructeur
    /// de production qui la branche est [`CodeHealth::with_lsp`] (utilisé par
    /// `./main.rs`).
    #[must_use]
    pub fn new(config: Arc<Config>) -> Arc<Self> {
        let cooldown = Duration::from_secs(config.health_cooldown_secs);
        let poll = Duration::from_secs(config.health_poll_secs);
        Self::with_timing(
            config,
            Arc::new(vanyline_health::checks::checks_for_languages),
            cooldown,
            poll,
            empty_preliminary(),
        )
    }

    /// Poignée de production complète (lot H3a) : comme [`CodeHealth::new`]
    /// (presets réels, délais de `config`) PLUS la collecte réelle des
    /// diagnostics LSP du pod ([`lsp_preliminary_source`] sur `lsp`), versés
    /// au runner comme `preliminary_issues` avant le lancement des commandes
    /// (spec : `./health.sdd`, `Handles`).
    #[must_use]
    pub fn with_lsp(config: Arc<Config>, lsp: Arc<LspManager>) -> Arc<Self> {
        let cooldown = Duration::from_secs(config.health_cooldown_secs);
        let poll = Duration::from_secs(config.health_poll_secs);
        let preliminary = lsp_preliminary_source(Arc::clone(&config), lsp);
        Self::with_timing(
            config,
            Arc::new(vanyline_health::checks::checks_for_languages),
            cooldown,
            poll,
            preliminary,
        )
    }

    /// Constructeur explicite : horloges, source de vérifications et
    /// collecteur préliminaire injectés (les tests d'intégration y branchent
    /// des presets factices et des délais réduits — l'horloge simulée de
    /// tokio est incompatible avec les faux subprocesses du runner, cf.
    /// `../tests/health.rs` ; le collecteur réel se branche via
    /// [`lsp_preliminary_source`]).
    #[must_use]
    pub fn with_timing(
        config: Arc<Config>,
        source: CheckSource,
        cooldown: Duration,
        poll: Duration,
        preliminary: PreliminarySource,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        // Base initiale de la vue « code neuf » (spec `Handles`, task
        // sandbox.sdd) : la branche par défaut du Project fournie par le
        // controller (`config.health_base_ref`) agit comme une base sticky
        // posée avant le premier calcul ; vide = absente (même règle de
        // vacuité que le controller). Elle n'est PAS validée ici : une base
        // qui ne résout pas n'est pas fatale (le runner rend le check
        // `new-code` `errored`, Scenario `echec_du_diff` de runner.sdd).
        let initial_base = config
            .health_base_ref
            .as_deref()
            .filter(|base| !base.is_empty())
            .map(str::to_string);
        Arc::new(Self {
            inner: Arc::new(Inner {
                config,
                source,
                preliminary,
                cooldown,
                poll,
                tx,
                rx: AsyncMutex::new(rx),
                fields: std::sync::Mutex::new(Fields {
                    // L'état naît « computing » : la sandbox prépare
                    // l'information au démarrage (spec `Purpose`).
                    phase: Phase::Computing,
                    report: None,
                    computed_at: None,
                    stale_since: None,
                    base: initial_base,
                    languages_override: None,
                    last_fingerprint: None,
                    computing: false,
                    dirty: false,
                    attempted: false,
                }),
                out_seq: AtomicU64::new(0),
                spawned: AtomicBool::new(false),
            }),
        })
    }

    /// Notifie une modification du worktree (spec : `Handles`, déclencheurs) :
    /// la phase passe à `stale` (horodatée une seule fois), le minuteur du délai
    /// de calme est (re)armé par l'émission. Appelable hors async (sync).
    pub fn notify_modified(&self) {
        {
            let mut f = self.fields();
            if f.computing {
                // Pas d'annulation : le calcul en cours ira au bout, le
                // résultat sera marqué « modifié pendant » et un cycle suivra.
                f.dirty = true;
            }
            if f.phase != Phase::Stale {
                f.phase = Phase::Stale;
                f.stale_since = Some(now_millis());
            }
        }
        if self.inner.tx.send(Event::Modified).is_err() {
            tracing::warn!(
                "VNL-SBX-CH-004: health background task gone, modification not scheduled"
            );
        }
    }

    /// Snapshot instantané de l'état, sans jamais attendre la fin d'un calcul
    /// (spec : `Must`).
    #[must_use]
    pub fn snapshot(&self) -> HealthState {
        let f = self.fields();
        HealthState {
            phase: f.phase,
            report: f.report.clone(),
            computed_at: f.computed_at,
            stale_since: f.stale_since,
            base: f.base.clone(),
        }
    }

    /// Recalcul forcé (spec : `@CodeHealth::refresh`). `base` est un double
    /// `Option` (décidé 2026-10-14) : `None` (champ absent) laisse la base
    /// sticky inchangée ; `Some(None)` (`"base": null`) l'EFFACE (jamais
    /// refusé) ; `Some(Some(candidate))` est VALIDÉE (garde syntaxique puis
    /// `git rev-parse --verify`, jamais interpolée dans un shell) AVANT d'être
    /// posée comme sticky. `languages` remplace la détection pour les
    /// recalculs suivants. L'événement est planifié derrière un calcul
    /// éventuellement en cours (jamais deux à la fois).
    ///
    /// # Errors
    ///
    /// [`HealthError::InvalidRef`] si `base` est invalide (rien n'est modifié
    /// dans ce cas) ; [`HealthError::RefCheckUnavailable`] si `git` n'est pas
    /// lançable pour la vérifier.
    pub async fn refresh(
        &self,
        // health.sdd `Exposes` (décidé 2026-10-14) : le double `Option` EST le
        // contrat — absent = inchangé, `Some(None)` = effacement, `Some(Some)` = pose.
        #[allow(clippy::option_option)] base: Option<Option<String>>,
        languages: Option<Vec<String>>,
    ) -> Result<(), HealthError> {
        if let Some(Some(candidate)) = &base {
            self.validate_base(candidate).await?;
        }
        {
            let mut f = self.fields();
            if let Some(base) = base {
                f.base = base;
            }
            if languages.is_some() {
                f.languages_override = languages;
            }
        }
        if self.inner.tx.send(Event::Refresh).is_err() {
            tracing::warn!("VNL-SBX-CH-004: health background task gone, refresh not scheduled");
        }
        Ok(())
    }

    /// Lance la tâche de fond (premier calcul dès que le worktree existe, puis
    /// boucle événements + sondage). Idempotent : seul le premier appel spawn ;
    /// sans runtime tokio, ne spawn rien et le signale (les `build_app` de test
    /// hors runtime ne crashent pas).
    pub fn spawn_background(&self) {
        if self.inner.spawned.swap(true, Ordering::SeqCst) {
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            self.inner.spawned.store(false, Ordering::SeqCst);
            tracing::warn!("VNL-SBX-CH-005: no tokio runtime, health background task not started");
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            me.run_background().await;
        });
    }

    // ── interne ──────────────────────────────────────────────────────────────

    /// Lock des champs : mutex empoisonné = sections qui ne font qu'affecter
    /// des champs → intérieur récupéré, jamais de propagation (règle du crate).
    fn fields(&self) -> std::sync::MutexGuard<'_, Fields> {
        self.inner
            .fields
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Racine d'exécution = `config.sandbox_root` (le worktree).
    fn root(&self) -> &Path {
        &self.inner.config.sandbox_root
    }

    /// Boucle de fond : premier calcul si le worktree existe déjà, puis
    /// alternance attente (événements + tick de sondage) / cycle.
    async fn run_background(self) {
        let mut interval = if self.inner.poll.is_zero() {
            None
        } else {
            let mut iv = tokio::time::interval(self.inner.poll);
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Le premier tick d'un `interval` est immédiat : on le consomme
            // pour que le sondage ne se déclenche qu'après une période pleine.
            iv.tick().await;
            Some(iv)
        };

        // Premier calcul du démarrage, sans attendre ni bloqueur : si le
        // worktree n'existe pas encore (PVC en cours de montage), le sondage
        // (ou le premier événement) reprendra la main plus tard.
        if self.worktree_exists().await {
            self.cycle(Trigger::Event(Event::Refresh)).await;
        }

        loop {
            let trigger = {
                let mut rx = self.inner.rx.lock().await;
                tokio::select! {
                    event = rx.recv() => match event {
                        Some(event) => Trigger::Event(event),
                        None => return, // sender déposé : l'objet est mort
                    },
                    () = async {
                        match interval.as_mut() {
                            Some(iv) => {
                                iv.tick().await;
                            }
                            None => std::future::pending::<()>().await,
                        }
                    } => Trigger::Poll,
                }
            };
            self.cycle(trigger).await;
        }
    }

    /// Un cycle complet : délai de calme (sauf refresh), empreinte (sauf
    /// refresh), calcul. Jamais réentrant : seul appelant = la tâche de fond.
    async fn cycle(&self, trigger: Trigger) {
        let mut forced = matches!(trigger, Trigger::Event(Event::Refresh));

        if matches!(trigger, Trigger::Poll) {
            // Sondage : empreinte comparée à celle du dernier calcul terminé.
            let fingerprint = self.fingerprint().await;
            let same = {
                let f = self.fields();
                fingerprint.is_some() && fingerprint == f.last_fingerprint
            };
            if same {
                return;
            }
            if !self.worktree_exists().await {
                return;
            }
            self.mark_stale();
        }

        // Délai de calme : chaque notification (re)armé ; un refresh en attente
        // force la sortie immédiate. `recv` est annulable sans perte de message
        // (contrat du canal tokio) : la re-création du `timeout` à chaque
        // itération EST le re-armement du minuteur.
        if !forced && !self.inner.cooldown.is_zero() {
            loop {
                let outcome = {
                    let mut rx = self.inner.rx.lock().await;
                    tokio::time::timeout(self.inner.cooldown, rx.recv()).await
                };
                let calm_over = match outcome {
                    // Notification : le minuteur est re-armé pour un tour.
                    Ok(Some(Event::Modified)) => false,
                    Ok(Some(Event::Refresh)) => {
                        forced = true;
                        true
                    }
                    // Timeout écoulé sans nouvelle notification : délai de
                    // calme atteint. `Ok(None)` = sender déposé, fin du cycle.
                    Err(_) | Ok(None) => true,
                };
                if calm_over {
                    break;
                }
            }
        }

        // Garde d'empreinte (ignorée par le refresh forcé).
        let fingerprint = self.fingerprint().await;
        if !forced {
            let unchanged = {
                let f = self.fields();
                fingerprint.is_some() && fingerprint == f.last_fingerprint
            };
            if unchanged {
                self.mark_fresh();
                return;
            }
        }

        self.compute(fingerprint).await;
    }

    /// Le calcul lui-même : état `computing`, exécution du runner, publication
    /// du rapport porteur de l'empreinte vue. Jamais annulé, jamais concurrent.
    async fn compute(&self, fingerprint: Option<String>) {
        let (base, languages_override) = {
            let mut f = self.fields();
            f.computing = true;
            f.attempted = true;
            f.dirty = false;
            if f.phase != Phase::Stale {
                f.phase = Phase::Computing;
            }
            (f.base.clone(), f.languages_override.clone())
        };

        let languages = match languages_override {
            Some(languages) => languages,
            None => self.detect_languages().await,
        };
        let checks = (self.inner.source)(&languages);
        let unchecked_languages = vanyline_health::checks::unchecked_languages(&languages);
        // Diagnostics LSP préliminaires : collectés AVANT le lancement des
        // commandes (spec `Handles`, lot H3a). Appel synchrone : aucun verrou
        // des caches LSP n'est tenu à travers un `await` vers le runner.
        let preliminary_issues = (self.inner.preliminary)();
        let out_dir = std::env::temp_dir().join("vanyline-health").join(
            self.inner
                .out_seq
                .fetch_add(1, Ordering::Relaxed)
                .to_string(),
        );
        let options = RunOptions {
            root: self.inner.config.sandbox_root.clone(),
            out_dir,
            base,
            env: venv_overlay(&self.inner.config.sandbox_root),
            check_timeout: CHECK_TIMEOUT,
            tail_bytes: DEFAULT_TAIL_BYTES,
            unchecked_languages,
            max_parallel: MAX_PARALLEL,
            // Diagnostics LSP : collecte AVANT les commandes (spec `Handles`,
            // lot H3a) — le collecteur injecté (`lsp_preliminary_source` en
            // production, `empty_preliminary` ou factice en test) est appelé
            // ici, jamais le runner ne le déclenche lui-même.
            preliminary_issues,
            niceness: Some(NICENESS),
            output_cap_bytes: DEFAULT_OUTPUT_CAP_BYTES,
        };

        match run_checks(&options, &checks).await {
            Ok(report) => {
                let mut f = self.fields();
                f.report = Some(report);
                f.computed_at = Some(now_millis());
                f.last_fingerprint = fingerprint;
                f.computing = false;
                f.phase = if f.dirty { Phase::Stale } else { Phase::Fresh };
                if !f.dirty {
                    f.stale_since = None;
                }
            }
            Err(error) => {
                // InvalidRef est inaccessible ici (base validée au refresh) ;
                // ReportIo = rare. Échec : rien n'est publié et l'empreinte du
                // dernier calcul TERMINÉ n'est pas posée — le cycle suivant
                // sur la même empreinte retente (jamais de calcul « consommé »
                // par un échec). Fraîcheur honnête (Must : un rapport périmé
                // n'est jamais présenté comme fresh) : le rapport en place ne
                // peut être dit `fresh` que s'il porte exactement l'empreinte
                // vue par ce calcul et que rien n'a bougé depuis (`dirty`).
                tracing::warn!("VNL-SBX-CH-006: health computation failed: {error}");
                let mut f = self.fields();
                f.computing = false;
                let rapport_courant = fingerprint.is_some()
                    && f.report.is_some()
                    && f.last_fingerprint == fingerprint;
                if rapport_courant && !f.dirty {
                    f.phase = Phase::Fresh;
                    f.stale_since = None;
                } else if f.phase != Phase::Stale {
                    f.phase = Phase::Stale;
                    f.stale_since = Some(now_millis());
                }
            }
        }
    }

    /// La phase repasse à `fresh` (empreinte identique : rien n'a bougé).
    fn mark_fresh(&self) {
        let mut f = self.fields();
        if f.computing {
            return;
        }
        f.phase = Phase::Fresh;
        f.stale_since = None;
    }

    /// L'état passe (ou reste) `stale`, horodaté une seule fois.
    fn mark_stale(&self) {
        let mut f = self.fields();
        if f.phase != Phase::Stale {
            f.phase = Phase::Stale;
            f.stale_since = Some(now_millis());
        }
    }

    /// Le worktree existe (le pod l'a monté/créé) : `<sandbox_root>/.git`
    /// présent. C'est le signal « prêt pour le premier calcul ».
    async fn worktree_exists(&self) -> bool {
        tokio::fs::try_exists(self.root().join(".git"))
            .await
            .unwrap_or(false)
    }

    /// Empreinte du worktree : `HEAD` + contenu du `git diff HEAD` + liste des
    /// fichiers non suivis (spec : `Handles`, empreinte). Trois git en argv
    /// (jamais de shell) ; `None` si l'un échoue (hors repo, zéro commit) :
    /// absence d'empreinte ⇒ pas de refus de recalcul sur cette base.
    async fn fingerprint(&self) -> Option<String> {
        let head = self.git(&["rev-parse", "HEAD"]).await.ok()?;
        let diff = self
            .git(&["-c", "core.quotepath=off", "diff", "HEAD"])
            .await
            .ok()?;
        let untracked = self
            .git(&["ls-files", "--others", "--exclude-standard"])
            .await
            .ok()?;
        Some(format!(
            "head\x1f{head}\x1fdiff\x1f{diff}\x1funtracked\x1f{untracked}"
        ))
    }

    /// Langages du worktree : `git ls-files` (suivis + non suivis hors ignores)
    /// puis mapping pur partagé avec `vanyline-maint detect`
    /// ([`languages_from_paths`], décision 2026-10-04).
    async fn detect_languages(&self) -> Vec<String> {
        let paths = match self
            .git(&["ls-files", "--cached", "--others", "--exclude-standard"])
            .await
        {
            Ok(stdout) => stdout
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect::<Vec<String>>(),
            Err(error) => {
                tracing::warn!("VNL-SBX-CH-007: language detection skipped: {error:?}");
                Vec::new()
            }
        };
        languages_from_paths(&paths)
    }

    /// Valide une `base` avant tout usage (spec : `base_invalide_refusee`) :
    /// garde syntaxique sévère (vide, `-` initial, espace, NUL, retour ligne —
    /// la même que le runner, qui protège du drapeau injecté) puis existence
    /// réelle par `git rev-parse --verify` en argv (jamais interpolée dans un
    /// shell, jamais de commande du client).
    async fn validate_base(&self, base: &str) -> Result<(), HealthError> {
        if base.is_empty()
            || base.starts_with('-')
            || base.contains(' ')
            || base.contains('\0')
            || base.contains('\n')
        {
            return Err(HealthError::InvalidRef {
                value: base.to_string(),
            });
        }
        match self.git(&["rev-parse", "--verify", base]).await {
            Ok(_) => Ok(()),
            Err(GitRun::Spawn(reason)) => Err(HealthError::RefCheckUnavailable { reason }),
            Err(GitRun::Failed(stderr)) => {
                tracing::debug!("VNL-SBX-CH-001: git rejected base ref {base}: {stderr}");
                Err(HealthError::InvalidRef {
                    value: base.to_string(),
                })
            }
        }
    }

    /// Lance un git en argv dans la racine (motif async du runner, jamais de
    /// shell) ; sortie stdout perdue ou stderr trimé.
    async fn git(&self, args: &[&str]) -> Result<String, GitRun> {
        let output = TokioCommand::new("git")
            .args(args)
            .current_dir(self.root())
            .output()
            .await
            .map_err(|source| GitRun::Spawn(source.to_string()))?;
        if !output.status.success() {
            return Err(GitRun::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

/// Échec d'un git interne (distingue « binaire non lançable » de « exit ≠ 0 »).
#[derive(Debug)]
enum GitRun {
    Spawn(String),
    Failed(String),
}

/// Collecteur vide : aucun diagnostic préliminaire (comportement historique
/// de [`CodeHealth::new`] et des `with_timing` de test sans collecte LSP —
/// `preliminary_issues` rendu vide, donc aucun résultat `lsp` au rapport).
#[must_use]
pub fn empty_preliminary() -> PreliminarySource {
    let none: fn() -> Vec<Issue> = Vec::new;
    Arc::new(none)
}

/// Collecteur réel des diagnostics LSP du pod (spec : `./health.sdd`,
/// `Handles`, lot H3a) : snapshot [`LspManager::all_cached_diagnostics`]
/// (dernier `publishDiagnostics` par URI, `./lsp.rs`), converti en [`Issue`]
/// par [`lsp_issue`] — URI hors `sandbox_root` ou non-`file://` ignorées.
/// Fermure synchrone : le lock des caches est pris et rendu sans aucun
/// `await` (`./lsp.sdd`).
#[must_use]
pub fn lsp_preliminary_source(config: Arc<Config>, lsp: Arc<LspManager>) -> PreliminarySource {
    Arc::new(move || {
        let root = config.sandbox_root.as_path(); // emprunt valable jusqu'au
        // `collect` : l'emprunt est Copié dans chaque fermeture intérieure.
        lsp.all_cached_diagnostics()
            .into_iter()
            .flat_map(|(uri, diagnostics)| {
                diagnostics
                    .into_iter()
                    .filter_map(move |diagnostic| lsp_issue(root, &uri, &diagnostic))
            })
            .collect()
    })
}

/// Convertit un diagnostic LSP publié sur `uri` en [`Issue`] préliminaire
/// (spec : `./health.sdd` `Handles`, lot H3a ; `health/src/report.sdd`
/// @Issue). Forme consommée : le diagnostic `PublishDiagnostics` du LSP
/// (`range.start.line`/`character` 0-based, `severity` 1..4, `code` chaîne
/// ou nombre, `message`). Mapping @Issue : `check_id` `lsp` (le résultat du
/// runner), `path` RELATIF à la racine, `line`/`column` 1-based (règle du
/// crate health), `severity` : 1 ⇒ `error`, toute autre valeur (2/3/4,
/// absente) ⇒ `warning` (@Severity n'a que deux niveaux). Ignoré, sans
/// erreur : URI non-`file://` ou hors racine (aucun chemin relatif
/// possible — `report.sdd` `Must not` « chemin absolu »), `message` ou
/// `range.start` absents, position non numérique.
///
/// Règle URI→chemin RÉUTILISÉE de `display_path_for_uri` (`./tools_impl.rs`) :
/// chemin relatif = URI amputée de son préfixe `file://{sandbox_root}/`
/// (`sandbox_root` rendu sans slash final) ; tout ce qui ne porte pas ce
/// préfixe est hors contrat de rendu — ici ignoré (le rendu brut de
/// `tools_impl` n'a pas de sens dans un @Issue).
fn lsp_issue(root: &Path, uri: &str, diagnostic: &Value) -> Option<Issue> {
    let path = uri.strip_prefix(&format!("file://{}/", root.display()))?;
    if path.is_empty() {
        return None; // l'URI de la racine elle-même : aucun fichier derrière.
    }
    let message = diagnostic.get("message").and_then(Value::as_str)?;
    let start = diagnostic.get("range")?.get("start")?;
    let line = u32::try_from(start.get("line")?.as_u64()?)
        .ok()?
        .saturating_add(1);
    let column = u32::try_from(start.get("character")?.as_u64()?)
        .ok()?
        .saturating_add(1);
    let severity = if diagnostic.get("severity").and_then(Value::as_u64) == Some(1) {
        Severity::Error
    } else {
        Severity::Warning
    };
    let code = diagnostic.get("code").and_then(|code| {
        code.as_str()
            .map(str::to_string)
            .or_else(|| code.as_i64().map(|n| n.to_string()))
    });
    Some(Issue {
        check_id: "lsp".to_string(),
        path: path.to_string(),
        line,
        column,
        severity,
        code,
        message: message.to_string(),
    })
}

// ── Handlers HTTP (routes protégées du routeur de ./lib.rs) ──────────────────

/// Corps optionnel de `POST /code-health/refresh` :
/// `{ "base": <ref git>?, "languages": [<nom>]? }`.
///
/// `base` est un double `Option` (décidé 2026-10-14, health.sdd `Exposes`) :
/// champ ABSENT (`None`) = base sticky inchangée ; `"base": null`
/// (`Some(None)`) = EFFACE la base sticky (jamais refusé) ; `"base": <ref>`
/// (`Some(Some(ref))`) = la ref devient la base sticky des recalculs suivants.
/// `serde_json` effondre un `null` explicite sur le `None` externe d'un
/// `Option<Option<…>>` standard (vérifié expérimentalement) : le champ porte
/// donc [`sticky_base`], qui ne court que lorsque le champ est PRÉSENT.
#[derive(Debug, Deserialize)]
struct RefreshBody {
    // health.sdd `Exposes` (décidé 2026-10-14) : le double `Option` EST le
    // contrat (absent = inchangé, null = effacement, `<ref>` = pose).
    #[allow(clippy::option_option)]
    #[serde(default, deserialize_with = "sticky_base")]
    base: Option<Option<String>>,
    languages: Option<Vec<String>>,
}

/// Désérialisation `base` du double `Option` : seulement appelée quand le
/// champ est présent (`#[serde(default)]` couvre l'absence → `None`).
/// `null` → `Some(None)` (effacement) ; chaîne → `Some(Some(ref))` (pose) ;
/// tout autre type JSON → erreur de corps (`VNL-SBX-CH-003`, 400).
#[allow(clippy::option_option)] // health.sdd `Exposes` (décidé 2026-10-14) : type de contrat.
fn sticky_base<'de, D>(deserializer: D) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    match serde_json::Value::deserialize(deserializer)? {
        Value::Null => Ok(Some(None)),
        Value::String(value) => Ok(Some(Some(value))),
        other => Err(D::Error::custom(format!(
            "`base` must be a git ref string or null, got {other}"
        ))),
    }
}

/// `GET /code-health` — 200 + `@HealthState`, TOUJOURS (jamais 404, y compris
/// pendant le tout premier calcul : `phase: "computing"`, `report: null`).
pub async fn handle_get(State(state): State<AppState>) -> axum::Json<HealthState> {
    axum::Json(state.health.snapshot())
}

/// `POST /code-health/refresh` — 202 (recalcul immédiat, replanifié derrière un
/// éventuel calcul en cours) ; 400 si `base` invalide (`@HealthError::InvalidRef`,
/// base sticky inchangée) ; 500 si `git` n'est pas lançable pour la vérifier
/// (`@HealthError::RefCheckUnavailable`, cause serveur — ratifié 2026-10-14 :
/// 400 reste réservé aux entrées client invalides).
pub async fn handle_refresh(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let parsed: RefreshBody = if body.is_empty() {
        RefreshBody {
            base: None,
            languages: None,
        }
    } else {
        match serde_json::from_slice(&body) {
            Ok(parsed) => parsed,
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "error": format!("VNL-SBX-CH-003: invalid JSON body: {error}")
                    })),
                )
                    .into_response();
            }
        }
    };
    match state.health.refresh(parsed.base, parsed.languages).await {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        // 400 = entrées client invalides ; 500 = cause serveur (git non
        // lançable, VNL-SBX-CH-002) — ratifié 2026-10-14 (health.sdd `Raises`).
        Err(error @ HealthError::InvalidRef { .. }) => (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
        Err(error @ HealthError::RefCheckUnavailable { .. }) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

/// Millisecondes unix (horodatages d'état seulement — les minuteurs du cycle
/// vivent dans `tokio::time`, horloge réelle).
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        std::path::PathBuf::from("/workspace")
    }

    /// Le volet LSP du Scenario `diagnostics_lsp_transmis_au_calcul` (côté
    /// conversion, lot H3a) : un diagnostic `PublishDiagnostics` sur une URI
    /// `file://` sous la racine rend une [`Issue`] — `check_id` `lsp`, chemin
    /// RELATIF à la racine, position 1-based (LSP est 0-based, `report.sdd`
    /// est 1-based), severité LSP 1 ⇒ `error`, code conservé en chaîne.
    #[test]
    fn lsp_issue_maps_relative_path_1based_and_error() {
        let diag = serde_json::json!({
            "range": {
                "start": { "line": 2, "character": 4 },
                "end": { "line": 2, "character": 9 }
            },
            "severity": 1,
            "code": "E0308",
            "message": "mismatched types"
        });
        let issue = lsp_issue(&root(), "file:///workspace/src/a.rs", &diag).expect("issue rendue");
        assert_eq!(issue.check_id, "lsp");
        assert_eq!(issue.path, "src/a.rs");
        assert_eq!(issue.line, 3, "ligne LSP 2 ⇒ 1-based 3");
        assert_eq!(issue.column, 5, "character LSP 4 ⇒ 1-based 5");
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.code.as_deref(), Some("E0308"));
        assert_eq!(issue.message, "mismatched types");
    }

    /// Severités LSP 2 (Warning), 3 (Information), 4 (Hint) et absente ⇒
    /// `warning` (@Severity n'a que deux niveaux ; jamais d'erreur inventée) ;
    /// code numérique rendu en chaîne.
    #[test]
    fn lsp_issue_maps_other_severities_to_warning_and_numeric_code() {
        for severity in [2u64, 3, 4] {
            let diag = serde_json::json!({
                "range": { "start": { "line": 0, "character": 0 },
                           "end": { "line": 0, "character": 1 } },
                "severity": severity,
                "code": 42,
                "message": "soft"
            });
            let issue = lsp_issue(&root(), "file:///workspace/a.rs", &diag).expect("issue rendue");
            assert_eq!(issue.severity, Severity::Warning, "severity LSP {severity}");
            assert_eq!(issue.code.as_deref(), Some("42"));
            assert_eq!(issue.line, 1);
            assert_eq!(issue.column, 1);
        }
        let no_sev = serde_json::json!({
            "range": { "start": { "line": 0, "character": 0 },
                       "end": { "line": 0, "character": 1 } },
            "message": "no severity"
        });
        let issue = lsp_issue(&root(), "file:///workspace/a.rs", &no_sev).expect("issue rendue");
        assert_eq!(issue.severity, Severity::Warning);
        assert_eq!(issue.code, None);
    }

    /// Diagnostic hors contrat de rendu : sans `message` exploitable ⇒ ignoré
    /// (pas d'issue vide au rapport).
    #[test]
    fn lsp_issue_without_message_is_ignored() {
        let diag = serde_json::json!({
            "range": { "start": { "line": 0, "character": 0 },
                       "end": { "line": 0, "character": 1 } },
            "severity": 1
        });
        assert!(lsp_issue(&root(), "file:///workspace/a.rs", &diag).is_none());
    }

    /// URI non-`file://` ou hors `sandbox_root` ⇒ ignorées, sans erreur (spec :
    /// « diagnostics sans URI file:// ou hors root : ignorés » — ils ne
    /// peuvent être rendus en chemin relatif au dépôt, `report.sdd` `Must`).
    #[test]
    fn lsp_issue_ignores_non_file_and_outside_root() {
        let diag = serde_json::json!({
            "range": { "start": { "line": 0, "character": 0 },
                       "end": { "line": 0, "character": 1 } },
            "severity": 1,
            "message": "x"
        });
        assert!(lsp_issue(&root(), "untitled:untitled-1", &diag).is_none());
        assert!(lsp_issue(&root(), "file:///elsewhere/lib.rs", &diag).is_none());
        // URI de la racine elle-même (aucun fichier derrière) : ignorée.
        assert!(lsp_issue(&root(), "file:///workspace/", &diag).is_none());
    }

    /// Cache vide ⇒ vec vide (comportement historique préservé) : le
    /// collecteur réel sur un manager sans aucune session ne verse rien et ne
    /// produit aucune erreur.
    #[test]
    fn lsp_preliminary_source_empty_manager_rend_empty() {
        let config = Arc::new(crate::auth::test_support::test_config(Path::new(
            "/workspace",
        )));
        let source = lsp_preliminary_source(config, Arc::new(LspManager::default()));
        assert!(source().is_empty(), "manager sans session ⇒ rien à verser");
    }
}
