//! Ossature du TUI ratatui du chat interactif — contrat complet : `./mod.sdd`.
//!
//! Initialisation/restauration du terminal ([`TerminalGuard`]), boucle
//! d'événements `tokio::select!` (clavier, flux [`ChatEvent`] du [`TuiSink`],
//! fin de tour, horloge 100 ms) et branchement sur
//! [`vanyline_lib::session::run_agent_turn`] via [`crate::chat`]. Seul ce
//! fichier fait de l'I/O terminal ; l'état ([`state`]) et la vue ([`view`])
//! sont purs et testables sans terminal. Aucune sortie stdout/stderr directe :
//! tout passe par le backend ratatui (harnais clippy).

pub mod editor;
pub mod markdown;
pub mod sink;
pub mod state;
pub mod view;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use vanyline_lib::event::ChatTurnResult;
use vanyline_lib::session::SessionContext;

/// Erreur du chemin TUI : I/O terminal ou backend ratatui en échec. Le message
/// est affiché par l'appelant (`crate::chat`) puis le processus sort en 1.
#[derive(Debug, Clone)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

/// Session passée à [`run_tui`] par `crate::chat` : contexte d'exécution (le
/// `sink` est remplacé par un [`TuiSink`] à l'entrée de la boucle),
/// conversation reprise ou neuve, agent résolu, contexte workspace (`AGENTS.md`),
/// résumé des sources workspace (première `Notice`), racine git (barre d'état).
pub struct TuiSession {
    /// Contexte d'exécution des tours (sink remplacé par un [`TuiSink`]).
    pub ctx: SessionContext,
    /// Conversation reprise (`-c`) ou neuve, enrichie et sauvée après chaque
    /// tour réussi.
    pub conversation: vanyline_lib::Conversation,
    /// Agent résolu (en-tête ; `/agent` le change au tour suivant).
    pub agent: String,
    /// Contenu d'`AGENTS.md` du cwd (contexte workspace des tours).
    pub workspace_context: Option<String>,
    /// Résumé des sources workspace (première `Notice` de la transcription).
    pub workspace_summary: Vec<String>,
    /// Racine git (résumé `git diff --shortstat` de la barre d'état).
    pub git_root: Option<PathBuf>,
    /// Cible toolbox pour le rafraîchissement du jeton d'Owner PAR TOUR
    /// (chat.sdd §Must « JETON DU TOOLBOX PAR TOUR ») ; `None` = sans toolbox.
    pub toolbox: Option<Arc<crate::ToolboxTarget>>,
    /// Capture de la souris (molette) : activée par défaut, `--no-mouse` la
    /// désactive (arbitrage `[?]` de `./mod.sdd`).
    pub mouse: bool,
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
    /// Applique les actions d'entrée ; en échec, les actions de sortie sont
    /// déjà appliquées et aucune garde n'est créée.
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

    /// Restaure immédiatement et de façon idempotente (appelé par `Drop` et
    /// par le hook de panique avant l'affichage du message).
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

/// Signaux d'arrêt externe (SIGTERM/SIGHUP) ; `None` si l'enregistrement a
/// échoué ou hors unix (la boucle n'a alors pas de bras signal).
#[cfg(unix)]
type Signals = Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)>;
#[cfg(not(unix))]
type Signals = Option<((), ())>;

/// Résultat d'un tour transmis par le canal dédié de la tâche de tour : le
/// tour lui-même (succès ou erreur) et l'état todo relu du `todo_state` du
/// contexte, à persister avec le tour (un tour annulé n'y passe jamais).
type TurnOutcome = (
    Result<ChatTurnResult, vanyline_lib::VnyError>,
    Option<String>,
);

/// Garde de boucle « un seul tour à la fois » (`mod.sdd` §Must not) : une
/// seule tâche de tour est active à la fois ; `start` refuse toute tâche
/// supplémentaire tant que la précédente n'est pas terminée ou annulée.
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
    fn start(&mut self, task: futures::future::BoxFuture<'static, ()>) -> bool {
        if self.busy() {
            return false;
        }
        self.handle = Some(tokio::spawn(task));
        true
    }

    /// Abandonne la tâche active (les connexions MCP se ferment à la chute de
    /// leur garde, `mod.sdd` §Handles « Annulation ») ; sans effet sinon.
    fn cancel(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    /// Libère la garde quand le résultat du tour est arrivé (sans abandon).
    fn finish(&mut self) {
        self.handle = None;
    }

    /// Polling de la garde au tick de 100 ms (`mod.sdd` §Must « Panique de la
    /// tâche de tour ») : si la handle est présente ET la tâche TERMINÉE
    /// (panique, ou fin anticipée sans envoi de `TurnOutcome`), consommer la
    /// garde, égoutter la `JoinError` (rendu immédiat, la tâche est finie) et
    /// la rendre à l'appelant. `None` sinon — aucun await bloquant (`await`
    /// uniquement derrière `is_finished()`).
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
/// (`mod.sdd` §Must not « Laisser une panique de rendu laisser le terminal en
/// mode raw ») puis enchaîne l'ancien hook.
fn install_panic_hook(actions: Arc<dyn TerminalActions>, mouse: bool, left: Arc<AtomicBool>) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        TerminalGuard::restore(&actions, mouse, &left);
        prev(info);
    }));
}

