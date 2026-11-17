//! Ossature du TUI ratatui du chat interactif — contrat complet : `./mod.sdd`.
//!
//! Initialisation/restauration du terminal ([`TerminalGuard`]), boucle
//! d'événements `tokio::select!` (clavier, événements de session lymaille, fin
//! de tour, horloge 100 ms). Le pilotage de la session passe par les seams
//! [`TurnDriver`], [`SessionHost`] et [`DirtyFiles`] : la boucle ne voit ni le
//! `TurnRunner` ni le cluster, et se teste sans LLM. Seul ce fichier fait de
//! l'I/O terminal ; l'état ([`state`]) et la vue ([`view`]) sont purs. Aucune
//! sortie stdout/stderr pendant l'écran alternatif.

pub mod editor;
pub mod markdown;
pub mod sink;
pub mod state;
pub mod view;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::BoxFuture;
use lymaille::{CancelHandle, Event, Outcome};
use tokio::sync::mpsc;

use crate::host::session_index::SessionEntry;
use crate::session_open::Resume;
use crate::tui::state::{Action, Effect, TodoItem, TranscriptItem};

/// Délai accordé à la tâche de tour pour finir (sauvegarde lymaille) à la sortie.
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Marqueur d'un tour dont la tâche s'est terminée sans résultat (panique, fin anticipée).
const INTERROMPU: &str = "VNL-INT-001";

/// Erreur du chemin TUI : I/O terminal ou backend ratatui en échec.
#[derive(Debug, Clone)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

/// Un tour lancé : son handle d'annulation et son issue.
pub struct TurnStart {
    /// Annule ce tour seul (la session reste ouverte).
    pub cancel: CancelHandle,
    /// Issue du tour : `Ok(outcome)` ou erreur lisible.
    pub run: BoxFuture<'static, Result<Outcome, String>>,
}

/// Pilotage du tour de la session (seam : le TUI ne connaît pas le `TurnRunner`).
pub trait TurnDriver: Send + Sync {
    /// Lance un tour sur `text`. Le tour rend son issue, jamais un panic traité ici.
    fn begin_turn(&self, text: String) -> TurnStart;
    /// Injecte `text` dans le tour en cours (entrée utilisateur de lymaille).
    fn inject(&self, text: String);
    /// Relit la liste de tâches de la session (attend la fin d'un tour en cours).
    fn todo(&self) -> BoxFuture<'static, Result<Vec<TodoItem>, String>>;
    /// Annule la session entière (handle racine) à la sortie.
    fn cancel_session(&self);
}

/// Une session ouverte, prête pour le TUI.
pub struct Opened {
    /// Pilotage des tours.
    pub driver: Arc<dyn TurnDriver>,
    /// Flux des événements de la session (sink de lymaille).
    pub events: mpsc::UnboundedReceiver<Event>,
    /// Identifiant de la session.
    pub id: String,
    /// Agent du contexte.
    pub agent: String,
    /// Titre de la session.
    pub title: Option<String>,
    /// Historique à afficher (reprise), dans l'ordre.
    pub transcript: Vec<TranscriptItem>,
    /// Notices d'ouverture (reprise sans session, restauration).
    pub notices: Vec<String>,
}

/// Ouverture et listage des sessions (`/new`, `/sessions`, reprise).
pub trait SessionHost: Send + Sync {
    /// Ouvre une session neuve, la dernière ou une précise.
    fn open(&self, resume: Resume) -> BoxFuture<'static, Result<Opened, String>>;
    /// Sessions du contexte, la plus récente d'abord.
    fn list(&self) -> BoxFuture<'static, Result<Vec<SessionEntry>, String>>;
}

/// Nombre de fichiers modifiés du worktree de la sandbox (`GET /git/status`).
pub trait DirtyFiles: Send + Sync {
    /// Compte les fichiers modifiés ; une erreur est une notice, jamais bloquante.
    fn count(&self) -> BoxFuture<'static, Result<usize, String>>;
}

/// Options du TUI : souris, worktree de la sandbox, ouverture des sessions, contexte.
pub struct TuiOptions {
    /// Capture de la molette (activée en dur par la CLI).
    pub mouse: bool,
    /// Worktree de la sandbox (`--sandbox`) ; `None` sans sandbox.
    pub sandbox: Option<Arc<dyn DirtyFiles>>,
    /// Ouverture et listage des sessions.
    pub host: Arc<dyn SessionHost>,
    /// Projet du contexte (ligne finale de reprise).
    pub context: String,
}

