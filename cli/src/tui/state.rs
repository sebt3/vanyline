//! Modèle du chat interactif et mise à jour pure — contrat complet : `./state.sdd`.
//!
//! Architecture « Elm » : `update(state, action) -> (state, Vec<Effect>)`, sans
//! aucune I/O, horloge ni aléa. La transcription, l'état d'attente, le défilement,
//! le panneau de tâches, l'historique et la politique des touches vivent ici ;
//! tout contact terminal/disque/réseau est un [`Effect`] exécuté par `./mod.rs`.
//! Les événements de session sont les [`Event`] de lymaille (`dsd/cli/chat.md#DEC-1`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use lymaille::{Event, EventKind, Outcome, SessionId};

use crate::host::session_index::SessionEntry;

use crate::tui::editor::{self, EditCommand, EditorState};

/// Période, en ticks de 100 ms, du sondage des sous-agents (`Tick` émet `PollRuns`).
const POLL_EVERY_TICKS: u64 = 10;

/// Niveau d'une `Notice` dans la transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    /// Information.
    Info,
    /// Avertissement (ex. reconnexion MCP, nouvelle tentative).
    Warn,
    /// Erreur (jamais silencieuse).
    Error,
}

/// État d'une ligne de sous-agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentStatus {
    /// Le run est en cours.
    // Construit par le sondage des runs, branché quand session_open exposera le registre (`tui/mod.sdd`, [?]).
    #[allow(dead_code)]
    Running,
    /// Le run est terminé avec cette issue.
    Ended(Outcome),
    /// Le run a été interrompu à la restauration de la session principale.
    Interrupted,
}

/// Une ligne de sous-agent telle que le sondage des runs la rapporte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLine {
    /// Identifiant du run.
    pub run: String,
    /// Nom du sous-agent.
    pub agent: String,
    /// Statut courant du run.
    pub status: SubagentStatus,
}

/// Élément de la transcription du chat.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptItem {
    /// Message de l'utilisateur ; `during_turn` s'il a été envoyé pendant un tour.
    User {
        /// Texte du message.
        text: String,
        /// Envoyé pendant un tour (injecté dans le tour en cours).
        during_turn: bool,
    },
    /// Bloc assistant (texte markdown en cours ou terminé).
    Assistant {
        /// Texte accumulé.
        text: String,
        /// `true` après `TurnEnded` (le bloc est fermé).
        done: bool,
    },
    /// Une exécution de script, repliée en une ligne (`dsd/cli.md#DEC-8`).
    Script {
        /// Rang de l'exécution dans le tour, à partir de 1.
        seq: u64,
        /// Fonctions MCP appelées par le script.
        functions: Vec<String>,
        /// Durée en millisecondes.
        duration_ms: u64,
        /// Succès (sans erreur).
        ok: bool,
        /// Erreur du script, masquée, s'il y en a une.
        error: Option<String>,
        /// Texte complet du script.
        script: String,
        /// Sortie (résultat) du script, masquée et bornée par lymaille.
        output: String,
        /// Déplié.
        expanded: bool,
    },
    /// Ligne de statut d'un sous-agent, rafraîchie par sondage.
    Subagent {
        /// Identifiant du run.
        run: String,
        /// Nom du sous-agent.
        agent: String,
        /// Statut.
        status: SubagentStatus,
    },
    /// Notification info/avertissement/erreur (visible, jamais avalée).
    Notice {
        /// Niveau de la notification.
        level: NoticeLevel,
        /// Texte.
        text: String,
    },
    /// Tour interrompu par l'utilisateur ; le texte déjà reçu est conservé.
    Interrupted,
}

/// État d'attente du tour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Au repos.
    Idle,
    /// Tour en cours depuis le tick `since`.
    Running {
        /// Valeur de l'horloge interne (`tick`) au lancement du tour.
        since: u64,
    },
}

/// Défilement de la transcription : suit la fin par défaut ; remonter fige la
/// position (décalage en lignes depuis le bas), revenir en bas reprend le suivi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    /// Suit la fin du contenu.
    pub follow: bool,
    /// Lignes de décalage depuis le bas quand `follow` est faux.
    pub offset: usize,
    /// Du contenu est arrivé pendant un figement (indicateur « ↓ nouveau »).
    pub pending: bool,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            follow: true,
            offset: 0,
            pending: false,
        }
    }
}

/// Entrée du panneau des tâches, telle que lue depuis la todo de la session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    /// Description de la tâche.
    pub content: String,
    /// Statut (`pending`, `in_progress`, `done`).
    pub status: String,
}

/// Cumul des jetons d'usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Jetons du prompt cumulés.
    pub prompt: u64,
    /// Jetons de la complétion cumulés.
    pub completion: u64,
}

/// Sélecteur de session ouvert par `/sessions`.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    /// Sessions du contexte, la plus récente d'abord.
    pub entries: Vec<SessionEntry>,
    /// Entrée sélectionnée.
    pub selected: usize,
}

/// État complet du chat (modèle pur).
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    /// Transcription, dans l'ordre d'apparition.
    pub transcript: Vec<TranscriptItem>,
    /// Attente d'un tour.
    pub status: Status,
    /// Zone de saisie (et son historique).
    pub input: EditorState,
    /// Défilement de la transcription.
    pub scroll: Scroll,
    /// Tâches du panneau.
    pub todo: Vec<TodoItem>,
    /// Panneau des tâches visible.
    pub todo_visible: bool,
    /// Jetons cumulés.
    pub usage: Usage,
    /// Agent du contexte.
    pub agent: String,
    /// Identifiant de la session affiché.
    pub session_id: String,
    /// Titre de la session.
    pub title: Option<String>,
    /// Nombre de fichiers modifiés du worktree (`None` sans sandbox).
    pub dirty_files: Option<usize>,
    /// Premier Ctrl+C au repos armé : le second quitte.
    pub quit_armed: bool,
    /// Taille du terminal (colonnes, lignes).
    pub size: (u16, u16),
    /// Horloge interne avancée par `Action::Tick` (animation, sondage).
    pub tick: u64,
    /// Index (transcription) de l'élément repliable sélectionné par Tab.
    pub selection: Option<usize>,
    /// Version de la transcription (clé du cache de lignes de la vue).
    pub revision: u64,
    /// Appels MCP du script en cours, rattachés au prochain `Script`.
    pub pending_functions: Vec<String>,
    /// Sélecteur de session ouvert, s'il y en a un.
    pub picker: Option<Picker>,
}