/// État de la boucle d'événements hors `AppState` : conversation, contexte
/// partagé, effets. `step` applique une `Action` (mise à jour pure de
/// [`state`]) puis exécute les `Effect` produits (`mod.sdd` §Handles « Boucle »).
struct Loop {
    /// Modèle pur du chat.
    state: state::AppState,
    /// Conversation portée (enrichie et sauvée après chaque tour réussi).
    conv: vanyline_lib::Conversation,
    /// Contexte de session (sink = [`TuiSink`](sink::TuiSink)) ; réutilisé
    /// tel quel sans toolbox, reconstruit par tour avec toolbox (jeton frais,
    /// `chat.sdd` §Must « JETON DU TOOLBOX PAR TOUR »).
    shared_ctx: Arc<SessionContext>,
    /// Cible toolbox pour le jeton par tour (`None` = sans toolbox).
    toolbox: Option<Arc<crate::ToolboxTarget>>,
    /// Contenu d'`AGENTS.md` du cwd.
    workspace_context: Option<String>,
    /// Racine git (résumé `git diff --shortstat` de la barre d'état).
    git_root: Option<PathBuf>,
    /// Sink d'agent (clone porté par chaque tâche de tour).
    sink: sink::TuiSink,
    /// Canal interne des actions asynchrones (résumé git rafraîchi).
    internal_tx: tokio::sync::mpsc::UnboundedSender<state::Action>,
    /// Canal interne : résultat de la tâche de tour vers la boucle.
    turn_tx: tokio::sync::mpsc::UnboundedSender<TurnOutcome>,
    /// Garde « un seul tour à la fois ».
    slot: TurnSlot,
    /// Texte du message utilisateur du tour en cours (persistance après succès).
    last_user: Option<String>,
    /// Résultat du dernier tour réussi : message assistant + todo relu.
    last_turn: Option<(vanyline_lib::Message, Option<String>)>,
    /// Effet `Quit` reçu : la boucle s'arrête.
    quit: bool,
}

impl Loop {
    /// Résumé des sources du workspace en première `Notice` de la
    /// transcription (`chat.sdd` §Must « sources passées au TUI »).
    fn push_workspace_notice(&mut self, summary: &[String]) {
        if summary.is_empty() {
            return;
        }
        self.state.transcript.push(state::TranscriptItem::Notice {
            level: state::NoticeLevel::Info,
            text: summary.join("\n"),
        });
        self.state.revision = self.state.revision.wrapping_add(1);
    }

    /// Applique une action puis exécute ses effets ; rend `true` quand la
    /// boucle doit se terminer (effet `Quit`).
    fn step(&mut self, action: state::Action) {
        let (next, effects) = state::update(std::mem::take(&mut self.state), action);
        self.state = next;
        for effect in effects {
            match effect {
                state::Effect::StartTurn(text) => {
                    self.last_user = Some(text.clone());
                    self.start_turn(text);
                }
                state::Effect::CancelTurn => self.slot.cancel(),
                state::Effect::Save => self.save_last_turn(),
                state::Effect::Quit => {
                    self.slot.cancel();
                    self.quit = true;
                }
                state::Effect::RefreshGit => self.refresh_git(),
                state::Effect::NewConversation => self.new_conversation(),
                // L'agent est déjà porté par l'état : le tour suivant le
                // prend (`state.sdd` §Handles, « valable au tour suivant »).
                state::Effect::SwitchAgent(_) => {}
            }
        }
    }