/// Issue du TUI : sortie normale ou arrêt par signal (`130`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiExit {
    /// Sortie normale (Ctrl+D, `/quit`).
    Normal,
    /// Arrêt par SIGTERM ou SIGHUP.
    Signal,
}

/// Actions de terminal du [`TerminalGuard`] — abstraction pour tester la
/// restauration (mode raw, écran alternatif, souris) sans vrai tty.
pub trait TerminalActions: Send + Sync {
    /// Active mode raw + écran alternatif (+ souris si `mouse`).
    fn enter(&self, mouse: bool) -> Result<(), std::io::Error>;
    /// Restaure : désactive souris, écran alternatif, mode raw — dans cet ordre.
    fn leave(&self, mouse: bool) -> Result<(), std::io::Error>;
}

/// Actions réelles via crossterm (tty du processus).
struct CrosstermActions;

impl TerminalActions for CrosstermActions {
    fn enter(&self, mouse: bool) -> Result<(), std::io::Error> {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
        if mouse {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
        }
        Ok(())
    }

    fn leave(&self, mouse: bool) -> Result<(), std::io::Error> {
        if mouse {
            crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
        }
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
        crossterm::terminal::disable_raw_mode()?;
        Ok(())
    }
}

/// Garde RAII : active le terminal à la création, le RESTAURE dans `Drop`,
/// y compris sur panique (le hook installé par [`run_tui`] appelle
/// [`TerminalGuard::restore`] avant tout message) et sur SIGTERM/SIGHUP
/// (boucle de [`run_tui`]). La restauration est idempotente (drapeau atomique).
pub struct TerminalGuard {
    actions: Arc<dyn TerminalActions>,
    mouse: bool,
    left: Arc<AtomicBool>,
}

impl TerminalGuard {
    /// Applique les actions d'entrée ; en échec, aucune garde n'est créée.
    pub fn enter(actions: Arc<dyn TerminalActions>, mouse: bool) -> Result<Self, CliError> {
        actions
            .enter(mouse)
            .map_err(|e| CliError(format!("terminal: {e}")))?;
        Ok(Self {
            actions,
            mouse,
            left: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Drapeau « déjà restauré » partagé avec le hook de panique.
    pub fn left_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.left)
    }

    /// Restaure immédiatement et de façon idempotente.
    pub fn restore(actions: &Arc<dyn TerminalActions>, mouse: bool, left: &AtomicBool) {
        if !left.swap(true, Ordering::SeqCst) {
            actions.leave(mouse).ok();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        TerminalGuard::restore(&self.actions, self.mouse, &self.left);
    }
}

/// Terminal réel du TUI (backend crossterm sur stdout).
type TuiTerminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>;

/// Signaux d'arrêt externe (SIGTERM/SIGHUP) ; `None` si l'enregistrement a échoué.
#[cfg(unix)]
type Signals = Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)>;
#[cfg(not(unix))]
type Signals = Option<((), ())>;

/// Garde de boucle « un seul tour à la fois » : une seule tâche de tour active.
#[derive(Default)]
struct TurnSlot {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl TurnSlot {
    /// Un tour est-il actif ?
    #[must_use]
    fn busy(&self) -> bool {
        self.handle.is_some()
    }

    /// Démarre la tâche de tour ; `false` si un tour est déjà actif (refus).
    fn start(&mut self, task: BoxFuture<'static, ()>) -> bool {
        if self.busy() {
            return false;
        }
        self.handle = Some(tokio::spawn(task));
        true
    }

    /// Libère la garde quand le résultat du tour est arrivé.
    fn finish(&mut self) {
        self.handle = None;
    }

    /// Abandonne la tâche de test (sans attendre).
    #[cfg(test)]
    fn cancel_for_test(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    /// Attend la fin du tour (après annulation) au plus [`DRAIN_TIMEOUT`], pour
    /// que lymaille sauvegarde le tour annulé avant la sortie ; au-delà, abandon.
    async fn drain(&mut self) {
        if let Some(handle) = self.handle.take() {
            let abort = handle.abort_handle();
            if tokio::time::timeout(DRAIN_TIMEOUT, handle).await.is_err() {
                abort.abort();
            }
        }
    }

    /// Polling de la garde au tick de 100 ms : si la tâche est TERMINÉE alors que
    /// la garde est prise (panique, fin anticipée), la consomme et rend le
    /// `JoinError` ; `None` sinon (aucun await bloquant).
    #[must_use]
    async fn take_finished(&mut self) -> Option<Result<(), tokio::task::JoinError>> {
        if !self
            .handle
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            return None;
        }
        let handle = self.handle.take()?;
        Some(handle.await)
    }
}

/// Installe le hook de panique qui RESTAURE le terminal AVANT tout message
/// puis enchaîne l'ancien hook.
fn install_panic_hook(actions: Arc<dyn TerminalActions>, mouse: bool, left: Arc<AtomicBool>) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        TerminalGuard::restore(&actions, mouse, &left);
        prev(info);
    }));
}

