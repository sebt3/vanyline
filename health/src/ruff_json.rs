//! Parseur pur de `ruff check --output-format json` vers [`Issue`].

use serde::Deserialize;
use std::path::Path;

use crate::error::HealthError;
use crate::report::Issue;
use crate::report::Severity;

/// Diagnostic ruff (`ruff check --output-format json`).
#[derive(Debug, Deserialize)]
struct RuffDiagnostic {
    code: Option<String>,
    message: String,
    filename: String,
    location: RuffLocation,
}

/// Position d'un diagnostic ruff.
#[derive(Debug, Deserialize)]
struct RuffLocation {
    row: u32,
    column: u32,
}

/// Parseur pur de `ruff check --output-format json` vers une liste triée et dédoublonnée
/// d'[`Issue`] : l'analyse statique Python. La sévérité est toujours [`Severity::Error`]
/// (ruff ne distingue pas : tout diagnostic est une erreur de lint). Un fichier hors de
/// `root` est ignoré. Une sortie vide (`[]` ou chaîne vide) donne une liste vide.
///
/// * `stdout` — tableau JSON de diagnostics (`code`, `message`, `filename`,
///   `location { row, column }`).
/// * `root` — racine du dépôt (relativisation).
/// * `check_id` — identifiant de la vérification émettrice.
///
/// # Errors
///
/// Retourne [`HealthError::InvalidCheck`] quand une sortie non vide n'est pas un tableau
/// JSON valide : c'est la vérification qui est « inexploitable » (le runner la marque
/// `errored`), jamais un vert.
pub fn parse_ruff_json(
    stdout: &str,
    root: &Path,
    check_id: &str,
) -> Result<Vec<Issue>, HealthError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let diagnostics: Vec<RuffDiagnostic> =
        serde_json::from_str(trimmed).map_err(|e| HealthError::InvalidCheck {
            id: check_id.to_string(),
            reason: e.to_string(),
        })?;
    let mut issues = Vec::new();
    for diagnostic in diagnostics {
        let Some(path) = relativize(&diagnostic.filename, root) else {
            continue;
        };
        issues.push(Issue {
            check_id: check_id.to_string(),
            path,
            line: diagnostic.location.row,
            column: diagnostic.location.column,
            severity: Severity::Error,
            code: diagnostic.code,
            message: diagnostic.message,
        });
    }
    sort_dedup(&mut issues);
    Ok(issues)
}

/// Rend un `filename` relatif à `root` (séparateur `/`, `./` retiré). Un chemin absolu hors
/// de `root` donne `None` (ignoré).
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
    use super::parse_ruff_json;
    use crate::error::HealthError;
    use crate::report::Severity;
    use std::path::Path;

    fn parse(stdout: &str) -> Result<Vec<crate::report::Issue>, HealthError> {
        parse_ruff_json(stdout, Path::new("/repo"), "ruff")
    }

    /// Scenario `diagnostic_simple`.
    #[test]
    fn diagnostic_simple() {
        let stdout = r#"[{"code":"F401","message":"x imported but unused","filename":"/repo/a.py","location":{"row":3,"column":1}}]"#;
        let issues = parse(stdout).expect("tableau JSON valide");
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(issue.check_id, "ruff");
        assert_eq!(issue.path, "a.py");
        assert_eq!(issue.line, 3);
        assert_eq!(issue.column, 1);
        assert_eq!(issue.code.as_deref(), Some("F401"));
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.message, "x imported but unused");
    }

    /// Scenario `aucun_diagnostic` — `[]` et chaîne vide donnent tous deux une liste vide.
    #[test]
    fn aucun_diagnostic() {
        assert_eq!(parse("[]").expect("tableau vide valide"), Vec::new());
        assert_eq!(
            parse("").expect("chaîne vide = aucun diagnostic"),
            Vec::new()
        );
    }

    /// Scenario `sortie_illisible` — une sortie non vide non-JSON est `InvalidCheck`,
    /// jamais un vert.
    #[test]
    fn sortie_illisible() {
        let err = parse("ruff: command not found").expect_err("sortie inexploitable");
        match err {
            HealthError::InvalidCheck { id, .. } => assert_eq!(id, "ruff"),
            other => panic!("attendu InvalidCheck, obtenu {other:?}"),
        }
    }

    /// Scenario `fichier_hors_racine` — un `filename` hors de `root` est ignoré.
    #[test]
    fn fichier_hors_racine() {
        let stdout = r#"[{"code":"F401","message":"x","filename":"/tmp/a.py","location":{"row":1,"column":1}}]"#;
        let issues = parse(stdout).expect("tableau JSON valide");
        assert_eq!(issues, Vec::new());
    }
}
