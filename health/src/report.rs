//! Modèle sérialisable unique du rapport de santé — type Rust exporté en TypeScript
//! (ts-rs) vers `@vanyline/protocol`. Types purs, sans I/O ; chemins toujours relatifs à la
//! racine du dépôt, séparateur `/`.

use serde::Deserialize;
use serde::Serialize;

/// Plage de lignes inclusive, base 1 (`start <= end`), dans un fichier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct LineRange {
    /// Première ligne de la plage (base 1).
    pub start: u32,
    /// Dernière ligne de la plage, inclusive (base 1).
    pub end: u32,
}

/// Nature d'une vérification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// Formatage (`fmt`, `prettier`…).
    Format,
    /// Analyse statique (`clippy`, `tsc`, `ruff`, `hadolint`).
    Lint,
    /// Tests.
    Tests,
    /// Couverture de lignes.
    Coverage,
    /// Vérification restreinte au code nouveau.
    NewCode,
}

/// État d'une vérification ou d'un rapport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// Code de sortie 0.
    Passed,
    /// Code de sortie ≠ 0 : la vérification a parlé.
    Failed,
    /// N'a pas pu s'exécuter ou sa sortie est inexploitable.
    Errored,
    /// Rien à vérifier (par ex. aucun Dockerfile suivi) ; n'altère pas `status()`.
    Skipped,
}

/// Degré de gravité d'un diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Erreur.
    Error,
    /// Avertissement.
    Warning,
}

/// Résultat d'une vérification exécutée (ou tentée).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct CheckResult {
    /// Identifiant de la vérification (ex. `clippy`, `test`).
    pub id: String,
    /// Nature de la vérification.
    pub kind: CheckKind,
    /// État atteint.
    pub status: CheckStatus,
    /// Code de sortie ; absent si non démarrée ou tuée par signal.
    pub exit_code: Option<i32>,
    /// Durée d'exécution en millisecondes.
    pub duration_ms: u64,
    /// Fin de la sortie, bornée par le `runner`.
    pub output_tail: String,
}

/// Diagnostic normalisé (lint, erreur de type…), chemin relatif à la racine du dépôt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Issue {
    /// Identifiant de la vérification émettrice.
    pub check_id: String,
    /// Chemin relatif à la racine du dépôt, séparateur `/`.
    pub path: String,
    /// Ligne (base 1).
    pub line: u32,
    /// Colonne (base 1).
    pub column: u32,
    /// Gravité rapportée par l'outil (jamais reclassée).
    pub severity: Severity,
    /// Code de règle (ex. `clippy::unwrap_used`, `TS2322`, `F401`) si l'outil en émet un.
    pub code: Option<String>,
    /// Message du diagnostic.
    pub message: String,
}

/// Couverture de lignes d'un fichier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct FileCoverage {
    /// Chemin relatif à la racine du dépôt, séparateur `/`.
    pub path: String,
    /// Lignes instrumentées.
    pub lines_found: u32,
    /// Lignes instrumentées atteintes au moins une fois.
    pub lines_hit: u32,
    /// Plages de lignes instrumentées à 0 passage, triées, disjointes, fusionnées si
    /// contiguës.
    pub uncovered: Vec<LineRange>,
}

/// Totaux de couverture (somme des fichiers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct CoverageTotals {
    /// Lignes instrumentées.
    pub lines_found: u32,
    /// Lignes instrumentées atteintes.
    pub lines_hit: u32,
}

impl CoverageTotals {
    /// Pourcentage de lignes couvertes, dans `[0, 100]` ; `100.0` quand `lines_found == 0`
    /// (rien à couvrir n'est pas un défaut).
    #[must_use]
    pub fn percent(&self) -> f64 {
        if self.lines_found == 0 {
            return 100.0;
        }
        f64::from(self.lines_hit) / f64::from(self.lines_found) * 100.0
    }
}

/// Rapport de couverture de lignes, fichiers triés par `path`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct CoverageReport {
    /// Fichiers instrumentés, triés par `path`.
    pub files: Vec<FileCoverage>,
}

impl CoverageReport {
    /// Somme des totaux des fichiers.
    #[must_use]
    pub fn totals(&self) -> CoverageTotals {
        let mut totals = CoverageTotals {
            lines_found: 0,
            lines_hit: 0,
        };
        for file in &self.files {
            totals.lines_found = totals.lines_found.saturating_add(file.lines_found);
            totals.lines_hit = totals.lines_hit.saturating_add(file.lines_hit);
        }
        totals
    }
}

/// Rapport restreint aux lignes modifiées par rapport à une base (vue du relecteur de MR).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct NewCode {
    /// Base de comparaison (ref git).
    pub base: String,
    /// Couverture restreinte aux lignes modifiées.
    pub coverage: CoverageReport,
    /// Diagnostics situés sur les lignes modifiées.
    pub issues: Vec<Issue>,
}

