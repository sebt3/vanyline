//! Rendu ratatui de l'état du chat — contrat complet : `./view.sdd`.
//!
//! Rendu PUR dans un tampon ratatui (`TestBackend` dans les tests) :
//! en-tête, transcription défilante, panneau de tâches (droite sur terminal
//! large, au-dessus de la saisie sinon), zone de saisie multi-lignes
//! adaptative (1 à 8 lignes), barre d'état. Le rendu ne lit que l'état passé,
//! ne panique jamais quelle que soit la taille, ne déborde jamais de la zone
//! et ne porte aucune information par la couleur seule. Les lignes rendues de
//! la transcription sont mises en cache par largeur et version de transcript.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::tui::editor;
use crate::tui::markdown;
use crate::tui::state::{AppState, NoticeLevel, Status, SubagentStatus, TranscriptItem};
use lymaille::Outcome;

/// Thème de rendu (styles sobres, lisibles à 16 couleurs ; les statuts portent
/// un symbole, jamais une couleur seule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Texte courant.
    pub text: Style,
    /// Titres markdown.
    pub title: Style,
    /// Code en ligne.
    pub inline_code: Style,
    /// Bloc de code (contenu).
    pub code_block: Style,
    /// Citation.
    pub quote: Style,
    /// Lien.
    pub link: Style,
    /// Puce de liste.
    pub bullet: Style,
    /// Règle horizontale.
    pub rule: Style,
    /// Style atténué (raisonnement, interrupted).
    pub dim: Style,
    /// Marqueur de message utilisateur.
    pub user: Style,
    /// Notice d'information.
    pub info: Style,
    /// Notice d'avertissement.
    pub warn: Style,
    /// Notice d'erreur.
    pub error: Style,
    /// Barre d'état / en-tête.
    pub chrome: Style,
}

impl Theme {
    /// Thème sombre par défaut.
    #[must_use]
    pub fn dark() -> Self {
        Self {
            text: Style::default(),
            title: Style::default().add_modifier(Modifier::BOLD),
            inline_code: Style::default().fg(Color::Yellow),
            code_block: Style::default().fg(Color::Cyan),
            quote: Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::ITALIC),
            link: Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::UNDERLINED),
            bullet: Style::default().fg(Color::Magenta),
            rule: Style::default().fg(Color::DarkGray),
            dim: Style::default().add_modifier(Modifier::DIM),
            user: Style::default().add_modifier(Modifier::BOLD),
            info: Style::default().fg(Color::Blue),
            warn: Style::default().fg(Color::Yellow),
            error: Style::default().fg(Color::Red),
            chrome: Style::default().bg(Color::DarkGray),
        }
    }

    /// Thème clair (couleurs adaptées aux fonds clairs).
    #[must_use]
    pub fn light() -> Self {
        Self {
            code_block: Style::default().fg(Color::Blue),
            link: Style::default().add_modifier(Modifier::UNDERLINED),
            ..Self::dark()
        }
    }
}

/// Déduit le thème de la variable `COLORFGBG` (dernier champ : fond ; « 0 »
/// ⇒ sombre) ; sombre par défaut.
#[must_use]
pub fn detect_theme() -> Theme {
    match std::env::var("COLORFGBG").ok().as_deref() {
        Some(v) => {
            let fundo = v.rsplit(';').next().unwrap_or("0");
            if fundo.starts_with('0') {
                Theme::dark()
            } else {
                Theme::light()
            }
        }
        None => Theme::dark(),
    }
}