/// Message du tour interrompu sans résultat (`VNL-INT-001`).
fn interrompu(reason: &str) -> String {
    format!(
        "{INTERROMPU}: tour interrompu : la tâche de tour s'est terminée sans résultat ({reason})"
    )
}

/// Résultat d'une ouverture, transmis à la boucle par le canal interne.
enum Internal {
    /// Action asynchrone ordinaire.
    Action(Action),
    /// Session ouverte (`/new`, `/sessions`) ou erreur d'ouverture.
    Opened(Result<Opened, String>),
}

/// État de la boucle d'événements hors `AppState` : pilotage et effets.
struct Loop {
    /// Modèle pur du chat.
    state: state::AppState,
    /// Pilotage de la session courante.
    driver: Arc<dyn TurnDriver>,
    /// Ouverture et listage des sessions.
    host: Arc<dyn SessionHost>,
    /// Worktree de la sandbox, s'il y en a un.
    dirty: Option<Arc<dyn DirtyFiles>>,
    /// Projet du contexte (ligne finale).
    context: String,
    /// Canal interne des actions asynchrones.
    internal_tx: mpsc::UnboundedSender<Internal>,
    /// Canal de fin de tour.
    turn_tx: mpsc::UnboundedSender<Result<Outcome, String>>,
    /// Garde « un seul tour à la fois ».
    slot: TurnSlot,
    /// Handle d'annulation du tour en cours.
    turn_cancel: Option<CancelHandle>,
    /// Flux d'événements d'une session fraîchement ouverte, à adopter par la boucle.
    next_events: Option<mpsc::UnboundedReceiver<Event>>,
    /// Effet `Quit` reçu : la boucle s'arrête.
    quit: bool,
}

impl Loop {
    /// Boucle sur `state`, `driver`, `host` et les canaux donnés.
    fn new(
        state: state::AppState,
        driver: Arc<dyn TurnDriver>,
        host: Arc<dyn SessionHost>,
        dirty: Option<Arc<dyn DirtyFiles>>,
        context: String,
        internal_tx: mpsc::UnboundedSender<Internal>,
        turn_tx: mpsc::UnboundedSender<Result<Outcome, String>>,
    ) -> Self {
        Self {
            state,
            driver,
            host,
            dirty,
            context,
            internal_tx,
            turn_tx,
            slot: TurnSlot::default(),
            turn_cancel: None,
            next_events: None,
            quit: false,
        }
    }

    /// Applique une action puis exécute ses effets.
    fn step(&mut self, action: Action) {
        let (next, effects) = state::update(std::mem::take(&mut self.state), action);
        self.state = next;
        for effect in effects {
            self.execute(effect);
        }
    }

    /// Exécute un effet produit par la mise à jour pure.
    fn execute(&mut self, effect: Effect) {
        match effect {
            Effect::StartTurn(text) => self.start_turn(text),
            Effect::InjectInput(text) => self.driver.inject(text),
            Effect::CancelTurn => {
                if let Some(cancel) = &self.turn_cancel {
                    cancel.cancel();
                }
            }
            Effect::Quit => {
                if let Some(cancel) = self.turn_cancel.take() {
                    cancel.cancel();
                }
                self.driver.cancel_session();
                self.quit = true;
            }
            Effect::RefreshTodo => self.refresh_todo(),
            Effect::RefreshDirtyFiles => self.refresh_dirty(),
            // [?] Le registre des runs des sous-agents n'est pas exposé par `session_open` :
            // le sondage reste sans effet tant que la session ne le porte pas.
            Effect::PollRuns => {}
            Effect::NewSession => self.reopen(Resume::New),
            Effect::ResumeSession(id) => self.reopen(Resume::Id(id)),
            Effect::ListSessions => self.list_sessions(),
        }
    }

