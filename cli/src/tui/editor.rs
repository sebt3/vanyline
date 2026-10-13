//! Zone de saisie multi-lignes du chat TUI — contrat complet : `./editor.sdd`.
//!
//! Éditeur minimal et PUR (aucune I/O, aucune dépendance au terminal) : tampon,
//! curseur en graphèmes, édition par commandes, historique des saisies. Gère
//! Unicode (clusters de graphèmes, largeur d'affichage via `unicode-width`).

use unicode_width::UnicodeWidthChar;

/// État de la zone de saisie : lignes du tampon, curseur (ligne, colonne en
/// graphèmes), historique des saisies envoyées et brouillon en cours de rappel.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EditorState {
    /// Lignes du tampon — jamais vide (au moins une ligne, possiblement vide).
    pub lines: Vec<String>,
    /// Curseur : (ligne, colonne en graphèmes).
    pub cursor: (usize, usize),
    /// Saisies précédentes, de la plus ancienne à la plus récente.
    pub history: Vec<String>,
    /// Position dans l'historique pendant un rappel (`None` = hors rappel).
    pub history_pos: Option<usize>,
    /// Brouillon mis de côté pendant un rappel d'historique.
    pub draft: Option<String>,
}

/// Commandes d'édition appliquées à [`EditorState`] par [`apply`].
///
/// La surface complète est contractée par `editor.sdd` §Exposes (exercée par
/// les tests de ce fichier) ; le chemin touches de `state.sdd` §Handles n'en
/// pose qu'une partie (aucune touche des commandes non listées n'est inventée).
#[allow(dead_code)] // variants Exposes de editor.sdd, sans touche dans state.sdd
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditCommand {
    /// Insertion d'un caractère (`\n` = `Newline`, contrôles ignorés).
    Insert(char),
    /// Insertion d'une chaîne (collage) : `\n` crée des lignes, les autres
    /// contrôles sont ignorés.
    InsertStr(String),
    /// Saut de ligne au curseur.
    Newline,
    /// Effacement du graphème avant le curseur (fusion de lignes en début).
    Backspace,
    /// Effacement du graphème sous le curseur.
    Delete,
    /// Curseur d'un graphème vers la gauche.
    Left,
    /// Curseur d'un graphème vers la droite.
    Right,
    /// Curseur d'une ligne vers le haut.
    Up,
    /// Curseur d'une ligne vers le bas.
    Down,
    /// Curseur d'un mot vers la gauche.
    WordLeft,
    /// Curseur d'un mot vers la droite.
    WordRight,
    /// Début de ligne.
    Home,
    /// Fin de ligne.
    End,
    /// Effacement du curseur à la fin de ligne.
    KillLine,
    /// Effacement du début de ligne jusqu'au curseur.
    KillToStart,
    /// Effacement du mot avant le curseur.
    DeleteWord,
    /// Tampon vidé (historique conservé).
    Clear,
    /// Rappel de l'entrée précédente de l'historique.
    HistoryPrev,
    /// Retour vers l'entrée suivante (ou restitution du brouillon).
    HistoryNext,
}

/// Itérateur de clusters de graphèmes minimal (ZWJ, sélecteurs de variation,
/// diacritiques combinés, tons de peau, indicateurs régionaux) — suffisant
/// pour l'édition de saisie ; UAX #29 complet hors périmètre (`./editor.sdd`
/// n'exige que « ne jamais couper un graphème » sur ces cas).
pub struct Graphemes<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.pos >= self.s.len() {
            return None;
        }
        let start = self.pos;
        let (i, first) = self.s[start..].char_indices().next()?;
        let mut end = start.saturating_add(i).saturating_add(first.len_utf8());
        let first_cp = first as u32;
        loop {
            let rest = &self.s[end..];
            let Some((_, c)) = rest.char_indices().next() else {
                break;
            };
            let cp = c as u32;
            if cp == 0x200D || cp == 0x200C {
                // ZWJ/ZWNJ : absorbe le séparateur ET le caractère suivant.
                end = end.saturating_add(c.len_utf8());
                if let Some((_, c2)) = self.s[end..].char_indices().next() {
                    end = end.saturating_add(c2.len_utf8());
                }
            } else if (0xFE00..=0xFE0F).contains(&cp)
                || (0x0300..=0x036F).contains(&cp)
                || (0x1AB0..=0x1AFF).contains(&cp)
                || (0x1F3FB..=0x1F3FF).contains(&cp)
                || ((0x1F1E6..=0x1F1FF).contains(&cp)
                    && (0x1F1E6..=0x1F1FF).contains(&first_cp)
                    && end == start.saturating_add(first.len_utf8()))
            {
                // Combineurs : sélecteurs de variation, diacritiques, tons de
                // peau, second indicateur régional (drapeau).
                end = end.saturating_add(c.len_utf8());
            } else {
                break;
            }
        }
        self.pos = end;
        Some(&self.s[start..end])
    }
}