/// Largeur minimale pour que le panneau des tâches s'affiche à droite.
const PANEL_RIGHT_MIN_COLS: u16 = 120;
/// Largeur du panneau des tâches quand il est à droite.
const PANEL_RIGHT_COLS: u16 = 40;
/// Largeur minimale du terminal pour la vue complète (en dessous : message).
const MIN_COLS: u16 = 40;
/// Hauteur minimale du terminal pour la vue complète.
const MIN_ROWS: u16 = 10;
/// Nombre de lignes visible maximal de la zone de saisie.
const EDITOR_MAX_ROWS: usize = 8;
/// Bornes du résultat déplié d'un appel d'outil (lignes).
const TOOL_RESULT_MAX_LINES: usize = 40;
/// Characters d'animation d'attente (rotation d'après le tick).
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Rend l'état complet dans la frame. Ne panique jamais, quelle que soit la
/// taille allouée ; place le curseur terminal dans la zone de saisie.
pub fn render(frame: &mut ratatui::Frame, state: &AppState) {
    let theme = detect_theme();
    let area = frame.area();
    if area.width < MIN_COLS || area.height < MIN_ROWS {
        render_too_small(frame.buffer_mut(), area, &theme);
        return;
    }
    let theme = &theme;
    let width = usize::from(area.width);

    // ── hauteurs ──
    let (editor_rows, editor_cursor) = {
        let (lines, cur) = editor::render_view(&state.input, width, EDITOR_MAX_ROWS);
        (lines, cur)
    };
    let editor_h = u16::try_from(editor_rows.len().max(1)).unwrap_or(1);
    let panel_right = state.todo_visible && area.width >= PANEL_RIGHT_MIN_COLS;
    let panel_top = state.todo_visible && !panel_right;
    let panel_h = if panel_top {
        u16::try_from(state.todo.len().saturating_add(1).min(6)).unwrap_or(1)
    } else {
        0
    };

    let transcript_h = area
        .height
        .saturating_sub(1) // en-tête
        .saturating_sub(1) // barre d'état
        .saturating_sub(editor_h)
        .saturating_sub(panel_h);

    let header = Rect::new(0, 0, area.width, 1);
    let body_y = 1u16;
    let (transcript, panel) = if panel_right {
        let pw = PANEL_RIGHT_COLS.min(area.width.saturating_sub(1));
        let tw = area.width.saturating_sub(pw);
        (
            Rect::new(0, body_y, tw, transcript_h),
            Some(Rect::new(tw, body_y, pw, transcript_h)),
        )
    } else {
        (Rect::new(0, body_y, area.width, transcript_h), None)
    };
    let panel_top_area = if panel_top {
        Some(Rect::new(
            0,
            body_y.saturating_add(transcript_h),
            area.width,
            panel_h,
        ))
    } else {
        None
    };
    let editor_area = Rect::new(
        0,
        body_y.saturating_add(transcript_h).saturating_add(panel_h),
        area.width,
        editor_h,
    );
    let status = Rect::new(0, area.height.saturating_sub(1), area.width, 1);

    render_header(frame.buffer_mut(), header, state, theme);
    let tw = usize::from(transcript.width);
    let lines = cached_lines(state, tw, theme);
    render_transcript(frame.buffer_mut(), transcript, &lines, state, tw);
    if let Some(p) = panel.or(panel_top_area) {
        render_todo(frame.buffer_mut(), p, state, theme);
    }
    render_editor(frame.buffer_mut(), editor_area, &editor_rows, theme);
    render_status(frame.buffer_mut(), status, state, theme);

    // curseur dans la saisie
    let cx = u16::try_from(editor_cursor.0)
        .unwrap_or(0)
        .min(area.width.saturating_sub(1));
    let cy = editor_area
        .top()
        .saturating_add(u16::try_from(editor_cursor.1).unwrap_or(0));
    frame.set_cursor_position(Position::new(cx, cy));
}

/// Message de terminal trop petit (aucun calcul sur des zones nulles).
fn render_too_small(buf: &mut Buffer, area: Rect, theme: &Theme) {
    if area.area() == 0 {
        return;
    }
    let msg = "Terminal trop petit (minimum 40×10)";
    let msg = crop_to_width(msg, usize::from(area.width));
    let line = Line::from(Span::styled(msg, theme.warn));
    line.render(area, buf);
}

/// Contenu de la ligne d'en-tête : agent, conversation, titre, état.
fn header_spans(state: &AppState, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled("vanyline", theme.user),
        Span::styled(format!(" · agent {} ", state.agent), theme.text),
    ];
    if let Some(title) = &state.title {
        spans.push(Span::styled(format!("· {title} · "), theme.dim));
    }
    spans.push(Span::styled(
        format!("· session {} ", short_id(&state.session_id)),
        theme.dim,
    ));
    match state.status {
        Status::Idle => spans.push(Span::styled("· prêt", theme.dim)),
        Status::Running { .. } => spans.push(Span::styled("· tour en cours", theme.warn)),
    }
    spans
}

fn render_header(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    let line = Line::from(header_spans(state, theme)).right_aligned();
    line.render(area, buf);
}