    /// Lance la tâche de tour (`mod.sdd` §Handles « Tour ») : le résultat
    /// arrive par le canal `turn_tx`, jamais par un prêt de la boucle. Avec
    /// toolbox, le contexte est reconstruit PAR TOUR (jeton d'Owner frais,
    /// `chat.sdd` §Must) ; sans toolbox, le contexte partagé (sink déjà
    /// remplacé) est réutilisé.
    fn start_turn(&mut self, text: String) {
        let shared = Arc::clone(&self.shared_ctx);
        let toolbox = self.toolbox.clone();
        let sink = self.sink.clone();
        let conv = self.conv.clone();
        let agent = self.state.agent.clone();
        let workspace = self.workspace_context.clone();
        let todo_seed = self.conv.todo.clone();
        let turn_tx = self.turn_tx.clone();
        let started = self.slot.start(Box::pin(async move {
            let ctx: Arc<SessionContext> = match toolbox.as_deref() {
                Some(target) => {
                    match crate::chat::try_build_turn_context(Some(target), false, None, todo_seed)
                        .await
                    {
                        Ok(mut fresh) => {
                            fresh.sink = Arc::new(sink);
                            Arc::new(fresh)
                        }
                        Err(message) => {
                            let outcome: TurnOutcome =
                                (Err(vanyline_lib::VnyError::ConfigError(message)), None);
                            let _ = turn_tx.send(outcome);
                            return;
                        }
                    }
                }
                None => shared,
            };
            let result =
                crate::chat::process_turn(&conv, &agent, &ctx, workspace.as_deref(), &text).await;
            let todo = crate::chat::read_todo_state(&ctx.todo_state);
            let _ = turn_tx.send((result, todo));
        }));
        if !started {
            // La politique de l'état ne produit jamais deux `StartTurn`
            // chevauchés (`state.sdd`) ; c'est la garde défensive de la boucle.
            tracing::warn!("second tour ignoré : une tâche de tour est déjà active");
        }
    }

    /// Fin de tour réussie (`mod.sdd` §Handles « Tour ») : message
    /// utilisateur, message assistant et todo reportés sur la conversation
    /// (mêmes règles que `chat.rs`), puis sauvegarde (erreur ignorée, comme
    /// avant — le tour est perdu si le disque refuse).
    fn save_last_turn(&mut self) {
        let parts = (self.last_user.take(), self.last_turn.take());
        if let (Some(user_text), Some((assistant, todo))) = parts {
            apply_turn(&mut self.conv, &user_text, assistant, todo);
            crate::store::save_conversation(&self.conv).ok();
        }
    }

    /// `git diff --shortstat` hors de la boucle (tâche bloquante dédiée,
    /// `mod.sdd` §Must « Ne jamais bloquer la boucle ») ; le résultat revient
    /// comme `Action::GitSummary`.
    fn refresh_git(&self) {
        let Some(root) = self.git_root.clone() else {
            return;
        };
        let tx = self.internal_tx.clone();
        tokio::task::spawn_blocking(move || {
            let summary = git_diff_shortstat(&root);
            let _ = tx.send(state::Action::GitSummary(summary));
        });
    }

    /// `/new` : conversation neuve, transcription vidée (le TUI reste ouvert).
    fn new_conversation(&mut self) {
        self.conv = vanyline_lib::Conversation {
            id: uuid::Uuid::new_v4(),
            agent: Some(self.state.agent.clone()),
            title: None,
            messages: Vec::new(),
            todo: None,
        };
        self.state.conversation_id = self.conv.id.to_string();
        self.state.title = None;
        self.state.transcript.clear();
        self.state.todo.clear();
        self.state.usage = state::Usage::default();
        self.state.revision = self.state.revision.wrapping_add(1);
        self.last_user = None;
        self.last_turn = None;
    }

    /// Rend l'état dans le terminal (`view::render` pur, `TestBackend` dans les
    /// tests de `./view.rs`).
    fn draw(&self, terminal: &mut TuiTerminal) -> Result<(), CliError> {
        let state = &self.state;
        terminal
            .draw(|frame| view::render(frame, state))
            .map_err(|e| CliError(format!("rendu: {e}")))?;
        Ok(())
    }
}