impl AppState {
    /// État initial : transcription vide, agent et session donnés.
    #[must_use]
    pub fn new(
        agent: impl Into<String>,
        session_id: String,
        title: Option<String>,
        size: (u16, u16),
    ) -> Self {
        Self {
            agent: agent.into(),
            session_id,
            title,
            size,
            ..Self::default()
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            transcript: Vec::new(),
            status: Status::Idle,
            input: EditorState::new(),
            scroll: Scroll::default(),
            todo: Vec::new(),
            todo_visible: false,
            usage: Usage::default(),
            agent: String::new(),
            session_id: String::new(),
            title: None,
            dirty_files: None,
            quit_armed: false,
            size: (80, 24),
            tick: 0,
            selection: None,
            revision: 0,
            pending_functions: Vec::new(),
            picker: None,
        }
    }
}

/// Action appliquée par [`update`] : tout événement extérieur devient une action.
#[derive(Debug)]
pub enum Action {
    /// Touche du clavier.
    Key(KeyEvent),
    /// Collage multi-lignes (entre en saisie, ne déclenche jamais d'envoi).
    Paste(String),
    /// Redimensionnement (colonnes, lignes).
    Resize(u16, u16),
    /// Souris (molette).
    Mouse(MouseEventKind),
    /// Horloge de rafraîchissement (100 ms).
    Tick,
    /// Événement de la session reçu du [`TuiSink`](crate::tui::TuiSink).
    Session(Event),
    /// Fin du tour : issue de lymaille, ou erreur de la tâche (`VNL-INT-001`…).
    TurnFinished(Result<Outcome, String>),
    /// Liste de tâches relue depuis la session.
    Todo(Vec<TodoItem>),
    /// Nombre de fichiers modifiés du worktree, `None` sans sandbox ou en cas d'échec.
    DirtyFiles(Option<usize>),
    /// Sondage des sous-agents.
    // Produit par le sondage des runs, branché avec le registre (`tui/mod.sdd`, [?]).
    #[allow(dead_code)]
    Runs(Vec<RunLine>),
    /// Sessions du contexte, pour le sélecteur de `/sessions`.
    SessionList(Vec<SessionEntry>),
    /// Erreur asynchrone non bloquante (worktree, todo, sessions) : notice d'avertissement.
    Warning(String),
}

/// Commande produite par [`update`] et exécutée par la boucle de `./mod.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Lancer un tour avec ce message.
    StartTurn(String),
    /// Injecter ce message dans le tour en cours (entrée utilisateur de lymaille).
    InjectInput(String),
    /// Annuler le tour en cours.
    CancelTurn,
    /// Quitter le TUI.
    Quit,
    /// Relire la liste de tâches de la session.
    RefreshTodo,
    /// Rafraîchir le nombre de fichiers modifiés du worktree.
    RefreshDirtyFiles,
    /// Sonder les runs des sous-agents.
    PollRuns,
    /// Nouvelle session neuve du contexte.
    NewSession,
    /// Lister les sessions du contexte.
    ListSessions,
    /// Reprendre cette session.
    ResumeSession(SessionId),
}

/// Mise à jour pure et TOTALE : toute séquence d'actions laisse un état valide.
#[must_use]
pub fn update(mut state: AppState, action: Action) -> (AppState, Vec<Effect>) {
    match action {
        Action::Key(key) => key_update(state, key),
        Action::Paste(text) => {
            state.input = editor::apply(state.input, EditCommand::InsertStr(text));
            state.quit_armed = false;
            (state, Vec::new())
        }
        Action::Resize(w, h) => {
            state.size = (w, h);
            (state, Vec::new())
        }
        Action::Mouse(kind) => {
            match kind {
                MouseEventKind::ScrollUp => scroll_by(&mut state, 3, true),
                MouseEventKind::ScrollDown => scroll_by(&mut state, 3, false),
                _ => {}
            }
            (state, Vec::new())
        }
        Action::Tick => {
            state.tick = state.tick.wrapping_add(1);
            let effects = if state.tick.is_multiple_of(POLL_EVERY_TICKS)
                && any_subagent_running(&state.transcript)
            {
                vec![Effect::PollRuns]
            } else {
                Vec::new()
            };
            (state, effects)
        }
        Action::Session(event) => {
            let effects = session_update(&mut state, event.kind);
            (state, effects)
        }
        Action::TurnFinished(result) => turn_finished(state, result),
        Action::Todo(items) => {
            state.todo = items;
            (state, Vec::new())
        }
        Action::DirtyFiles(count) => {
            state.dirty_files = count;
            (state, Vec::new())
        }
        Action::Runs(lines) => {
            for line in lines {
                apply_run_line(&mut state, line);
            }
            (state, Vec::new())
        }
        Action::Warning(text) => {
            push_notice(&mut state, NoticeLevel::Warn, text);
            (state, Vec::new())
        }
        Action::SessionList(entries) => {
            if entries.is_empty() {
                push_notice(
                    &mut state,
                    NoticeLevel::Info,
                    "aucune session pour ce contexte".to_string(),
                );
            } else {
                state.picker = Some(Picker {
                    entries,
                    selected: 0,
                });
            }
            (state, Vec::new())
        }
    }
}

