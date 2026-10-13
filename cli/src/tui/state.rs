//! Modèle du chat interactif et mise à jour pure — contrat complet : `./state.sdd`.
//!
//! Architecture « Elm » : `update(state, action) -> (state, Vec<Effect>)`,
//! sans aucune I/O, horloge ni aléa. La transcription, l'état d'attente, le
//! défilement, le panneau de tâches, l'historique et la politique des touches
//! vivent ici ; tout contact terminal/disque/réseau est un [`Effect`] exécuté
//! par `./mod.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use vanyline_lib::VnyError;
use vanyline_lib::event::{ChatEvent, ChatTurnResult};

use crate::tui::editor::{self, EditCommand, EditorState};

/// Niveau d'une `Notice` dans la transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    /// Information.
    Info,
    /// Avertissement (ex. serveur MCP indisponible).
    Warn,
    /// Erreur (jamais silencieuse).
    Error,
}

/// Élément de la transcription du chat.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptItem {
    /// Message de l'utilisateur.
    User(String),
    /// Bloc assistant (texte markdown en cours ou terminé).
    Assistant {
        /// Texte accumulé.
        text: String,
        /// `true` après `Done` (le bloc est fermé).
        done: bool,
    },
    /// Raisonnement du modèle (replié par défaut).
    Reasoning {
        /// Texte accumulé.
        text: String,
        /// Replié/déplié.
        expanded: bool,
    },
    /// Appel d'outil et son résultat (replié par défaut).
    ToolCall {
        /// `internal_call_id` de corrélation.
        id: String,
        /// Nom de l'outil.
        name: String,
        /// Arguments (JSON compact).
        args: String,
        /// Résultat, une fois reçu.
        result: Option<String>,
        /// Le résultat portait `is_error`.
        is_error: bool,
        /// Replié/déplié.
        expanded: bool,
    },
    /// Tour d'un sous-agent, éléments imbriqués.
    Subagent {
        /// Identifiant du sous-agent.
        id: String,
        /// Nom de l'agent invoqué.
        agent: String,
        /// Tâme confiée.
        task: String,
        /// Événements imbriqués du sous-agent.
        items: Vec<TranscriptItem>,
        /// Terminé (`SubagentEnd` reçu).
        done: bool,
    },
    /// Notification info/avertissement/erreur (visible, jamais avalée).
    Notice {
        /// Niveau de la notification.
        level: NoticeLevel,
        /// Texte.
        text: String,
    },
    /// Tour interrompu par l'utilisateur.
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

/// Entrée du panneau des tâches (relue des appels d'outil de tâches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    /// Description de la tâche.
    pub content: String,
    /// Statut (`pending`/`in_progress`/`done`…).
    pub status: String,
}

/// Cumul des jetons d'usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Jetons d'entrée cumulés.
    pub input: u64,
    /// Jetons de sortie cumulés.
    pub output: u64,
}

/// État complet du chat (modèle pur).
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    /// Transcription, dans l'ordre d'apparition.
    pub transcript: Vec<TranscriptItem>,
    /// Attente d'un tour.
    pub status: Status,
    /// Zone de saisie.
    pub input: EditorState,
    /// Historique de défilement.
    pub scroll: Scroll,
    /// Tâches du panneau.
    pub todo: Vec<TodoItem>,
    /// Panneau des tâches visible.
    pub todo_visible: bool,
    /// Jetons cumulés.
    pub usage: Usage,
    /// Agent courant (le changement prend effet au tour suivant).
    pub agent: String,
    /// Identifiant de conversation affiché.
    pub conversation_id: String,
    /// Titre de conversation.
    pub title: Option<String>,
    /// Résumé `git diff` de la barre d'état.
    pub git_summary: Option<String>,
    /// Premier Ctrl+C au repos armé : le second quitte.
    pub quit_armed: bool,
    /// Taille du terminal (lignes colonnes).
    pub size: (u16, u16),
    /// Horloge interne avancée par `Action::Tick` (animation d'attente).
    pub tick: u64,
    /// Index (transcription) de l'élément repliable sélectionné par Tab.
    pub selected: Option<usize>,
    /// Version de la transcription (clé du cache de lignes de la vue).
    pub revision: u64,
}

impl AppState {
    /// État initial : transcription vide, agent et conversation donnés.
    #[must_use]
    pub fn new(
        agent: impl Into<String>,
        conversation_id: String,
        title: Option<String>,
        size: (u16, u16),
    ) -> Self {
        Self {
            transcript: Vec::new(),
            status: Status::Idle,
            input: EditorState::new(),
            scroll: Scroll::default(),
            todo: Vec::new(),
            todo_visible: false,
            usage: Usage::default(),
            agent: agent.into(),
            conversation_id,
            title,
            git_summary: None,
            quit_armed: false,
            size,
            tick: 0,
            selected: None,
            revision: 0,
        }
    }
}

/// Valeurs par défaut (agent et conversation vides) — pratique pour déplacer
/// un état hors d'une variable (`std::mem::take`) dans la boucle et les tests.
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
            conversation_id: String::new(),
            title: None,
            git_summary: None,
            quit_armed: false,
            size: (80, 24),
            tick: 0,
            selected: None,
            revision: 0,
        }
    }
}

