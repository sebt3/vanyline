//! Rendu markdown du texte de l'agent — contrat complet : `./markdown.sdd`.
//!
//! Convertit le texte (markdown) d'une réponse en lignes stylisées ratatui
//! déjà retournées à la largeur demandée. Sous-ensemble volontairement
//! limité (titres, listes, citations, code, gras/italique, liens, règles).
//! Pur et rejouable à chaque jeton (le bloc grandit pendant le streaming).
//! Les séquences de contrôle du texte sont neutralisées : un modèle ne doit
//! jamais pouvoir piloter le terminal.

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::editor::{display_width, graphemes};
use crate::tui::view::Theme;

/// Rend le markdown `text` en lignes stylées, aucune ne dépassant `width`
/// colonnes d'affichage. Pur, sans panique sur toute entrée. Rejouable à
/// chaque jeton (un bloc de code non fermé est rendu comme bloc ouvert).
#[must_use]
pub fn render(text: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    let safe = neutralize(text);
    let mut b = Builder {
        width,
        theme,
        out: Vec::new(),
        inline: Vec::new(),
        code: None,
        lists: Vec::new(),
        quote_depth: 0usize,
        styles: Vec::new(),
        link: None,
        block: theme.text,
        prefix: String::new(),
        cont: String::new(),
    };
    b.run(&safe);
    clip_all(b.out, width)
}

/// Neutralise les caractères de contrôle (ESC, BEL, collants…) en gardant
/// les sauts de ligne ; les tabulations deviennent des espaces.
#[must_use]
pub(crate) fn neutralize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' => c,
            '\t' => ' ',
            other if other.is_control() => '\u{fffd}',
            other => other,
        })
        .collect()
}

/// Sûreté finale : aucune ligne ne doit dépasser `width`, quitte à couper.
fn clip_all(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let w: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
            if w <= width {
                return line;
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut used = 0usize;
            'outer: for span in line.spans {
                let mut piece = String::new();
                for g in graphemes(&span.content) {
                    let gw = display_width(g);
                    if used.saturating_add(gw) > width {
                        break 'outer;
                    }
                    piece.push_str(g);
                    used = used.saturating_add(gw);
                }
                spans.push(Span::styled(piece, span.style));
            }
            Line::from(spans)
        })
        .collect()
}

/// État d'un bloc de code en cours (langage affiché, contenu brut accumulé).
struct CodeState {
    lang: String,
    buf: String,
}

/// Contexte de liste courant (numérotée ou à puces, compteur d'items).
struct ListCtx {
    ordered: Option<u64>,
    counter: u64,
}

/// Machine à événements pulldown-cmark → lignes déjà retournées.
struct Builder<'t> {
    width: usize,
    theme: &'t Theme,
    out: Vec<Line<'static>>,
    inline: Vec<Span<'static>>,
    code: Option<CodeState>,
    lists: Vec<ListCtx>,
    quote_depth: usize,
    styles: Vec<Style>,
    link: Option<String>,
    block: Style,
    prefix: String,
    cont: String,
}