/// Graphèmes d'une chaîne (cluster par cluster, jamais un milieu de caractère).
pub fn graphemes(s: &str) -> Graphemes<'_> {
    Graphemes { s, pos: 0 }
}

/// Largeur d'affichage d'une chaîne (somme des largeurs Unicode, `None`→0).
pub fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// Saisie vide.
impl EditorState {
    /// Tampon vide avec historique vierge.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor: (0, 0),
            history: Vec::new(),
            history_pos: None,
            draft: None,
        }
    }
}

/// Index d'octets du début du graphème en colonne `col` (saturé à la fin).
fn grapheme_offset(line: &str, col: usize) -> usize {
    let mut off = 0usize;
    for (i, _) in graphemes(line).enumerate() {
        if i >= col {
            return off;
        }
        off = off.saturating_add(graphemes(&line[off..]).next().map_or(0, str::len));
    }
    line.len()
}

/// Nombre de graphèmes d'une ligne.
fn grapheme_count(line: &str) -> usize {
    graphemes(line).count()
}

/// Remplace les lignes par celles d'un texte (au moins une ligne).
fn set_text(mut state: EditorState, entry: &str) -> EditorState {
    state.lines = entry.split('\n').map(str::to_string).collect();
    if state.lines.is_empty() {
        state.lines.push(String::new());
    }
    let row = state.lines.len().saturating_sub(1);
    let col = grapheme_count(&state.lines[row]);
    state.cursor = (row, col);
    state
}