    /// Lance le tour : le résultat arrive par `turn_tx`, jamais par un prêt de la boucle.
    fn start_turn(&mut self, text: String) {
        let start = self.driver.begin_turn(text);
        self.turn_cancel = Some(start.cancel);
        let tx = self.turn_tx.clone();
        let run = start.run;
        let started = self.slot.start(Box::pin(async move {
            let outcome = run.await;
            let _ = tx.send(outcome);
        }));
        if !started {
            tracing::warn!("second tour ignoré : une tâche de tour est déjà active");
        }
    }

    /// Relit la liste de tâches hors de la boucle.
    fn refresh_todo(&self) {
        let fut = self.driver.todo();
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let action = match fut.await {
                Ok(items) => Action::Todo(items),
                Err(message) => Action::Warning(message),
            };
            let _ = tx.send(Internal::Action(action));
        });
    }

    /// Rafraîchit le nombre de fichiers modifiés (sans sandbox : rien).
    fn refresh_dirty(&self) {
        let Some(dirty) = self.dirty.clone() else {
            return;
        };
        let fut = dirty.count();
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            match fut.await {
                Ok(n) => {
                    let _ = tx.send(Internal::Action(Action::DirtyFiles(Some(n))));
                }
                Err(message) => {
                    let _ = tx.send(Internal::Action(Action::DirtyFiles(None)));
                    let _ = tx.send(Internal::Action(Action::Warning(format!(
                        "état du worktree indisponible : {message}"
                    ))));
                }
            }
        });
    }

    /// Liste les sessions du contexte pour le sélecteur.
    fn list_sessions(&self) {
        let fut = self.host.list();
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let action = match fut.await {
                Ok(list) => Action::SessionList(list),
                Err(message) => Action::Warning(message),
            };
            let _ = tx.send(Internal::Action(action));
        });
    }

    /// Ouvre une autre session (`/new`, reprise). Refusé pendant un tour.
    fn reopen(&mut self, resume: Resume) {
        if self.slot.busy() {
            self.state.transcript.push(TranscriptItem::Notice {
                level: state::NoticeLevel::Warn,
                text: "tour en cours — patientez".to_string(),
            });
            return;
        }
        let fut = self.host.open(resume);
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Internal::Opened(fut.await));
        });
    }

    /// Adopte une session ouverte : transcription, identifiant, flux d'événements.
    fn adopt(&mut self, opened: Opened) {
        let Opened {
            driver,
            events,
            id,
            agent,
            title,
            transcript,
            notices,
        } = opened;
        self.driver = driver;
        self.next_events = Some(events);
        self.turn_cancel = None;
        let mut next = state::AppState::new(agent, id, title, self.state.size);
        next.input = std::mem::take(&mut self.state.input);
        next.todo_visible = self.state.todo_visible;
        next.transcript = transcript;
        for text in notices {
            next.transcript.push(TranscriptItem::Notice {
                level: state::NoticeLevel::Info,
                text,
            });
        }
        next.revision = self.state.revision.wrapping_add(1);
        self.state = next;
        self.refresh_dirty();
    }

    /// Rend l'état dans le terminal (`view::render` pur).
    fn draw(&self, terminal: &mut TuiTerminal) -> Result<(), CliError> {
        let state = &self.state;
        terminal
            .draw(|frame| view::render(frame, state))
            .map_err(|e| CliError(format!("rendu: {e}")))?;
        Ok(())
    }
}

