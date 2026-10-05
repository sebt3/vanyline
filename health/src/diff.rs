//! Périmètre « code neuf » d'une MR : lignes ajoutées/modifiées d'un `git diff -U0`, et
//! restriction de couverture et d'issues à ces lignes. Pur : le diff arrive en texte, c'est
//! `./runner.rs` qui lance git.

use std::collections::BTreeMap;

use crate::error::HealthError;
use crate::report::CoverageReport;
use crate::report::FileCoverage;
use crate::report::Issue;
use crate::report::LineRange;

/// Chemin relatif → plages de lignes du côté NOUVEAU (ajoutées/modifiées), normalisées
/// (triées, disjointes, fusionnées si contiguës).
pub type ChangedLines = BTreeMap<String, Vec<LineRange>>;

/// Extrait les lignes modifiées de la sortie d'un
/// `git -c core.quotepath=off diff -U0 --no-color --no-ext-diff <merge-base>`.
///
/// Sont lus : `+++ b/<chemin>` (fichier cible) et en-têtes de hunk `@@ -a[,b] +c[,d] @@`
/// (`d` absent vaut 1 ; `d == 0` n'ajoute aucune ligne). Un fichier supprimé
/// (`+++ /dev/null`) et un fichier binaire (sans section `+++`) sont absents du résultat.
///
/// # Errors
///
/// [`HealthError::InvalidDiff`] si un hunk est mal formé (nombres non numériques), si un
/// hunk apparaît avant tout `+++`, ou si un nom de fichier est entre guillemets (git cite
/// les chemins à caractères spéciaux) — refusé plutôt qu'ignoré, car ignorer un fichier
/// ferait paraître le code neuf plus sain qu'il n'est.
pub fn parse_changed_lines(diff: &str) -> Result<ChangedLines, HealthError> {
    let mut collected: BTreeMap<String, Vec<LineRange>> = BTreeMap::new();
    // `None` : aucun en-tête de fichier cible vu ; `Some(None)` : fichier ignoré (supprimé) ;
    // `Some(Some(path))` : fichier cible courant.
    let mut current: Option<Option<String>> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let target = rest.strip_suffix('\r').unwrap_or(rest);
            current = Some(if target == "/dev/null" {
                // Fichier supprimé : absent du résultat.
                None
            } else if target.starts_with('"') {
                // git cite les chemins à caractères spéciaux : refuser plutôt qu'omettre.
                return Err(HealthError::InvalidDiff {
                    reason: format!("quoted file name in diff: {target}"),
                });
            } else if let Some(path) = target.strip_prefix("b/") {
                Some(path.to_string())
            } else {
                return Err(HealthError::InvalidDiff {
                    reason: format!("unexpected +++ target: {target}"),
                });
            });
            if let Some(Some(path)) = &current {
                collected.entry(path.clone()).or_default();
            }
            continue;
        }
        if let Some(header) = line.strip_prefix("@@ ") {
            match &current {
                // Hunk avant tout en-tête de fichier cible : refusé.
                None => {
                    return Err(HealthError::InvalidDiff {
                        reason: "hunk before any +++ file header".to_string(),
                    });
                }
                // Fichier ignoré (supprimé) : ses hunks ne changent rien.
                Some(None) => {}
                Some(Some(path)) => {
                    let range = parse_hunk_new_side(header)?;
                    if let Some(range) = range {
                        collected.entry(path.clone()).or_default().push(range);
                    }
                }
            }
        }
    }
    let mut changed: ChangedLines = BTreeMap::new();
    for (path, ranges) in collected {
        let normalized = normalize_ranges(&ranges);
        if !normalized.is_empty() {
            changed.insert(path, normalized);
        }
    }
    Ok(changed)
}