/// Action appliquée par [`update`] — tout événement extérieur devient une
/// action (clavier, collage, redimension, souris, jeton d'agent, horloge).
/// Pas de derive : `ChatTurnResult`/`VnyError` ne sont ni `Clone` ni `Debug`.
pub enum Action {
    /// Touche du clavier.
    Key(KeyEvent),
    /// Collage multi-lignes (entre en saisie, ne déclenche jamais d'envoi).
    Paste(String),
    /// Redimensionnement (colonnes, lignes).
    Resize(u16, u16),
    /// Souris (molette).
    Mouse(crossterm::event::MouseEventKind),
    /// Horloge de rafraîchissement (100 ms).
    Tick,
    /// Événement de l'agent reçu du [`TuiSink`](crate::tui::TuiSink).
    Agent(ChatEvent),
    /// Fin de la tâche de tour (succès ou erreur).
    TurnFinished(Result<ChatTurnResult, VnyError>),
    /// Résumé `git diff` rafraîchi (barre d'état).
    GitSummary(Option<String>),
}

/// Commande produite par [`update`] et exécutée par la boucle de `./mod.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Lancer un tour avec ce message.
    StartTurn(String),
    /// Annuler le tour en cours.
    CancelTurn,
    /// Persister la conversation (uniquement après un tour réussi).
    Save,
    /// Quitter le TUI.
    Quit,
    /// Rafraîchir le résumé `git diff`.
    RefreshGit,
    /// Nouvelle conversation.
    NewConversation,
    /// Changer d'agent au tour suivant.
    SwitchAgent(String),
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
            (state, Vec::new())
        }
        Action::Agent(event) => {
            agent_update(&mut state, event);
            (state, Vec::new())
        }
        Action::TurnFinished(result) => {
            state.status = Status::Idle;
            let effects = match result {
                Ok(_) => vec![Effect::Save, Effect::RefreshGit],
                Err(e) => {
                    push_notice(&mut state, NoticeLevel::Error, e.to_string());
                    Vec::new()
                }
            };
            (state, effects)
        }
        Action::GitSummary(summary) => {
            state.git_summary = summary;
            (state, Vec::new())
        }
    }
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
            state.scroll.follow = true;
            state.scroll.pending = false;
        }
    }
}

/// Gèle la position de défilement à `offset` lignes depuis le bas.
fn scroll_page(state: &mut AppState, page: usize, up: bool) {
    scroll_by(state, page.max(1), up);
}

/// Reprend le suivi de la fin.
fn scroll_follow(state: &mut AppState) {
    state.scroll.follow = true;
    state.scroll.offset = 0;
    state.scroll.pending = false;
}

/// Indices des éléments repliables (raisonnement et appels d'outils) au niveau
/// racine de la transcription.
fn collapsible_indices(transcript: &[TranscriptItem]) -> Vec<usize> {
    transcript
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            TranscriptItem::Reasoning { .. } | TranscriptItem::ToolCall { .. } => Some(i),
            _ => None,
        })
        .collect()
}

/// Déplace la sélection Tab/Maj+Tab vers l'élément repliable suivant/précédent.
fn move_selection(state: &mut AppState, forward: bool) {
    let candidates = collapsible_indices(&state.transcript);
    if candidates.is_empty() {
        state.selected = None;
        return;
    }
    let pos = state
        .selected
        .and_then(|sel| candidates.iter().position(|i| *i == sel));
    let next = match (pos, forward) {
        (Some(p), true) => p.saturating_add(1).min(candidates.len().saturating_sub(1)),
        (Some(p), false) => p.saturating_sub(1),
        (None, true) => 0,
        (None, false) => candidates.len().saturating_sub(1),
    };
    state.selected = candidates.get(next).copied();
}

/// Bascule le repli de l'élément sélectionné.
fn toggle_selected(state: &mut AppState) {
    let Some(idx) = state.selected else {
        return;
    };
    if let Some(
        TranscriptItem::Reasoning { expanded, .. } | TranscriptItem::ToolCall { expanded, .. },
    ) = state.transcript.get_mut(idx)
    {
        *expanded = !*expanded;
        state.revision = state.revision.wrapping_add(1);
    }
}

/// Ctrl+R : si un raisonnement est déplié, replie tous ; sinon déplie tous.
fn toggle_all_reasoning(state: &mut AppState) {
    let any_open = state
        .transcript
        .iter()
        .any(|i| matches!(i, TranscriptItem::Reasoning { expanded: true, .. }));
    for item in &mut state.transcript {
        if let TranscriptItem::Reasoning { expanded, .. } = item {
            *expanded = !any_open;
        }
    }
    state.revision = state.revision.wrapping_add(1);
}