/// Entrée du mode interactif, appelée par `crate::chat` (stdin ET stdout sont
/// alors garantis terminaux). Restaure le terminal dans tous les cas de sortie.
// Une seule boucle par contrat (mod.sdd §Boucle) : la longueur est le contrat.
#[allow(clippy::too_many_lines)]
pub async fn run_tui(opened: Opened, options: TuiOptions) -> Result<TuiExit, CliError> {
    let mouse = options.mouse;
    let actions: Arc<dyn TerminalActions> = Arc::new(CrosstermActions);
    let guard = TerminalGuard::enter(Arc::clone(&actions), mouse)?;
    install_panic_hook(Arc::clone(&actions), mouse, guard.left_flag());

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal =
        ratatui::Terminal::new(backend).map_err(|e| CliError(format!("terminal: {e}")))?;
    let size = terminal
        .size()
        .map_err(|e| CliError(format!("taille du terminal: {e}")))?;

    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<Internal>();
    let (turn_tx, mut turn_rx) = mpsc::unbounded_channel::<Result<Outcome, String>>();

    let Opened {
        driver,
        events,
        id,
        agent,
        title,
        transcript,
        notices,
    } = opened;
    let mut events_rx = events;
    let mut events_open = true;
    let initial = state::AppState::new(agent, id, title, (size.width, size.height));
    let mut lp = Loop::new(
        initial,
        driver,
        options.host,
        options.sandbox,
        options.context,
        internal_tx,
        turn_tx,
    );
    lp.state.transcript = transcript;
    for text in notices {
        lp.state.transcript.push(TranscriptItem::Notice {
            level: state::NoticeLevel::Info,
            text,
        });
    }
    lp.refresh_dirty();
    lp.draw(&mut terminal)?;

    let mut keys = crossterm::event::EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
    let mut gate = RenderGate::new(std::time::Duration::from_millis(33));
    #[cfg(unix)]
    let mut signals: Signals = {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::terminate())
            .ok()
            .zip(signal(SignalKind::hangup()).ok())
    };
    #[cfg(not(unix))]
    let mut signals: Signals = None;
    let mut exit = TuiExit::Normal;

    while !lp.quit {
        let action = tokio::select! {
            maybe_event = futures::StreamExt::next(&mut keys) => {
                match maybe_event {
                    Some(Ok(crossterm::event::Event::Key(key)))
                        if key.kind == crossterm::event::KeyEventKind::Press
                            || key.kind == crossterm::event::KeyEventKind::Repeat =>
                    {
                        Some(Action::Key(key))
                    }
                    Some(Ok(crossterm::event::Event::Paste(text))) => Some(Action::Paste(text)),
                    Some(Ok(crossterm::event::Event::Resize(w, h))) => {
                        terminal
                            .resize(ratatui::layout::Size::new(w, h).into())
                            .map_err(|e| CliError(format!("redimension: {e}")))?;
                        Some(Action::Resize(w, h))
                    }
                    Some(Ok(crossterm::event::Event::Mouse(mouse_ev))) => {
                        Some(Action::Mouse(mouse_ev.kind))
                    }
                    Some(Ok(_)) => None,
                    Some(Err(e)) => {
                        return Err(CliError(format!("flux d'événements: {e}")));
                    }
                    None => {
                        lp.quit = true;
                        None
                    }
                }
            }
            maybe_session = events_rx.recv(), if events_open => {
                if let Some(event) = maybe_session {
                    Some(Action::Session(event))
                } else {
                    events_open = false;
                    None
                }
            }
            maybe_internal = internal_rx.recv() => match maybe_internal {
                Some(Internal::Action(action)) => Some(action),
                Some(Internal::Opened(Ok(opened))) => {
                    lp.adopt(opened);
                    None
                }
                Some(Internal::Opened(Err(message))) => Some(Action::Warning(message)),
                None => None,
            },
            maybe_outcome = turn_rx.recv() => {
                lp.slot.finish();
                lp.turn_cancel = None;
                maybe_outcome.map(Action::TurnFinished)
            }
            _ = ticker.tick() => {
                // Panique de la tâche de tour : une tâche terminée alors que la
                // garde est prise est consommée ici (`VNL-INT-001`).
                if let Some(join) = lp.slot.take_finished().await {
                    lp.turn_cancel = None;
                    if let Ok(outcome) = turn_rx.try_recv() {
                        lp.step(Action::TurnFinished(outcome));
                    } else {
                        let reason = match join {
                            Ok(()) => "fin anticipée".to_string(),
                            Err(e) => e.to_string(),
                        };
                        lp.step(Action::TurnFinished(Err(interrompu(&reason))));
                    }
                }
                Some(Action::Tick)
            }
            () = async {
                match signals.as_mut() {
                    #[cfg(unix)]
                    Some((term, hup)) => {
                        tokio::select! {
                            Some(()) = term.recv() => {}
                            Some(()) = hup.recv() => {}
                        }
                    }
                    _ => std::future::pending::<()>().await,
                }
            } => {
                exit = TuiExit::Signal;
                lp.quit = true;
                None
            }
        };
        if let Some(action) = action {
            lp.step(action);
        }
        if let Some(rx) = lp.next_events.take() {
            events_rx = rx;
            events_open = true;
        }
        if lp.quit {
            break;
        }
        if gate.allow(std::time::Instant::now()) {
            lp.draw(&mut terminal)?;
        }
    }
    // Sortie : le tour en cours est annulé et laissé finir (sauvegarde lymaille),
    // puis le terminal est restauré AVANT la ligne finale.
    if let Some(cancel) = lp.turn_cancel.take() {
        cancel.cancel();
    }
    lp.slot.drain().await;
    drop(guard);
    let mut out = std::io::stdout();
    let trace = format!(
        "session: {} — reprendre avec `vanyline chat --context {} --resume {}`\n",
        lp.state.session_id, lp.context, lp.state.session_id
    );
    let _ = std::io::Write::write_all(&mut out, trace.as_bytes());
    let _ = std::io::Write::flush(&mut out);
    Ok(exit)
}