/// Tronque une chaîne à `width` colonnes d'affichage (jamais au milieu d'un
/// graphème large : on rend à la borne inférieure).
fn crop_to_width(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for g in editor::graphemes(s) {
        let gw = editor::display_width(g);
        if w.saturating_add(gw) > width {
            break;
        }
        out.push_str(g);
        w = w.saturating_add(gw);
    }
    out
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

// ── transcription (avec cache de lignes par largeur et révision) ──

/// Entrée du cache de lignes : clé (révision, sélection, largeur) → lignes
/// stylées.
type LinesCache = (u64, Option<usize>, usize, Vec<Line<'static>>);

thread_local! {
    static LINES_CACHE: std::cell::RefCell<Option<LinesCache>> =
        const { std::cell::RefCell::new(None) };
}

/// Lignes stylées de la transcription (cache : (révision, sélection, largeur)).
fn cached_lines(state: &AppState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let key = (state.revision, state.selection, width);
    let hit = LINES_CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(rev, sel, w, _)| (*rev, *sel, *w) == key)
            .map(|(_, _, _, lines)| lines.clone())
    });
    if let Some(lines) = hit {
        return lines;
    }
    let lines = build_lines(state, width, theme);
    LINES_CACHE.with(|c| {
        *c.borrow_mut() = Some((state.revision, state.selection, width, lines.clone()));
    });
    lines
}

/// Construit les lignes de la transcription (éléments dans l'ordre ;
/// Assistant rendu par ./markdown.rs).
fn build_lines(state: &AppState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for (idx, item) in state.transcript.iter().enumerate() {
        let selected = state.selection == Some(idx);
        push_item_lines(&mut out, item, width, theme, selected, 0);
    }
    out
}

#[allow(clippy::too_many_lines)] // une branche par élément de transcription : view.sdd §Handles — la longueur est le contrat
fn push_item_lines(
    out: &mut Vec<Line<'static>>,
    item: &TranscriptItem,
    width: usize,
    theme: &Theme,
    selected: bool,
    indent: usize,
) {
    let inner_width = width.saturating_sub(indent);
    let emit = |out: &mut Vec<Line<'static>>, line: Line<'static>| {
        out.push(prefix_indent(line, indent));
    };
    match item {
        TranscriptItem::User { text, during_turn } => {
            let marque = if *during_turn {
                "❯ (pendant le tour) "
            } else {
                "❯ "
            };
            let largeur = inner_width
                .saturating_sub(editor::display_width(marque))
                .max(1);
            for (n, chunk) in wrap_plain(text, largeur).into_iter().enumerate() {
                let prefixe = if n == 0 { marque } else { "  " };
                emit(
                    out,
                    Line::from(vec![
                        Span::styled(prefixe, theme.user),
                        Span::styled(
                            chunk,
                            if selected {
                                base_selected(theme)
                            } else {
                                theme.user
                            },
                        ),
                    ]),
                );
            }
        }
        TranscriptItem::Assistant { text, .. } => {
            for line in markdown::render(text, u16::try_from(inner_width).unwrap_or(0), theme) {
                let line = if selected {
                    restyle(line, base_selected(theme))
                } else {
                    line
                };
                emit(out, line);
            }
        }
        TranscriptItem::Script {
            seq,
            functions,
            duration_ms,
            ok,
            error,
            script,
            output,
            expanded,
        } => {
            // Une seule ligne repliée : fonctions MCP, durée, statut (symbole, pas couleur).
            let arrow = if *expanded { "▾" } else { "▸" };
            let fonctions = if functions.is_empty() {
                String::new()
            } else {
                format!(" · {}", truncate_mid(&functions.join(", "), 40))
            };
            let statut = if *ok { " ✓" } else { " ✗" };
            let head = format!("{arrow} script {seq}{fonctions} · {duration_ms} ms");
            emit(
                out,
                Line::from(vec![
                    Span::styled(
                        crop_to_width(&head, inner_width),
                        if selected {
                            base_selected(theme)
                        } else {
                            theme.text
                        },
                    ),
                    Span::styled(statut, if *ok { theme.dim } else { theme.error }),
                ]),
            );
            if *expanded {
                let mut corps: Vec<(String, Style)> = Vec::new();
                for l in script.lines() {
                    corps.push((l.to_string(), theme.code_block));
                }
                match error {
                    Some(message) => corps.push((format!("erreur : {message}"), theme.error)),
                    None => {
                        for l in output.lines() {
                            corps.push((l.to_string(), theme.dim));
                        }
                    }
                }
                let total = corps.len();
                for (n, (l, style)) in corps.into_iter().enumerate() {
                    if n >= TOOL_RESULT_MAX_LINES {
                        let rest = total.saturating_sub(n);
                        emit(
                            out,
                            Line::from(Span::styled(
                                format!("… {rest} lignes supplémentaires tronquées"),
                                theme.dim,
                            )),
                        );
                        break;
                    }
                    for chunk in wrap_plain(&l, inner_width.max(1)) {
                        emit(out, Line::from(Span::styled(chunk, style)));
                    }
                }
            }
        }
        TranscriptItem::Subagent { run, agent, status } => {
            let etat = match status {
                SubagentStatus::Running => "…",
                SubagentStatus::Ended(Outcome::Failed) => "✗",
                SubagentStatus::Ended(Outcome::Cancelled) => "⊘ annulé",
                SubagentStatus::Ended(_) => "✓",
                SubagentStatus::Interrupted => "⊘ interrompu",
            };
            let head = format!("◆ sous-agent {agent} ({}) {etat}", truncate_mid(run, 24));
            emit(out, Line::from(Span::styled(head, theme.bullet)));
        }
        TranscriptItem::Notice { level, text } => {
            let (sym, style) = match level {
                NoticeLevel::Info => ("ℹ ", theme.info),
                NoticeLevel::Warn => ("⚠ ", theme.warn),
                NoticeLevel::Error => ("✗ ", theme.error),
            };
            let style = if selected {
                base_selected(theme)
            } else {
                style
            };
            for chunk in wrap_plain(text, inner_width.saturating_sub(2).max(1)) {
                emit(
                    out,
                    Line::from(vec![Span::styled(sym, style), Span::styled(chunk, style)]),
                );
            }
        }
        TranscriptItem::Interrupted => emit(
            out,
            Line::from(Span::styled(
                "— tour interrompu —",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::DIM),
            )),
        ),
    }
}

