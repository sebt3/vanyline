//! Presets de vérifications par langage : la liste des INVOCATIONS (argv) que lancent la
//! sandbox, le cli, l'agent `validator` et le service central. La connaissance « comment
//! vérifier un projet Rust » vit ici ; les règles (lints, tests) viennent du projet analysé.
//! Pur : aucun accès au dépôt analysé, jamais de chaîne shell.

use crate::error::HealthError;
use crate::report::CheckKind;

/// Comment la sortie d'une vérification est interprétée.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputKind {
    /// Sortie non structurée (le verdict est le code de sortie).
    None,
    /// Fichier `lcov` écrit par la commande, chemin relatif à `out_dir`, jetons admis.
    Lcov {
        /// Chemin relatif au répertoire de sortie du run.
        path: String,
    },
    /// Diagnostics JSON sur stdout, parsés par `crate::cargo_messages`.
    CargoMessages,
    /// Sortie texte sur stdout, parsée par `crate::tsc_text`.
    TscText,
    /// Sortie JSON sur stdout, parsée par `crate::ruff_json`.
    RuffJson,
    /// Sortie JSON sur stdout, parsée par `crate::hadolint_json`.
    HadolintJson,
}

/// Comment le runner multiplie UNE déclaration en plusieurs vérifications avant exécution.
#[derive(Debug, Clone, PartialEq)]
pub enum Expand {
    /// Une vérification par entrée du champ `workspaces` du `package.json` situé à `cwd` ;
    /// jetons `{workspace}` (chemin du workspace) et `{workspace_slug}` (ce chemin avec `/`
    /// remplacé par `-`).
    NpmWorkspaces,
    /// Une seule vérification dont l'élément d'argv exactement égal à `{files}` est remplacé
    /// par la liste des fichiers SUIVIS par git correspondant aux globs ; aucun fichier ⇒
    /// la vérification est `skipped`.
    Files {
        /// Globs appliqués à la sortie de `git ls-files`.
        globs: Vec<String>,
    },
}

/// Déclaration d'une vérification : une commande (argv, jamais de shell) et son sort.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// Identifiant stable (clé des rapports stockés). Après expansion `NpmWorkspaces`,
    /// devient `<id>:<workspace>`.
    pub id: String,
    /// Nature de la vérification.
    pub kind: CheckKind,
    /// Commande complète ; `argv[0]` est le programme.
    pub argv: Vec<String>,
    /// Répertoire d'exécution, relatif à la racine du dépôt (`""` = racine).
    pub cwd: String,
    /// Interprétation de la sortie.
    pub output: OutputKind,
    /// Mode de multiplication avant exécution.
    pub expand: Option<Expand>,
    /// Deux vérifications du même groupe ne s'exécutent jamais ensemble (absent ⇒ groupe
    /// implicite `argv[0]`+`cwd`).
    pub serial_group: Option<String>,
    /// Id d'une vérification à terminer d'abord.
    pub after: Option<String>,
}

/// Sous-chaîne d'un élément d'argv (ou d'un chemin `Lcov`) remplacée par le runner par le
/// répertoire de sortie du run.
pub const OUT_DIR_TOKEN: &str = "{out_dir}";

/// Preset d'UN langage (`rust`, `js-ts`, `vue`, `python`, `dockerfile`) ; `[]` si inconnu.
/// Construit une déclaration de vérification (cwd racine, jamais de shell).
fn check(
    id: &str,
    kind: CheckKind,
    argv: &[&str],
    output: OutputKind,
    expand: Option<Expand>,
    serial_group: Option<&str>,
    after: Option<&str>,
) -> Check {
    Check {
        id: id.to_string(),
        kind,
        argv: argv.iter().map(|a| (*a).to_string()).collect(),
        cwd: String::new(),
        output,
        expand,
        serial_group: serial_group.map(str::to_string),
        after: after.map(str::to_string),
    }
}

/// Les langages reconnus, dans l'ordre fixe de concaténation ; `js-ts` et `vue` partagent
/// un même preset (un seul jeu si les deux sont détectés).
const PRESET_ORDER: [&str; 4] = ["rust", "js-ts", "python", "dockerfile"];

/// Preset d'UN langage (`rust`, `js-ts`, `vue`, `python`, `dockerfile`) ; `[]` si inconnu.
#[must_use]
pub fn preset_checks(language: &str) -> Vec<Check> {
    match language {
        "rust" => rust_preset(),
        "js-ts" | "vue" => ts_preset(),
        "python" => python_preset(),
        "dockerfile" => dockerfile_preset(),
        _ => Vec::new(),
    }
}