/// Fin du tour : l'état repasse au repos ; une erreur est une notice visible.
fn turn_finished(mut state: AppState, result: Result<Outcome, String>) -> (AppState, Vec<Effect>) {
    state.status = Status::Idle;
    match result {
        Ok(Outcome::Cancelled) => {
            if !matches!(state.transcript.last(), Some(TranscriptItem::Interrupted)) {
                state.transcript.push(TranscriptItem::Interrupted);
            }
            close_open_blocks(&mut state.transcript);
        }
        Ok(Outcome::Failed) => {
            close_open_blocks(&mut state.transcript);
            push_notice(
                &mut state,
                NoticeLevel::Error,
                "le tour a échoué".to_string(),
            );
        }
        Ok(_) => {}
        Err(message) => {
            push_notice(&mut state, NoticeLevel::Error, message);
        }
    }
    state.revision = state.revision.wrapping_add(1);
    (state, vec![Effect::RefreshTodo, Effect::RefreshDirtyFiles])
}

/// Pousse une `Notice` (jamais silencieuse).
fn push_notice(state: &mut AppState, level: NoticeLevel, text: String) {
    state
        .transcript
        .push(TranscriptItem::Notice { level, text });
    state.revision = state.revision.wrapping_add(1);
    mark_new_content(state);
}

/// Contenu qui arrive : si l'utilisateur a figé le défilement, arme « ↓ nouveau ».
fn mark_new_content(state: &mut AppState) {
    if !state.scroll.follow {
        state.scroll.pending = true;
    }
}

/// Défile la transcription de `lines` lignes (`up` = vers le haut). Arrivé en
/// bas, le suivi de la fin reprend.
fn scroll_by(state: &mut AppState, lines: usize, up: bool) {
    if up {
        state.scroll.follow = false;
        state.scroll.offset = state.scroll.offset.saturating_add(lines);
    } else {
        state.scroll.offset = state.scroll.offset.saturating_sub(lines);
        if state.scroll.offset == 0 {
            scroll_follow(state);
        }
    }
}

/// Gèle la position de défilement à `lines` lignes depuis le bas (page).
fn scroll_page(state: &mut AppState, page: usize, up: bool) {
    scroll_by(state, page.max(1), up);
}

/// Reprend le suivi de la fin.
fn scroll_follow(state: &mut AppState) {
    state.scroll.follow = true;
    state.scroll.offset = 0;
    state.scroll.pending = false;
}

/// Indices des éléments repliables (exécutions de script) de la transcription.
fn collapsible_indices(transcript: &[TranscriptItem]) -> Vec<usize> {
    transcript
        .iter()
        .enumerate()
        .filter_map(|(i, item)| matches!(item, TranscriptItem::Script { .. }).then_some(i))
        .collect()
}

/// Déplace la sélection Tab/Maj+Tab vers l'élément repliable suivant/précédent.
fn move_selection(state: &mut AppState, forward: bool) {
    let candidates = collapsible_indices(&state.transcript);
    if candidates.is_empty() {
        state.selection = None;
        return;
    }
    let pos = state
        .selection
        .and_then(|sel| candidates.iter().position(|i| *i == sel));
    let next = match (pos, forward) {
        (Some(p), true) => p.saturating_add(1).min(candidates.len().saturating_sub(1)),
        (Some(p), false) => p.saturating_sub(1),
        (None, true) => 0,
        (None, false) => candidates.len().saturating_sub(1),
    };
    state.selection = candidates.get(next).copied();
}

/// Bascule le repli de l'élément sélectionné (Espace).
fn toggle_selected(state: &mut AppState) {
    let Some(idx) = state.selection else {
        return;
    };
    if let Some(TranscriptItem::Script { expanded, .. }) = state.transcript.get_mut(idx) {
        *expanded = !*expanded;
        state.revision = state.revision.wrapping_add(1);
    }
}

/// Politique des touches (état `Idle` ou `Running`).
///
/// Une table unique parce que c'est le contrat entier de `state.sdd` §Handles
/// « Touches » — la découper éparpillerait la politique.
#[allow(clippy::too_many_lines)] // table contractée par state.sdd §Handles
fn key_update(mut state: AppState, key: KeyEvent) -> (AppState, Vec<Effect>) {
    if state.picker.is_some() {
        return picker_key(state, key);
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('c') if ctrl => {
            if matches!(state.status, Status::Running { .. }) {
                state.transcript.push(TranscriptItem::Interrupted);
                state.revision = state.revision.wrapping_add(1);
                return (state, vec![Effect::CancelTurn]);
            }
            if !editor::is_blank(&state.input) {
                state.input = editor::apply(state.input, EditCommand::Clear);
                state.quit_armed = false;
                return (state, Vec::new());
            }
            if state.quit_armed {
                return (state, vec![Effect::Quit]);
            }
            state.quit_armed = true;
            (state, Vec::new())
        }
        KeyCode::Char('d') if ctrl => {
            if editor::is_blank(&state.input) {
                (state, vec![Effect::Quit])
            } else {
                state.input = editor::apply(state.input, EditCommand::Delete);
                (state, Vec::new())
            }
        }
        KeyCode::Enter if alt => {
            state.input = editor::apply(state.input, EditCommand::Newline);
            (state, Vec::new())
        }
        KeyCode::Char('j') if ctrl => {
            state.input = editor::apply(state.input, EditCommand::Newline);
            (state, Vec::new())
        }
        KeyCode::Enter => submit(state),
        KeyCode::Char('l') if ctrl => (state, Vec::new()),
        KeyCode::Char('t') if ctrl => {
            state.todo_visible = !state.todo_visible;
            (state, Vec::new())
        }
        KeyCode::Tab => {
            move_selection(&mut state, true);
            (state, Vec::new())
        }
        KeyCode::BackTab => {
            move_selection(&mut state, false);
            (state, Vec::new())
        }
        KeyCode::Char(' ') if state.selection.is_some() && editor::is_blank(&state.input) => {
            toggle_selected(&mut state);
            (state, Vec::new())
        }
        KeyCode::PageUp => {
            let page = usize::from(state.size.1.saturating_sub(4)).max(1);
            scroll_page(&mut state, page, true);
            (state, Vec::new())
        }
        KeyCode::PageDown => {
            let page = usize::from(state.size.1.saturating_sub(4)).max(1);
            scroll_page(&mut state, page, false);
            (state, Vec::new())
        }
        KeyCode::End => {
            // Fin : retour au suivi de la fin, et fin de ligne pour l'éditeur.
            scroll_follow(&mut state);
            state.input = editor::apply(state.input, EditCommand::End);
            (state, Vec::new())
        }
        KeyCode::Home => {
            state.input = editor::apply(state.input, EditCommand::Home);
            (state, Vec::new())
        }
        KeyCode::Up => {
            if state.input.cursor.0 == 0 {
                state.input = editor::apply(state.input, EditCommand::HistoryPrev);
            } else {
                state.input = editor::apply(state.input, EditCommand::Up);
            }
            (state, Vec::new())
        }
        KeyCode::Down => {
            let last_row = state.input.cursor.0.saturating_add(1) >= state.input.lines.len();
            if last_row {
                state.input = editor::apply(state.input, EditCommand::HistoryNext);
            } else {
                state.input = editor::apply(state.input, EditCommand::Down);
            }
            (state, Vec::new())
        }
        KeyCode::Left => {
            state.input = editor::apply(state.input, EditCommand::Left);
            (state, Vec::new())
        }
        KeyCode::Right => {
            state.input = editor::apply(state.input, EditCommand::Right);
            (state, Vec::new())
        }
        KeyCode::Backspace => {
            state.input = editor::apply(state.input, EditCommand::Backspace);
            state.quit_armed = false;
            (state, Vec::new())
        }
        KeyCode::Delete => {
            state.input = editor::apply(state.input, EditCommand::Delete);
            (state, Vec::new())
        }
        KeyCode::Char(c) if !ctrl => {
            state.input = editor::apply(state.input, EditCommand::Insert(c));
            state.quit_armed = false;
            (state, Vec::new())
        }
        _ => (state, Vec::new()),
    }
}