/// Applique une commande d'édition (pur : consomme et rend l'état).
/// Ne panique jamais, opère toujours en graphèmes entiers.
// Une branche par commande d'édition (contrat editor.sdd) : la longueur est le contrat.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn apply(mut state: EditorState, command: EditCommand) -> EditorState {
    if state.lines.is_empty() {
        state.lines.push(String::new());
    }
    // Clamp du curseur (défensif : un état entrant peut venir d'ailleurs).
    {
        let row = state.cursor.0.min(state.lines.len().saturating_sub(1));
        let col = state.cursor.1.min(grapheme_count(&state.lines[row]));
        state.cursor = (row, col);
    }
    match command {
        EditCommand::Insert(c) => {
            if c == '\n' {
                return apply(state, EditCommand::Newline);
            }
            if c.is_control() {
                return state;
            }
            let row = state.cursor.0;
            let off = grapheme_offset(&state.lines[row], state.cursor.1);
            state.lines[row].insert(off, c);
            state.cursor.1 = state.cursor.1.saturating_add(1);
        }
        EditCommand::InsertStr(s) => {
            // Le collage insère des GRAPHÈMES entiers (un cluster = une
            // colonne de curseur), pas des points de code isolés.
            for cluster in graphemes(&s) {
                let first = cluster.chars().next();
                let Some(c) = first else { continue };
                if c == '\n' {
                    state = apply(state, EditCommand::Newline);
                    continue;
                }
                if c.is_control() {
                    // Cluster dont la base est un contrôle : on ne garde que
                    // ses éventuels combineurs non contrôles.
                    let clean: String = cluster.chars().filter(|x| !x.is_control()).collect();
                    if clean.is_empty() {
                        continue;
                    }
                    let row = state.cursor.0;
                    let off = grapheme_offset(&state.lines[row], state.cursor.1);
                    state.lines[row].replace_range(off..off, &clean);
                    state.cursor.1 = state.cursor.1.saturating_add(1);
                    continue;
                }
                let row = state.cursor.0;
                let off = grapheme_offset(&state.lines[row], state.cursor.1);
                state.lines[row].replace_range(off..off, cluster);
                state.cursor.1 = state.cursor.1.saturating_add(1);
            }
        }
        EditCommand::Newline => {
            let row = state.cursor.0;
            let off = grapheme_offset(&state.lines[row], state.cursor.1);
            let tail = state.lines[row].split_off(off);
            state.lines.insert(row.saturating_add(1), tail);
            state.cursor = (row.saturating_add(1), 0);
        }
        EditCommand::Backspace => {
            let (row, col) = state.cursor;
            if col > 0 {
                let line = &state.lines[row];
                let start = grapheme_offset(line, col.saturating_sub(1));
                let end = grapheme_offset(line, col);
                state.lines[row].replace_range(start..end, "");
                state.cursor.1 = col.saturating_sub(1);
            } else if row > 0 {
                let prev_len = grapheme_count(&state.lines[row.saturating_sub(1)]);
                let tail = state.lines.remove(row);
                state.lines[row.saturating_sub(1)].push_str(&tail);
                state.cursor = (row.saturating_sub(1), prev_len);
            }
        }
        EditCommand::Delete => {
            let (row, col) = state.cursor;
            let len = grapheme_count(&state.lines[row]);
            if col < len {
                let start = grapheme_offset(&state.lines[row], col);
                let end = grapheme_offset(&state.lines[row], col.saturating_add(1));
                state.lines[row].replace_range(start..end, "");
            } else if row.saturating_add(1) < state.lines.len() {
                let tail = state.lines.remove(row.saturating_add(1));
                state.lines[row].push_str(&tail);
            }
        }
        EditCommand::Left => {
            let (row, col) = state.cursor;
            if col > 0 {
                state.cursor.1 = col.saturating_sub(1);
            } else if row > 0 {
                let prev = row.saturating_sub(1);
                let len = grapheme_count(&state.lines[prev]);
                state.cursor = (prev, len);
            }
        }
        EditCommand::Right => {
            let (row, col) = state.cursor;
            if col < grapheme_count(&state.lines[row]) {
                state.cursor.1 = col.saturating_add(1);
            } else if row.saturating_add(1) < state.lines.len() {
                state.cursor = (row.saturating_add(1), 0);
            }
        }
        EditCommand::Up => {
            let row = state.cursor.0.saturating_sub(1);
            let col = state.cursor.1.min(grapheme_count(&state.lines[row]));
            state.cursor = (row, col);
        }
        EditCommand::Down => {
            let row = (state.cursor.0.saturating_add(1)).min(state.lines.len().saturating_sub(1));
            let col = state.cursor.1.min(grapheme_count(&state.lines[row]));
            state.cursor = (row, col);
        }
        EditCommand::WordLeft => {
            let (row, col) = state.cursor;
            let gs: Vec<&str> = graphemes(&state.lines[row]).take(col).collect();
            let mut i = gs.len();
            while i > 0 && gs[i.saturating_sub(1)].trim().is_empty() {
                i = i.saturating_sub(1);
            }
            while i > 0 && !gs[i.saturating_sub(1)].trim().is_empty() {
                i = i.saturating_sub(1);
            }
            state.cursor.1 = i;
        }
        EditCommand::WordRight => {
            let (row, col) = state.cursor;
            let line = state.lines[row].clone();
            let gs: Vec<&str> = graphemes(&line).collect();
            let len = gs.len();
            let mut i = col.min(len);
            while i < len && !gs[i].trim().is_empty() {
                i = i.saturating_add(1);
            }
            while i < len && gs[i].trim().is_empty() {
                i = i.saturating_add(1);
            }
            state.cursor.1 = i;
        }
        EditCommand::Home => state.cursor.1 = 0,
        EditCommand::End => {
            let row = state.cursor.0;
            let len = grapheme_count(&state.lines[row]);
            state.cursor.1 = len;
        }
        EditCommand::KillLine => {
            let (row, col) = state.cursor;
            let off = grapheme_offset(&state.lines[row], col);
            state.lines[row].truncate(off);
        }
        EditCommand::KillToStart => {
            let (row, col) = state.cursor;
            let off = grapheme_offset(&state.lines[row], col);
            state.lines[row] = state.lines[row][off..].to_string();
            state.cursor.1 = 0;
        }
        EditCommand::DeleteWord => {
            let (row, col) = state.cursor;
            let gs: Vec<&str> = graphemes(&state.lines[row]).take(col).collect();
            let mut i = gs.len();
            while i > 0 && !gs[i.saturating_sub(1)].trim().is_empty() {
                i = i.saturating_sub(1);
            }
            let start = grapheme_offset(&state.lines[row], i);
            let end = grapheme_offset(&state.lines[row], col);
            state.lines[row].replace_range(start..end, "");
            state.cursor.1 = i;
        }
        EditCommand::Clear => {
            state.lines = vec![String::new()];
            state.cursor = (0, 0);
            state.history_pos = None;
            state.draft = None;
        }
        EditCommand::HistoryPrev => {
            if state.history.is_empty() {
                return state;
            }
            match state.history_pos {
                None => {
                    state.draft = Some(text(&state));
                    let pos = state.history.len().saturating_sub(1);
                    state.history_pos = Some(pos);
                    let entry = state.history[pos].clone();
                    state = set_text(state, &entry);
                }
                Some(pos) if pos > 0 => {
                    let pos = pos.saturating_sub(1);
                    state.history_pos = Some(pos);
                    let entry = state.history[pos].clone();
                    state = set_text(state, &entry);
                }
                Some(_) => {}
            }
        }
        EditCommand::HistoryNext => match state.history_pos {
            None => {}
            Some(pos) if pos.saturating_add(1) < state.history.len() => {
                let pos = pos.saturating_add(1);
                state.history_pos = Some(pos);
                let entry = state.history[pos].clone();
                state = set_text(state, &entry);
            }
            Some(_) => {
                state.history_pos = None;
                let draft = state.draft.take().unwrap_or_default();
                state = set_text(state, &draft);
            }
        },
    }
    state
}