/// Passerelle de rendu : au plus un rendu par intervalle (fusion des événements rapprochés).
#[derive(Debug, Clone)]
pub(crate) struct RenderGate {
    min: std::time::Duration,
    last: Option<std::time::Instant>,
}

impl RenderGate {
    pub(crate) fn new(min: std::time::Duration) -> Self {
        Self { min, last: None }
    }

    /// `true` si un rendu est autorisé à `now` (et le consomme).
    pub(crate) fn allow(&mut self, now: std::time::Instant) -> bool {
        match self.last {
            None => {
                self.last = Some(now);
                true
            }
            Some(prev) if now.saturating_duration_since(prev) >= self.min => {
                self.last = Some(now);
                true
            }
            Some(_) => false,
        }
    }
}

/// Nombre de fichiers modifiés lu dans la réponse de `GET /git/status`
/// (`{"branch":…,"files":[…],"clean":…}`).
///
/// # Errors
///
/// Returns an explicit message when the body is not the expected JSON.
pub fn dirty_count(body: &str) -> Result<usize, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("réponse illisible : {e}"))?;
    value
        .get("files")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| "réponse sans liste de fichiers".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::Mutex;

    /// Actions enregistrées dans un ordre observable (test de restauration).
    #[derive(Default)]
    struct RecordingActions {
        events: Mutex<Vec<String>>,
    }

    impl RecordingActions {
        fn log(&self, e: &str) {
            if let Ok(mut v) = self.events.lock() {
                v.push(e.to_string());
            }
        }
        fn snapshot(&self) -> Vec<String> {
            self.events.lock().map(|v| v.clone()).unwrap_or_default()
        }
    }

    impl TerminalActions for RecordingActions {
        fn enter(&self, _mouse: bool) -> Result<(), std::io::Error> {
            self.log("enter");
            Ok(())
        }
        fn leave(&self, _mouse: bool) -> Result<(), std::io::Error> {
            self.log("leave");
            Ok(())
        }
    }

    /// Pilotage factice : le tour dure jusqu'à son annulation, les injections sont notées.
    #[derive(Default)]
    struct FakeDriver {
        injected: Mutex<Vec<String>>,
        cancelled: std::sync::atomic::AtomicBool,
    }

    impl TurnDriver for FakeDriver {
        fn begin_turn(&self, _text: String) -> TurnStart {
            let cancel = CancelHandle::new();
            let waiter = cancel.clone();
            TurnStart {
                cancel,
                run: Box::pin(async move {
                    waiter.cancelled().await;
                    Ok(Outcome::Cancelled)
                }),
            }
        }
        fn inject(&self, text: String) {
            if let Ok(mut v) = self.injected.lock() {
                v.push(text);
            }
        }
        fn todo(&self) -> BoxFuture<'static, Result<Vec<TodoItem>, String>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn cancel_session(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
        }
    }

    /// Hôte factice : aucune session ouvrable dans ces tests.
    struct FakeHost;

    impl SessionHost for FakeHost {
        fn open(&self, _resume: Resume) -> BoxFuture<'static, Result<Opened, String>> {
            Box::pin(async { Err("non utilisé".to_string()) })
        }
        fn list(&self) -> BoxFuture<'static, Result<Vec<SessionEntry>, String>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    type Channels = (
        mpsc::UnboundedReceiver<Internal>,
        mpsc::UnboundedReceiver<Result<Outcome, String>>,
    );

    fn boucle(driver: Arc<FakeDriver>) -> (Loop, Channels) {
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let (turn_tx, turn_rx) = mpsc::unbounded_channel();
        let st = state::AppState::new("a1", "session-1".to_string(), None, (100, 30));
        let lp = Loop::new(
            st,
            driver,
            Arc::new(FakeHost),
            None,
            "projet".to_string(),
            internal_tx,
            turn_tx,
        );
        (lp, (internal_rx, turn_rx))
    }

    fn touche(code: KeyCode, mods: KeyModifiers) -> Action {
        Action::Key(KeyEvent::new(code, mods))
    }

    /// Scenario: `terminal_restaure_apres_sortie_normale` — Ctrl-D sur saisie vide
    /// rend `Quit` ; la garde restaure le terminal à sa chute.
    #[test]
    fn terminal_restaure_apres_sortie_normale() {
        let rec = Arc::new(RecordingActions::default());
        let quit_effects = {
            let _guard = TerminalGuard::enter(Arc::clone(&rec) as Arc<dyn TerminalActions>, false)
                .expect("entrée garde factice");
            let st = state::AppState::new("a1", "conv-1".to_string(), None, (100, 30));
            let (_st, effects) = state::update(
                st,
                Action::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            );
            effects
        };
        assert!(
            quit_effects.contains(&Effect::Quit),
            "Ctrl-D sur saisie vide rend Quit"
        );
        assert_eq!(
            rec.snapshot(),
            vec!["enter".to_string(), "leave".to_string()]
        );
    }

    /// Scenario: `terminal_restaure_apres_panique` — le hook de panique restaure
    /// le terminal AVANT tout message de panique, une seule fois.
    #[test]
    fn terminal_restaure_apres_panique() {
        let rec = Arc::new(RecordingActions::default());
        let guard = TerminalGuard::enter(Arc::clone(&rec) as Arc<dyn TerminalActions>, false)
            .expect("entrée garde factice");
        let orig = std::panic::take_hook();
        let rec_prev = Arc::clone(&rec);
        std::panic::set_hook(Box::new(move |_info| {
            rec_prev.log("message de panique");
        }));
        install_panic_hook(Arc::clone(&guard.actions), false, guard.left_flag());
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            panic!("rendu qui panique");
        }));
        let _ = std::panic::take_hook();
        std::panic::set_hook(orig);
        assert!(r.is_err(), "la panique doit se propager");
        drop(guard);
        let events = rec.snapshot();
        let i_restore = events.iter().position(|e| e == "leave").expect("restauré");
        let i_message = events
            .iter()
            .position(|e| e == "message de panique")
            .expect("message affiché");
        assert!(
            i_restore < i_message,
            "restauration AVANT le message: {events:?}"
        );
        assert_eq!(events.iter().filter(|e| *e == "leave").count(), 1);
    }

    /// Scenario: `terminal_trop_petit` — 30×6 : message, aucun plantage.
    #[test]
    fn terminal_trop_petit_seuil() {
        let mut st = state::AppState::new("a1", "c".to_string(), None, (30, 6));
        st.size = (30, 6);
        let backend = view::render_to_string(&st);
        assert!(
            backend.to_lowercase().contains("trop petit"),
            "30×6 doit afficher « terminal trop petit », obtenu: {backend:?}"
        );
    }

    /// Scenario: `collage_multiligne` — trois lignes en saisie, aucun envoi.
    #[test]
    fn collage_multiligne() {
        let st = state::AppState::new("a1", "c".to_string(), None, (100, 30));
        let (st, effects) = state::update(st, Action::Paste("ligne1\nligne2\nligne3".to_string()));
        assert!(effects.is_empty(), "un collage ne lance aucun tour");
        assert_eq!(st.input.lines.len(), 3);
        assert_eq!(editor::text(&st.input), "ligne1\nligne2\nligne3");
    }

    /// Scenario: `fragments_fusionnes` — au plus un rendu par intervalle.
    #[test]
    fn jetons_fusionnes() {
        let mut gate = RenderGate::new(std::time::Duration::from_millis(33));
        let t0 = std::time::Instant::now();
        let rendus = (0u64..200)
            .filter(|i| gate.allow(t0 + std::time::Duration::from_micros(i.saturating_mul(50))))
            .count();
        assert!(rendus <= 2, "rendus fusionnés, obtenu {rendus}");
        assert!(rendus >= 1, "au moins un rendu");
    }

    /// Scenario: `un_seul_tour_a_la_fois` — un second tour est refusé par la garde.
    #[tokio::test]
    async fn un_seul_tour_a_la_fois_garde_boucle() {
        let mut slot = TurnSlot::default();
        let premier: BoxFuture<'static, ()> = Box::pin(std::future::pending());
        assert!(slot.start(premier));
        let second: BoxFuture<'static, ()> = Box::pin(std::future::pending());
        assert!(!slot.start(second), "un second tour est refusé");
        slot.cancel_for_test();
    }

    /// Scenario: `tour_interrompu_panique_tache_de_tour` — une tâche qui panique
    /// est consommée par `take_finished` ; la garde est libre ensuite.
    #[tokio::test]
    async fn tour_interrompu_panique_take_finished_egoutte_join_error() {
        let orig_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_info| {}));
        let mut slot = TurnSlot::default();
        let boom: BoxFuture<'static, ()> =
            Box::pin(async { panic!("boom : tâche de tour qui panique (test)") });
        assert!(slot.start(boom));
        assert!(
            slot.take_finished().await.is_none(),
            "tâche non finie : rien"
        );
        assert!(slot.busy(), "la garde est restée prise");
        let mut spins = 0u32;
        while !slot
            .handle
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            spins += 1;
            assert!(spins < 1000, "la tâche doit finir promptement");
            tokio::task::yield_now().await;
        }
        let drained = slot.take_finished().await.expect("garde consommée");
        assert!(drained.expect_err("panique").is_panic());
        assert!(!slot.busy(), "garde libre");
        std::panic::set_hook(orig_hook);
    }

    /// Scenario: `tour_interrompu_panique` — `TurnFinished(Err)` avec `VNL-INT-001`
    /// sort de `Running` vers `Idle`, sans sauvegarde visible.
    #[test]
    fn tour_interrompu_panique_sort_de_running() {
        let st = state::AppState::new("a1", "conv-1".to_string(), None, (100, 30));
        let (st, _) = state::update(st, touche(KeyCode::Char('x'), KeyModifiers::NONE));
        let (st, effects) = state::update(st, touche(KeyCode::Enter, KeyModifiers::NONE));
        assert!(effects.contains(&Effect::StartTurn("x".to_string())));
        let (st, _) = state::update(st, Action::TurnFinished(Err(interrompu("sans résultat"))));
        assert_eq!(st.status, state::Status::Idle, "jamais Running à vie");
        assert!(st.transcript.iter().any(|i| matches!(
            i,
            TranscriptItem::Notice { level: state::NoticeLevel::Error, text }
                if text.contains(INTERROMPU) && text.contains("interrompu")
        )));
    }

    /// Ctrl+C pendant un tour annule le tour du pilotage : son issue remonte.
    #[tokio::test]
    async fn ctrl_c_annule_le_tour_via_le_pilotage() {
        let driver = Arc::new(FakeDriver::default());
        let (mut lp, (_internal, mut turn_rx)) = boucle(Arc::clone(&driver));
        lp.step(touche(KeyCode::Char('x'), KeyModifiers::NONE));
        lp.step(touche(KeyCode::Enter, KeyModifiers::NONE));
        lp.step(touche(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let issue = turn_rx.recv().await.expect("le tour rend son issue");
        assert_eq!(issue, Ok(Outcome::Cancelled));
    }

    /// Un message pendant un tour est injecté, sans second tour.
    #[tokio::test]
    async fn message_pendant_un_tour_est_injecte() {
        let driver = Arc::new(FakeDriver::default());
        let (mut lp, _channels) = boucle(Arc::clone(&driver));
        lp.step(touche(KeyCode::Char('x'), KeyModifiers::NONE));
        lp.step(touche(KeyCode::Enter, KeyModifiers::NONE));
        lp.step(touche(KeyCode::Char('b'), KeyModifiers::NONE));
        lp.step(touche(KeyCode::Enter, KeyModifiers::NONE));
        let injected = driver.injected.lock().expect("verrou").clone();
        assert_eq!(injected, vec!["b".to_string()]);
        assert!(lp.slot.busy(), "toujours un seul tour");
        lp.step(touche(KeyCode::Char('c'), KeyModifiers::CONTROL));
        lp.slot.drain().await;
    }

    /// Ctrl+D sur saisie vide quitte et annule la session entière.
    #[tokio::test]
    async fn ctrl_d_quitte_et_annule_la_session() {
        let driver = Arc::new(FakeDriver::default());
        let (mut lp, _channels) = boucle(Arc::clone(&driver));
        lp.step(touche(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(lp.quit);
        assert!(
            driver.cancelled.load(Ordering::SeqCst),
            "la session est annulée"
        );
    }

    /// Le nombre de fichiers modifiés se lit dans `GET /git/status`.
    #[test]
    fn dirty_count_lit_le_git_status() {
        let corps = r#"{"branch":"main","files":[{"path":"a.rs","state":"modified","staged":false},{"path":"b.rs","state":"modified","staged":false}],"clean":false}"#;
        assert_eq!(dirty_count(corps), Ok(2));
        assert_eq!(
            dirty_count(r#"{"branch":"main","files":[],"clean":true}"#),
            Ok(0)
        );
        assert!(dirty_count("pas du json").is_err());
        assert!(dirty_count(r#"{"branch":"main"}"#).is_err());
    }
}
