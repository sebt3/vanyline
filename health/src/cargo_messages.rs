//! Parseur pur des diagnostics JSON de cargo (`--message-format=json`) vers [`Issue`].

use serde_json::Value;
use std::path::Path;

use crate::report::Issue;
use crate::report::Severity;

/// Parseur pur des diagnostics JSON de cargo (`--message-format=json`, utilisé par
/// `cargo clippy` et `cargo check`) vers une liste triée et dédoublonnée d'[`Issue`].
/// Infaillible : les lignes non JSON, les autres `reason`, les JSON de forme inattendue, les
/// niveaux `note`/`help`/`failure-note`, les diagnostics sans span primaire et les spans hors
/// de `root` sont ignorés sans erreur.
///
/// * `stdout` — une ligne JSON par message cargo.
/// * `root` — racine du dépôt (relativisation et exclusion des dépendances).
/// * `check_id` — identifiant de la vérification émettrice, reporté dans chaque [`Issue`].
///
/// Ne reclasse jamais la sévérité : un `warning` rapporté par cargo reste `warning`, même
/// sous `-D warnings`.
pub fn parse_cargo_messages(stdout: &str, root: &Path, check_id: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let severity = match message.get("level").and_then(Value::as_str) {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            _ => continue,
        };
        let Some(span) = message
            .get("spans")
            .and_then(Value::as_array)
            .and_then(|spans| {
                spans
                    .iter()
                    .find(|s| s.get("is_primary").and_then(Value::as_bool) == Some(true))
            })
        else {
            continue;
        };
        let Some(path) = span
            .get("file_name")
            .and_then(Value::as_str)
            .and_then(|file_name| relativize(file_name, root))
        else {
            continue;
        };
        let (Some(line_no), Some(column_no)) =
            (as_u32(span, "line_start"), as_u32(span, "column_start"))
        else {
            continue;
        };
        issues.push(Issue {
            check_id: check_id.to_string(),
            path,
            line: line_no,
            column: column_no,
            severity,
            code: message
                .pointer("/code/code")
                .and_then(Value::as_str)
                .map(str::to_string),
            message: message
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    sort_dedup(&mut issues);
    issues
}

fn as_u32(value: &Value, key: &str) -> Option<u32> {
    value.get(key).and_then(Value::as_u64)?.try_into().ok()
}

/// Rend un `file_name` de span relatif à `root` : un chemin absolu doit être sous `root`
/// (sinon `None` : source de dépendance, ignorée) ; un chemin relatif est normalisé.
fn relativize(file_name: &str, root: &Path) -> Option<String> {
    if file_name.is_empty() {
        return None;
    }
    let joined = if file_name.starts_with('/') {
        Path::new(file_name)
            .strip_prefix(root)
            .ok()?
            .to_str()?
            .to_string()
    } else {
        file_name.to_string()
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
/// `(path, line, column, code, message)` — cargo émet le même diagnostic une fois par cible
/// avec `--all-targets`.
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
    use super::parse_cargo_messages;
    use crate::report::Severity;
    use serde_json::json;
    use std::path::Path;

    fn msg_line(
        level: &str,
        code: Option<&str>,
        file_name: &str,
        line: u64,
        column: u64,
    ) -> String {
        let code_json = match code {
            Some(c) => json!({ "code": c, "explanation": null }),
            None => json!(null),
        };
        json!({
            "reason": "compiler-message",
            "package_id": "path+file:///repo#0.1.0",
            "target": { "kind": ["lib"], "name": "x", "src_path": null },
            "message": {
                "message": "used `unwrap()`",
                "code": code_json,
                "level": level,
                "spans": [{
                    "file_name": file_name,
                    "byte_start": 0, "byte_end": 1,
                    "line_start": line, "line_end": line,
                    "column_start": column, "column_end": column.saturating_add(1),
                    "is_primary": true,
                    "text": [], "label": null,
                    "suggested_replacement": null, "suggestion_applicability": null,
                    "expansion": null
                }],
                "children": [],
                "rendered": "warning: used `unwrap()`"
            }
        })
        .to_string()
    }

    fn parse(stdout: &str) -> Vec<crate::report::Issue> {
        parse_cargo_messages(stdout, Path::new("/repo"), "clippy")
    }

    /// Scenario `warning_clippy_simple`.
    #[test]
    fn warning_clippy_simple() {
        let line = msg_line("warning", Some("clippy::unwrap_used"), "src/a.rs", 12, 5);
        let issues = parse(&line);
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(issue.check_id, "clippy");
        assert_eq!(issue.path, "src/a.rs");
        assert_eq!(issue.line, 12);
        assert_eq!(issue.column, 5);
        assert_eq!(issue.severity, Severity::Warning);
        assert_eq!(issue.code.as_deref(), Some("clippy::unwrap_used"));
        assert_eq!(issue.message, "used `unwrap()`");
    }

    /// Scenario `erreur_sous_deny` — un niveau `error` donne severity `error`.
    #[test]
    fn erreur_sous_deny() {
        let line = msg_line("error", Some("clippy::unwrap_used"), "src/a.rs", 12, 5);
        let issues = parse(&line);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
    }

    /// Scenario `doublon_par_cible` — même diagnostic sur deux cibles : une seule Issue.
    #[test]
    fn doublon_par_cible() {
        let line = msg_line("warning", Some("clippy::unwrap_used"), "src/a.rs", 12, 5);
        let issues = parse(&format!("{line}\n{line}\n"));
        assert_eq!(issues.len(), 1);
    }

    /// Scenario `note_et_help_ignores`.
    #[test]
    fn note_et_help_ignores() {
        let stdout = format!(
            "{}\n{}\n",
            msg_line("note", None, "src/a.rs", 1, 1),
            msg_line("help", None, "src/a.rs", 1, 1),
        );
        assert_eq!(parse(&stdout), Vec::new());
    }

    /// Scenario `resume_sans_span` — « aborting due to 3 previous errors » (spans vides).
    #[test]
    fn resume_sans_span() {
        let line = json!({
            "reason": "compiler-message",
            "message": {
                "message": "aborting due to 3 previous errors",
                "code": null,
                "level": "error",
                "spans": [],
                "children": [],
                "rendered": "error: aborting due to 3 previous errors"
            }
        })
        .to_string();
        assert_eq!(parse(&line), Vec::new());
    }

    /// Scenario `dependance_ignoree` — span dans le registre cargo, hors root.
    #[test]
    fn dependance_ignoree() {
        let line = msg_line(
            "warning",
            Some("clippy::pedantic"),
            "/home/u/.cargo/registry/src/index.crates.io/x/src/lib.rs",
            3,
            1,
        );
        assert_eq!(parse(&line), Vec::new());
    }

    /// Scenario `ligne_non_json` — une ligne de texte humain entre deux lignes JSON valides
    /// ne masque pas les messages valides, pas de panique.
    #[test]
    fn ligne_non_json() {
        let good = msg_line("warning", Some("clippy::unwrap_used"), "src/a.rs", 1, 1);
        let good2 = msg_line("warning", Some("clippy::expect_used"), "src/b.rs", 2, 2);
        let stdout = format!("{good}\nwarning: unused\n{good2}\n");
        let issues = parse(&stdout);
        assert_eq!(issues.len(), 2);
    }

    /// Scenario `chemin_absolu_dans_racine` — `/repo/src/b.rs` → `src/b.rs`.
    #[test]
    fn chemin_absolu_dans_racine() {
        let line = msg_line("warning", None, "/repo/src/b.rs", 4, 2);
        let issues = parse(&line);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "src/b.rs");
        assert_eq!(issues[0].code, None);
    }
}