/// Touches du sélecteur de session ouvert : Haut/Bas, Entrée reprend, Échap ferme.
fn picker_key(mut state: AppState, key: KeyEvent) -> (AppState, Vec<Effect>) {
    let Some(picker) = state.picker.as_mut() else {
        return (state, Vec::new());
    };
    let last = picker.entries.len().saturating_sub(1);
    match key.code {
        KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
        KeyCode::Down => picker.selected = picker.selected.saturating_add(1).min(last),
        KeyCode::Esc => state.picker = None,
        KeyCode::Enter => {
            let choisie = picker.entries.get(picker.selected).map(|s| s.id.clone());
            state.picker = None;
            if let Some(id) = choisie {
                if matches!(state.status, Status::Running { .. }) {
                    push_notice(
                        &mut state,
                        NoticeLevel::Warn,
                        "tour en cours — patientez".to_string(),
                    );
                    return (state, Vec::new());
                }
                return (state, vec![Effect::ResumeSession(id)]);
            }
        }
        _ => {}
    }
    (state, Vec::new())
}

/// Entrée : envoi de la saisie, injection pendant un tour, ou commande slash.
fn submit(state: AppState) -> (AppState, Vec<Effect>) {
    let raw = editor::text(&state.input);
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        return (state, Vec::new());
    }
    // Échappement : un message commençant par `/` s'écrit `//`.
    let text = if let Some(rest) = trimmed.strip_prefix("//") {
        format!("/{rest}")
    } else if trimmed.starts_with('/') && !trimmed.contains('\n') {
        return slash_command(state, &trimmed);
    } else {
        trimmed
    };
    if matches!(state.status, Status::Running { .. }) {
        return inject(state, &text);
    }
    send_message(state, &text)
}

/// Pendant un tour : le message est injecté dans le tour en cours, sans second tour.
fn inject(mut state: AppState, text: &str) -> (AppState, Vec<Effect>) {
    state.transcript.push(TranscriptItem::User {
        text: text.to_string(),
        during_turn: true,
    });
    state.revision = state.revision.wrapping_add(1);
    scroll_follow(&mut state);
    state.input = editor::push_history(
        editor::apply(std::mem::take(&mut state.input), EditCommand::Clear),
        text,
    );
    state.quit_armed = false;
    (state, vec![Effect::InjectInput(text.to_string())])
}

/// Envoie `text` comme message utilisateur : effet `StartTurn`, état Running.
fn send_message(mut state: AppState, text: &str) -> (AppState, Vec<Effect>) {
    state.transcript.push(TranscriptItem::User {
        text: text.to_string(),
        during_turn: false,
    });
    state.revision = state.revision.wrapping_add(1);
    scroll_follow(&mut state);
    state.input = editor::push_history(
        editor::apply(std::mem::take(&mut state.input), EditCommand::Clear),
        text,
    );
    state.status = Status::Running { since: state.tick };
    state.quit_armed = false;
    state.selection = None;
    (state, vec![Effect::StartTurn(text.to_string())])
}

/// Interprète une commande slash (une ligne commençant par `/`).
fn slash_command(mut state: AppState, line: &str) -> (AppState, Vec<Effect>) {
    let cmd = line.split_once(' ').map_or(line, |(c, _)| c);
    let clear = |state: &mut AppState| {
        state.input = editor::apply(std::mem::take(&mut state.input), EditCommand::Clear);
    };
    let running = matches!(state.status, Status::Running { .. });
    match cmd {
        "/help" => {
            clear(&mut state);
            push_notice(
                &mut state,
                NoticeLevel::Info,
                "Entrée envoyer (ou injecter pendant un tour) · Alt+Entrée/Ctrl+J nouvelle ligne \
                 · Ctrl+C annuler/vider · Ctrl+D quitter · PgUp/PgDn/molette défiler · Fin suit la fin \
                 · Tab sélection · Espace replier · Ctrl+T tâches · /new /sessions /todo /quit"
                    .to_string(),
            );
            (state, Vec::new())
        }
        "/new" | "/sessions" if running => {
            clear(&mut state);
            push_notice(
                &mut state,
                NoticeLevel::Warn,
                "tour en cours — patientez".to_string(),
            );
            (state, Vec::new())
        }
        "/new" => {
            clear(&mut state);
            (state, vec![Effect::NewSession])
        }
        "/sessions" => {
            clear(&mut state);
            (state, vec![Effect::ListSessions])
        }
        "/todo" => {
            clear(&mut state);
            state.todo_visible = !state.todo_visible;
            (state, Vec::new())
        }
        "/quit" => {
            clear(&mut state);
            (state, vec![Effect::Quit])
        }
        _ => {
            clear(&mut state);
            push_notice(
                &mut state,
                NoticeLevel::Warn,
                format!("commande inconnue : {cmd} (voir /help)"),
            );
            (state, Vec::new())
        }
    }
}