/// Preset Rust : fmt, clippy (mêmes drapeaux que la CI), couverture llvm-cov — trois
/// groupes de build distincts, donc parallélisables.
fn rust_preset() -> Vec<Check> {
    vec![
        check(
            "rust-fmt",
            CheckKind::Format,
            &["cargo", "fmt", "--all", "--", "--check"],
            OutputKind::None,
            None,
            Some("fmt"),
            None,
        ),
        check(
            "rust-clippy",
            CheckKind::Lint,
            &[
                "cargo",
                "clippy",
                "--workspace",
                "--all-targets",
                "--message-format=json",
                "--",
                "-D",
                "warnings",
            ],
            OutputKind::CargoMessages,
            None,
            Some("cargo-debug"),
            None,
        ),
        check(
            "rust-coverage",
            CheckKind::Coverage,
            &[
                "cargo",
                "llvm-cov",
                "--workspace",
                "--lcov",
                "--output-path",
                "{out_dir}/rust.lcov",
            ],
            OutputKind::Lcov {
                path: "rust.lcov".to_string(),
            },
            None,
            Some("cargo-llvm-cov"),
            None,
        ),
    ]
}

/// Preset JS/TS (aussi `vue`) : une vérification par workspace npm ; le fournisseur de
/// couverture s'installe `--no-save` avant la mesure.
fn ts_preset() -> Vec<Check> {
    let npm = Some(Expand::NpmWorkspaces);
    vec![
        check(
            "ts-check",
            CheckKind::Lint,
            &[
                "npm",
                "run",
                "check",
                "--workspace={workspace}",
                "--if-present",
            ],
            OutputKind::TscText,
            npm.clone(),
            None,
            None,
        ),
        check(
            "ts-coverage-provider",
            CheckKind::Tests,
            &[
                "npm",
                "install",
                "--no-save",
                "--no-audit",
                "--no-fund",
                "@vitest/coverage-v8@{vitest_version}",
            ],
            OutputKind::None,
            npm.clone(),
            None,
            None,
        ),
        check(
            "ts-coverage",
            CheckKind::Coverage,
            &[
                "npm",
                "run",
                "test",
                "--workspace={workspace}",
                "--if-present",
                "--",
                "--coverage",
                "--coverage.reporter=lcov",
                "--coverage.reportsDirectory={out_dir}/ts-cov/{workspace_slug}",
            ],
            OutputKind::Lcov {
                path: "ts-cov/{workspace_slug}/lcov.info".to_string(),
            },
            npm,
            None,
            Some("ts-coverage-provider"),
        ),
    ]
}

/// Preset Python : ruff, tests sous coverage ; la couverture consomme les données des
/// tests (même run, même cwd).
fn python_preset() -> Vec<Check> {
    vec![
        check(
            "py-ruff",
            CheckKind::Lint,
            &["ruff", "check", "--output-format", "json", "."],
            OutputKind::RuffJson,
            None,
            None,
            None,
        ),
        check(
            "py-tests",
            CheckKind::Tests,
            &["coverage", "run", "-m", "pytest"],
            OutputKind::None,
            None,
            None,
            None,
        ),
        check(
            "py-coverage",
            CheckKind::Coverage,
            &["coverage", "lcov", "-o", "{out_dir}/py.lcov"],
            OutputKind::Lcov {
                path: "py.lcov".to_string(),
            },
            None,
            None,
            Some("py-tests"),
        ),
    ]
}

/// Preset Dockerfile : hadolint sur les fichiers suivis par git.
fn dockerfile_preset() -> Vec<Check> {
    vec![check(
        "docker-hadolint",
        CheckKind::Lint,
        &["hadolint", "--format", "json", "{files}"],
        OutputKind::HadolintJson,
        Some(Expand::Files {
            globs: [
                "Dockerfile",
                "**/Dockerfile",
                "**/Dockerfile.*",
                "**/*.Dockerfile",
                "**/Containerfile",
            ]
            .iter()
            .map(|g| (*g).to_string())
            .collect(),
        }),
        None,
        None,
    )]
}