impl Builder<'_> {
    #[allow(clippy::too_many_lines)] // rendu construit en un seul builder : markdown.sdd §Handles (blocs ouverts, retours à la ligne, non supporté en clair) — la longueur est le contrat
    fn run(&mut self, text: &str) {
        for ev in Parser::new(text) {
            match ev {
                Event::Start(Tag::Heading { .. }) => {
                    self.flush();
                    self.block = self.theme.title;
                    self.set_plain_prefix();
                }
                Event::End(TagEnd::Heading(..) | TagEnd::Paragraph | TagEnd::Item)
                | Event::SoftBreak
                | Event::HardBreak => self.flush(),
                Event::Start(Tag::Paragraph) => self.apply_block_prefix(),
                Event::Start(Tag::List(start)) => {
                    self.flush();
                    self.lists.push(ListCtx {
                        ordered: start,
                        counter: 0,
                    });
                }
                Event::End(TagEnd::List(_)) => {
                    self.flush();
                    self.lists.pop();
                    self.apply_block_prefix();
                }
                Event::Start(Tag::Item) => {
                    self.flush();
                    if let Some(list) = self.lists.last_mut() {
                        list.counter = list.counter.saturating_add(1);
                        let prefix = if let Some(start) = list.ordered {
                            let n = start.saturating_add(list.counter.saturating_sub(1));
                            format!("{n}. ")
                        } else {
                            "• ".to_string()
                        };
                        self.cont = " ".repeat(display_width(&prefix));
                        self.prefix = prefix;
                    }
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    self.flush();
                    self.quote_depth = self.quote_depth.saturating_add(1);
                    self.block = self.theme.quote;
                    self.apply_block_prefix();
                }
                Event::End(TagEnd::BlockQuote(_)) => {
                    self.flush();
                    self.quote_depth = self.quote_depth.saturating_sub(1);
                    self.block = self.theme.text;
                    self.apply_block_prefix();
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    self.flush();
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    self.code = Some(CodeState {
                        lang,
                        buf: String::new(),
                    });
                }
                Event::End(TagEnd::CodeBlock) => self.flush_code(),
                Event::Start(Tag::Emphasis) => self.styles.push(Style::new().italic()),
                Event::Start(Tag::Strong) => self.styles.push(Style::new().bold()),
                Event::End(TagEnd::Emphasis | TagEnd::Strong) => {
                    self.styles.pop();
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    self.link = Some(dest_url.to_string());
                }
                Event::End(TagEnd::Link) => {
                    // lien rendu « texte (URL) », l'URL dans le style lien.
                    if let Some(url) = self.link.take()
                        && !url.is_empty()
                    {
                        self.inline
                            .push(Span::styled(format!(" ({url})"), self.theme.link));
                    }
                }
                Event::Text(t) => {
                    if let Some(code) = &mut self.code {
                        code.buf.push_str(&t);
                    } else {
                        self.push_inline(&t);
                    }
                }
                Event::Code(t) => {
                    self.inline
                        .push(Span::styled(t.to_string(), self.theme.inline_code));
                }
                Event::Rule => {
                    self.flush();
                    self.out.push(Line::from(Span::styled(
                        "─".repeat(self.width),
                        self.theme.rule,
                    )));
                }
                Event::TaskListMarker(checked) => {
                    self.push_inline(if checked { "[✓] " } else { "[ ] " });
                }
                Event::Html(t) | Event::InlineHtml(t) => {
                    // HTML non interprété : rendu en texte brut, jamais perdu.
                    if self.code.is_some() {
                        if let Some(code) = &mut self.code {
                            code.buf.push_str(&t);
                        }
                    } else {
                        self.push_inline(&t);
                    }
                }
                Event::FootnoteReference(name) => self.push_inline(&format!("[{name}]")),
                // Tout le reste (tags non pris en charge, événements de table
                // — l'option tables est éteinte, les tableaux arrivent en
                // texte brut) : rien à émettre de spécifique.
                _ => {}
            }
        }
        // Fin de flux : un bloc de code ouvert est rendu comme bloc ouvert.
        self.flush();
        if self.code.is_some() {
            self.flush_code();
        }
    }

    /// Empile un fragment inline avec les styles gras/italique en cours.
    fn push_inline(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        let mut style = self.block;
        for s in &self.styles {
            style = style.patch(*s);
        }
        self.inline.push(Span::styled(t.to_string(), style));
    }

    fn set_plain_prefix(&mut self) {
        self.prefix = String::new();
        self.cont = String::new();
    }

    /// Préfixe de ligne : citation si citée, sinon préfixe de liste en cours.
    fn apply_block_prefix(&mut self) {
        if self.quote_depth > 0 {
            self.prefix = "│ ".to_string();
            self.cont = "│ ".to_string();
        } else if let Some(list) = self.lists.last() {
            let _ = list; // le préfixe d'item est posé à Start(Tag::Item)
        } else {
            self.set_plain_prefix();
        }
    }

    /// Plie les spans inline accumulés en lignes ≤ width et les émet avec
    /// préfixe (lignes suivantes : préfixe de continuation).
    fn flush(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.inline);
        let prefix_w = display_width(&self.prefix);
        let lines = wrap_spans(&spans, self.width.saturating_sub(prefix_w).max(1));
        for (i, line) in lines.into_iter().enumerate() {
            let prefix = if i == 0 {
                self.prefix.clone()
            } else {
                self.cont.clone()
            };
            let mut out_spans: Vec<Span<'static>> = Vec::new();
            if !prefix.is_empty() {
                let pstyle = if prefix.contains('•') {
                    self.theme.bullet
                } else {
                    self.theme.text
                };
                out_spans.push(Span::styled(prefix, pstyle));
            }
            out_spans.extend(line.spans);
            self.out.push(Line::from(out_spans));
        }
    }

    /// Émet le bloc de code en cours : étiquette du langage, contenu sans
    /// retour à la ligne par mots (coupure dure + marqueur de continuation).
    fn flush_code(&mut self) {
        let Some(code) = self.code.take() else {
            return;
        };
        if !code.lang.is_empty() {
            self.out.push(Line::from(Span::styled(
                format!("``` {}", code.lang),
                self.theme.code_block,
            )));
        }
        for raw in code.buf.split('\n') {
            if raw.is_empty() {
                self.out.push(Line::from(Span::styled(
                    String::new(),
                    self.theme.code_block,
                )));
                continue;
            }
            let pieces = hard_split(raw, self.width.saturating_sub(1).max(1));
            for (i, piece) in pieces.into_iter().enumerate() {
                let text = if i == 0 { piece } else { format!("↩{piece}") };
                self.out
                    .push(Line::from(Span::styled(text, self.theme.code_block)));
            }
        }
        self.set_plain_prefix();
    }
}