/// Entrée du mode interactif, appelée par `crate::chat` (stdin ET stdout sont
/// alors garantis terminaux). Restaure le terminal dans tous les cas de sortie
/// (normale, erreur, panique via le hook, SIGTERM/SIGHUP) — contrat complet :
/// `mod.sdd` §Handles « Boucle », « Tour », « Sortie ».
// Une seule boucle par contrat (mod.sdd §Boucle : sélection multiple, garde de tour,
// throttle de rendu, sauvegarde, signaux) : la longueur est le contrat.
#[allow(clippy::too_many_lines)]
pub async fn run_tui(session: TuiSession) -> Result<(), CliError> {
    let mouse = session.mouse;
    let actions: Arc<dyn TerminalActions> = Arc::new(CrosstermActions);
    let guard = TerminalGuard::enter(Arc::clone(&actions), mouse)?;
    install_panic_hook(Arc::clone(&actions), mouse, guard.left_flag());

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal =
        ratatui::Terminal::new(backend).map_err(|e| CliError(format!("terminal: {e}")))?;
    let size = terminal
        .size()
        .map_err(|e| CliError(format!("taille du terminal: {e}")))?;

    let (sink, mut agent_rx) = sink::TuiSink::channel();
    let (internal_tx, mut internal_rx) = tokio::sync::mpsc::unbounded_channel();
    let (turn_tx, mut turn_rx) = tokio::sync::mpsc::unbounded_channel::<TurnOutcome>();

    let TuiSession {
        mut ctx,
        conversation,
        agent,
        workspace_context,
        workspace_summary,
        git_root,
        toolbox,
        mouse: _,
    } = session;
    // `mod.sdd` Exposes : le `sink` du contexte est remplacé par le TuiSink à
    // l'entrée de la boucle.
    ctx.sink = Arc::new(sink.clone());
    let mut lp = Loop {
        state: state::AppState::new(
            agent,
            conversation.id.to_string(),
            conversation.title.clone(),
            (size.width, size.height),
        ),
        conv: conversation,
        shared_ctx: Arc::new(ctx),
        toolbox,
        workspace_context,
        git_root,
        sink,
        internal_tx,
        turn_tx,
        slot: TurnSlot::default(),
        last_user: None,
        last_turn: None,
        quit: false,
    };
    lp.push_workspace_notice(&workspace_summary);
    lp.draw(&mut terminal)?;

    let mut events = crossterm::event::EventStream::new();
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

    while !lp.quit {
        let action = tokio::select! {
            maybe_event = futures::StreamExt::next(&mut events) => {
                match maybe_event {
                    Some(Ok(crossterm::event::Event::Key(key)))
                        if key.kind == crossterm::event::KeyEventKind::Press
                            || key.kind == crossterm::event::KeyEventKind::Repeat =>
                    {
                        Some(state::Action::Key(key))
                    }
                    Some(Ok(crossterm::event::Event::Paste(text))) => {
                        Some(state::Action::Paste(text))
                    }
                    Some(Ok(crossterm::event::Event::Resize(w, h))) => {
                        terminal
                            .resize(ratatui::layout::Size::new(w, h).into())
                            .map_err(|e| CliError(format!("redimension: {e}")))?;
                        Some(state::Action::Resize(w, h))
                    }
                    Some(Ok(crossterm::event::Event::Mouse(mouse_ev))) => {
                        Some(state::Action::Mouse(mouse_ev.kind))
                    }
                    // touches sans effet (Relâchement, focus…) : rien.
                    Some(Ok(_)) => None,
                    // erreur de lecture du tty : le flux ne peut plus rien
                    // promettre — sortie par erreur, garde RAII restaurée.
                    Some(Err(e)) => {
                        return Err(CliError(format!("flux d'événements: {e}")));
                    }
                    // flux fermé (fin de stream) : sortie propre.
                    None => {
                        lp.quit = true;
                        None
                    }
                }
            }
            maybe_chat_event = agent_rx.recv() => maybe_chat_event.map(state::Action::Agent),
            maybe_internal = internal_rx.recv() => maybe_internal,
            maybe_outcome = turn_rx.recv() => {
                lp.slot.finish();
                match maybe_outcome {
                    Some((Ok(result), todo)) => {
                        // le résultat utile est rangé ici ; l'état ne reçoit
                        // que le signal de fin (`state::update` n'exploite
                        // pas le contenu du `Ok`).
                        lp.last_turn = Some((
                            crate::chat::result_to_assistant_message(result),
                            todo,
                        ));
                        Some(state::Action::TurnFinished(Ok(ChatTurnResult {
                            response_text: String::new(),
                            tool_calls: Vec::new(),
                        })))
                    }
                    Some((Err(e), _)) => Some(state::Action::TurnFinished(Err(e))),
                    None => None,
                }
            }
            _ = ticker.tick() => {
                // §Must « Panique de la tâche de tour » : à chaque tick,
                // sonder la garde ; une tâche terminée alors que la garde est
                // encore prise (panique ou fin anticipée sans TurnOutcome)
                // est consommée ici. Pas de double traitement : un résultat
                // malgré tout en attente sur le canal passe d'abord par le
                // chemin normal (même traitement que le bras `turn_rx`).
                if let Some(join) = lp.slot.take_finished().await {
                    match turn_rx.try_recv() {
                        Ok((Ok(result), todo)) => {
                            lp.last_turn = Some((
                                crate::chat::result_to_assistant_message(result),
                                todo,
                            ));
                            lp.slot.finish();
                            lp.step(state::Action::TurnFinished(Ok(ChatTurnResult {
                                response_text: String::new(),
                                tool_calls: Vec::new(),
                            })));
                        }
                        Ok((Err(e), _)) => {
                            lp.slot.finish();
                            lp.step(state::Action::TurnFinished(Err(e)));
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty
                        | tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            let reason = match join {
                                Ok(()) => "fin anticipée sans résultat".to_string(),
                                Err(e) => e.to_string(),
                            };
                            lp.step(state::Action::TurnFinished(Err(
                                vanyline_lib::VnyError::InternalError(format!(
                                    "tour interrompu : la tâche de tour s'est terminée sans résultat ({reason})"
                                )),
                            )));
                        }
                    }
                }
                Some(state::Action::Tick)
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
                lp.quit = true;
                None
            }
        };
        if let Some(action) = action {
            lp.step(action);
        }
        if lp.quit {
            break;
        }
        // rendu throttlé (~30 Hz, jetons rapprochés fusionnés — Scenario
        // `jetons_fusionnes`) ; le redimension et la première image rendent
        // immédiatement.
        if gate.allow(std::time::Instant::now()) {
            lp.draw(&mut terminal)?;
        }
    }
    lp.slot.cancel();
    drop(guard);
    // ligne finale APRÈS restauration, hors écran alternatif et hors macros
    // print (harnais) : le terminal garde la trace de la conversation
    // (`mod.sdd` §Handles « Sortie »).
    let mut out = std::io::stdout();
    let trace = format!(
        "conversation: {} — reprendre avec `vanyline -c`\n",
        lp.conv.id
    );
    let _ = std::io::Write::write_all(&mut out, trace.as_bytes());
    let _ = std::io::Write::flush(&mut out);
    Ok(())
}