/// Applique un événement de la session à la transcription, et rend les effets.
// Une branche par variante d'`EventKind` pertinente : la longueur est la table de `state.sdd` §Handles.
#[allow(clippy::too_many_lines)]
fn session_update(state: &mut AppState, kind: EventKind) -> Vec<Effect> {
    match kind {
        EventKind::TextDelta { text, .. } => {
            push_text(&mut state.transcript, &text);
            state.revision = state.revision.wrapping_add(1);
            mark_new_content(state);
            Vec::new()
        }
        EventKind::TurnEnded { response, .. } => {
            finish_assistant(&mut state.transcript, &response);
            state.revision = state.revision.wrapping_add(1);
            Vec::new()
        }
        EventKind::Script(record) => {
            let seq = state
                .transcript
                .iter()
                .filter(|item| matches!(item, TranscriptItem::Script { .. }))
                .count()
                .saturating_add(1);
            let functions = std::mem::take(&mut state.pending_functions);
            state.transcript.push(TranscriptItem::Script {
                seq: u64::try_from(seq).unwrap_or(u64::MAX),
                functions,
                duration_ms: record.duration_ms,
                ok: record.error.is_none(),
                error: record.error,
                script: record.script,
                output: record.result,
                expanded: false,
            });
            state.revision = state.revision.wrapping_add(1);
            mark_new_content(state);
            Vec::new()
        }
        EventKind::McpCall(record) => {
            state.pending_functions.push(record.function);
            Vec::new()
        }
        EventKind::TodoChanged => vec![Effect::RefreshTodo],
        EventKind::MemoryChanged { level, id } => {
            push_notice(
                state,
                NoticeLevel::Info,
                format!("mémoire modifiée : {level}/{id}"),
            );
            Vec::new()
        }
        EventKind::TokenUsage { prompt, completion } => {
            state.usage.prompt = state.usage.prompt.saturating_add(prompt);
            state.usage.completion = state.usage.completion.saturating_add(completion);
            Vec::new()
        }
        EventKind::InferenceRetry {
            attempt,
            delay_ms,
            error_class,
        } => {
            push_notice(
                state,
                NoticeLevel::Warn,
                format!(
                    "nouvelle tentative d'inférence n°{attempt} ({error_class}) dans {delay_ms} ms"
                ),
            );
            Vec::new()
        }
        EventKind::McpReconnect {
            role,
            attempt,
            delay_ms,
        } => {
            push_notice(
                state,
                NoticeLevel::Warn,
                format!("reconnexion MCP « {role} » n°{attempt} dans {delay_ms} ms"),
            );
            Vec::new()
        }
        EventKind::Overrun { elapsed_ms } => {
            push_notice(
                state,
                NoticeLevel::Warn,
                format!("le tour dépasse son délai ({elapsed_ms} ms), il continue"),
            );
            Vec::new()
        }
        EventKind::EmptyResponse { will_retry } => {
            let suite = if will_retry {
                "le modèle est relancé"
            } else {
                "pas de relance"
            };
            push_notice(
                state,
                NoticeLevel::Warn,
                format!("réponse vide écartée, {suite}"),
            );
            Vec::new()
        }
        EventKind::ResponseTruncated { .. } => {
            push_notice(
                state,
                NoticeLevel::Warn,
                "réponse coupée par la limite de sortie du modèle".to_string(),
            );
            Vec::new()
        }
        EventKind::TurnLeaseLost { .. } => {
            push_notice(
                state,
                NoticeLevel::Warn,
                "bail du tour perdu : le tour est annulé, rien n'est sauvegardé".to_string(),
            );
            Vec::new()
        }
        EventKind::TitleChanged { title } => {
            state.title = Some(title);
            Vec::new()
        }
        EventKind::RunEnded { run, outcome } => {
            set_subagent_status(
                &mut state.transcript,
                run.as_str(),
                SubagentStatus::Ended(outcome),
            );
            state.revision = state.revision.wrapping_add(1);
            Vec::new()
        }
        EventKind::RunInterrupted { run, .. } => {
            set_subagent_status(
                &mut state.transcript,
                run.as_str(),
                SubagentStatus::Interrupted,
            );
            state.revision = state.revision.wrapping_add(1);
            Vec::new()
        }
        // Raisonnement absent (`chat#DEC-2`) et autres variantes : ignorées sans erreur.
        _ => Vec::new(),
    }
}

/// Prolonge le dernier bloc assistant ouvert, ou en ouvre un nouveau.
fn push_text(items: &mut Vec<TranscriptItem>, text: &str) {
    if let Some(TranscriptItem::Assistant {
        text: bloc,
        done: false,
    }) = items.last_mut()
    {
        bloc.push_str(text);
        return;
    }
    items.push(TranscriptItem::Assistant {
        text: text.to_string(),
        done: false,
    });
}

/// Ferme le bloc assistant ouvert à `TurnEnded` ; le texte complet de `response`
/// fait foi quand il n'est pas vide.
fn finish_assistant(items: &mut Vec<TranscriptItem>, response: &str) {
    match items.last_mut() {
        Some(TranscriptItem::Assistant { text, done }) => {
            if !response.is_empty() {
                *text = response.to_string();
            }
            *done = true;
        }
        _ if !response.is_empty() => items.push(TranscriptItem::Assistant {
            text: response.to_string(),
            done: true,
        }),
        _ => {}
    }
}