/// Coupure dure en graphèmes, morceaux de `width` colonnes au plus
/// (partagée avec ./view.rs pour le rendu des résultats d'outils).
#[must_use]
pub(crate) fn hard_split(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut w = 0usize;
    for g in graphemes(s) {
        let gw = display_width(g);
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

/// Plie une suite de spans (par mots, coupure dure des mots trop longs) en
/// lignes ≤ `width`, styles préservés.
fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    let flush_line = |cur: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>| {
        cur.retain(|s| !s.content.is_empty());
        lines.push(Line::from(std::mem::take(cur)));
    };
    for span in spans {
        let style = span.style;
        let mut first_word_of_span = true;
        for word in span.content.split(' ') {
            if !word.is_empty() && !first_word_of_span {
                // l'espace sautée par le split devient une séparation de ligne
                if cur_w > 0 {
                    flush_line(&mut cur, &mut lines);
                    cur_w = 0;
                }
            }
            first_word_of_span = false;
            if word.is_empty() {
                // espaces significatives du texte d'origine
                if cur_w > 0 && cur_w < width {
                    cur.push(Span::styled(" ", style));
                    cur_w = cur_w.saturating_add(1);
                }
                continue;
            }
            let mut rest = word;
            while !rest.is_empty() {
                let rw: usize = graphemes(rest).map(display_width).sum();
                if cur_w.saturating_add(rw) <= width {
                    cur.push(Span::styled(rest.to_string(), style));
                    cur_w = cur_w.saturating_add(rw);
                    break;
                }
                // place restante : préfixe du mot, puis nouvelle ligne.
                let room = width.saturating_sub(cur_w);
                if room == 0 {
                    flush_line(&mut cur, &mut lines);
                    cur_w = 0;
                    continue;
                }
                let mut head = String::new();
                let mut hw = 0usize;
                let mut consumed = 0usize;
                for g in graphemes(rest) {
                    let gw = display_width(g);
                    if hw.saturating_add(gw) > room {
                        break;
                    }
                    head.push_str(g);
                    hw = hw.saturating_add(gw);
                    consumed = consumed.saturating_add(g.len());
                }
                if head.is_empty() {
                    // graphe unique plus large que la ligne : il occupe sa ligne.
                    let g: String = graphemes(rest).next().unwrap_or_default().to_string();
                    let glen = g.len();
                    cur.push(Span::styled(g, style));
                    rest = rest.get(glen..).unwrap_or("");
                    flush_line(&mut cur, &mut lines);
                    cur_w = 0;
                } else {
                    cur.push(Span::styled(head, style));
                    rest = &rest[consumed..];
                    flush_line(&mut cur, &mut lines);
                    cur_w = 0;
                }
            }
        }
    }
    if !cur.is_empty() {
        flush_line(&mut cur, &mut lines);
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;
    use unicode_width::UnicodeWidthStr;

    fn flat(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn theme() -> Theme {
        Theme::dark()
    }

    /// Scenario: `titre_et_paragraphe` — `# Titre\n\ntexte` à largeur 40 :
    /// une ligne de titre stylée puis une ligne de texte.
    #[test]
    fn titre_et_paragraphe() {
        let out = render("# Titre\n\ntexte", 40, &theme());
        let l = flat(&out);
        assert!(l.iter().any(|s| s.contains("Titre")), "{l:?}");
        assert!(l.iter().any(|s| s.trim() == "texte"), "{l:?}");
        let titre = &out
            .iter()
            .find(|line| flat(std::slice::from_ref(line))[0].contains("Titre"))
            .expect("ligne de titre");
        assert!(
            titre.spans.iter().any(|s| s.style != Style::default()),
            "le titre est stylé"
        );
    }

    /// Scenario: `bloc_de_code` — un bloc ```rust … ``` : langage `rust`
    /// affiché, contenu style code, PAS de retour à la ligne par mots.
    #[test]
    fn bloc_de_code() {
        let md = "```rust\nfn main() { println!(\"hi\"); }\n```";
        let out = render(md, 40, &theme());
        let l = flat(&out);
        assert!(
            l.iter().any(|s| s.contains("rust")),
            "le langage est affiché: {l:?}"
        );
        assert!(
            l.iter().any(|s| s.contains("fn main()")),
            "le code est rendu: {l:?}"
        );
        assert!(
            !l.iter()
                .any(|s| s.contains("println!(") && !s.contains("hi")),
            "pas de retour à la ligne par mots dans le code: {l:?}"
        );
    }

    /// Scenario: `bloc_non_termine` — un bloc de code ouvert sans fermeture est
    /// rendu comme un bloc de code ouvert (pas comme du texte ordinaire).
    #[test]
    fn bloc_non_termine() {
        let md = "voici :\n```python\nprint(1";
        let out = render(md, 40, &theme());
        let l = flat(&out);
        assert!(
            l.iter().any(|s| s.contains("python")),
            "le bloc ouvert garde son langage: {l:?}"
        );
        assert!(
            l.iter().any(|s| s.contains("print(1")),
            "le contenu du bloc ouvert est rendu: {l:?}"
        );
    }

    /// Scenario: `liste` — deux puces et une liste numérotée : puces `•`,
    /// numéros, retour à la ligne indenté.
    #[test]
    fn liste() {
        let md = "- un\n- deux\n\n1. premier\n2. second";
        let out = render(md, 40, &theme());
        let l = flat(&out);
        assert!(
            l.iter().any(|s| s.contains('•')) && l.iter().any(|s| s.contains("un")),
            "puces •: {l:?}"
        );
        assert!(
            l.iter().any(|s| s.contains("1.") && s.contains("premier")),
            "liste numérotée: {l:?}"
        );
    }

    /// Scenario: `largeur_respectee` — paragraphe de 500 caractères et texte
    /// CJK à largeur 30 : aucune ligne ne dépasse 30 colonnes d'affichage.
    #[test]
    fn largeur_respectee() {
        let long = "mot ".repeat(125); // 500 caractères
        let cjk = "日本語テキスト".repeat(30);
        let out = render(&format!("{long}\n\n{cjk}"), 30, &theme());
        for line in &out {
            let w: usize = line
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(
                w <= 30,
                "ligne de {w} > 30 : {:?}",
                flat(std::slice::from_ref(line))[0]
            );
        }
    }

    /// Scenario: `sequences_de_controle_neutralisees` — un texte contenant
    /// `\u{1b}[2J` : aucun caractère d'échappement dans la sortie.
    #[test]
    fn sequences_de_controle_neutralisees() {
        let out = render("avant \u{1b}[2J après", 40, &theme());
        let joined = flat(&out).join("\n");
        assert!(!joined.contains('\u{1b}'), "ESC neutralisé: {joined:?}");
        assert!(
            joined.contains("avant") && joined.contains("après"),
            "texte conservé"
        );
    }

    /// Scenario: `markdown_malforme` — `**`, crochets et backticks non
    /// appariés : aucun plantage, le texte reste visible.
    #[test]
    fn markdown_malforme() {
        let out = render("**gras ??? [lien ??? `tick", 40, &theme());
        let joined = flat(&out).join(" ");
        assert!(joined.contains("gras"), "texte visible: {joined:?}");
        assert!(joined.contains("lien"), "texte visible: {joined:?}");
        assert!(joined.contains("tick"), "texte visible: {joined:?}");
    }

    /// Scenario: `tableau_non_supporte` — un tableau markdown : son texte
    /// apparaît en clair, rien n'est perdu.
    #[test]
    fn tableau_non_supporte() {
        let md = "| colonne | autre |\n| --- | --- |\n| value | 42 |";
        let out = render(md, 60, &theme());
        let joined = flat(&out).join(" ");
        assert!(
            joined.contains("value") && joined.contains("42"),
            "rien n'est perdu: {joined:?}"
        );
    }

    /// Cas additionnel : texte vide ou largeur nulle ⇒ aucune panique.
    #[test]
    fn vide_et_largeur_nulle() {
        assert!(
            render("", 40, &theme())
                .iter()
                .all(|l| flat(std::slice::from_ref(l))[0].is_empty()
                    || flat(std::slice::from_ref(l))[0] == " ")
        );
        let _ = render("texte", 0, &theme());
        let _ = render("```rust\ncode", 1, &theme());
    }
}