/// Passerelle de rendu : au plus un rendu par intervalle (fusion des jetons
/// rapprochés, `mod.sdd` §Handles « au plus 30 fois par seconde »).
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

/// Report d'un tour réussi sur la conversation (mêmes règles que
/// `chat.rs` : message utilisateur, message assistant, todo rechargé) — la
/// sauvegarde effective (`store::save_conversation`) est faite par la boucle.
fn apply_turn(
    conv: &mut vanyline_lib::Conversation,
    user_text: &str,
    assistant: vanyline_lib::Message,
    todo: Option<String>,
) {
    conv.messages.push(vanyline_lib::Message {
        role: "user".to_string(),
        content: user_text.to_string(),
        tool_calls: None,
    });
    conv.messages.push(assistant);
    conv.todo = todo;
}

/// `git diff --shortstat` dans `root` : résumé court pour la barre d'état ;
/// `None` hors dépôt, git en échec ou diff vide.
fn git_diff_shortstat(root: &std::path::Path) -> Option<String> {
    if !root.join(".git").exists() {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["diff", "--shortstat"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Actions enregistrées dans un ordre observable (test de restauration).
    #[derive(Default)]
    struct RecordingActions {
        events: std::sync::Mutex<Vec<String>>,
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

    /// Scenario: `terminal_restaure_apres_sortie_normale` — la sortie de
    /// boucle (ici, la fin de portée de la garde, atteinte après l'effet
    /// `Quit` rendu par Ctrl-D sur saisie vide) restaure le terminal.
    #[test]
    fn terminal_restaure_apres_sortie_normale() {
        let rec = Arc::new(RecordingActions::default());
        let quit_effects = {
            let _guard = TerminalGuard::enter(Arc::clone(&rec) as Arc<dyn TerminalActions>, false)
                .expect("entrée garde factice");
            let mut st = state::AppState::new("a1", "conv-1".to_string(), None, (100, 30));
            // Ctrl-D sur saisie vide ⇒ effet Quit (la boucle sort, la garde tombe).
            let (_st, effects) = state::update(
                std::mem::take(&mut st),
                state::Action::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char('d'),
                    crossterm::event::KeyModifiers::CONTROL,
                )),
            );
            effects
        }; // portée de `guard` terminée : restauration
        assert!(
            quit_effects.contains(&state::Effect::Quit),
            "Ctrl-D sur saisie vide doit rendre l'effet Quit"
        );
        let events = rec.snapshot();
        assert_eq!(
            events,
            vec!["enter".to_string(), "leave".to_string()],
            "raw mode et écran alternatif doivent être désactivés à la sortie"
        );
    }

    /// Scenario: `terminal_restaure_apres_panique` — le VRAI hook de panique
    /// production [`install_panic_hook`] (installé par `run_tui`) RESTAURE le
    /// terminal AVANT tout message de panique ; un hook factice joue le rôle
    /// du hook précédent (affichage du message) et la panique est déclenchée
    /// via `catch_unwind`.
    #[test]
    fn terminal_restaure_apres_panique() {
        let rec = Arc::new(RecordingActions::default());
        let guard = TerminalGuard::enter(Arc::clone(&rec) as Arc<dyn TerminalActions>, false)
            .expect("entrée garde factice");
        let orig = std::panic::take_hook();
        let rec_prev = Arc::clone(&rec);
        std::panic::set_hook(Box::new(move |_info| {
            // hook précédent simulé : affiche le message de panique
            rec_prev.log("message de panique");
        }));
        // le hook production réel s'installe par-dessus le hook simulé
        install_panic_hook(Arc::clone(&guard.actions), false, guard.left_flag());
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            panic!("rendu qui panique");
        }));
        let _ = std::panic::take_hook(); // désinstalle le hook production
        std::panic::set_hook(orig);
        assert!(r.is_err(), "la panique doit se propager");
        drop(guard);
        let events = rec.snapshot();
        let i_restore = events
            .iter()
            .position(|e| e == "leave")
            .expect("le terminal doit être restauré");
        let i_message = events
            .iter()
            .position(|e| e == "message de panique")
            .expect("le message de panique est affiché");
        assert!(
            i_restore < i_message,
            "restauration AVANT le message de panique, ordre observé: {events:?}"
        );
        assert_eq!(
            events.iter().filter(|e| *e == "leave").count(),
            1,
            "restauration idempotente (Drop après le hook ne doit pas restaurer deux fois)"
        );
    }

    /// Scenario: `terminal_trop_petit` — vue rendue à 30×6 : message et aucun
    /// plantage (vérification complète côté `view.rs` ; ici le seuil).
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

    /// Scenario: `collage_multiligne` — un collage de trois lignes entre dans
    /// la saisie sans déclencher d'envoi.
    #[test]
    fn collage_multiligne() {
        let st = state::AppState::new("a1", "c".to_string(), None, (100, 30));
        let (st, effects) = state::update(
            st,
            state::Action::Paste("ligne1\nligne2\nligne3".to_string()),
        );
        assert!(
            effects.is_empty(),
            "un collage ne lance aucun tour, effets: {effects:?}"
        );
        assert_eq!(
            st.input.lines.len(),
            3,
            "les trois lignes entrent en saisie"
        );
        assert_eq!(
            editor::text(&st.input),
            "ligne1\nligne2\nligne3",
            "le collage est intact"
        );
    }

    /// Scenario: `jetons_fusionnes` — 200 jetons en 10 ms : au plus un rendu
    /// par intervalle de rafraîchissement et aucun jeton perdu.
    #[test]
    fn jetons_fusionnes() {
        let mut st = state::AppState::new("a1", "c".to_string(), None, (100, 30));
        let mut gate = RenderGate::new(std::time::Duration::from_millis(33));
        let start = std::time::Instant::now();
        let mut renders = 0usize;
        for i in 0..200 {
            let ev = vanyline_lib::event::ChatEvent::Token {
                content: format!("j{i} "),
            };
            let (st2, _effects) = state::update(st, state::Action::Agent(ev));
            st = st2;
            if gate
                .allow(start + std::time::Duration::from_millis(u64::try_from(i).unwrap_or(0) / 20))
            {
                renders += 1;
            }
        }
        assert_eq!(renders, 1, "200 jetons sur 10 ms ⇒ un seul rendu");
        let text = st
            .transcript
            .iter()
            .filter_map(|i| match i {
                state::TranscriptItem::Assistant { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<String>();
        assert!(
            text.contains("j0 ") && text.contains("j199 "),
            "aucun jeton perdu"
        );
    }

    /// Scenario: `tour_annule_non_persiste` (volet persistance) — la
    /// conversation n'est mutée QUE par `apply_turn` (chemin succès) ; sur
    /// annulation (`CancelTurn`, aucun effet `Save`), elle reste inchangée.
    #[test]
    fn tour_annule_non_persiste() {
        let conv = vanyline_lib::Conversation {
            id: uuid::Uuid::new_v4(),
            agent: Some("a1".to_string()),
            title: None,
            messages: Vec::new(),
            todo: None,
        };
        let before = conv.clone();
        let st = state::AppState::new("a1", "c".to_string(), None, (100, 30));
        let mut st = {
            let (st, _e) = state::update(
                st,
                state::Action::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char('x'),
                    crossterm::event::KeyModifiers::NONE,
                )),
            );
            let (mut st, _e) = state::update(
                st,
                state::Action::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Enter,
                    crossterm::event::KeyModifiers::NONE,
                )),
            );
            st.input.lines = vec![String::new()];
            st.input.cursor = (0, 0);
            st
        };
        // Ctrl-C pendant le tour : CancelTurn + Interrupted…
        let (_st, effects) = state::update(
            std::mem::take(&mut st),
            state::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('c'),
                crossterm::event::KeyModifiers::CONTROL,
            )),
        );
        assert!(
            effects.contains(&state::Effect::CancelTurn) && !effects.contains(&state::Effect::Save),
            "l'annulation n'offre aucun effet de sauvegarde: {effects:?}"
        );
        // … aucun apply_turn n'est appelé sur ce chemin : rien n'est muté.
        assert_eq!(conv.messages.len(), before.messages.len());
        assert_eq!(conv.todo, before.todo);
    }

    /// Scenario: `tour_reussi_persiste` (volet persistance) — message
    /// utilisateur, message assistant et todo sont reportés sur la
    /// conversation par `apply_turn`, dans cet ordre.
    #[test]
    fn tour_reussi_persiste_applique_messages_et_todo() {
        let mut conv = vanyline_lib::Conversation {
            id: uuid::Uuid::new_v4(),
            agent: Some("a1".to_string()),
            title: None,
            messages: Vec::new(),
            todo: None,
        };
        let assistant =
            crate::chat::result_to_assistant_message(vanyline_lib::event::ChatTurnResult {
                response_text: "ok".to_string(),
                tool_calls: Vec::new(),
            });
        apply_turn(&mut conv, "bonjour", assistant, Some("[]".to_string()));
        assert_eq!(conv.messages.len(), 2);
        assert_eq!(conv.messages[0].role, "user");
        assert_eq!(conv.messages[0].content, "bonjour");
        assert_eq!(conv.messages[1].role, "assistant");
        assert_eq!(conv.messages[1].content, "ok");
        assert_eq!(conv.todo, Some("[]".to_string()));
    }

    /// Scenario: `un_seul_tour_a_la_fois` (volet garde de boucle) — la garde
    /// `TurnSlot` utilisée par la boucle refuse un second tour pendant qu'une
    /// tâche est active ; l'annulation et la fin libèrent la garde.
    #[tokio::test]
    async fn un_seul_tour_a_la_fois_garde_boucle() {
        let mut slot = TurnSlot::default();
        assert!(!slot.busy(), "aucun tour au départ");
        let pending: futures::future::BoxFuture<'static, ()> = Box::pin(std::future::pending());
        assert!(slot.start(pending), "premier tour accepté");
        assert!(slot.busy(), "un tour est actif");
        let second: futures::future::BoxFuture<'static, ()> = Box::pin(std::future::pending());
        assert!(
            !slot.start(second),
            "Entrée avec une saisie pendant le tour : aucun second tour lancé"
        );
        slot.cancel();
        assert!(!slot.busy(), "l'annulation (Ctrl-C) libère la garde");
        let done: futures::future::BoxFuture<'static, ()> = Box::pin(async {});
        assert!(slot.start(done), "un tour est possible après annulation");
        slot.finish();
        assert!(!slot.busy(), "la fin du tour libère la garde");
    }

    /// Scenario: `tour_interrompu_panique_tache_de_tour` (volet garde —
    /// §Must « Panique de la tâche de tour ») — une tâche de tour qui panique
    /// (panique capturée en `JoinError` par tokio, hook global non installé en
    /// test) : `take_finished` rend `None` tant que la tâche vit sans consommer
    /// la garde, puis consomme la garde, égoutte la `JoinError` de panique et
    /// la libère ; un tour suivant est accepté.
    #[tokio::test]
    async fn tour_interrompu_panique_take_finished_egoutte_join_error() {
        // Le hook par défaut afficherait la panique capturée sur stderr :
        // on le remplace pour la durée du test (aucune sortie parasite).
        let orig_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_info| {}));
        let mut slot = TurnSlot::default();
        let boom: futures::future::BoxFuture<'static, ()> =
            Box::pin(async { panic!("boom : tâche de tour qui panique (test)") });
        assert!(slot.start(boom), "la tâche de tour est acceptée");
        // Tâche non finie (runtime mono-thread, aucun yield émis) : None et
        // la garde n'est PAS consommée.
        assert!(
            slot.take_finished().await.is_none(),
            "tâche non finie : take_finished rend None sans consommer la garde"
        );
        assert!(slot.busy(), "la garde est restée prise");
        // Attendre la fin de la tâche sans sleep long : quelques yields
        // bornés suffisent (la panique est capturée en JoinError par tokio).
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
        let drained = slot
            .take_finished()
            .await
            .expect("garde terminée : elle doit être consommée");
        let err = drained.expect_err("la panique est rendue en JoinError");
        assert!(
            err.is_panic(),
            "JoinError de panique attendue, obtenu: {err}"
        );
        assert!(!slot.busy(), "la garde est libre après consommation");
        assert!(
            slot.take_finished().await.is_none(),
            "plus de garde : rien à consommer"
        );
        let next: futures::future::BoxFuture<'static, ()> = Box::pin(std::future::pending());
        assert!(
            slot.start(next),
            "la garde libre rend un tour suivant possible"
        );
        slot.cancel();
        std::panic::set_hook(orig_hook);
    }

    /// Scenario: `tour_interrompu_panique_tache_de_tour` (volet décision +
    /// état) — l'action injectée par le polling après panique,
    /// `TurnFinished(Err(InternalError(« … tour interrompu … »)))`, sort de
    /// `Running` vers `Idle`, rend une `Notice` Error contenant
    /// `VNL-INT-001` et « interrompu », et n'émet AUCUN effet `Save` (le tour
    /// interrompu n'est pas persisté) — jamais `Running` à vie.
    #[test]
    fn tour_interrompu_panique_sort_de_running_sans_sauvegarde() {
        // État `Running` obtenu par le chemin de préparation déjà testé
        // (saisie « x » puis Entrée ⇒ StartTurn + Running).
        let st = state::AppState::new("a1", "conv-1".to_string(), None, (100, 30));
        let (st, _e) = state::update(
            st,
            state::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('x'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        let (st, effects) = state::update(
            st,
            state::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert!(
            effects.contains(&state::Effect::StartTurn("x".to_string())),
            "préparation Running: {effects:?}"
        );
        assert!(
            matches!(st.status, state::Status::Running { .. }),
            "l'état est Running avant l'injection"
        );
        // Action injectée par le polling de la garde (message rendu :
        // « VNL-INT-001: Internal error: tour interrompu… »).
        let (st, effects) = state::update(
            st,
            state::Action::TurnFinished(Err(vanyline_lib::VnyError::InternalError(
                "tour interrompu : la tâche de tour s'est terminée sans résultat".to_string(),
            ))),
        );
        assert_eq!(st.status, state::Status::Idle, "jamais Running à vie");
        let notices: Vec<&String> = st
            .transcript
            .iter()
            .filter_map(|i| match i {
                state::TranscriptItem::Notice {
                    level: state::NoticeLevel::Error,
                    text,
                } => Some(text),
                _ => None,
            })
            .collect();
        assert!(
            notices
                .iter()
                .any(|t| t.contains("VNL-INT-001") && t.contains("interrompu")),
            "une Notice Error contenant VNL-INT-001 et « interrompu », obtenues: {notices:?}"
        );
        assert!(
            !effects.contains(&state::Effect::Save),
            "le tour interrompu n'est jamais persisté: {effects:?}"
        );
    }

    /// Le résumé `git diff --shortstat` de la barre d'état : `Some` sur dépôt
    /// modifié, `None` hors dépôt et sur dépôt propre.
    #[test]
    fn git_diff_shortstat_resume_le_diff() {
        fn git(root: &std::path::Path, args: &[&str]) {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .is_ok_and(|o| o.status.success());
            assert!(ok, "git {args:?} doit réussir");
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        assert!(
            git_diff_shortstat(root).is_none(),
            "hors dépôt : aucun résumé"
        );
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@t.tld"]);
        git(root, &["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "un\n").expect("écriture");
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "base"]);
        assert!(
            git_diff_shortstat(root).is_none(),
            "dépôt propre : diff vide = aucun résumé"
        );
        std::fs::write(root.join("a.txt"), "deux\n").expect("écriture");
        let summary = git_diff_shortstat(root).expect("dépôt modifié : résumé");
        assert!(
            summary.contains("1 file changed"),
            "résumé court attendu, obtenu: {summary}"
        );
    }
}