/// Parse un en-tête de hunk `a[,b] +c[,d] @@…` : `Ok(None)` si `d == 0`
/// (suppression pure), `Err` si mal formé.
fn parse_hunk_new_side(header: &str) -> Result<Option<LineRange>, HealthError> {
    let invalid = |reason: &str| HealthError::InvalidDiff {
        reason: format!("malformed hunk header: {reason}"),
    };
    let mut tokens = header.split_whitespace();
    let old = tokens.next().ok_or_else(|| invalid("missing old range"))?;
    let new = tokens.next().ok_or_else(|| invalid("missing new range"))?;
    if !tokens.next().is_some_and(|rest| rest.starts_with("@@")) {
        return Err(invalid("missing closing @@"));
    }
    let old_start = old
        .strip_prefix('-')
        .ok_or_else(|| invalid("old range must start with -"))?;
    parse_range(old_start).ok_or_else(|| invalid("old range is not numeric"))?;
    let new_start = new
        .strip_prefix('+')
        .ok_or_else(|| invalid("new range must start with +"))?;
    let (start, count) =
        parse_range(new_start).ok_or_else(|| invalid("new range is not numeric"))?;
    let count = count.unwrap_or(1);
    if count == 0 {
        return Ok(None);
    }
    if start == 0 {
        return Err(invalid("new range starts at 0 with added lines"));
    }
    let end = start.saturating_add(count).saturating_sub(1);
    Ok(Some(LineRange { start, end }))
}

/// Parse une plage `a` ou `a,b` (compte absent vaut 1 ; vide = 0 lignes). `None` si un
/// champ n'est pas numérique.
fn parse_range(text: &str) -> Option<(u32, Option<u32>)> {
    if text.is_empty() {
        return Some((0, Some(0)));
    }
    let (start_text, count_text) = match text.split_once(',') {
        Some((start, count)) => (start, Some(count)),
        None => (text, None),
    };
    let start: u32 = start_text.parse().ok()?;
    let count = match count_text {
        Some(c) => Some(c.parse::<u32>().ok()?),
        None => None,
    };
    Some((start, count))
}

/// Trie, fusionne les plages qui se chevauchent ou se touchent, rend un vecteur disjoint.
fn normalize_ranges(ranges: &[LineRange]) -> Vec<LineRange> {
    let mut sorted = ranges.to_vec();
    sorted.sort_unstable_by_key(|r| (r.start, r.end));
    let mut out: Vec<LineRange> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match out.last_mut() {
            Some(last) if range.start <= last.end.saturating_add(1) => {
                if range.end > last.end {
                    last.end = range.end;
                }
            }
            _ => out.push(range),
        }
    }
    out
}

/// Restreint un rapport de couverture au périmètre modifié : ne garde que les fichiers
/// présents dans `changed` et, pour chacun, que les lignes instrumentées comprises dans les
/// plages modifiées ; `lines_found`/`lines_hit`/`uncovered` sont recalculés sur ce
/// sous-ensemble. Un fichier modifié dont AUCUNE ligne modifiée n'est instrumentée est omis
/// (rien à couvrir).
#[must_use]
pub fn restrict_coverage(report: &CoverageReport, changed: &ChangedLines) -> CoverageReport {
    let mut files: Vec<FileCoverage> = Vec::new();
    for file in &report.files {
        let Some(ranges) = changed.get(&file.path) else {
            continue;
        };
        // `FileCoverage` est un agrégat (trouvé/touché/plages non couvertes) : la position
        // des lignes COUVERTES n'y est pas. Le périmètre instrumenté connu est donc borné
        // par `max(lines_found, plus grande ligne non couverte)` — les lignes modifiées
        // au-delà de cette borne ne peuvent rien avoir d'instrumenté connu (rien à couvrir ⇒
        // fichier omis). À l'intérieur de la borne, une ligne modifiée non listée comme non
        // couverte est tenue pour couverte, dans la limite de `lines_hit`.
        let span_end = file
            .uncovered
            .iter()
            .map(|r| r.end)
            .max()
            .unwrap_or(0)
            .max(file.lines_found);
        let clipped: Vec<LineRange> = normalize_ranges(
            &ranges
                .iter()
                .filter_map(|r| {
                    let end = r.end.min(span_end);
                    if r.start > end {
                        return None;
                    }
                    Some(LineRange {
                        start: r.start,
                        end,
                    })
                })
                .collect::<Vec<_>>(),
        );
        if clipped.is_empty() {
            continue;
        }
        let modified = count_lines(&clipped);
        let uncovered = intersect(&file.uncovered, &clipped);
        let uncovered_count = count_lines(&uncovered);
        let lines_hit = modified.saturating_sub(uncovered_count);
        files.push(FileCoverage {
            path: file.path.clone(),
            lines_found: modified,
            lines_hit,
            uncovered,
        });
    }
    CoverageReport { files }
}

