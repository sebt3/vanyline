//! Parseur pur de la sortie TEXTE de `tsc`/`vue-tsc` vers [`Issue`].

use crate::report::Issue;
use crate::report::Severity;

/// Parseur pur de la sortie TEXTE de `tsc`/`vue-tsc` vers une liste triée et dédoublonnée
/// d'[`Issue`]. Le contrat est le format ligne
/// `<chemin>(<ligne>,<colonne>): error|warning TS<n>: <message>` (variante `-pretty false`).
/// Infaillible : le bruit (bannières npm, `Found N errors`, séquences ANSI retirées avant
/// l'analyse) et les chemins absolus (hors dépôt, non vérifiables sans racine) sont ignorés.
///
/// * `stdout` — sortie combinée de la commande.
/// * `prefix` — chemin du workspace relatif à la racine du dépôt (`""` pour la racine),
///   préfixé aux chemins rapportés.
/// * `check_id` — identifiant de la vérification émettrice, reporté dans chaque [`Issue`].
///
/// Les lignes de continuation indentées qui suivent un diagnostic sont ajoutées à son
/// message, séparées par un espace. Ne reclasse jamais la sévérité et n'ignore aucun
/// `error`.
#[must_use]
pub fn parse_tsc_text(stdout: &str, prefix: &str, check_id: &str) -> Vec<Issue> {
    let cleaned = strip_ansi(stdout);
    let mut issues: Vec<Issue> = Vec::new();
    for line in cleaned.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = issues.last_mut() {
                let detail = line.trim();
                if !detail.is_empty() {
                    last.message.push(' ');
                    last.message.push_str(detail);
                }
            }
            continue;
        }
        if let Some(issue) = parse_diagnostic(line, prefix, check_id) {
            issues.push(issue);
        }
    }
    sort_dedup(&mut issues);
    issues
}

/// Retire les séquences ANSI : CSI (`ESC [ … octet final '@'..='~'`) et OSC (`ESC ] …
/// BEL` ou `ESC ] … ESC \`), un `ESC` seul étant simplement droppé.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for terminator in chars.by_ref() {
                    if ('@'..='~').contains(&terminator) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(terminator) = chars.next() {
                    if terminator == '\u{7}' {
                        break;
                    }
                    if terminator == '\u{1b}' && chars.peek().copied() == Some('\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn parse_diagnostic(line: &str, prefix: &str, check_id: &str) -> Option<Issue> {
    let open = line.find('(')?;
    let path_raw = &line[..open];
    let after_open = &line[open.saturating_add(1)..];
    let close = after_open.find("): ")?;
    let mut positions = after_open[..close].split(',');
    let line_no: u32 = positions.next()?.trim().parse().ok()?;
    let column_no: u32 = positions.next()?.trim().parse().ok()?;
    if positions.next().is_some() {
        return None;
    }
    let rest = &after_open[close.saturating_add(3)..];
    let (severity_token, rest) = rest.split_once(' ')?;
    let severity = match severity_token {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        _ => return None,
    };
    let (code_token, message) = rest.split_once(": ")?;
    let digits = code_token.strip_prefix("TS")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let path = normalize_join(prefix, path_raw)?;
    Some(Issue {
        check_id: check_id.to_string(),
        path,
        line: line_no,
        column: column_no,
        severity,
        code: Some(code_token.to_string()),
        message: message.to_string(),
    })
}

/// Joint `prefix` au chemin rapporté et normalise (séparateur `/`, `./` retiré, `..`
/// résolus). Un chemin absolu est ignoré (hors dépôt : `prefix` est relatif au dépôt).
fn normalize_join(prefix: &str, path: &str) -> Option<String> {
    if path.is_empty() || path.starts_with('/') {
        return None;
    }
    let joined = if prefix.is_empty() {
        path.to_string()
    } else {
        format!("{prefix}/{path}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for component in joined.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Tri total par `(path, line, column, code)` puis dédoublonnage sur
/// `(path, line, column, code, message)`.
fn sort_dedup(issues: &mut Vec<Issue>) {
    issues.sort_by(|a, b| {
        (&a.path, a.line, a.column, a.code.as_deref()).cmp(&(
            &b.path,
            b.line,
            b.column,
            b.code.as_deref(),
        ))
    });
    issues.dedup_by(|a, b| {
        a.path == b.path
            && a.line == b.line
            && a.column == b.column
            && a.code == b.code
            && a.message == b.message
    });
}

#[cfg(test)]
mod tests {
    use super::parse_tsc_text;
    use crate::report::Severity;

    fn parse(stdout: &str, prefix: &str) -> Vec<crate::report::Issue> {
        parse_tsc_text(stdout, prefix, "tsc")
    }

    /// Scenario `erreur_simple`.
    #[test]
    fn erreur_simple() {
        let stdout =
            "src/a.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.\n";
        let issues = parse(stdout, "frontend");
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(issue.check_id, "tsc");
        assert_eq!(issue.path, "frontend/src/a.ts");
        assert_eq!(issue.line, 12);
        assert_eq!(issue.column, 5);
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.code.as_deref(), Some("TS2322"));
        assert_eq!(
            issue.message,
            "Type 'string' is not assignable to type 'number'."
        );
    }

    /// Scenario `ligne_de_continuation` — le détail indenté est ajouté au message.
    #[test]
    fn ligne_de_continuation() {
        let stdout = "src/a.ts(12,5): error TS2322: Type 'string' is not assignable.\n    The types of 'x.y' are incompatible between these types.\n";
        let issues = parse(stdout, "");
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].message,
            "Type 'string' is not assignable. The types of 'x.y' are incompatible between these types."
        );
    }

    /// Scenario `bruit_ignore` — bannières npm, compteur final et séquences ANSI ignorés ;
    /// seule la ligne de diagnostic (portée par ses séquences ANSI) donne une Issue.
    #[test]
    fn bruit_ignore() {
        let stdout = "> tsc --noEmit\n\u{1b}[96msrc/a.ts\u{1b}[0m(1,2): \u{1b}[91merror\u{1b}[0m TS1005: x\nFound 1 errors in 1 file.\n";
        let issues = parse(stdout, "");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "src/a.ts");
        assert_eq!(issues[0].code.as_deref(), Some("TS1005"));
    }

    /// Scenario `doublon` — deux fois la même ligne : une seule Issue.
    #[test]
    fn doublon() {
        let line = "src/a.ts(1,2): error TS1005: x\n";
        let issues = parse(&format!("{line}{line}"), "");
        assert_eq!(issues.len(), 1);
    }

    /// Scenario `racine_sans_prefixe`.
    #[test]
    fn racine_sans_prefixe() {
        let issues = parse("a.ts(1,1): error TS1005: x\n", "");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "a.ts");
    }
}