fn base_selected(theme: &Theme) -> Style {
    theme.text.add_modifier(Modifier::REVERSED)
}

fn prefix_indent(line: Line<'static>, indent: usize) -> Line<'static> {
    if indent == 0 {
        return line;
    }
    let mut spans = Vec::with_capacity(line.spans.len().saturating_add(1));
    spans.push(Span::styled(" ".repeat(indent), Style::default()));
    spans.extend(line.spans);
    Line::from(spans)
}

fn restyle(line: Line<'static>, style: Style) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(|s| Span::styled(s.content, s.style.patch(style)))
            .collect::<Vec<_>>(),
    )
}

/// Découpe un texte brut en morceaux ≤ width colonnes (par mots, coupure
/// dure si un mot déborde).
pub(crate) fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for word in para.split(' ') {
            let ww = editor::display_width(word);
            if cur_w == 0 && ww > width {
                // mot plus large que la ligne : coupure dure en graphèmes,
                // le dernier morceau devient la ligne courante.
                let mut pieces = hard_split(word, width);
                let last = pieces.pop().unwrap_or_default();
                out.extend(pieces);
                cur = last;
                cur_w = editor::display_width(&cur);
                continue;
            }
            let add = if cur_w == 0 { ww } else { ww.saturating_add(1) };
            if cur_w.saturating_add(add) <= width {
                if cur_w > 0 {
                    cur.push(' ');
                }
                cur.push_str(word);
                cur_w = cur_w.saturating_add(add);
            } else {
                out.push(std::mem::take(&mut cur));
                if ww > width {
                    let mut pieces = hard_split(word, width);
                    let last = pieces.pop().unwrap_or_default();
                    out.extend(pieces);
                    cur = last;
                    cur_w = editor::display_width(&cur);
                } else {
                    cur.push_str(word);
                    cur_w = ww;
                }
            }
        }
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Coupure dure en graphèmes, morceaux de `width` colonnes d'affichage au plus.
pub(crate) fn hard_split(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut w = 0usize;
    for g in editor::graphemes(s) {
        let gw = editor::display_width(g);
        if w.saturating_add(gw) > width && w > 0 {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push_str(g);
        w = w.saturating_add(gw);
    }
    out.push(cur);
    out
}

fn truncate_mid(s: &str, max: usize) -> String {
    if editor::display_width(s) <= max {
        return s.to_string();
    }
    let keep = max.saturating_div(2);
    let mut head = String::new();
    let mut w = 0usize;
    for g in editor::graphemes(s) {
        let gw = editor::display_width(g);
        if w.saturating_add(gw) > keep {
            break;
        }
        head.push_str(g);
        w = w.saturating_add(gw);
    }
    format!("{head}…")
}

/// Rend la fenêtre de transcription défilée dans `area` (troncature stricte).
fn render_transcript(
    buf: &mut Buffer,
    area: Rect,
    lines: &[Line<'static>],
    state: &AppState,
    width: usize,
) {
    if area.area() == 0 {
        return;
    }
    let h = usize::from(area.height);
    let total = lines.len();
    let top = if state.scroll.follow {
        total.saturating_sub(h)
    } else {
        total
            .saturating_sub(h)
            .saturating_sub(state.scroll.offset.min(total))
    };
    for (row, line) in lines.iter().skip(top).take(h).enumerate() {
        let y = area.top().saturating_add(u16::try_from(row).unwrap_or(0));
        if y >= area.bottom() {
            break;
        }
        let cell = Rect::new(area.left(), y, area.width, 1);
        let clipped = clip_line(line, width);
        clipped.render(cell, buf);
    }
}

/// Tronque une ligne à la largeur de la zone (aucune écriture hors zone).
fn clip_line(line: &Line<'static>, width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;
    for span in &line.spans {
        if truncated {
            break;
        }
        let mut piece = String::new();
        let mut piece_w = 0usize;
        for g in editor::graphemes(span.content.as_ref()) {
            let gw = editor::display_width(g);
            if used.saturating_add(piece_w).saturating_add(gw) > width {
                truncated = true;
                break;
            }
            piece.push_str(g);
            piece_w = piece_w.saturating_add(gw);
        }
        if piece_w > 0 {
            spans.push(Span::styled(piece, span.style));
            used = used.saturating_add(piece_w);
        }
    }
    Line::from(spans)
}

/// Panneau des tâches (visible seulement, liste bornée).
fn render_todo(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    if area.area() == 0 {
        return;
    }
    let title = Line::from(Span::styled("tâches", theme.chrome));
    title.render(Rect::new(area.left(), area.top(), area.width, 1), buf);
    let h = usize::from(area.height).saturating_sub(1);
    for (row, item) in state.todo.iter().take(h).enumerate() {
        let y = area
            .top()
            .saturating_add(1)
            .saturating_add(u16::try_from(row).unwrap_or(0));
        if y >= area.bottom() {
            break;
        }
        let mark = match item.status.as_str() {
            "done" => "[✓]",
            "in_progress" => "[…]",
            _ => "[ ]",
        };
        let line = Line::from(Span::styled(
            crop_to_width(&format!("{mark} {}", item.content), usize::from(area.width)),
            theme.text,
        ));
        line.render(Rect::new(area.left(), y, area.width, 1), buf);
    }
}

/// Zone de saisie : lignes déjà rendues par l'éditeur (fenêtre adaptative).
fn render_editor(buf: &mut Buffer, area: Rect, rows: &[String], theme: &Theme) {
    if area.area() == 0 {
        return;
    }
    for (row, text) in rows.iter().enumerate() {
        let y = area.top().saturating_add(u16::try_from(row).unwrap_or(0));
        if y >= area.bottom() {
            break;
        }
        let line = Line::from(Span::styled(
            crop_to_width(text, usize::from(area.width)),
            theme.text,
        ));
        line.render(Rect::new(area.left(), y, area.width, 1), buf);
    }
}

/// Barre d'état : animation, jetons, fichiers modifiés, aide minimale, « ↓ nouveau ».
fn render_status(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    if area.area() == 0 {
        return;
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    if matches!(state.status, Status::Running { .. }) {
        let len = u64::try_from(SPINNER.len()).unwrap_or(1);
        let idx = state.tick.checked_rem(len).unwrap_or(0);
        let ch = SPINNER[usize::try_from(idx).unwrap_or(0)];
        spans.push(Span::styled(format!("{ch} "), theme.warn));
    }
    spans.push(Span::styled(
        format!("↑{} ↓{}", state.usage.prompt, state.usage.completion),
        theme.dim,
    ));
    if let Some(n) = state.dirty_files {
        let texte = if n == 1 {
            " · 1 fichier modifié".to_string()
        } else {
            format!(" · {n} fichiers modifiés")
        };
        spans.push(Span::styled(texte, theme.dim));
    }
    if state.scroll.pending {
        spans.push(Span::styled(" ↓ nouveau", theme.warn));
    }
    if state.quit_armed {
        spans.push(Span::styled(" · Ctrl+C encore pour quitter", theme.error));
    }
    spans.push(Span::styled(" · Ctrl-D quitter · /help", theme.dim));
    let line = Line::from(spans);
    let line = clip_line(&line, usize::from(area.width));
    line.render(area, buf);
}

// ── helpers de test ──

/// Rend l'état dans un `TestBackend` de la taille `state.size` et rend le
/// contenu texte ligne à ligne (tests de `./view.sdd`, `./mod.sdd`).
#[cfg(test)]
#[must_use]
pub fn render_to_string(state: &AppState) -> String {
    buffer_lines(&render_buffer(state)).join("\n")
}

/// Rend l'état dans un `TestBackend` et rend le tampon (tests).
#[cfg(test)]
#[must_use]
pub fn render_buffer(state: &AppState) -> Buffer {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(state.size.0.max(1), state.size.1.max(1)))
        .expect_test("backend");
    terminal
        .draw(|frame| render(frame, state))
        .expect_test("dessin");
    terminal.backend().buffer().clone()
}

/// Contenu texte d'un tampon, ligne par ligne.
#[cfg(test)]
#[must_use]
pub fn buffer_lines(buf: &Buffer) -> Vec<String> {
    let area = buf.area();
    (0..area.height)
        .map(|y| {
            let mut s = String::new();
            let mut x = 0u16;
            while x < area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    let sym = cell.symbol();
                    if sym == " " {
                        s.push(' ');
                        x = x.saturating_add(1);
                    } else {
                        s.push_str(sym);
                        x = x.saturating_add(
                            u16::try_from(editor::display_width(sym).max(1)).unwrap_or(1),
                        );
                    }
                } else {
                    break;
                }
            }
            s.trim_end().to_string()
        })
        .collect()
}

