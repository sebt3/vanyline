//! Parseur pur du format `lcov` (couverture de lignes) vers [`CoverageReport`].

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::HealthError;
use crate::report::CoverageReport;
use crate::report::FileCoverage;
use crate::report::LineRange;

/// Parseur pur du format `lcov` (sortie de `cargo llvm-cov --lcov`, c8, coverage.py) vers un
/// [`CoverageReport`] normalisé. Seule la couverture de LIGNES est retenue (fonctions et
/// branches ignorées en v1).
///
/// * `input` — texte `lcov` (`SF:`/`DA:`/`end_of_record`, autres balises ignorées).
/// * `root` — racine du dépôt, pour relativiser les chemins absolus et exclure ce qui est
///   hors dépôt (dépendances, registre cargo).
/// * `prefix` — chemin relatif à `root` du répertoire où l'outil de couverture s'est exécuté
///   (`""` pour la racine) ; un `SF` RELATIF est interprété relativement à ce répertoire.
///
/// Les totaux sont RECALCULÉS depuis les `DA` (jamais depuis `LF`/`LH`). Un fichier cité dans
/// plusieurs enregistrements est fusionné (passages additionnés ligne à ligne). Un rapport
/// tronqué est rejeté, jamais rendu partiel. Une entrée vide donne un rapport vide.
///
/// # Errors
///
/// Retourne [`HealthError::InvalidLcov`] avec le numéro de ligne 1-indexé du texte pour :
/// `DA` avant tout `SF` ; `DA` mal formé (champ manquant, non numérique, ligne 0) ; `SF`
/// répété sans `end_of_record` intermédiaire ; enregistrement non terminé en fin d'entrée.
pub fn parse_lcov(input: &str, root: &Path, prefix: &str) -> Result<CoverageReport, HealthError> {
    let mut files: BTreeMap<String, BTreeMap<u32, u64>> = BTreeMap::new();
    // `None` = aucun enregistrement ouvert ; `Some(None)` = enregistrement ouvert sur un
    // fichier hors de `root` (consommé mais exclu du rapport).
    let mut open: Option<Option<String>> = None;
    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = idx.saturating_add(1);
        let line = raw_line.trim();
        if line == "end_of_record" {
            open = None;
            continue;
        }
        if let Some(raw_path) = line.strip_prefix("SF:") {
            if open.is_some() {
                return Err(invalid(line_no, "duplicate SF: without end_of_record"));
            }
            open = Some(relativize(raw_path.trim(), root, prefix));
            continue;
        }
        if let Some(rest) = line.strip_prefix("DA:") {
            let Some(current) = open.as_ref() else {
                return Err(invalid(line_no, "DA before any SF"));
            };
            let mut fields = rest.split(',');
            let number_field = fields
                .next()
                .filter(|field| !field.trim().is_empty())
                .ok_or_else(|| invalid(line_no, "DA: missing line number"))?;
            let hits_field = fields
                .next()
                .filter(|field| !field.trim().is_empty())
                .ok_or_else(|| invalid(line_no, "DA: missing hit count"))?;
            let number: u32 = number_field
                .trim()
                .parse()
                .map_err(|_| invalid(line_no, "DA: line number is not a number"))?;
            if number == 0 {
                return Err(invalid(line_no, "DA: line number 0"));
            }
            let hits: u64 = hits_field
                .trim()
                .parse()
                .map_err(|_| invalid(line_no, "DA: hit count is not a number"))?;
            if let Some(path) = current {
                let slot = files
                    .entry(path.clone())
                    .or_default()
                    .entry(number)
                    .or_insert(0_u64);
                *slot = slot.saturating_add(hits);
            }
        }
        // Autres balises (`TN`, `FN`, `FNDA`, `FNF`, `FNH`, `BRDA`, `BRF`, `BRH`, `LF`,
        // `LH`) et lignes inconnues : ignorées.
    }
    if open.is_some() {
        return Err(invalid(
            input.lines().count().max(1),
            "unterminated lcov record (truncated report)",
        ));
    }
    let mut out_files = Vec::with_capacity(files.len());
    for (path, lines) in files {
        let hit_count = lines.values().filter(|&&hits| hits != 0).count();
        let mut uncovered: Vec<LineRange> = Vec::new();
        for (&number, &hits) in &lines {
            if hits != 0 {
                continue;
            }
            match uncovered.last_mut() {
                Some(last) if last.end.checked_add(1) == Some(number) => last.end = number,
                _ => uncovered.push(LineRange {
                    start: number,
                    end: number,
                }),
            }
        }
        out_files.push(FileCoverage {
            path,
            // Un fichier à 4 milliards de lignes instrumentées n'existe pas ; saturation
            // pour respecter `arithmetic_side_effects` sans cas pratique.
            lines_found: u32::try_from(lines.len()).unwrap_or(u32::MAX),
            lines_hit: u32::try_from(hit_count).unwrap_or(u32::MAX),
            uncovered,
        });
    }
    Ok(CoverageReport { files: out_files })
}

fn invalid(line: usize, reason: &str) -> HealthError {
    HealthError::InvalidLcov {
        line,
        reason: reason.to_string(),
    }
}

/// Rend un chemin `SF:` relatif à `root`, séparateur `/`. Un chemin absolu doit être sous
/// `root` (sinon `None` : hors dépôt, exclu). Un chemin relatif est joint à `prefix` puis
/// normalisé (`./` retiré, `..` résolus). `None` si la normalisation sort de la racine.
fn relativize(raw: &str, root: &Path, prefix: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let joined = if raw.starts_with('/') {
        Path::new(raw)
            .strip_prefix(root)
            .ok()?
            .to_str()?
            .to_string()
    } else if prefix.is_empty() {
        raw.to_string()
    } else {
        format!("{prefix}/{raw}")
    };
    normalize(&joined)
}