/// Politique des touches (état `Idle` ou `Running`).
///
/// Une table unique parce que c'est le contrat entier de `state.sdd`
/// §Handles « Touches » — la découper éparpillerait la politique.
#[allow(clippy::too_many_lines)] // table contractée par state.sdd §Handles
fn key_update(mut state: AppState, key: KeyEvent) -> (AppState, Vec<Effect>) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('c') if ctrl => {
            if matches!(state.status, Status::Running { .. }) {
                state.transcript.push(TranscriptItem::Interrupted);
                state.revision = state.revision.wrapping_add(1);
                state.status = Status::Idle;
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
            if state.input.lines.len() == 1 && state.input.lines[0].is_empty() {
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
        KeyCode::Enter => {
            let (state, effects) = submit(state);
            (state, effects)
        }
        KeyCode::Char('l') if ctrl => (state, Vec::new()),
        KeyCode::Char('r') if ctrl => {
            toggle_all_reasoning(&mut state);
            (state, Vec::new())
        }
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
        KeyCode::Char(' ') if state.selected.is_some() && editor::is_blank(&state.input) => {
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

/// Entrée : envoi de la saisie, commande slash, ou Notice « tour en cours ».
fn submit(mut state: AppState) -> (AppState, Vec<Effect>) {
    if matches!(state.status, Status::Running { .. }) {
        push_notice(
            &mut state,
            NoticeLevel::Warn,
            "tour en cours — patientez".to_string(),
        );
        return (state, Vec::new());
    }
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
    send_message(state, &text)
}

/// Envoie `text` comme message utilisateur : effet `StartTurn`, état Running.
fn send_message(mut state: AppState, text: &str) -> (AppState, Vec<Effect>) {
    state
        .transcript
        .push(TranscriptItem::User(text.to_string()));
    state.revision = state.revision.wrapping_add(1);
    scroll_follow(&mut state);
    state.input = editor::push_history(
        editor::apply(std::mem::take(&mut state.input), EditCommand::Clear),
        text,
    );
    state.status = Status::Running { since: state.tick };
    state.quit_armed = false;
    state.selected = None;
    (state, vec![Effect::StartTurn(text.to_string())])
}

/// Interprète une commande slash (une ligne commençant par `/`).
fn slash_command(mut state: AppState, line: &str) -> (AppState, Vec<Effect>) {
    let (cmd, arg) = match line.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    let clear = |state: &mut AppState| {
        state.input = editor::apply(state.input.clone(), EditCommand::Clear);
    };
    match cmd {
        "/help" => {
            clear(&mut state);
            push_notice(
                &mut state,
                NoticeLevel::Info,
                "Entrée envoyer · Alt+Entrée/Ctrl+J nouvelle ligne · Ctrl+C annuler/vider \
                 · Ctrl+D quitter · PgUp/PgDn/molette défiler · Fin suit la fin · Tab sélection \
                 · Espace replier · Ctrl+R raisonnements · Ctrl+T tâches · /new /agent <nom> \
                 /todo /quit"
                    .to_string(),
            );
            (state, Vec::new())
        }
        "/new" => {
            clear(&mut state);
            (state, vec![Effect::NewConversation])
        }
        "/agent" => {
            if arg.is_empty() {
                push_notice(
                    &mut state,
                    NoticeLevel::Warn,
                    "usage : /agent <nom>".to_string(),
                );
                return (state, Vec::new());
            }
            clear(&mut state);
            state.agent = arg.to_string();
            (state, vec![Effect::SwitchAgent(arg.to_string())])
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

/// Applique un événement de l'agent à la transcription (racine).
fn agent_update(state: &mut AppState, event: ChatEvent) {
    match event {
        ChatEvent::Token { content } => {
            push_token(&mut state.transcript, &content);
            state.revision = state.revision.wrapping_add(1);
            mark_new_content(state);
        }
        ChatEvent::ReasoningDelta { content } => {
            push_reasoning(&mut state.transcript, &content);
            state.revision = state.revision.wrapping_add(1);
            mark_new_content(state);
        }
        ChatEvent::ToolCall { id, name, args } => {
            if name == TODO_TOOL {
                reload_todo(state, &args);
            }
            state.transcript.push(TranscriptItem::ToolCall {
                id,
                name,
                args: args.to_string(),
                result: None,
                is_error: false,
                expanded: false,
            });
            state.revision = state.revision.wrapping_add(1);
            mark_new_content(state);
        }
        ChatEvent::ToolResult {
            id,
            result,
            is_error,
            ..
        } => {
            attach_result(&mut state.transcript, &id, result, is_error);
            state.revision = state.revision.wrapping_add(1);
        }
        ChatEvent::SubagentStart { id, agent, task } => {
            state.transcript.push(TranscriptItem::Subagent {
                id,
                agent,
                task,
                items: Vec::new(),
                done: false,
            });
            state.revision = state.revision.wrapping_add(1);
            mark_new_content(state);
        }
        ChatEvent::SubagentEvent { id, event } => {
            if let Some(inner) = find_subagent_items(&mut state.transcript, &id) {
                apply_nested(inner, *event);
            }
            state.revision = state.revision.wrapping_add(1);
            mark_new_content(state);
        }
        ChatEvent::SubagentEnd { id, result } => {
            mark_subagent_done(&mut state.transcript, &id, &result);
            state.revision = state.revision.wrapping_add(1);
        }
        ChatEvent::Usage {
            input_tokens,
            output_tokens,
        } => {
            state.usage.input = state.usage.input.saturating_add(input_tokens);
            state.usage.output = state.usage.output.saturating_add(output_tokens);
        }
        ChatEvent::ToolUnavailable { server, reason } => {
            push_notice(
                state,
                NoticeLevel::Warn,
                format!("serveur MCP indisponible : {server} ({reason})"),
            );
        }
        ChatEvent::Error { code, message } => {
            push_notice(state, NoticeLevel::Error, format!("[{code}] {message}"));
        }
        ChatEvent::Done => {
            close_open_blocks(&mut state.transcript);
            state.revision = state.revision.wrapping_add(1);
        }
        ChatEvent::SkillLoaded { .. } => {}
    }
}

/// Prolonge le dernier bloc assistant ouvert, ou en ouvre un nouveau.
fn push_token(items: &mut Vec<TranscriptItem>, content: &str) {
    if let Some(TranscriptItem::Assistant { text, done: false }) = items.last_mut() {
        text.push_str(content);
        return;
    }
    items.push(TranscriptItem::Assistant {
        text: content.to_string(),
        done: false,
    });
}

/// Prolonge le dernier bloc de raisonnement ouvert, ou en ouvre un nouveau.
fn push_reasoning(items: &mut Vec<TranscriptItem>, content: &str) {
    if let Some(TranscriptItem::Reasoning { text, .. }) = items.last_mut() {
        text.push_str(content);
        return;
    }
    items.push(TranscriptItem::Reasoning {
        text: content.to_string(),
        expanded: false,
    });
}

/// Rattache un `ToolResult` à l'appel de même `id` (replié conservé) ; un id
/// inconnu est ignoré sans perte ni panique (ordre inattendu toléré).
fn attach_result(items: &mut [TranscriptItem], id: &str, result: String, is_error: bool) {
    for item in items.iter_mut() {
        match item {
            TranscriptItem::ToolCall {
                id: tid,
                result: slot,
                is_error: flag,
                ..
            } if tid == id => {
                *slot = Some(result);
                *flag = is_error;
                return;
            }
            TranscriptItem::Subagent { items, .. } => {
                attach_result(items, id, result.clone(), is_error);
            }
            _ => {}
        }
    }
}

/// Retrouve les éléments imbriqués du sous-agent d'`id` (récursif).
fn find_subagent_items<'a>(
    items: &'a mut [TranscriptItem],
    id: &str,
) -> Option<&'a mut Vec<TranscriptItem>> {
    for item in items.iter_mut() {
        if let TranscriptItem::Subagent {
            id: sid,
            items: inner,
            ..
        } = item
        {
            if sid == id {
                return Some(inner);
            }
            if let Some(found) = find_subagent_items(inner, id) {
                return Some(found);
            }
        }
    }
    None
}

/// Marque le sous-agent d'`id` terminé (récursif) ; un résultat non vide est
/// ajouté à ses éléments comme bloc assistant.
fn mark_subagent_done(items: &mut [TranscriptItem], id: &str, result: &str) {
    for item in items.iter_mut() {
        if let TranscriptItem::Subagent {
            id: sid,
            items: inner,
            done,
            ..
        } = item
        {
            if sid == id {
                *done = true;
                if !result.is_empty() {
                    // Le résultat forme son propre bloc (les blocs ouverts sont
                    // fermés d'abord) : rien de ce qui a été émis n'est fondu.
                    close_open_blocks(inner);
                    push_token(inner, result);
                }
            }
            mark_subagent_done(inner, id, result);
        }
    }
}

/// Applique un événement imbriqué à la liste d'éléments d'un sous-agent
/// (mêmes règles que la racine, sans effet sur le défilement).
// `&mut Vec` requis : cette fonction pousse des éléments (ptr_arg non applicable ici).
fn apply_nested(items: &mut Vec<TranscriptItem>, event: ChatEvent) {
    match event {
        ChatEvent::Token { content } => push_token(items, &content),
        ChatEvent::ReasoningDelta { content } => push_reasoning(items, &content),
        ChatEvent::ToolCall { id, name, args } => {
            items.push(TranscriptItem::ToolCall {
                id,
                name,
                args: args.to_string(),
                result: None,
                is_error: false,
                expanded: false,
            });
        }
        ChatEvent::ToolResult {
            id,
            result,
            is_error,
            ..
        } => attach_result(items, &id, result, is_error),
        ChatEvent::SubagentStart { id, agent, task } => items.push(TranscriptItem::Subagent {
            id,
            agent,
            task,
            items: Vec::new(),
            done: false,
        }),
        ChatEvent::SubagentEvent { id, event } => {
            if let Some(inner) = find_subagent_items(items, &id) {
                apply_nested(inner, *event);
            }
        }
        ChatEvent::SubagentEnd { id, .. } => {
            for i in items.iter_mut() {
                if let TranscriptItem::Subagent { id: sid, done, .. } = i
                    && sid == &id
                {
                    *done = true;
                }
            }
        }
        ChatEvent::Done => close_open_blocks(items),
        ChatEvent::Error { code, message } => items.push(TranscriptItem::Notice {
            level: NoticeLevel::Error,
            text: format!("[{code}] {message}"),
        }),
        ChatEvent::ToolUnavailable { server, reason } => items.push(TranscriptItem::Notice {
            level: NoticeLevel::Warn,
            text: format!("serveur MCP indisponible : {server} ({reason})"),
        }),
        ChatEvent::Usage { .. } | ChatEvent::SkillLoaded { .. } => {}
    }
}

/// Ferme le dernier bloc assistant ouvert (réception de `Done`).
fn close_open_blocks(items: &mut [TranscriptItem]) {
    if let Some(TranscriptItem::Assistant { done, .. }) = items.last_mut() {
        *done = true;
    }
}

/// Relit la liste de tâches depuis les arguments d'un `ToolCall` de
/// l'outil de tâches (`todos`) — pure, sans accès au `Mutex` partagé.
fn reload_todo(state: &mut AppState, args: &serde_json::Value) {
    let Some(todos) = args.get("todos").and_then(|v| v.as_array()) else {
        return;
    };
    state.todo = todos
        .iter()
        .filter_map(|t| {
            let content = t.get("content")?.as_str()?.to_string();
            let status = t
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("pending")
                .to_string();
            Some(TodoItem { content, status })
        })
        .collect();
}

/// Nom de l'outil de tâches dont le `ToolCall` porte la liste (`todos` en args).
pub(crate) const TODO_TOOL: &str = "todowrite";

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> AppState {
        AppState::new("a1", "conv-1".to_string(), None, (100, 30))
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> Action {
        Action::Key(KeyEvent::new(code, mods))
    }

    fn with_input(text: &str) -> AppState {
        let mut st = idle();
        st.input = editor::apply(EditorState::new(), EditCommand::InsertStr(text.to_string()));
        st
    }

    fn running() -> AppState {
        let st = with_input("question");
        let (mut st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            effects.contains(&Effect::StartTurn("question".to_string())),
            "préparation Running"
        );
        st.input = EditorState::new();
        st
    }

    /// Scenario: `envoi_d_un_message` — Entrée sur « bonjour » au repos :
    /// `StartTurn`, état Running, élément User, saisie vidée, historique.
    #[test]
    fn envoi_d_un_message() {
        let st = with_input("bonjour");
        let (st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(effects, vec![Effect::StartTurn("bonjour".to_string())]);
        assert!(matches!(st.status, Status::Running { .. }));
        assert!(
            st.transcript
                .iter()
                .any(|i| *i == TranscriptItem::User("bonjour".to_string())),
            "l'élément User('bonjour') est dans la transcription: {:?}",
            st.transcript
        );
        assert!(editor::is_blank(&st.input), "la saisie est vidée");
        assert_eq!(
            st.input.history,
            vec!["bonjour".to_string()],
            "l'historique est mis à jour"
        );
    }

    /// Scenario: `saisie_vide_n_envoie_rien` — Entrée sur saisie blanche :
    /// aucun effet.
    #[test]
    fn saisie_vide_n_envoie_rien() {
        let st = with_input("   ");
        let (_st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(effects.is_empty(), "aucun effet, obtenu: {effects:?}");
    }

    /// Scenario: `entree_pendant_un_tour` — Entrée avec saisie pendant
    /// `Running` : aucun `StartTurn`, une `Notice` « tour en cours ».
    #[test]
    fn entree_pendant_un_tour() {
        let st = running();
        let st = {
            let mut st = st;
            st.input = editor::apply(
                EditorState::new(),
                EditCommand::InsertStr("bis".to_string()),
            );
            st
        };
        let (st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::StartTurn(_))),
            "aucun second tour: {effects:?}"
        );
        let indice = st.transcript.iter().any(|i| match i {
            TranscriptItem::Notice { text, .. } => text.contains("tour en cours"),
            _ => false,
        });
        assert!(indice, "une Notice « tour en cours » est affichée");
    }

    /// Scenario: `ctrl_c_annule_le_tour` — Ctrl+C pendant `Running` :
    /// effet `CancelTurn`, élément `Interrupted`, état `Idle`.
    #[test]
    fn ctrl_c_annule_le_tour() {
        let st = running();
        let (st, effects) = update(st, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(
            effects.contains(&Effect::CancelTurn),
            "CancelTurn: {effects:?}"
        );
        assert_eq!(st.status, Status::Idle);
        assert!(
            st.transcript.contains(&TranscriptItem::Interrupted),
            "la transcription marque le tour interrompu"
        );
    }

    /// Scenario: `double_ctrl_c_quitte` — deux Ctrl+C consécutifs au repos
    /// sur saisie vide : Quit au second.
    #[test]
    fn double_ctrl_c_quitte() {
        let (st, effects) = update(idle(), key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(
            !effects.contains(&Effect::Quit),
            "le premier Ctrl+C ne quitte pas"
        );
        let (_st, effects) = update(st, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(
            effects.contains(&Effect::Quit),
            "le second quitte: {effects:?}"
        );
    }

    /// Scenario: `ctrl_d_quitte_sur_saisie_vide` — Ctrl+D sur saisie vide :
    /// effet Quit.
    #[test]
    fn ctrl_d_quitte_sur_saisie_vide() {
        let (_st, effects) = update(idle(), key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(effects, vec![Effect::Quit]);
    }

    /// Ctrl+D sur saisie non vide : supprime le caractère sous le curseur
    /// (pas de Quit).
    #[test]
    fn ctrl_d_sur_saisie_non_vide_supprime() {
        let st = with_input("abc");
        let (st, effects) = update(st, key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(!effects.contains(&Effect::Quit));
        assert_eq!(editor::text(&st.input), "abc");
        let st = {
            let mut st = st;
            st.input.cursor = (0, 0);
            st
        };
        let (st, _e) = update(st, key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(
            editor::text(&st.input),
            "bc",
            "le caractère sous le curseur saute"
        );
    }

    /// Scenario: `jetons_prolongent_le_dernier_bloc` — trois `Token` sur un
    /// bloc assistant en cours : un seul bloc, concaténation.
    #[test]
    fn jetons_prolongent_le_dernier_bloc() {
        let mut st = idle();
        for c in ["Bon", "jou", "r"] {
            let (st2, _e) = update(st, Action::Agent(ChatEvent::Token { content: c.into() }));
            st = st2;
        }
        let assistants: Vec<_> = st
            .transcript
            .iter()
            .filter(|i| matches!(i, TranscriptItem::Assistant { .. }))
            .collect();
        assert_eq!(assistants.len(), 1, "un seul bloc assistant");
        assert!(
            matches!(
                &st.transcript[0],
                TranscriptItem::Assistant { text, .. } if text == "Bonjour"
            ),
            "concaténé: {:?}",
            st.transcript[0]
        );
    }

    /// Scenario: `appel_d_outil_et_resultat` — `ToolCall` `t1` puis `ToolResult`
    /// `t1` : l'élément porte le résultat, replié.
    #[test]
    fn appel_d_outil_et_resultat() {
        let mut st = idle();
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::ToolCall {
                id: "t1".into(),
                name: "read_file".into(),
                args: serde_json::json!({"path": "f"}),
            }),
        );
        st = st2;
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::ToolResult {
                id: "t1".into(),
                name: "read_file".into(),
                result: "contenu".into(),
                is_error: false,
            }),
        );
        st = st2;
        let item = st
            .transcript
            .iter()
            .find_map(|i| match i {
                TranscriptItem::ToolCall { id, .. } if id == "t1" => Some(i.clone()),
                _ => None,
            })
            .expect("l'élément ToolCall existe");
        match item {
            TranscriptItem::ToolCall {
                result, expanded, ..
            } => {
                assert_eq!(result.as_deref(), Some("contenu"));
                assert!(!expanded, "replié");
            }
            other => panic!("mauvais type: {other:?}"),
        }
    }

    /// Scenario: `suivi_de_la_fin` — jetons qui arrivent sans remontée : le
    /// défilement reste en bas (follow) ; après un `PgUp` il ne bouge plus seul
    /// (figé, indicateur « nouveau »).
    #[test]
    fn suivi_de_la_fin() {
        let mut st = idle();
        for i in 0..5 {
            let (st2, _e) = update(
                st,
                Action::Agent(ChatEvent::Token {
                    content: format!("l{i}"),
                }),
            );
            st = st2;
        }
        assert!(st.scroll.follow, "suit la fin par défaut");
        let (st2, _e) = update(st, key(KeyCode::PageUp, KeyModifiers::NONE));
        st = st2;
        assert!(!st.scroll.follow, "PgUp fige la position");
        let frozen = st.scroll.offset;
        let (st2, _e) = update(
            st.clone(),
            Action::Agent(ChatEvent::Token {
                content: "x".into(),
            }),
        );
        st = st2;
        assert_eq!(st.scroll.offset, frozen, "le décalage figé ne bouge plus");
        assert!(st.scroll.pending, "indicateur de nouveau contenu");
        let (_st, _e) = update(st, key(KeyCode::End, KeyModifiers::NONE));
    }

    /// Scenario: `commande_slash_inconnue` — `/inconnue` : une Notice, aucun tour.
    #[test]
    fn commande_slash_inconnue() {
        let st = with_input("/inconnue");
        let (st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(effects.is_empty(), "aucun tour: {effects:?}");
        assert!(
            st.transcript.iter().any(|i| matches!(
                i,
                TranscriptItem::Notice { text, .. } if text.contains("inconnue")
            )),
            "une Notice mentionne la commande: {:?}",
            st.transcript
        );
    }

    /// Scenario: `echappement_double_slash` — `//ls` envoie `/ls`.
    #[test]
    fn echappement_double_slash() {
        let st = with_input("//ls");
        let (_st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(effects, vec![Effect::StartTurn("/ls".to_string())]);
    }

    /// Scenario: `agent_indisponible_visible` — `ToolUnavailable` « grafana » :
    /// Notice d'avertissement mentionnant « grafana ».
    #[test]
    fn agent_indisponible_visible() {
        let st = idle();
        let (st, _e) = update(
            st,
            Action::Agent(ChatEvent::ToolUnavailable {
                server: "grafana".into(),
                reason: "injoignable".into(),
            }),
        );
        assert!(
            st.transcript.iter().any(|i| matches!(
                i,
                TranscriptItem::Notice {
                    level: NoticeLevel::Warn,
                    text
                } if text.contains("grafana")
            )),
            "Notice warn mentionnant grafana: {:?}",
            st.transcript
        );
    }

    /// Scenario: `erreur_de_tour_visible` — `TurnFinished(Err)` : `Notice`
    /// d'erreur et état `Idle`.
    #[test]
    fn erreur_de_tour_visible() {
        let st = running();
        let (st, _e) = update(
            st,
            Action::TurnFinished(Err(VnyError::LlmError("boom".into()))),
        );
        assert_eq!(st.status, Status::Idle);
        assert!(
            st.transcript.iter().any(|i| matches!(
                i,
                TranscriptItem::Notice {
                    level: NoticeLevel::Error,
                    ..
                }
            )),
            "une Notice d'erreur visible: {:?}",
            st.transcript
        );
    }

    /// Scenario: `sequence_inattendue` — `ToolResult` d'id inconnu puis `Done`
    /// sans bloc : aucun plantage, état valide.
    #[test]
    fn sequence_inattendue() {
        let st = idle();
        let (st, _e) = update(
            st,
            Action::Agent(ChatEvent::ToolResult {
                id: "fantome".into(),
                name: String::new(),
                result: "?".into(),
                is_error: false,
            }),
        );
        let (_st, _e) = update(st, Action::Agent(ChatEvent::Done));
    }

    /// Scenario: `panneau_des_taches` — un `ToolCall` de l'outil de tâches
    /// posant deux éléments, puis Ctrl+T : le panneau liste deux tâches et
    /// devient visible.
    #[test]
    fn panneau_des_taches() {
        let st = idle();
        let (st, _e) = update(
            st,
            Action::Agent(ChatEvent::ToolCall {
                id: "t1".into(),
                name: TODO_TOOL.into(),
                args: serde_json::json!({"todos": [
                    {"content": "écrire les tests", "status": "pending"},
                    {"content": "implémenter", "status": "pending"}
                ]}),
            }),
        );
        assert_eq!(st.todo.len(), 2, "deux tâches relues du ToolCall");
        let (st, _e) = update(st, key(KeyCode::Char('t'), KeyModifiers::CONTROL));
        assert!(st.todo_visible, "Ctrl+T rend le panneau visible");
        assert!(st.todo.iter().all(|t| !t.content.is_empty()));
    }

    /// Scenario: `un_seul_tour_a_la_fois` (volet politique) — Entrée avec une
    /// saisie pendant `Running` ne produit AUCUN `StartTurn`.
    #[test]
    fn un_seul_tour_a_la_fois_politique() {
        let mut st = running();
        st.input = editor::apply(EditorState::new(), EditCommand::InsertStr("x".into()));
        let (_st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::StartTurn(_))),
            "aucun StartTurn pendant un tour: {effects:?}"
        );
    }

    /// Scenario: `tour_reussi_persiste` (volet effets) — `TurnFinished(Ok)` :
    /// effets `Save` + `RefreshGit`, état `Idle` ; `GitSummary` alimente la barre.
    #[test]
    fn tour_reussi_effets_save_et_git() {
        let st = running();
        let (st, effects) = update(
            st,
            Action::TurnFinished(Ok(ChatTurnResult {
                response_text: "ok".into(),
                tool_calls: Vec::new(),
            })),
        );
        assert_eq!(st.status, Status::Idle);
        assert!(effects.contains(&Effect::Save), "Save: {effects:?}");
        assert!(
            effects.contains(&Effect::RefreshGit),
            "RefreshGit: {effects:?}"
        );
        let (st, _e) = update(st, Action::GitSummary(Some("2 files changed".into())));
        assert_eq!(st.git_summary.as_deref(), Some("2 files changed"));
    }

    /// Alt+Entrée et Ctrl+J créent une nouvelle ligne sans envoyer.
    #[test]
    fn alt_entree_et_ctrl_j_nouvelle_ligne() {
        let st = with_input("ab");
        let (st, effects) = update(st, key(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(effects.len(), 0, "Alt+Entrée n'émet aucun effet");
        assert_eq!(st.input.lines.len(), 2, "Alt+Entrée ajoute une ligne");
        let (st, effects) = update(st, key(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(effects.len(), 0, "Ctrl+J n'émet aucun effet");
        assert_eq!(st.input.lines.len(), 3, "Ctrl+J ajoute une ligne");
    }

    /// Tab sélectionne un élément repliable, Espace le déplie ; Ctrl+R
    /// bascule tous les raisonnements.
    #[test]
    fn tab_espace_et_ctrl_r() {
        let mut st = idle();
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::ReasoningDelta {
                content: "ligne1\nligne2".into(),
            }),
        );
        st = st2;
        let (st2, _) = update(st.clone(), key(KeyCode::Tab, KeyModifiers::NONE));
        st = st2;
        assert!(st.selected.is_some(), "Tab sélectionne le raisonnement");
        let (st2, _) = update(st.clone(), key(KeyCode::Char(' '), KeyModifiers::NONE));
        st = st2;
        assert!(
            matches!(
                st.transcript[0],
                TranscriptItem::Reasoning { expanded: true, .. }
            ),
            "Espace déplie la sélection: {:?}",
            st.transcript[0]
        );
        let (st2, _) = update(st, key(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(
            matches!(
                st2.transcript[0],
                TranscriptItem::Reasoning {
                    expanded: false,
                    ..
                }
            ),
            "Ctrl+R replie tous les raisonnements"
        );
    }

    /// Commandes slash `/new`, `/agent x`, `/todo`, `/quit`.
    #[test]
    fn commandes_slash_effets() {
        let (_st, e) = update(with_input("/new"), key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(e.contains(&Effect::NewConversation), "/new: {e:?}");
        let (st, e) = update(
            with_input("/agent fortran"),
            key(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(
            e.contains(&Effect::SwitchAgent("fortran".to_string())),
            "/agent: {e:?}"
        );
        assert_eq!(st.agent, "fortran");
        let (st0, e) = update(idle(), key(KeyCode::Char('t'), KeyModifiers::CONTROL));
        assert!(
            e.is_empty() && st0.todo_visible,
            "Ctrl+T bascule le panneau"
        );
        let (_st, e) = update(with_input("/quit"), key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(e.contains(&Effect::Quit), "/quit: {e:?}");
    }

    /// Usage cumule ; Subagent start/event/end imbriquent et marquent done.
    #[test]
    fn usage_et_subagent() {
        let mut st = idle();
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::Usage {
                input_tokens: 10,
                output_tokens: 5,
            }),
        );
        st = st2;
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::Usage {
                input_tokens: 3,
                output_tokens: 1,
            }),
        );
        st = st2;
        assert_eq!(st.usage.input, 13);
        assert_eq!(st.usage.output, 6);
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::SubagentStart {
                id: "s1".into(),
                agent: "explorateur".into(),
                task: "chercher".into(),
            }),
        );
        st = st2;
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::SubagentEvent {
                id: "s1".into(),
                event: Box::new(ChatEvent::Token {
                    content: "trouvé".into(),
                }),
            }),
        );
        st = st2;
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::SubagentEnd {
                id: "s1".into(),
                result: "fini".into(),
            }),
        );
        st = st2;
        let sub = st.transcript.iter().find_map(|i| match i {
            TranscriptItem::Subagent {
                items, done, agent, ..
            } if agent == "explorateur" => Some((items.clone(), *done)),
            _ => None,
        });
        let (items, done) = sub.expect("le sous-agent est imbriqué");
        assert!(done, "marqué terminé");
        assert!(
            items.iter().any(|i| matches!(i,
                TranscriptItem::Assistant { text, .. } if text == "trouvé")),
            "l'événement imbriqué est stocké: {items:?}"
        );
    }

    /// La molette verso fige le suivi, la molette retour ramène en bas
    /// (Fin reprend le suivi).
    #[test]
    fn molette_defile() {
        let st = idle();
        let (st, _e) = update(st, Action::Mouse(MouseEventKind::ScrollUp));
        assert!(!st.scroll.follow);
        let (st, _e) = update(st, Action::Mouse(MouseEventKind::ScrollDown));
        assert!(st.scroll.follow, "retour en bas reprend le suivi");
    }

    /// Redimensionner met à jour la taille ; Tick avance l'horloge.
    #[test]
    fn resize_et_tick() {
        let (st, _e) = update(idle(), Action::Resize(120, 40));
        assert_eq!(st.size, (120, 40));
        let (st, _e) = update(st, Action::Tick);
        assert!(st.tick >= 1);
    }

    /// Toute insertion désarme `quit_armed` (le double Ctrl+C doit être consécutif).
    #[test]
    fn insertion_desarme_quit() {
        let (st, _e) = update(idle(), key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(st.quit_armed);
        let st = {
            let mut st = st;
            st.input = editor::apply(EditorState::new(), EditCommand::InsertStr("a".into()));
            st
        };
        let (_st, e) = update(st, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(
            !e.contains(&Effect::Quit),
            "Ctrl+C sur saisie non vide ne quitte pas"
        );
    }

    /// Un `ToolResult` avec un préfixe d'ordre inattendu ne perd AUCUN texte
    /// émis (« ne jamais perdre de texte »).
    #[test]
    fn texte_emis_survit_aux_evenements_inattendus() {
        let mut st = idle();
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::Token {
                content: "important".into(),
            }),
        );
        st = st2;
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::Error {
                code: "VNL-LLM-001".into(),
                message: "boom".into(),
            }),
        );
        st = st2;
        let (st2, _) = update(
            st.clone(),
            Action::Agent(ChatEvent::ToolResult {
                id: "x".into(),
                name: String::new(),
                result: "?".into(),
                is_error: true,
            }),
        );
        let _ = st2;
        assert!(
            st.transcript.iter().any(|i| matches!(i,
                TranscriptItem::Assistant { text, .. } if text == "important")),
            "le texte émis reste dans la transcription"
        );
    }
}