/// Rapport de santé complet d'un dépôt à une révision donnée.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct HealthReport {
    /// Révision git analysée, si connue.
    pub revision: Option<String>,
    /// Vérifications exécutées.
    pub checks: Vec<CheckResult>,
    /// Couverture, absente si aucune vérification de couverture n'a produit de données
    /// (« pas de mesure » ≠ « 0 % »).
    pub coverage: Option<CoverageReport>,
    /// Diagnostics agrégés.
    pub issues: Vec<Issue>,
    /// Vue restreinte aux lignes modifiées, si calculée.
    pub new_code: Option<NewCode>,
    /// Langages détectés sans aucune vérification (aucun preset) : information pour le
    /// relecteur, sans effet sur [`HealthReport::status`] mais jamais omise.
    pub unchecked_languages: Vec<String>,
}

impl HealthReport {
    /// Seule définition du « vert » : `failed` si au moins une vérification est `failed`,
    /// sinon `errored` si au moins une est `errored`, sinon `passed`. Un rapport sans
    /// aucune vérification est `passed`. `skipped` et `unchecked_languages` n'ont aucun
    /// effet.
    #[must_use]
    pub fn status(&self) -> CheckStatus {
        let has = |wanted: CheckStatus| self.checks.iter().any(|c| c.status == wanted);
        if has(CheckStatus::Failed) {
            CheckStatus::Failed
        } else if has(CheckStatus::Errored) {
            CheckStatus::Errored
        } else {
            CheckStatus::Passed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &str, status: CheckStatus) -> CheckResult {
        let exit_code = Some(i32::from(status != CheckStatus::Passed));
        CheckResult {
            id: id.to_string(),
            kind: CheckKind::Lint,
            status,
            exit_code,
            duration_ms: 10,
            output_tail: String::new(),
        }
    }

    fn full_report() -> HealthReport {
        HealthReport {
            revision: Some("abc123".to_string()),
            checks: vec![check("clippy", CheckStatus::Passed)],
            coverage: Some(CoverageReport {
                files: vec![FileCoverage {
                    path: "src/a.rs".to_string(),
                    lines_found: 4,
                    lines_hit: 2,
                    uncovered: vec![LineRange { start: 2, end: 3 }],
                }],
            }),
            issues: vec![Issue {
                check_id: "clippy".to_string(),
                path: "src/a.rs".to_string(),
                line: 12,
                column: 5,
                severity: Severity::Warning,
                code: Some("clippy::unwrap_used".to_string()),
                message: "used `unwrap()`".to_string(),
            }],
            new_code: Some(NewCode {
                base: "origin/main".to_string(),
                coverage: CoverageReport { files: vec![] },
                issues: vec![],
            }),
            unchecked_languages: vec!["go".to_string()],
        }
    }

    /// Scenario `status_failed_prime_sur_errored`.
    #[test]
    fn status_failed_prime_sur_errored() {
        let report = HealthReport {
            revision: None,
            checks: vec![
                check("cov", CheckStatus::Errored),
                check("clippy", CheckStatus::Failed),
            ],
            coverage: None,
            issues: vec![],
            new_code: None,
            unchecked_languages: vec![],
        };
        assert_eq!(report.status(), CheckStatus::Failed);
    }

    /// Scenario `status_errored_sans_failed`.
    #[test]
    fn status_errored_sans_failed() {
        let report = HealthReport {
            revision: None,
            checks: vec![
                check("fmt", CheckStatus::Passed),
                check("cov", CheckStatus::Errored),
            ],
            coverage: None,
            issues: vec![],
            new_code: None,
            unchecked_languages: vec![],
        };
        assert_eq!(report.status(), CheckStatus::Errored);
    }

    /// Scenario `langage_non_verifie_visible_sans_changer_le_statut`.
    #[test]
    fn langage_non_verifie_visible_sans_changer_le_statut() {
        let report = HealthReport {
            revision: None,
            checks: vec![check("clippy", CheckStatus::Passed)],
            coverage: None,
            issues: vec![],
            new_code: None,
            unchecked_languages: vec!["go".to_string()],
        };
        assert_eq!(report.status(), CheckStatus::Passed);
        let json = serde_json::to_string(&report).expect("sérialisable");
        assert!(
            json.contains(r#""unchecked_languages":["go"]"#),
            "JSON obtenu : {json}"
        );
    }

    /// Scenario `rapport_vide_est_passed`.
    #[test]
    fn rapport_vide_est_passed() {
        let report = HealthReport {
            revision: None,
            checks: vec![],
            coverage: None,
            issues: vec![],
            new_code: None,
            unchecked_languages: vec![],
        };
        assert_eq!(report.status(), CheckStatus::Passed);
    }

    /// Scenario `percent_sans_ligne_instrumentee`.
    #[test]
    fn percent_sans_ligne_instrumentee() {
        let totals = CoverageTotals {
            lines_found: 0,
            lines_hit: 0,
        };
        assert_eq!(totals.percent(), 100.0);
    }

    /// Scenario `percent_normal`.
    #[test]
    fn percent_normal() {
        let totals = CoverageTotals {
            lines_found: 8,
            lines_hit: 6,
        };
        assert_eq!(totals.percent(), 75.0);
    }

    /// Scenario `roundtrip_json_stable`.
    #[test]
    fn roundtrip_json_stable() {
        let report = full_report();
        let json = serde_json::to_string(&report).expect("sérialisable");
        let back: HealthReport = serde_json::from_str(&json).expect("désérialisable");
        assert_eq!(back, report);
        // Les enums apparaissent en snake_case.
        assert!(json.contains(r#""kind":"lint""#), "JSON obtenu : {json}");
        assert!(
            json.contains(r#""status":"passed""#),
            "JSON obtenu : {json}"
        );
        assert!(
            json.contains(r#""severity":"warning""#),
            "JSON obtenu : {json}"
        );
        assert!(json.contains(r#""new_code""#), "JSON obtenu : {json}");
    }

    /// Scenario `export_typescript` — sous la feature `ts-rs`, génère
    /// `packages/protocol/src/generated/health-report.ts` (fichier commité ; la CI vérifie
    /// `git diff --exit-code`). `TS_RS_LARGE_INT = "number"` est posé dans
    /// `.cargo/config.toml` (racine) — lisible par `Config::from_env()`.
    #[cfg(feature = "ts-rs")]
    #[test]
    fn export_typescript() {
        use ts_rs::Config;
        use ts_rs::TS;

        let cfg = Config::from_env();
        let body = [
            <LineRange as TS>::export_to_string(&cfg).expect("LineRange exportable"),
            <CheckKind as TS>::export_to_string(&cfg).expect("CheckKind exportable"),
            <CheckStatus as TS>::export_to_string(&cfg).expect("CheckStatus exportable"),
            <CheckResult as TS>::export_to_string(&cfg).expect("CheckResult exportable"),
            <Severity as TS>::export_to_string(&cfg).expect("Severity exportable"),
            <Issue as TS>::export_to_string(&cfg).expect("Issue exportable"),
            <FileCoverage as TS>::export_to_string(&cfg).expect("FileCoverage exportable"),
            <CoverageTotals as TS>::export_to_string(&cfg).expect("CoverageTotals exportable"),
            <CoverageReport as TS>::export_to_string(&cfg).expect("CoverageReport exportable"),
            <NewCode as TS>::export_to_string(&cfg).expect("NewCode exportable"),
            <HealthReport as TS>::export_to_string(&cfg).expect("HealthReport exportable"),
        ]
        .map(|chunk| strip_import_lines(&chunk))
        .join("\n");
        let out = format!(
            "// Generated by vanyline-health (ts-rs) from the health report model.\n\
             // DO NOT EDIT — regenerate: cargo test -p vanyline-health --features ts-rs\n\n\
             {body}\n"
        );
        let target = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packages/protocol/src/generated/health-report.ts"
        );
        std::fs::write(target, out).expect("écriture de health-report.ts");
        assert!(std::path::Path::new(target).exists());

        // (a) Auto-suffisance : aucune ligne `import` (regex `^\s*import\b`).
        let written = std::fs::read_to_string(target).expect("lecture de health-report.ts");
        for line in written.lines() {
            assert!(
                !starts_with_import(line),
                "ligne `import` pendante dans le fichier généré : {line}"
            );
        }
        // (b) Aucun identifiant déclaré deux fois : chaque type est déclaré exactement une fois.
        for name in [
            "LineRange",
            "CheckKind",
            "CheckStatus",
            "CheckResult",
            "Severity",
            "Issue",
            "FileCoverage",
            "CoverageTotals",
            "CoverageReport",
            "NewCode",
            "HealthReport",
        ] {
            let needle = format!("export type {name}");
            let count = written.match_indices(&needle).count();
            assert_eq!(
                count, 1,
                "{name} déclaré {count} fois dans le fichier généré (1 attendu)"
            );
        }
    }

    /// Retire les lignes `import` d'un `export_to_string` de ts-rs : les onze types
    /// sont concaténés dans un fichier unique auto-suffisant, les imports de fichiers
    /// frères émis par ts-rs seraient des références cassées (TS2307) et des doublons
    /// de déclaration.
    #[cfg(feature = "ts-rs")]
    fn strip_import_lines(chunk: &str) -> String {
        chunk
            .lines()
            .filter(|line| !starts_with_import(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Ligne débutant par `import` (regex `^\s*import\b`) — motif des imports
    /// frères émis par ts-rs, intolérables dans un fichier unique auto-suffisant.
    #[cfg(feature = "ts-rs")]
    fn starts_with_import(line: &str) -> bool {
        match line.trim_start().strip_prefix("import") {
            Some(rest) => rest.starts_with([' ', '\t', '{']),
            None => false,
        }
    }
}
