//! Parseur pur de `hadolint --format json` vers [`Issue`].

use serde::Deserialize;
use std::path::Path;

use crate::error::HealthError;
use crate::report::Issue;
use crate::report::Severity;

/// Diagnostic hadolint (`hadolint --format json`).
#[derive(Debug, Deserialize)]
struct HadolintDiagnostic {
    code: Option<String>,
    message: String,
    file: String,
    line: u32,
    column: u32,
    level: String,
}

/// Parseur pur de `hadolint --format json` vers une liste triée et dédoublonnée d'[`Issue`]
/// : l'analyse statique des Dockerfile. Seuls les niveaux `error` et `warning` sont retenus
/// (`info`/`style` ignorés, bruit sans conséquence sur la santé). Une sortie vide donne une
/// liste vide.
///
/// * `stdout` — tableau JSON de diagnostics (`code`, `message`, `file`, `line`, `column`,
///   `level`).
/// * `root` — racine du dépôt (relativisation).
/// * `check_id` — identifiant de la vérification émettrice.
///
/// # Errors
///
/// Retourne [`HealthError::InvalidCheck`] quand une sortie non vide n'est pas un tableau
/// JSON valide — jamais un vert.
pub fn parse_hadolint_json(
    stdout: &str,
    root: &Path,
    check_id: &str,
) -> Result<Vec<Issue>, HealthError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let diagnostics: Vec<HadolintDiagnostic> =
        serde_json::from_str(trimmed).map_err(|e| HealthError::InvalidCheck {
            id: check_id.to_string(),
            reason: e.to_string(),
        })?;
    let mut issues = Vec::new();
    for diagnostic in diagnostics {
        let severity = match diagnostic.level.as_str() {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            _ => continue,
        };
        let Some(path) = relativize(&diagnostic.file, root) else {
            continue;
        };
        issues.push(Issue {
            check_id: check_id.to_string(),
            path,
            line: diagnostic.line,
            column: diagnostic.column,
            severity,
            code: diagnostic.code,
            message: diagnostic.message,
        });
    }
    sort_dedup(&mut issues);
    Ok(issues)
}

/// Rend un `file` relatif à `root` (séparateur `/`, `./` retiré). Un chemin absolu hors de
/// `root` donne `None` (ignoré).
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
/// `(path, line, column, code, message)`, comme `ruff_json`.
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
    use super::parse_hadolint_json;
    use crate::error::HealthError;
    use crate::report::Severity;
    use std::path::Path;

    fn diag(level: &str) -> String {
        format!(
            r#"{{"code":"DL3008","message":"Pin versions in apt get ({level})","file":"/repo/deploy/Dockerfile","line":4,"column":3,"level":"{level}"}}"#
        )
    }

    fn parse(stdout: &str) -> Result<Vec<crate::report::Issue>, HealthError> {
        parse_hadolint_json(stdout, Path::new("/repo"), "hadolint")
    }

    /// Scenario `warning_et_error_retenus`.
    #[test]
    fn warning_et_error_retenus() {
        let stdout = format!("[{},{}]", diag("warning"), diag("error"));
        let issues = parse(&stdout).expect("tableau JSON valide");
        assert_eq!(issues.len(), 2);
        let severities: Vec<Severity> = issues.iter().map(|i| i.severity.clone()).collect();
        assert!(severities.contains(&Severity::Warning));
        assert!(severities.contains(&Severity::Error));
    }

    /// Scenario `info_et_style_ignores`.
    #[test]
    fn info_et_style_ignores() {
        let stdout = format!("[{},{}]", diag("info"), diag("style"));
        let issues = parse(&stdout).expect("tableau JSON valide");
        assert_eq!(issues, Vec::new());
    }

    /// Scenario `sortie_illisible`.
    #[test]
    fn sortie_illisible() {
        let err = parse("hadolint: not found").expect_err("sortie inexploitable");
        match err {
            HealthError::InvalidCheck { id, .. } => assert_eq!(id, "hadolint"),
            other => panic!("attendu InvalidCheck, obtenu {other:?}"),
        }
    }

    /// Scenario `chemin_relatif_a_la_racine`.
    #[test]
    fn chemin_relatif_a_la_racine() {
        let stdout = format!("[{}]", diag("warning"));
        let issues = parse(&stdout).expect("tableau JSON valide");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "deploy/Dockerfile");
        assert_eq!(issues[0].code.as_deref(), Some("DL3008"));
    }
}