/// Lignes jointes par `\n`.
#[must_use]
pub fn text(state: &EditorState) -> String {
    state.lines.join("\n")
}

/// Vrai si toutes les lignes sont vides ou en blancs.
#[must_use]
pub fn is_blank(state: &EditorState) -> bool {
    state.lines.iter().all(|l| l.trim().is_empty())
}

/// Ajoute une saisie à l'historique (sans doublon consécutif, borné à 500
/// entrées — les plus anciennes tombent). Sort d'un rappel d'historique.
#[must_use]
pub fn push_history(mut state: EditorState, entry: &str) -> EditorState {
    state.history_pos = None;
    state.draft = None;
    if entry.is_empty() {
        return state;
    }
    if state.history.last().is_some_and(|last| last == entry) {
        return state;
    }
    state.history.push(entry.to_string());
    while state.history.len() > 500 {
        state.history.remove(0);
    }
    state
}

/// Nombre de lignes d'affichage qu'occupe une ligne logique de largeur `w`
/// dans une zone de `width` colonnes (retour à la ligne inclus).
fn wrapped_rows(w: usize, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    if w == 0 {
        return 1;
    }
    w.div_ceil(width)
}

/// Position d'affichage du curseur (x, y) pour une largeur de zone donnée,
/// en tenant compte des largeurs Unicode et du retour à la ligne.
#[must_use]
pub fn display_cursor(state: &EditorState, width: usize) -> (usize, usize) {
    if width == 0 {
        return (0, 0);
    }
    let (crow, ccol) = state.cursor;
    let mut y = 0usize;
    for (i, line) in state.lines.iter().enumerate() {
        if i >= crow {
            break;
        }
        y = y.saturating_add(wrapped_rows(display_width(line), width));
    }
    let Some(cur_line) = state.lines.get(crow) else {
        return (0, y);
    };
    let mut x = 0usize;
    for g in graphemes(cur_line).take(ccol) {
        let gw = display_width(g);
        if x.saturating_add(gw) > width {
            y = y.saturating_add(1);
            x = 0;
        }
        x = x.saturating_add(gw);
    }
    (x, y)
}