/// Ferme le dernier bloc assistant ouvert, sans changer son texte.
fn close_open_blocks(items: &mut [TranscriptItem]) {
    if let Some(TranscriptItem::Assistant { done, .. }) = items.last_mut() {
        *done = true;
    }
}

/// Tient vivant au moins un sous-agent de la transcription (sondage).
fn any_subagent_running(items: &[TranscriptItem]) -> bool {
    items.iter().any(|item| {
        matches!(
            item,
            TranscriptItem::Subagent {
                status: SubagentStatus::Running,
                ..
            }
        )
    })
}

/// Pose le statut de la ligne du run `run` (sans effet si elle n'existe pas).
fn set_subagent_status(items: &mut [TranscriptItem], run: &str, status: SubagentStatus) {
    for item in items.iter_mut() {
        if let TranscriptItem::Subagent {
            run: id,
            status: slot,
            ..
        } = item
            && id == run
        {
            *slot = status;
        }
    }
}

/// Crée ou met à jour la ligne de statut d'un sous-agent rapportée par le sondage.
fn apply_run_line(state: &mut AppState, line: RunLine) {
    let present = state
        .transcript
        .iter()
        .any(|item| matches!(item, TranscriptItem::Subagent { run, .. } if *run == line.run));
    if present {
        set_subagent_status(&mut state.transcript, &line.run, line.status);
    } else {
        state.transcript.push(TranscriptItem::Subagent {
            run: line.run,
            agent: line.agent,
            status: line.status,
        });
        mark_new_content(state);
    }
    state.revision = state.revision.wrapping_add(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lymaille::{Redactor, ScriptInput, ScriptRecord, Timestamp, TurnId};
    use std::time::Duration;

    fn idle() -> AppState {
        AppState::new("agent-x", "session-1".to_string(), None, (100, 30))
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> Action {
        Action::Key(KeyEvent::new(code, mods))
    }

    fn with_input(text: &str) -> AppState {
        let mut st = idle();
        st.input = editor::apply(EditorState::new(), EditCommand::InsertStr(text.to_string()));
        st
    }

    /// Évènement de la session `session-1` portant `kind`.
    fn evt(kind: EventKind) -> Action {
        Action::Session(Event::new(
            SessionId::new("session-1").expect("un identifiant non vide"),
            Timestamp::from_millis(0),
            kind,
        ))
    }

    fn texte(turn: u64, text: &str) -> EventKind {
        EventKind::TextDelta {
            turn: TurnId::new(turn),
            text: text.to_string(),
        }
    }

    fn fin(turn: u64, response: &str) -> EventKind {
        EventKind::TurnEnded {
            turn: TurnId::new(turn),
            outcome: Outcome::Valid,
            response: response.to_string(),
        }
    }

    /// État au repos avec un tour lancé par « question », saisie vidée.
    fn running() -> AppState {
        let (mut st, effects) = update(
            with_input("question"),
            key(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(effects, vec![Effect::StartTurn("question".to_string())]);
        st.input = EditorState::new();
        st
    }

    fn notices(st: &AppState) -> Vec<(NoticeLevel, String)> {
        st.transcript
            .iter()
            .filter_map(|i| match i {
                TranscriptItem::Notice { level, text } => Some((*level, text.clone())),
                _ => None,
            })
            .collect()
    }

    /// Scenario: envoi d'un message au repos.
    #[test]
    fn envoi_d_un_message() {
        let (st, effects) = update(
            with_input("bonjour"),
            key(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(effects, vec![Effect::StartTurn("bonjour".to_string())]);
        assert!(matches!(st.status, Status::Running { .. }));
        assert!(st.transcript.contains(&TranscriptItem::User {
            text: "bonjour".to_string(),
            during_turn: false,
        }));
        assert!(editor::is_blank(&st.input), "la saisie est vidée");
        assert_eq!(st.input.history, vec!["bonjour".to_string()]);
    }

    /// Scenario: saisie vide n'envoie rien.
    #[test]
    fn saisie_vide_n_envoie_rien() {
        let (_st, effects) = update(with_input("   "), key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(effects.is_empty(), "aucun effet, obtenu: {effects:?}");
    }

    /// Scenario: message pendant un tour — injection, pas de second tour.
    #[test]
    fn message_pendant_un_tour() {
        let st = {
            let mut st = running();
            st.input = editor::apply(
                EditorState::new(),
                EditCommand::InsertStr("bis".to_string()),
            );
            st
        };
        let (st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(effects, vec![Effect::InjectInput("bis".to_string())]);
        assert!(st.transcript.contains(&TranscriptItem::User {
            text: "bis".to_string(),
            during_turn: true,
        }));
        assert!(matches!(st.status, Status::Running { .. }));
    }

    /// Scenario: Ctrl+C annule le tour, le texte reçu est conservé.
    #[test]
    fn ctrl_c_annule_le_tour() {
        let st = running();
        let (st, _) = update(st, evt(texte(1, "déjà reçu")));
        let (st, effects) = update(st, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(effects, vec![Effect::CancelTurn]);
        assert!(st.transcript.contains(&TranscriptItem::Interrupted));
        assert!(st.transcript.contains(&TranscriptItem::Assistant {
            text: "déjà reçu".to_string(),
            done: false,
        }));
        let (st, _) = update(st, Action::TurnFinished(Ok(Outcome::Cancelled)));
        assert_eq!(st.status, Status::Idle, "repos à la fin du tour annulé");
        assert_eq!(
            st.transcript
                .iter()
                .filter(|i| **i == TranscriptItem::Interrupted)
                .count(),
            1,
            "une seule interruption dans la transcription"
        );
    }

    /// Scenario: double Ctrl+C quitte.
    #[test]
    fn double_ctrl_c_quitte() {
        let st = idle();
        let (st, e1) = update(st, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(e1.is_empty());
        let (_st, e2) = update(st, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(e2, vec![Effect::Quit]);
    }

    /// Ctrl+D quitte sur saisie vide et supprime sinon.
    #[test]
    fn ctrl_d_quitte_sur_saisie_vide() {
        let (_st, effects) = update(idle(), key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(effects, vec![Effect::Quit]);
    }

    /// Scenario: fragments de texte prolongent le dernier bloc.
    #[test]
    fn fragments_prolongent_le_dernier_bloc() {
        let st = running();
        let (st, _) = update(st, evt(texte(1, "Bon")));
        let (st, _) = update(st, evt(texte(1, "jour")));
        let (st, _) = update(st, evt(texte(1, " !")));
        let (st, _) = update(st, evt(fin(1, "Bonjour !")));
        let blocs: Vec<&TranscriptItem> = st
            .transcript
            .iter()
            .filter(|i| matches!(i, TranscriptItem::Assistant { .. }))
            .collect();
        assert_eq!(
            blocs,
            vec![&TranscriptItem::Assistant {
                text: "Bonjour !".to_string(),
                done: true,
            }],
            "un seul bloc, dont le texte final est response"
        );
    }

    /// Scenario: exécution de script repliée, dépliable par Espace.
    #[test]
    fn script_replie_et_deplie() {
        let record = ScriptRecord::new(
            &Redactor::new(),
            10,
            ScriptInput::new("agent-x", "print(1)", "1", Duration::from_millis(4)),
        );
        let st = running();
        let (st, _) = update(
            st,
            evt(EventKind::McpCall(lymaille::McpRecord::new(
                &Redactor::new(),
                lymaille::McpInput::new(
                    "monitoring",
                    "query",
                    &serde_json::json!({}),
                    "ok",
                    Duration::from_millis(1),
                ),
            ))),
        );
        let (st, _) = update(st, evt(EventKind::Script(record)));
        let TranscriptItem::Script {
            functions,
            script,
            output,
            expanded,
            ok,
            ..
        } = st.transcript.last().cloned().expect("un élément")
        else {
            panic!("le dernier élément est un Script")
        };
        assert_eq!(functions, vec!["query".to_string()]);
        assert_eq!(script, "print(1)");
        assert_eq!(output, "1");
        assert!(!expanded, "replié à l'arrivée");
        assert!(ok);
        let (st, _) = update(st, key(KeyCode::Tab, KeyModifiers::NONE));
        let (st, _) = update(st, key(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(matches!(
            st.transcript.last(),
            Some(TranscriptItem::Script { expanded: true, .. })
        ));
    }

    /// Scenario: suivi de la fin — remonter fige, les fragments n'avancent plus seuls.
    #[test]
    fn suivi_de_la_fin() {
        let st = idle();
        let (st, _) = update(st, Action::Resize(100, 30));
        let (st, _) = update(st, key(KeyCode::PageUp, KeyModifiers::NONE));
        assert!(!st.scroll.follow, "PgUp fige la position");
        let (st, _) = update(st, evt(texte(1, "x")));
        assert!(!st.scroll.follow);
        assert!(
            st.scroll.pending,
            "du contenu est arrivé pendant le figement"
        );
        let (st, _) = update(st, key(KeyCode::End, KeyModifiers::NONE));
        assert!(st.scroll.follow, "Fin reprend le suivi");
    }

    /// Scenario: commande slash inconnue et /agent — notice, aucun tour.
    #[test]
    fn commande_slash_inconnue_et_agent() {
        for saisie in ["/inconnue", "/agent x"] {
            let (st, effects) = update(with_input(saisie), key(KeyCode::Enter, KeyModifiers::NONE));
            assert!(effects.is_empty(), "aucun tour pour {saisie}: {effects:?}");
            assert!(
                notices(&st)
                    .iter()
                    .any(|(l, t)| *l == NoticeLevel::Warn && t.contains("commande inconnue")),
                "notice « commande inconnue » pour {saisie}"
            );
        }
    }

    /// Scenario: échappement double slash.
    #[test]
    fn echappement_double_slash() {
        let (_st, effects) = update(
            with_input("//ceci"),
            key(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(effects, vec![Effect::StartTurn("/ceci".to_string())]);
    }

    /// Scenario: avertissements de session visibles.
    #[test]
    fn avertissements_de_session_visibles() {
        let st = running();
        let (st, _) = update(
            st,
            evt(EventKind::InferenceRetry {
                attempt: 1,
                delay_ms: 2000,
                error_class: "timeout".into(),
            }),
        );
        let (st, _) = update(
            st,
            evt(EventKind::McpReconnect {
                role: "monitoring".into(),
                attempt: 1,
                delay_ms: 500,
            }),
        );
        let (st, _) = update(st, evt(EventKind::Overrun { elapsed_ms: 90_000 }));
        let warnings = notices(&st)
            .into_iter()
            .filter(|(l, _)| *l == NoticeLevel::Warn)
            .count();
        assert_eq!(warnings, 3, "trois notices d'avertissement");
    }

    /// Scenario: changement de mémoire sans corps.
    #[test]
    fn changement_de_memoire_sans_corps() {
        let (st, _) = update(
            idle(),
            evt(EventKind::MemoryChanged {
                level: "shared".into(),
                id: "deploy-notes".into(),
            }),
        );
        let textes: Vec<String> = notices(&st).into_iter().map(|(_, t)| t).collect();
        assert!(
            textes
                .iter()
                .any(|t| t.contains("shared") && t.contains("deploy-notes"))
        );
    }

    /// Scenario: erreur de tour visible, état au repos.
    #[test]
    fn erreur_de_tour_visible() {
        let (st, effects) = update(running(), Action::TurnFinished(Err("boum".into())));
        assert_eq!(st.status, Status::Idle);
        assert!(
            notices(&st)
                .iter()
                .any(|(l, t)| *l == NoticeLevel::Error && t == "boum")
        );
        assert!(effects.contains(&Effect::RefreshDirtyFiles));
    }

    /// Scenario: panneau des tâches — `TodoChanged` demande la relecture, Ctrl+T affiche.
    #[test]
    fn panneau_des_taches() {
        let (st, effects) = update(idle(), evt(EventKind::TodoChanged));
        assert_eq!(effects, vec![Effect::RefreshTodo]);
        let (st, _) = update(
            st,
            Action::Todo(vec![
                TodoItem {
                    content: "a".into(),
                    status: "pending".into(),
                },
                TodoItem {
                    content: "b".into(),
                    status: "done".into(),
                },
            ]),
        );
        assert_eq!(st.todo.len(), 2);
        let (st, _) = update(st, key(KeyCode::Char('t'), KeyModifiers::CONTROL));
        assert!(st.todo_visible);
    }

    /// Scenario: ligne de sous-agent sondée — Running puis Ended.
    #[test]
    fn ligne_de_subagent_sondee() {
        let (st, _) = update(
            idle(),
            Action::Runs(vec![RunLine {
                run: "run-7".into(),
                agent: "sub".into(),
                status: SubagentStatus::Running,
            }]),
        );
        let (st, _) = update(
            st,
            Action::Runs(vec![RunLine {
                run: "run-7".into(),
                agent: "sub".into(),
                status: SubagentStatus::Ended(Outcome::Valid),
            }]),
        );
        assert!(st.transcript.iter().any(|i| matches!(
            i,
            TranscriptItem::Subagent {
                status: SubagentStatus::Ended(Outcome::Valid),
                ..
            }
        )));
    }

    /// Le sondage ne se déclenche que pour un sous-agent en cours.
    #[test]
    fn tick_sonde_les_subagents_en_cours() {
        let (st, _) = update(
            idle(),
            Action::Runs(vec![RunLine {
                run: "run-7".into(),
                agent: "sub".into(),
                status: SubagentStatus::Running,
            }]),
        );
        let mut effets = Vec::new();
        let mut courant = st;
        for _ in 0..POLL_EVERY_TICKS {
            let (suivant, e) = update(courant, Action::Tick);
            courant = suivant;
            effets.extend(e);
        }
        assert_eq!(effets, vec![Effect::PollRuns]);
    }

    /// Scenario: séquence inattendue — aucun plantage, état valide.
    #[test]
    fn sequence_inattendue() {
        let record = lymaille::McpRecord::new(
            &Redactor::new(),
            lymaille::McpInput::new(
                "monitoring",
                "query",
                &serde_json::json!({}),
                "ok",
                Duration::from_millis(1),
            ),
        );
        let (st, _) = update(idle(), evt(EventKind::McpCall(record)));
        let (st, _) = update(st, evt(fin(1, "")));
        assert_eq!(st.status, Status::Idle);
    }

    /// Les jetons s'additionnent, le titre suit `TitleChanged`.
    #[test]
    fn usage_et_titre() {
        let (st, _) = update(
            idle(),
            evt(EventKind::TokenUsage {
                prompt: 3,
                completion: 4,
            }),
        );
        let (st, _) = update(
            st,
            evt(EventKind::TokenUsage {
                prompt: 1,
                completion: 1,
            }),
        );
        assert_eq!(
            st.usage,
            Usage {
                prompt: 4,
                completion: 5
            }
        );
        let (st, _) = update(
            st,
            evt(EventKind::TitleChanged {
                title: "Migration Loki".into(),
            }),
        );
        assert_eq!(st.title.as_deref(), Some("Migration Loki"));
    }

    /// `/sessions` demande la liste ; Entrée sur le sélecteur reprend la session choisie.
    #[test]
    fn sessions_selecteur_et_reprise() {
        let (_st, effects) = update(
            with_input("/sessions"),
            key(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(effects, vec![Effect::ListSessions]);
        let (st, _) = update(idle(), Action::SessionList(Vec::new()));
        assert!(st.picker.is_none(), "liste vide : pas de sélecteur");
        assert!(
            notices(&st)
                .iter()
                .any(|(_, t)| t.contains("aucune session"))
        );
    }

    /// Une erreur asynchrone non bloquante devient une notice d'avertissement.
    #[test]
    fn avertissement_asynchrone_visible() {
        let (st, _) = update(idle(), Action::Warning("worktree illisible".into()));
        assert!(
            notices(&st)
                .iter()
                .any(|(l, t)| *l == NoticeLevel::Warn && t == "worktree illisible")
        );
    }

    /// Fin de tour réussie : repos, et relecture du worktree et des tâches.
    #[test]
    fn fin_de_tour_sans_resultat_reste_valide() {
        let (st, effects) = update(idle(), Action::TurnFinished(Ok(Outcome::Valid)));
        assert_eq!(st.status, Status::Idle);
        assert!(effects.contains(&Effect::RefreshDirtyFiles));
    }

    /// Les totaux du worktree sont posés tels que lus.
    #[test]
    fn fichiers_modifies() {
        let (st, _) = update(idle(), Action::DirtyFiles(Some(3)));
        assert_eq!(st.dirty_files, Some(3));
        let (st, _) = update(st, Action::DirtyFiles(None));
        assert_eq!(st.dirty_files, None);
    }

    /// Alt+Entrée et Ctrl+J créent une nouvelle ligne sans envoyer.
    #[test]
    fn alt_entree_et_ctrl_j_nouvelle_ligne() {
        let st = with_input("ab");
        let (st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::ALT));
        assert!(effects.is_empty());
        assert_eq!(st.input.lines.len(), 2);
        let (st, effects) = update(st, key(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert!(effects.is_empty());
        assert_eq!(st.input.lines.len(), 3);
    }

    /// Le collage ne déclenche jamais d'envoi.
    #[test]
    fn collage_multiligne_sans_envoi() {
        let (st, effects) = update(idle(), Action::Paste("a\nb\nc".to_string()));
        assert!(effects.is_empty());
        assert_eq!(st.input.lines.len(), 3);
    }
}