fn normalize(joined: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::parse_lcov;
    use crate::error::HealthError;
    use std::path::Path;

    const ROOT: &str = "/repo";

    fn parse(input: &str, prefix: &str) -> Result<crate::report::CoverageReport, HealthError> {
        parse_lcov(input, Path::new(ROOT), prefix)
    }

    /// Scenario `fichier_simple`.
    #[test]
    fn fichier_simple() {
        let input = "SF:/repo/src/a.rs\nDA:1,1\nDA:2,0\nDA:3,0\nDA:4,5\nend_of_record\n";
        let report = parse(input, "").expect("lcov valide");
        assert_eq!(report.files.len(), 1);
        let file = &report.files[0];
        assert_eq!(file.path, "src/a.rs");
        assert_eq!(file.lines_found, 4);
        assert_eq!(file.lines_hit, 2);
        assert_eq!(
            file.uncovered,
            vec![crate::report::LineRange { start: 2, end: 3 }]
        );
    }

    /// Scenario `lf_lh_ignores` — les totaux reflètent les `DA`, jamais `LF`/`LH`.
    #[test]
    fn lf_lh_ignores() {
        let input = "SF:/repo/src/a.rs\nDA:1,1\nDA:2,0\nLF:99\nLH:99\nend_of_record\n";
        let report = parse(input, "").expect("lcov valide");
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].lines_found, 2);
        assert_eq!(report.files[0].lines_hit, 1);
    }

    /// Scenario `fusion_de_deux_enregistrements`.
    #[test]
    fn fusion_de_deux_enregistrements() {
        let input =
            "SF:/repo/src/a.rs\nDA:1,0\nend_of_record\nSF:/repo/src/a.rs\nDA:1,2\nend_of_record\n";
        let report = parse(input, "").expect("lcov valide");
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].lines_found, 1);
        assert_eq!(report.files[0].lines_hit, 1);
        assert_eq!(report.files[0].uncovered, Vec::new());
    }

    /// Scenario `fichier_hors_racine_exclu`.
    #[test]
    fn fichier_hors_racine_exclu() {
        let input = "SF:/home/u/.cargo/registry/x/lib.rs\nDA:1,1\nend_of_record\n";
        let report = parse(input, "").expect("lcov valide");
        assert_eq!(report.files, Vec::new());
    }

    /// Scenario `da_avant_sf`.
    #[test]
    fn da_avant_sf() {
        let err = parse("DA:1,1\n", "").expect_err("DA avant SF invalide");
        match err {
            HealthError::InvalidLcov { line, .. } => assert_eq!(line, 1),
            other => panic!("attendu InvalidLcov, obtenu {other:?}"),
        }
    }

    /// Scenario `da_mal_forme`.
    #[test]
    fn da_mal_forme() {
        let input = "SF:/repo/a.rs\nDA:abc,1\nend_of_record\n";
        let err = parse(input, "").expect_err("DA non numérique invalide");
        match err {
            HealthError::InvalidLcov { line, .. } => assert_eq!(line, 2),
            other => panic!("attendu InvalidLcov, obtenu {other:?}"),
        }
    }

    /// Scenario `rapport_tronque` — jamais un rapport partiel.
    #[test]
    fn rapport_tronque() {
        let input = "SF:/repo/src/a.rs\nDA:1,1\n";
        let err = parse(input, "").expect_err("enregistrement non terminé invalide");
        assert!(
            matches!(err, HealthError::InvalidLcov { .. }),
            "attendu InvalidLcov, obtenu {err:?}"
        );
    }

    /// Scenario `entree_vide` — `Ok` avec `files` vide (c'est l'appelant qui décide du
    /// statut).
    #[test]
    fn entree_vide() {
        let report = parse("", "").expect("entrée vide = rapport vide");
        assert_eq!(report.files, Vec::new());
    }

    /// Scenario `tri_deterministe`.
    #[test]
    fn tri_deterministe() {
        let input = "SF:b.rs\nDA:1,1\nend_of_record\nSF:a.rs\nDA:1,1\nend_of_record\n";
        let report = parse(input, "").expect("lcov valide");
        let paths: Vec<&str> = report.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.rs", "b.rs"]);
    }

    /// Scenario `chemin_relatif_avec_prefixe`.
    #[test]
    fn chemin_relatif_avec_prefixe() {
        let input = "SF:src/a.ts\nDA:1,1\nend_of_record\n";
        let report = parse(input, "frontend").expect("lcov valide");
        assert_eq!(report.files[0].path, "frontend/src/a.ts");
    }

    /// Scenario `chemin_relatif_sans_prefixe`.
    #[test]
    fn chemin_relatif_sans_prefixe() {
        let input = "SF:src/a.rs\nDA:1,1\nend_of_record\n";
        let report = parse(input, "").expect("lcov valide");
        assert_eq!(report.files[0].path, "src/a.rs");
    }

    /// Test additionnel couvrant `Raises` (hors Scenario) : `SF` répété sans
    /// `end_of_record` intermédiaire.
    #[test]
    fn sf_repete_sans_end_of_record() {
        let input = "SF:/repo/a.rs\nDA:1,1\nSF:/repo/b.rs\n";
        let err = parse(input, "").expect_err("SF répété invalide");
        match err {
            HealthError::InvalidLcov { line, .. } => assert_eq!(line, 3),
            other => panic!("attendu InvalidLcov, obtenu {other:?}"),
        }
    }
}