/// Somme des lignes couvertes par des plages disjointes triées.
fn count_lines(ranges: &[LineRange]) -> u32 {
    ranges
        .iter()
        .map(|r| r.end.saturating_sub(r.start).saturating_add(1))
        .fold(0_u32, u32::saturating_add)
}

/// Intersection de deux jeux de plages triées disjointes → plages normalisées.
fn intersect(a: &[LineRange], b: &[LineRange]) -> Vec<LineRange> {
    let mut out: Vec<LineRange> = Vec::new();
    for left in a {
        for right in b {
            let start = left.start.max(right.start);
            let end = left.end.min(right.end);
            if start <= end {
                out.push(LineRange { start, end });
            }
        }
    }
    normalize_ranges(&out)
}

/// Ne garde que les issues dont `(path, line)` tombe dans une plage modifiée.
#[must_use]
pub fn restrict_issues(issues: &[Issue], changed: &ChangedLines) -> Vec<Issue> {
    issues
        .iter()
        .filter(|issue| {
            changed.get(&issue.path).is_some_and(|ranges| {
                ranges
                    .iter()
                    .any(|r| issue.line >= r.start && issue.line <= r.end)
            })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Severity;

    fn parse(text: &str) -> ChangedLines {
        parse_changed_lines(text).expect("diff attendu valide")
    }

    fn rng(start: u32, end: u32) -> LineRange {
        LineRange { start, end }
    }

    fn ranges(changed: &ChangedLines, path: &str) -> Vec<LineRange> {
        changed.get(path).cloned().unwrap_or_default()
    }

    fn issue_at(path: &str, line: u32) -> Issue {
        Issue {
            check_id: "c".to_string(),
            path: path.to_string(),
            line,
            column: 1,
            severity: Severity::Warning,
            code: None,
            message: "m".to_string(),
        }
    }

    /// Scenario `ajout_simple`.
    #[test]
    fn ajout_simple() {
        let changed = parse(
            "diff --git a/src/a.rs b/src/a.rs\n\
             index 1111111..2222222 100644\n\
             --- a/src/a.rs\n\
             +++ b/src/a.rs\n\
             @@ -0,0 +1,3 @@\n\
             +ligne 1\n+ligne 2\n+ligne 3\n",
        );
        assert_eq!(ranges(&changed, "src/a.rs"), vec![rng(1, 3)]);
        assert_eq!(changed.len(), 1, "un seul fichier attendu");
    }

    /// Scenario `modification_une_ligne` — compte omis vaut 1.
    #[test]
    fn modification_une_ligne() {
        let changed = parse("--- a/x.rs\n+++ b/x.rs\n@@ -10 +10 @@\n-old\n+new\n");
        assert_eq!(ranges(&changed, "x.rs"), vec![rng(10, 10)]);
    }

    /// Scenario `suppression_pure` — `+4,0` n'ajoute aucune ligne.
    #[test]
    fn suppression_pure() {
        let changed = parse("--- a/x.rs\n+++ b/x.rs\n@@ -5,2 +4,0 @@\n-g1\n-g2\n");
        assert!(
            changed.get("x.rs").is_none_or(Vec::is_empty),
            "aucune ligne ajoutée attendue, obtenu {changed:?}"
        );
    }

    /// Scenario `fichier_supprime` — `+++ /dev/null` est absent du résultat.
    #[test]
    fn fichier_supprime() {
        let changed =
            parse("diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n");
        assert!(
            changed.is_empty(),
            "fichier supprimé absent, obtenu {changed:?}"
        );
    }

    /// Scenario `plusieurs_hunks_fusionnes` — `+1,2` et `+3,1` fusionnent en `[1..3]`.
    #[test]
    fn plusieurs_hunks_fusionnes() {
        let changed =
            parse("--- a/f.rs\n+++ b/f.rs\n@@ -0,0 +1,2 @@\n+a\n+b\n@@ -9,2 +3,1 @@\n+c\n");
        assert_eq!(ranges(&changed, "f.rs"), vec![rng(1, 3)]);
    }

    /// Scénario `nom_cite_refuse` — git cite les chemins à caractères spéciaux ; refusé.
    #[test]
    fn nom_cite_refuse() {
        let err =
            parse_changed_lines("--- a/x\n+++ \"b/src/\\303\\251.rs\"\n@@ -1 +1 @@\n-a\n+b\n")
                .expect_err("nom entre guillemets doit être refusé");
        assert!(
            matches!(err, HealthError::InvalidDiff { .. }),
            "attendu InvalidDiff, obtenu {err:?}"
        );
    }

    /// Scenario `hunk_mal_forme` — nombre non numérique côté nouveau.
    #[test]
    fn hunk_mal_forme() {
        let err = parse_changed_lines("--- a/x\n+++ b/x\n@@ -1 +x @@\n-a\n+b\n")
            .expect_err("hunk mal formé doit être refusé");
        assert!(
            matches!(err, HealthError::InvalidDiff { .. }),
            "attendu InvalidDiff, obtenu {err:?}"
        );
    }

    /// Hors Scenario (clause `Raises`) : hunk avant tout `+++`.
    #[test]
    fn hunk_avant_tout_header() {
        let err = parse_changed_lines("diff --git a/x b/x\n@@ -1,1 +1,1 @@\n-a\n+b\n")
            .expect_err("hunk sans fichier cible doit être refusé");
        assert!(
            matches!(err, HealthError::InvalidDiff { .. }),
            "attendu InvalidDiff, obtenu {err:?}"
        );
    }

    /// Clause `Returns` : un fichier binaire est absent du résultat.
    #[test]
    fn fichier_binaire_absent() {
        let changed = parse(
            "diff --git a/logo.png b/logo.png\nindex 1111111..2222222\n\
             Binary files a/logo.png and b/logo.png differ\n",
        );
        assert!(changed.is_empty(), "binaire absent, obtenu {changed:?}");
    }

    /// Scenario `couverture_sur_le_diff` — rapport `src/a.rs` lignes 1..4 instrumentées
    /// (2..3 non couvertes), changed `[3..4]` → found 2, hit 1, uncovered [3..3].
    #[test]
    fn couverture_sur_le_diff() {
        let report = CoverageReport {
            files: vec![FileCoverage {
                path: "src/a.rs".to_string(),
                lines_found: 4,
                lines_hit: 2,
                uncovered: vec![rng(2, 3)],
            }],
        };
        let mut changed = ChangedLines::new();
        changed.insert("src/a.rs".to_string(), vec![rng(3, 4)]);
        let out = restrict_coverage(&report, &changed);
        assert_eq!(out.files.len(), 1);
        let file = &out.files[0];
        assert_eq!(file.path, "src/a.rs");
        assert_eq!(file.lines_found, 2);
        assert_eq!(file.lines_hit, 1);
        assert_eq!(file.uncovered, vec![rng(3, 3)]);
    }

    /// Scenario `fichier_non_modifie_exclu`.
    #[test]
    fn fichier_non_modifie_exclu() {
        let report = CoverageReport {
            files: vec![FileCoverage {
                path: "src/b.rs".to_string(),
                lines_found: 10,
                lines_hit: 10,
                uncovered: vec![],
            }],
        };
        let changed = ChangedLines::new();
        let out = restrict_coverage(&report, &changed);
        assert!(out.files.is_empty(), "obtenu {:?}", out.files);
    }

    /// Clause `Returns` : un fichier modifié sans aucune ligne modifiée instrumentée est omis
    /// (ici : instrumentées = 1..4 par hypothèse span, modifié = 100..101 disjoint).
    #[test]
    fn lignes_modifiees_hors_couverture_omises() {
        let report = CoverageReport {
            files: vec![FileCoverage {
                path: "a.rs".to_string(),
                lines_found: 4,
                lines_hit: 2,
                uncovered: vec![rng(2, 3)],
            }],
        };
        let mut changed = ChangedLines::new();
        changed.insert("a.rs".to_string(), vec![rng(100, 101)]);
        let out = restrict_coverage(&report, &changed);
        assert!(
            out.files.is_empty(),
            "rien à couvrir ⇒ omis, obtenu {:?}",
            out.files
        );
    }

    /// Scenario `issues_dans_le_diff_seulement`.
    #[test]
    fn issues_dans_le_diff_seulement() {
        let mut changed = ChangedLines::new();
        changed.insert("a.rs".to_string(), vec![rng(1, 5)]);
        let issues = vec![issue_at("a.rs", 3), issue_at("a.rs", 30)];
        let kept = restrict_issues(&issues, &changed);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].line, 3);
    }
}