/// Lignes visibles de la zone de saisie (hauteur `height`, contenu qui défile
/// pour suivre le curseur) + position du curseur relative à la fenêtre.
#[must_use]
pub fn render_view(
    state: &EditorState,
    width: usize,
    height: usize,
) -> (Vec<String>, (usize, usize)) {
    if height == 0 {
        return (Vec::new(), (0, 0));
    }
    let mut wrapped: Vec<String> = Vec::new();
    for line in &state.lines {
        if width == 0 || display_width(line) <= width {
            wrapped.push(line.clone());
        } else {
            let mut cur = String::new();
            let mut cur_w = 0usize;
            for g in graphemes(line) {
                let gw = display_width(g);
                if cur_w.saturating_add(gw) > width {
                    wrapped.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push_str(g);
                cur_w = cur_w.saturating_add(gw);
            }
            wrapped.push(cur);
        }
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    let (x, cy) = display_cursor(state, width);
    // Fenêtre glissante suivant le curseur (le bas suit, le haut défile).
    let first = if cy >= height {
        cy.saturating_sub(height.saturating_sub(1))
    } else {
        0
    };
    let first = first.min(wrapped.len().saturating_sub(1));
    let last = first.saturating_add(height).min(wrapped.len());
    (wrapped[first..last].to_vec(), (x, cy.saturating_sub(first)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> EditorState {
        EditorState::new()
    }

    fn type_str(st: EditorState, s: &str) -> EditorState {
        apply(st, EditCommand::InsertStr(s.to_string()))
    }

    /// Scenario: `insertion_et_curseur` — `Insert('a')`, `Insert('é')` sur
    /// saisie vide ⇒ texte `aé`, curseur en colonne 2.
    #[test]
    fn insertion_et_curseur() {
        let st = apply(empty(), EditCommand::Insert('a'));
        let st = apply(st, EditCommand::Insert('é'));
        assert_eq!(text(&st), "aé");
        assert_eq!(st.cursor, (0, 2), "colonne en graphèmes");
    }

    /// Scenario: `nouvelle_ligne` — `Newline` entre `a` et `b` sépare en deux
    /// lignes avec le curseur au début de la seconde.
    #[test]
    fn nouvelle_ligne() {
        let st = type_str(empty(), "ab");
        let st = apply(st, EditCommand::Left);
        let st = apply(st, EditCommand::Newline);
        assert_eq!(st.lines, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(st.cursor, (1, 0));
    }

    /// Scenario: `suppression_en_debut_de_ligne` — `Backspace` au début de la
    /// seconde ligne fusionne les lignes.
    #[test]
    fn suppression_en_debut_de_ligne() {
        let st = type_str(empty(), "ab\ncd");
        let st = apply(st, EditCommand::Home);
        assert_eq!(st.cursor, (1, 0), "curseur placé en début de seconde ligne");
        let st = apply(st, EditCommand::Backspace);
        assert_eq!(
            st.lines,
            vec!["abcd".to_string()],
            "une seule ligne fusionnée"
        );
        assert_eq!(st.cursor, (0, 2));
    }

    /// Scenario: `graphemes_composes` — « 👨‍👩‍👧 » est un seul graphème ;
    /// `Backspace` le retire entier.
    #[test]
    fn graphemes_composes() {
        let famille = "👨‍👩‍👧";
        assert_eq!(graphemes(famille).count(), 1, "un seul graphème attendu");
        let st = apply(empty(), EditCommand::InsertStr(famille.to_string()));
        assert_eq!(st.cursor, (0, 1), "colonne = 1 graphème");
        let st = apply(st, EditCommand::Backspace);
        assert_eq!(text(&st), "", "le graphème entier disparaît");
    }

    /// Scenario: `largeur_unicode` — « 日本 » (largeur 2 chacun) en fin de
    /// saisie ⇒ `display_cursor` rend x = 4.
    #[test]
    fn largeur_unicode() {
        let st = type_str(empty(), "日本");
        let (x, y) = display_cursor(&st, 80);
        assert_eq!((x, y), (4, 0), "x doit valoir 4 en fin de « 日本 »");
    }

    /// Scenario: `historique` — deux entrées et un brouillon `x` :
    /// Prev, Prev, Next, Next parcourent l'entrée 2, l'entrée 1, l'entrée 2,
    /// puis restituent le brouillon.
    #[test]
    fn historique() {
        let st = EditorState {
            lines: vec!["x".to_string()],
            cursor: (0, 1),
            history: vec!["entrée 1".to_string(), "entrée 2".to_string()],
            history_pos: None,
            draft: None,
        };
        let st = apply(st, EditCommand::HistoryPrev);
        assert_eq!(text(&st), "entrée 2");
        assert_eq!(st.draft.as_deref(), Some("x"), "le brouillon est gardé");
        let st = apply(st, EditCommand::HistoryPrev);
        assert_eq!(text(&st), "entrée 1");
        let st = apply(st, EditCommand::HistoryNext);
        assert_eq!(text(&st), "entrée 2");
        let st = apply(st, EditCommand::HistoryNext);
        assert_eq!(text(&st), "x", "le brouillon est restitué");
        assert_eq!(st.history_pos, None);
    }

    /// Scenario: `historique_sans_doublon` — `push_history` d'une entrée égale
    /// à la dernière laisse l'historique inchangé.
    #[test]
    fn historique_sans_doublon() {
        let st = EditorState {
            history: vec!["a".to_string()],
            ..empty()
        };
        let st2 = push_history(st.clone(), "a");
        assert_eq!(st2.history, vec!["a".to_string()]);
        let st3 = push_history(st2.clone(), "b");
        assert_eq!(st3.history, vec!["a".to_string(), "b".to_string()]);
    }

    /// Scenario: `entree_arbitraire` — 100 000 caractères aléatoires avec
    /// séquences de contrôle : aucun plantage, les contrôles autres que `\n`
    /// sont ignorés.
    #[test]
    fn entree_arbitraire() {
        let alphabet: Vec<char> = "a\t\u{1b}é日本 \n0".chars().collect();
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut s = String::new();
        for _ in 0..100_000 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let idx = usize::try_from(seed >> 59).unwrap_or(0) % alphabet.len();
            s.push(alphabet[idx]);
        }
        let st = apply(empty(), EditCommand::InsertStr(s.clone()));
        let rendu = text(&st);
        for c in rendu.chars() {
            assert!(!c.is_control() || c == '\n', "contrôle {c:?} conservé");
        }
        let attendu: String = s
            .chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .collect();
        assert_eq!(rendu, attendu, "tout le texte noncontrôle est conservé");
    }

    /// Scenario: `suppression_de_mot` — `foo bar` curseur en fin, `DeleteWord`
    /// ⇒ texte `foo `.
    #[test]
    fn suppression_de_mot() {
        let st = type_str(empty(), "foo bar");
        let st = apply(st, EditCommand::DeleteWord);
        assert_eq!(text(&st), "foo ");
    }

    /// Cas additionnels verrouillant le contrat « opérer en graphèmes, jamais
    /// paniquer » : Left/Right/Home/End/Up/Down sur multi-lignes Unicode.
    #[test]
    fn navigation_ne_casse_aucun_grapheme() {
        let st = type_str(empty(), "é\n日本");
        let st = apply(st, EditCommand::Up);
        let st = apply(st, EditCommand::Home);
        let st = apply(st, EditCommand::Right);
        let st = apply(st, EditCommand::Backspace);
        assert_eq!(text(&st), "\n日本", "le graphème « é » retiré entier");
        let st = apply(st, EditCommand::Down);
        let st = apply(st, EditCommand::End);
        let st = apply(st, EditCommand::Backspace);
        let st = apply(st, EditCommand::Backspace);
        assert_eq!(text(&st), "\n", "les graphèmes larges retirés entiers");
    }

    /// `render_view` : 12 lignes ⇒ 8 lignes visibles maximum, fenêtre qui suit
    /// le curseur (cohérence avec `display_cursor`).
    #[test]
    fn render_view_borne_a_huit_lignes() {
        let mut st = empty();
        st.lines = (0..12).map(|i| format!("ligne {i}")).collect();
        st.cursor = (11, 8);
        let (lines, (_x, y)) = render_view(&st, 40, 8);
        assert_eq!(lines.len(), 8, "8 lignes au plus");
        assert!(y < 8, "curseur dans la fenêtre");
    }
}