/// Extension minuscule réservée aux tests : expect avec message (cfg(test)).
#[cfg(test)]
trait ExpectTest<T> {
    fn expect_test(self, what: &str) -> T;
}
#[cfg(test)]
impl<T, E: std::fmt::Debug> ExpectTest<T> for Result<T, E> {
    fn expect_test(self, what: &str) -> T {
        match self {
            Ok(v) => v,
            Err(e) => panic!("{what}: {e:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::{self as st, AppState, TranscriptItem};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn base_state(w: u16, h: u16) -> AppState {
        let mut s = AppState::new("agent-demo", "abcdef01-2345".to_string(), None, (w, h));
        s.transcript = vec![
            TranscriptItem::User {
                text: "question".to_string(),
                during_turn: false,
            },
            TranscriptItem::Assistant {
                text: "réponse".to_string(),
                done: true,
            },
            TranscriptItem::Notice {
                level: st::NoticeLevel::Info,
                text: "info utile".to_string(),
            },
        ];
        s
    }

    fn script(ok: bool, output: &str) -> TranscriptItem {
        TranscriptItem::Script {
            seq: 1,
            functions: vec!["query".to_string()],
            duration_ms: 12,
            ok,
            error: if ok { None } else { Some("échec".to_string()) },
            script: "print(1)".to_string(),
            output: output.to_string(),
            expanded: false,
        }
    }

    fn draw(s: &AppState) -> TestBackend {
        let mut terminal =
            Terminal::new(TestBackend::new(s.size.0.max(1), s.size.1.max(1))).expect_test("b");
        terminal.draw(|f| render(f, s)).expect_test("d");
        terminal.backend().clone()
    }

    fn lines_of(s: &AppState) -> Vec<String> {
        buffer_lines(&draw(s).buffer().clone())
    }

    /// Scenario: `disposition_nominale` — 3 éléments, 100×30 : en-tête en
    /// première ligne, saisie en bas, barre d'état en dernière ligne,
    /// transcription entre les deux.
    #[test]
    fn disposition_nominale() {
        let mut s = base_state(100, 30);
        s.input = editor::apply(
            editor::EditorState::new(),
            editor::EditCommand::InsertStr("ma saisie".into()),
        );
        let l = lines_of(&s);
        assert_eq!(l.len(), 30);
        assert!(
            l[0].contains("agent-demo"),
            "en-tête première ligne: {:?}",
            l[0]
        );
        assert!(
            l[0].contains("session abcdef01"),
            "session dans l'en-tête: {:?}",
            l[0]
        );
        assert!(
            l[29].contains("Ctrl-D"),
            "barre d'état dernière ligne: {:?}",
            l[29]
        );
        assert!(l[28].contains("ma saisie"), "saisie en bas: {:?}", l[28]);
        let joined: String = l[1..28].join("\n");
        assert!(
            joined.contains("question") && joined.contains("réponse"),
            "transcription entre les deux: {joined}"
        );
    }

    /// Un message envoyé pendant un tour porte son marqueur.
    #[test]
    fn message_pendant_le_tour_marque() {
        let mut s = base_state(100, 30);
        s.transcript.push(TranscriptItem::User {
            text: "bis".to_string(),
            during_turn: true,
        });
        let joined = lines_of(&s).join("\n");
        assert!(
            joined.contains("(pendant le tour) bis"),
            "marque visible: {joined}"
        );
    }

    /// Scenario: `panneau_des_taches_large` — à 140 colonnes le panneau est à
    /// droite ; à 80 colonnes il est au-dessus de la saisie.
    #[test]
    fn panneau_des_taches_large() {
        let todo = vec![
            st::TodoItem {
                content: "tâche-alpha".into(),
                status: "pending".into(),
            },
            st::TodoItem {
                content: "tâche-beta".into(),
                status: "done".into(),
            },
        ];
        let mut wide = base_state(140, 30);
        wide.todo = todo.clone();
        wide.todo_visible = true;
        let b = draw(&wide).buffer().clone();
        let right_hit = (100..140u16).any(|x| (1..29u16).any(|y| starts_task_cell(&b, x, y)));
        assert!(
            right_hit,
            "à 140 colonnes, les tâches sont dans la colonne de droite"
        );

        let mut narrow = base_state(80, 30);
        narrow.todo = todo;
        narrow.todo_visible = true;
        let b = draw(&narrow).buffer().clone();
        let l = buffer_lines(&b);
        let todo_row = l
            .iter()
            .position(|row| row.contains("tâche-alpha"))
            .expect("les tâches sont listées (au-dessus de la saisie)");
        let status_row = l.len().saturating_sub(1);
        assert!(
            todo_row < status_row,
            "panneau au-dessus de la barre d'état ({todo_row} !< {status_row})"
        );
    }

    fn starts_task_cell(b: &Buffer, x: u16, y: u16) -> bool {
        if let Some(cell) = b.cell((x, y))
            && cell.symbol() == "["
        {
            let mut s = String::new();
            let mut xx = x;
            let stop = x.saturating_add(30);
            while xx < stop && xx < b.area().width {
                if let Some(c) = b.cell((xx, y)) {
                    s.push_str(c.symbol());
                } else {
                    break;
                }
                xx = xx.saturating_add(1);
            }
            return s.contains("tâche-alpha");
        }
        false
    }

    /// Scenario: `script_replie_puis_deplie` — une ligne `▸ script …` avec
    /// durée et statut ; déplié : le script complet puis sa sortie.
    #[test]
    fn script_replie_puis_deplie() {
        let mut s = base_state(100, 30);
        s.transcript.push(script(true, "le résultat"));
        let joined = lines_of(&s).join("\n");
        assert!(
            joined.contains("▸ script 1 · query · 12 ms ✓"),
            "ligne repliée: {joined}"
        );
        assert!(!joined.contains("le résultat"), "sortie masquée repliée");

        let mut deplie = s.clone();
        if let Some(TranscriptItem::Script { expanded, .. }) = deplie.transcript.last_mut() {
            *expanded = true;
        }
        deplie.revision = deplie.revision.wrapping_add(1);
        let joined = lines_of(&deplie).join("\n");
        assert!(joined.contains("▾ script 1"), "ligne dépliée: {joined}");
        assert!(joined.contains("print(1)"), "script complet visible");
        assert!(joined.contains("le résultat"), "sortie visible");
    }

    /// Scenario: `sortie_tronquee` — un résultat de 500 lignes déplié : au
    /// plus 40 lignes et un marqueur de troncature.
    #[test]
    fn sortie_tronquee() {
        let big = (0..500)
            .map(|i| format!("ligne {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut s = AppState::new("a", "c".into(), None, (100, 30));
        let mut item = script(true, &big);
        if let TranscriptItem::Script { expanded, .. } = &mut item {
            *expanded = true;
        }
        s.transcript.push(item);
        let joined = lines_of(&s).join("\n");
        assert!(!joined.contains("ligne 499"), "au plus 40 lignes rendues");
        assert!(joined.contains("tronquées"), "marqueur de troncature");
    }

    /// Scenario: `terminal_minuscule` — 0×0, 1×1, 20×5 : aucun plantage ;
    /// message « terminal trop petit » quand applicable.
    #[test]
    fn terminal_minuscule() {
        for (w, h) in [(0u16, 0u16), (1, 1), (20, 5)] {
            let s = base_state(w, h);
            let out = render_to_string(&s);
            if w == 20 {
                assert!(
                    out.to_lowercase().contains("trop petit"),
                    "20×5 affiche le message, obtenu {out:?}"
                );
            }
        }
    }

    /// Scenario: `statut_sans_couleur` — un script qui réussit (`✓`) et un
    /// qui échoue (`✗`) sont distingués par leurs symboles.
    #[test]
    fn statut_sans_couleur() {
        let mut s = AppState::new("a", "c".into(), None, (100, 30));
        s.transcript.push(script(true, "bien"));
        s.transcript.push(script(false, ""));
        let joined = lines_of(&s).join("\n");
        assert!(
            joined.contains('✓') && joined.contains('✗'),
            "symboles présents: {joined}"
        );
    }

    /// Scenario: `indicateur_nouveau_contenu` — l'utilisateur a remonté et un
    /// jeton arrive : « ↓ nouveau » est visible.
    #[test]
    fn indicateur_nouveau_contenu() {
        let mut s = base_state(100, 30);
        s.scroll.follow = false;
        s.scroll.offset = 2;
        s.scroll.pending = true;
        let joined = lines_of(&s).join("\n");
        assert!(joined.contains("↓ nouveau"), "indicateur visible: {joined}");
    }

    /// Scenario: `nombre_de_fichiers_modifies` — `Some(3)` puis `None`.
    #[test]
    fn nombre_de_fichiers_modifies() {
        let mut s = base_state(100, 30);
        s.dirty_files = Some(3);
        let l = lines_of(&s);
        assert!(
            l[29].contains("3 fichiers modifiés"),
            "barre d'état: {:?}",
            l[29]
        );
        s.dirty_files = None;
        let l = lines_of(&s);
        assert!(!l[29].contains("modifié"), "rien sans sandbox: {:?}", l[29]);
    }

    /// Une ligne de sous-agent montre son agent et son statut.
    #[test]
    fn ligne_de_subagent() {
        let mut s = base_state(100, 30);
        s.transcript.push(TranscriptItem::Subagent {
            run: "run-7".into(),
            agent: "sub".into(),
            status: st::SubagentStatus::Ended(Outcome::Valid),
        });
        let joined = lines_of(&s).join("\n");
        assert!(
            joined.contains("sous-agent sub (run-7) ✓"),
            "ligne de statut: {joined}"
        );
    }

    /// Scenario: `curseur_dans_la_saisie` — texte Unicode large : le curseur
    /// terminal est sur la bonne colonne d'affichage.
    #[test]
    fn curseur_dans_la_saisie() {
        let mut s = AppState::new("a", "c".into(), None, (100, 30));
        s.input = editor::EditorState {
            lines: vec!["日本".to_string(), "abé".to_string()],
            cursor: (1, 3),
            ..editor::EditorState::new()
        };
        let pos = draw(&s).cursor_position();
        assert_eq!(
            pos.x, 3,
            "curseur en fin de « abé » (3 colonnes d'affichage)"
        );

        let mut s2 = s.clone();
        s2.input.cursor = (0, 2);
        let pos = draw(&s2).cursor_position();
        assert_eq!(
            pos.x, 4,
            "curseur en fin de « 日本 » (4 colonnes d'affichage)"
        );
    }

    /// Scenario: `saisie_adaptative` — une saisie de 12 lignes : la zone
    /// occupe 8 lignes au plus et fait défiler son contenu.
    #[test]
    fn saisie_adaptative() {
        let mut s = AppState::new("a", "c".into(), None, (100, 30));
        s.input = editor::EditorState {
            lines: (0..12).map(|i| format!("zz{i} fin de ligne")).collect(),
            cursor: (11, 4),
            ..editor::EditorState::new()
        };
        let l = lines_of(&s);
        assert!(
            !l.join("\n").contains("zz0"),
            "les premières lignes ont défilé"
        );
        assert!(
            l.join("\n").contains("zz11"),
            "la ligne du curseur est visible"
        );
    }

    /// Transcription plus haute que l'écran et suivi de la fin : le bas est visible.
    #[test]
    fn suivi_affiche_le_bas() {
        let mut s = AppState::new("a", "c".into(), None, (100, 20));
        s.transcript = (0..200)
            .map(|i| TranscriptItem::User {
                text: format!("u{i}"),
                during_turn: false,
            })
            .collect();
        let joined = lines_of(&s).join("\n");
        assert!(
            joined.contains("u199"),
            "le bas de la transcription est visible"
        );
        assert!(!joined.contains("u0 "), "le haut a défilé");
    }
}