/// Concaténation des presets dans l'ordre fixe `rust`, `js-ts`/`vue`, `python`,
/// `dockerfile`, sans doublon d'id.
#[must_use]
pub fn checks_for_languages(languages: &[String]) -> Vec<Check> {
    let wanted: std::collections::BTreeSet<&str> = languages.iter().map(String::as_str).collect();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut checks = Vec::new();
    for preset in PRESET_ORDER {
        let hit = wanted.contains(preset) || (preset == "js-ts" && wanted.contains("vue"));
        if !hit {
            continue;
        }
        for check in preset_checks(preset) {
            if seen.insert(check.id.clone()) {
                checks.push(check);
            }
        }
    }
    checks
}

/// Les langages reçus pour lesquels [`preset_checks`] rend `[]` : langages « non vérifiés »,
/// jamais passés sous silence.
#[must_use]
pub fn unchecked_languages(languages: &[String]) -> Vec<String> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    languages
        .iter()
        .filter(|language| preset_checks(language).is_empty())
        .filter(|language| seen.insert(language.as_str()))
        .cloned()
        .collect()
}

/// Vérifie la cohérence d'une déclaration : `id` non vide, `argv` non vide, `argv[0]` non
/// vide, `cwd` relatif sans composant `..`.
///
/// # Errors
///
/// [`HealthError::InvalidCheck`] au premier manquement.
pub fn validate(check: &Check) -> Result<(), HealthError> {
    let reject = |reason: &str| {
        Err(HealthError::InvalidCheck {
            id: check.id.clone(),
            reason: reason.to_string(),
        })
    };
    if check.id.is_empty() {
        return reject("empty id");
    }
    let Some(first) = check.argv.first() else {
        return reject("empty argv");
    };
    if first.is_empty() {
        return reject("empty argv[0]");
    }
    if check.cwd.starts_with('/') || check.cwd.split('/').any(|c| c == "..") {
        return reject(&format!("cwd escapes the repository: {}", check.cwd));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::CheckKind;

    fn langs(list: &[&str]) -> Vec<String> {
        list.iter().map(|l| (*l).to_string()).collect()
    }

    fn ids(checks: &[Check]) -> Vec<&str> {
        checks.iter().map(|c| c.id.as_str()).collect()
    }

    /// Scenario `preset_rust_complet` (+ clause Returns : argv et groupes exacts).
    #[test]
    fn preset_rust_complet() {
        let checks = preset_checks("rust");
        assert_eq!(
            ids(&checks),
            vec!["rust-fmt", "rust-clippy", "rust-coverage"]
        );
        assert_eq!(
            checks.iter().map(|c| &c.kind).collect::<Vec<_>>(),
            vec![&CheckKind::Format, &CheckKind::Lint, &CheckKind::Coverage]
        );
        assert_eq!(checks[0].argv, ["cargo", "fmt", "--all", "--", "--check"]);
        assert_eq!(
            checks[1].argv,
            [
                "cargo",
                "clippy",
                "--workspace",
                "--all-targets",
                "--message-format=json",
                "--",
                "-D",
                "warnings"
            ]
        );
        assert_eq!(checks[1].output, OutputKind::CargoMessages);
        assert_eq!(
            checks[2].argv,
            [
                "cargo",
                "llvm-cov",
                "--workspace",
                "--lcov",
                "--output-path",
                "{out_dir}/rust.lcov"
            ]
        );
        assert_eq!(
            checks[2].output,
            OutputKind::Lcov {
                path: "rust.lcov".to_string()
            }
        );
        assert_eq!(
            checks
                .iter()
                .map(|c| c.serial_group.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("fmt"), Some("cargo-debug"), Some("cargo-llvm-cov")]
        );
    }

    /// Scenario `clippy_memes_drapeaux_que_la_ci` — mêmes drapeaux que
    /// `.github/workflows/test.yml` (leçon F2 : dérive local/CI).
    #[test]
    fn clippy_memes_drapeaux_que_la_ci() {
        let yaml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../.github/workflows/test.yml"
        ))
        .expect("test.yml lisible");
        assert!(
            yaml.contains("cargo clippy --workspace --all-targets") && yaml.contains("-D warnings"),
            "la CI doit porter les drapeaux attendus, lu : {yaml}"
        );
        let clippy = preset_checks("rust")
            .into_iter()
            .find(|c| c.id == "rust-clippy")
            .expect("preset rust-clippy");
        let argv = clippy.argv.join(" ");
        for flag in ["--workspace", "--all-targets", "-D", "warnings"] {
            assert!(
                argv.split(' ').any(|a| a == flag),
                "drapeau {flag} manquant : {argv}"
            );
        }
    }

    /// Extrait d'un texte YAML de CI l'argv de la commande commençant par `prefix`
    /// (ex. `"cargo fmt "`), sous forme de tokens.
    fn ci_argv(yaml: &str, prefix: &str) -> Vec<String> {
        yaml.lines()
            .find_map(|line| line.find(prefix).map(|i| line[i..].trim().to_string()))
            .unwrap_or_else(|| panic!("step « {prefix}… » absent de test.yml"))
            .split_whitespace()
            .map(String::from)
            .collect()
    }

    /// Normalise un argv pour comparaison preset↔CI : retire `--message-format=json`
    /// (ajout légitime du preset pour la sortie structurée) et le séparateur `--`
    /// (la CI écrit `cargo fmt --all --check`, le preset `cargo fmt --all -- --check`).
    fn normalize(argv: &[String]) -> Vec<String> {
        argv.iter()
            .filter(|a| a.as_str() != "--" && !a.starts_with("--message-format"))
            .cloned()
            .collect()
    }

    /// Task « Test de non-dérive » (checks.sdd) — verrou : `rust-fmt` est comparé
    /// bidirectionnellement au pas fmt de `.github/workflows/test.yml`, comme clippy.
    #[test]
    fn fmt_memes_drapeaux_que_la_ci() {
        let yaml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../.github/workflows/test.yml"
        ))
        .expect("test.yml lisible");
        let fmt = preset_checks("rust")
            .into_iter()
            .find(|c| c.id == "rust-fmt")
            .expect("preset rust-fmt");
        let preset = normalize(&fmt.argv);
        let ci = normalize(&ci_argv(&yaml, "cargo fmt "));
        assert_eq!(
            preset, ci,
            "rust-fmt doit retrouver le pas fmt de la CI (hors séparateur --)"
        );
    }

    /// Scenario `langage_inconnu`.
    #[test]
    fn langage_inconnu() {
        assert!(
            preset_checks("cobol").is_empty(),
            "langage inconnu sans preset"
        );
    }

    /// Scenario `ordre_et_unicite` + ordre fixe de `checks_for_languages`.
    #[test]
    fn ordre_et_unicite() {
        let checks = checks_for_languages(&langs(&["rust", "rust"]));
        assert_eq!(
            ids(&checks),
            vec!["rust-fmt", "rust-clippy", "rust-coverage"]
        );
        let checks = checks_for_languages(&langs(&["dockerfile", "rust"]));
        assert_eq!(
            ids(&checks)[0],
            "rust-fmt",
            "ordre fixe : rust avant dockerfile"
        );
        assert_eq!(*ids(&checks).last().expect("non vide"), "docker-hadolint");
    }

    /// Scenario `preset_ts_par_workspace` — `js-ts` et `vue` ensemble ne produisent qu'un jeu.
    #[test]
    fn preset_ts_par_workspace() {
        let checks = checks_for_languages(&langs(&["js-ts", "vue"]));
        assert_eq!(
            ids(&checks),
            vec!["ts-check", "ts-coverage-provider", "ts-coverage"]
        );
        for check in &checks {
            assert_eq!(check.expand, Some(Expand::NpmWorkspaces));
            assert_eq!(check.cwd, "", "cwd racine");
        }
        assert_eq!(checks[0].output, OutputKind::TscText);
        assert_eq!(checks[1].kind, CheckKind::Tests);
        assert_eq!(
            checks[1].argv,
            [
                "npm",
                "install",
                "--no-save",
                "--no-audit",
                "--no-fund",
                "@vitest/coverage-v8@{vitest_version}"
            ]
        );
        assert_eq!(checks[2].kind, CheckKind::Coverage);
        assert_eq!(
            checks[2].output,
            OutputKind::Lcov {
                path: "ts-cov/{workspace_slug}/lcov.info".to_string()
            }
        );
        assert_eq!(checks[2].after.as_deref(), Some("ts-coverage-provider"));
        assert_eq!(
            checks[2].argv,
            [
                "npm",
                "run",
                "test",
                "--workspace={workspace}",
                "--if-present",
                "--",
                "--coverage",
                "--coverage.reporter=lcov",
                "--coverage.reportsDirectory={out_dir}/ts-cov/{workspace_slug}"
            ]
        );
    }

    /// Scenario `preset_python_et_dockerfile`.
    #[test]
    fn preset_python_et_dockerfile() {
        let checks = checks_for_languages(&langs(&["python", "dockerfile"]));
        assert_eq!(
            ids(&checks),
            vec!["py-ruff", "py-tests", "py-coverage", "docker-hadolint"]
        );
        assert_eq!(checks[0].output, OutputKind::RuffJson);
        assert_eq!(
            checks[0].argv,
            ["ruff", "check", "--output-format", "json", "."]
        );
        assert_eq!(checks[1].argv, ["coverage", "run", "-m", "pytest"]);
        assert_eq!(
            checks[2].argv,
            ["coverage", "lcov", "-o", "{out_dir}/py.lcov"]
        );
        assert_eq!(checks[2].after.as_deref(), Some("py-tests"));
        assert_eq!(
            checks[2].output,
            OutputKind::Lcov {
                path: "py.lcov".to_string()
            }
        );
        assert_eq!(checks[3].argv, ["hadolint", "--format", "json", "{files}"]);
        assert_eq!(
            checks[3].expand,
            Some(Expand::Files {
                globs: [
                    "Dockerfile",
                    "**/Dockerfile",
                    "**/Dockerfile.*",
                    "**/*.Dockerfile",
                    "**/Containerfile"
                ]
                .iter()
                .map(|g| (*g).to_string())
                .collect()
            })
        );
        assert_eq!(checks[3].output, OutputKind::HadolintJson);
    }

    /// Scenario `langages_non_verifies`.
    #[test]
    fn langages_non_verifies() {
        assert_eq!(
            unchecked_languages(&langs(&["rust", "go", "terraform"])),
            vec!["go".to_string(), "terraform".to_string()]
        );
    }

    /// Scenario `argv_sans_shell` — aucun preset ne passe par un shell.
    #[test]
    fn argv_sans_shell() {
        for language in ["rust", "js-ts", "vue", "python", "dockerfile"] {
            for check in preset_checks(language) {
                assert!(
                    !["sh", "bash", "zsh", "dash"].contains(&check.argv[0].as_str()),
                    "{} : argv[0] shell interdit : {:?}",
                    check.id,
                    check.argv
                );
                assert!(
                    !check.argv.iter().any(|a| a == "-c"),
                    "{} : `-c` interdit : {:?}",
                    check.id,
                    check.argv
                );
            }
        }
    }

    /// Scenario `validate_rejette_cwd_evade`.
    #[test]
    fn validate_rejette_cwd_evade() {
        for cwd in ["../x", "/etc", "a/../b"] {
            let check = Check {
                id: "c".to_string(),
                kind: CheckKind::Lint,
                argv: vec!["true".to_string()],
                cwd: cwd.to_string(),
                output: OutputKind::None,
                expand: None,
                serial_group: None,
                after: None,
            };
            assert!(
                matches!(validate(&check), Err(HealthError::InvalidCheck { .. })),
                "cwd {cwd} doit être rejeté"
            );
        }
    }

    /// Scenario `validate_rejette_argv_vide` (+ clauses Raises : `id` vide, `argv[0]` vide).
    #[test]
    fn validate_rejette_argv_vide() {
        let base = Check {
            id: "c".to_string(),
            kind: CheckKind::Lint,
            argv: vec!["true".to_string()],
            cwd: String::new(),
            output: OutputKind::None,
            expand: None,
            serial_group: None,
            after: None,
        };
        assert!(validate(&base).is_ok(), "un check minimal est valide");
        let mut no_argv = base.clone();
        no_argv.argv = Vec::new();
        assert!(matches!(
            validate(&no_argv),
            Err(HealthError::InvalidCheck { .. })
        ));
        let mut empty_id = base.clone();
        empty_id.id = String::new();
        assert!(matches!(
            validate(&empty_id),
            Err(HealthError::InvalidCheck { .. })
        ));
        let mut empty_prog = base.clone();
        empty_prog.argv = vec![String::new()];
        assert!(matches!(
            validate(&empty_prog),
            Err(HealthError::InvalidCheck { .. })
        ));
    }

    /// Les presets passent leur propre validation.
    #[test]
    fn presets_valident() {
        for language in ["rust", "js-ts", "vue", "python", "dockerfile"] {
            for check in preset_checks(language) {
                assert!(validate(&check).is_ok(), "{} invalide", check.id);
            }
        }
    }
}
