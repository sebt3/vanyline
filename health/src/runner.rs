//! Seule I/O du crate : exécute une liste de [`Check`] dans un répertoire de dépôt, parse
//! leurs sorties avec les modules purs, restreint au code neuf si une référence de base est
//! fournie, et assemble le [`HealthReport`]. Jamais de shell : uniquement des argv.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncSeekExt as _;
use tokio::process::Command as TokioCommand;

use crate::cargo_messages::parse_cargo_messages;
use crate::checks::Check;
use crate::checks::Expand;
use crate::checks::OUT_DIR_TOKEN;
use crate::checks::OutputKind;
use crate::checks::validate;
use crate::diff::ChangedLines;
use crate::diff::parse_changed_lines;
use crate::diff::restrict_coverage;
use crate::diff::restrict_issues;
use crate::error::HealthError;
use crate::hadolint_json::parse_hadolint_json;
use crate::lcov::parse_lcov;
use crate::report::CheckKind;
use crate::report::CheckResult;
use crate::report::CheckStatus;
use crate::report::CoverageReport;
use crate::report::HealthReport;
use crate::report::Issue;
use crate::report::NewCode;
use crate::report::Severity;
use crate::ruff_json::parse_ruff_json;
use crate::tsc_text::parse_tsc_text;

/// `output_tail` par défaut : fin de sortie bornée à 4 Kio.
pub const DEFAULT_TAIL_BYTES: usize = 4096;

/// Intervalle de respiration des attendes d'ordonnancement (groupe, plafond, préalable).
const POLL: Duration = Duration::from_millis(5);

/// Codes d'état partagés de l'ordonnancement (les statuts terminaux sont `>= PASSED`).
const ST_PENDING: u8 = 0;
const ST_PASSED: u8 = 1;
const ST_FAILED: u8 = 2;
const ST_ERRORED: u8 = 3;
const ST_SKIPPED: u8 = 4;

fn status_code(status: &CheckStatus) -> u8 {
    match status {
        CheckStatus::Passed => ST_PASSED,
        CheckStatus::Failed => ST_FAILED,
        CheckStatus::Errored => ST_ERRORED,
        CheckStatus::Skipped => ST_SKIPPED,
    }
}

/// Paramètres d'un run de vérifications.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Racine du dépôt analysé.
    pub root: PathBuf,
    /// Répertoire de sortie du run (fichiers `<id>.stdout`/`<id>.stderr`, lcov).
    pub out_dir: PathBuf,
    /// Référence de base pour le calcul du code neuf ; validée avant toute exécution.
    pub base: Option<String>,
    /// Paires d'environnement ADDITIVES appliquées avant chaque spawn (jamais de vidage de
    /// l'environnement hérité), dernière gagnante.
    pub env: Vec<(String, String)>,
    /// Délai au-delà duquel la commande est tuée avec tout son groupe de processus.
    pub check_timeout: Duration,
    /// Bornes d'`output_tail` (défaut : [`DEFAULT_TAIL_BYTES`]).
    pub tail_bytes: usize,
    /// Langages sans preset, recopiés tels quels dans le rapport.
    pub unchecked_languages: Vec<String>,
    /// Plafond de vérifications exécutées simultanément (défaut usuel : 2).
    pub max_parallel: usize,
    /// Diagnostics déjà connus (serveurs LSP de la sandbox), versés au rapport sous un
    /// résultat `lsp` avant l'exécution des commandes.
    pub preliminary_issues: Vec<Issue>,
    /// Niveau de priorité (`nice`) des commandes ; `None` = priorité normale, défaut usuel
    /// `Some(10)`.
    pub niceness: Option<i32>,
}

/// Environnement immuable partagé par toutes les tâches d'un run.
struct Ctx {
    root: PathBuf,
    out_dir: PathBuf,
    env: Vec<(String, String)>,
    check_timeout: Duration,
    tail_bytes: usize,
    niceness: Option<i32>,
}

/// Vérification prête à exécuter, après expansion.
struct Ready {
    /// Index de la vérification dans le rapport (ordre reçu).
    slot: usize,
    id: String,
    kind: CheckKind,
    /// argv avec les jetons d'expansion (`{workspace}`, `{workspace_slug}`, `{files}`)
    /// substitués ; `{out_dir}` et `{vitest_version}` le sont à l'exécution.
    argv: Vec<String>,
    /// Répertoire d'exécution relatif à la racine.
    cwd: String,
    /// Répertoire d'exécution relatif à la racine vu des parseurs (lcov `SF` relatifs,
    /// préfixe tsc) — `cwd` + workspace après expansion npm.
    prefix: String,
    output: OutputKind,
    /// Clé du groupe de sérialisation.
    group: String,
    after: Option<String>,
}

/// Créneau du rapport : soit prêt à exécuter, soit déjà rendu (skipped/errored d'expansion).
enum Slot {
    Ready(Ready),
    Done(CheckResult),
}

/// Résultat d'une tâche d'exécution.
struct Outcome {
    result: CheckResult,
    issues: Vec<Issue>,
    /// Texte lcov produit (chemins `SF` déjà rendus absolus), pour la fusion.
    lcov_text: Option<String>,
}

/// Travail remis à une tâche d'exécution.
struct Job {
    ready: Ready,
    /// Index du créneau du préalable, s'il a pu être résolu.
    dep: Option<usize>,
    /// Id déclaré du préalable (pour le message de cascade `errored`).
    after_id: Option<String>,
    /// Verrou du groupe de sérialisation.
    flag: Arc<AtomicBool>,
}

/// Exécute les vérifications et assemble le rapport (« catch all the failures » : chaque
/// vérification est rapportée, jamais de fail-fast).
///
/// # Errors
///
/// [`HealthError::InvalidRef`] si `base` est invalide (avant toute exécution) ;
/// [`HealthError::ReportIo`] si `out_dir` est impossible à créer. Tout le reste est
/// rapporté PAR vérification.
pub async fn run_checks(
    options: &RunOptions,
    checks: &[Check],
) -> Result<HealthReport, HealthError> {
    if let Some(base) = &options.base {
        validate_base(base)?;
    }
    tokio::fs::create_dir_all(&options.out_dir)
        .await
        .map_err(|source| HealthError::ReportIo {
            path: options.out_dir.display().to_string(),
            source,
        })?;
    let revision = run_git(&options.root, &["rev-parse", "HEAD"]).await.ok();
    let ctx = Arc::new(Ctx {
        root: options.root.clone(),
        out_dir: options.out_dir.clone(),
        env: options.env.clone(),
        check_timeout: options.check_timeout,
        tail_bytes: options.tail_bytes,
        niceness: options.niceness,
    });
    // Diagnostics préliminaires : versés au rapport AVANT l'exécution des commandes.
    let mut issues: Vec<Issue> = Vec::new();
    let mut checks_out: Vec<CheckResult> = Vec::new();
    if !options.preliminary_issues.is_empty() {
        let has_error = options
            .preliminary_issues
            .iter()
            .any(|issue| issue.severity == Severity::Error);
        issues.extend(options.preliminary_issues.iter().cloned());
        checks_out.push(CheckResult {
            id: "lsp".to_string(),
            kind: CheckKind::Lint,
            status: if has_error {
                CheckStatus::Failed
            } else {
                CheckStatus::Passed
            },
            exit_code: None,
            duration_ms: 0,
            output_tail: String::new(),
        });
    }
    let mut slots: Vec<Slot> = Vec::new();
    expand_all(&ctx, checks, &mut slots).await;
    let slot_outcomes = execute_all(&ctx, &slots, options.max_parallel.max(1)).await;
    let mut lcov_texts: Vec<String> = Vec::new();
    for outcome in slot_outcomes {
        checks_out.push(outcome.result);
        issues.extend(outcome.issues);
        if let Some(text) = outcome.lcov_text {
            lcov_texts.push(text);
        }
    }
    let coverage = merge_lcov(&lcov_texts, &ctx.root);
    let mut report = HealthReport {
        revision,
        checks: checks_out,
        coverage,
        issues,
        new_code: None,
        unchecked_languages: options.unchecked_languages.clone(),
    };
    if let Some(base) = &options.base {
        match compute_new_code(&ctx.root, base, report.coverage.as_ref(), &report.issues).await {
            Ok(new_code) => report.new_code = Some(new_code),
            Err(reason) => {
                tracing::warn!(base, error = %reason, "calcul du code neuf en échec");
                report.checks.push(CheckResult {
                    id: "new-code".to_string(),
                    kind: CheckKind::NewCode,
                    status: CheckStatus::Errored,
                    exit_code: None,
                    duration_ms: 0,
                    output_tail: cap_bytes(&reason, ctx.tail_bytes),
                });
            }
        }
    }
    Ok(report)
}

/// Refuse toute référence git pouvant devenir un drapeau ou un argument multiple.
fn validate_base(base: &str) -> Result<(), HealthError> {
    if base.is_empty()
        || base.starts_with('-')
        || base.contains(' ')
        || base.contains('\0')
        || base.contains('\n')
    {
        return Err(HealthError::InvalidRef {
            value: base.to_string(),
        });
    }
    Ok(())
}

// --------------------------------------------------------------------------- expansion

/// Développe chaque déclaration en créneaux (prêts ou déjà rendus).
async fn expand_all(ctx: &Arc<Ctx>, checks: &[Check], slots: &mut Vec<Slot>) {
    for check in checks {
        expand_one(ctx, check, slots).await;
    }
}

async fn expand_one(ctx: &Arc<Ctx>, check: &Check, slots: &mut Vec<Slot>) {
    if let Err(err) = validate(check) {
        slots.push(Slot::Done(plain_result(
            &check.id,
            &check.kind,
            CheckStatus::Errored,
            &err.to_string(),
            ctx.tail_bytes,
        )));
        return;
    }
    match &check.expand {
        None => slots.push(Slot::Ready(ready_single(
            check,
            check.id.clone(),
            check.argv.clone(),
            check.cwd.clone(),
            slots.len(),
        ))),
        Some(Expand::NpmWorkspaces) => expand_npm_workspaces(ctx, check, slots).await,
        Some(Expand::Files { globs }) => expand_files(ctx, check, globs, slots).await,
    }
}

/// Branche `NpmWorkspaces` : un créneau par workspace lu dans le `package.json` de
/// `root`+`cwd` ; projet sans workspaces : un seul créneau `.` sans `--workspace=.` ;
/// lecture en échec : créneau déjà rendu `errored`.
async fn expand_npm_workspaces(ctx: &Arc<Ctx>, check: &Check, slots: &mut Vec<Slot>) {
    let manifest = ctx.root.join(&check.cwd).join("package.json");
    match read_npm_workspaces(&manifest).await {
        Err(reason) => slots.push(Slot::Done(plain_result(
            &check.id,
            &check.kind,
            CheckStatus::Errored,
            &reason,
            ctx.tail_bytes,
        ))),
        Ok(None) => {
            // Projet sans workspaces : un seul workspace `.`, argument
            // `--workspace=.` retiré.
            let argv: Vec<String> = substitute_tokens(&check.argv, ".", ".")
                .into_iter()
                .filter(|arg| arg != "--workspace=.")
                .collect();
            slots.push(Slot::Ready(ready_single(
                check,
                format!("{}:.", check.id),
                argv,
                join_rel(&check.cwd, "."),
                slots.len(),
            )));
        }
        Ok(Some(workspaces)) => {
            for workspace in workspaces {
                let slug = workspace.replace('/', "-");
                let argv = substitute_tokens(&check.argv, &workspace, &slug);
                let mut ready = ready_single(
                    check,
                    format!("{}:{workspace}", check.id),
                    argv,
                    join_rel(&check.cwd, &workspace),
                    slots.len(),
                );
                ready.output = substitute_output(&check.output, &workspace, &slug);
                slots.push(Slot::Ready(ready));
            }
        }
    }
}

/// Branche `Files` : un seul créneau dont l'élément `{files}` est remplacé par les fichiers
/// suivis par git correspondant aux globs ; aucun fichier : créneau `skipped` ; `git
/// ls-files` en échec : créneau déjà rendu `errored`.
async fn expand_files(ctx: &Arc<Ctx>, check: &Check, globs: &[String], slots: &mut Vec<Slot>) {
    match run_git(&ctx.root, &["ls-files"]).await {
        Err(err) => slots.push(Slot::Done(plain_result(
            &check.id,
            &check.kind,
            CheckStatus::Errored,
            &err.to_string(),
            ctx.tail_bytes,
        ))),
        Ok(listing) => {
            let matched: Vec<String> = listing
                .lines()
                .filter(|file| globs.iter().any(|glob| glob_match(glob, file)))
                .map(str::to_string)
                .collect();
            if matched.is_empty() {
                slots.push(Slot::Done(plain_result(
                    &check.id,
                    &check.kind,
                    CheckStatus::Skipped,
                    "aucun fichier suivi à vérifier",
                    ctx.tail_bytes,
                )));
                return;
            }
            let mut argv: Vec<String> =
                Vec::with_capacity(check.argv.len().saturating_add(matched.len()));
            for arg in &check.argv {
                if arg == "{files}" {
                    argv.extend(matched.iter().cloned());
                } else {
                    argv.push(arg.clone());
                }
            }
            slots.push(Slot::Ready(ready_single(
                check,
                check.id.clone(),
                argv,
                check.cwd.clone(),
                slots.len(),
            )));
        }
    }
}

/// Construit un créneau prêt sans expansion de workspace (cwd et groupe par défaut).
fn ready_single(
    check: &Check,
    id: String,
    argv: Vec<String>,
    prefix: String,
    slot: usize,
) -> Ready {
    Ready {
        slot,
        id,
        kind: check.kind.clone(),
        argv,
        cwd: check.cwd.clone(),
        prefix,
        output: check.output.clone(),
        group: check
            .serial_group
            .clone()
            .unwrap_or_else(|| default_group(check)),
        after: check.after.clone(),
    }
}

/// Groupe implicite : programme + cwd.
fn default_group(check: &Check) -> String {
    format!(
        "{}\u{1}{}",
        check.argv.first().map(String::as_str).unwrap_or_default(),
        check.cwd
    )
}

fn substitute_tokens(argv: &[String], workspace: &str, slug: &str) -> Vec<String> {
    argv.iter()
        .map(|arg| {
            arg.replace("{workspace_slug}", slug)
                .replace("{workspace}", workspace)
        })
        .collect()
}

fn substitute_output(output: &OutputKind, workspace: &str, slug: &str) -> OutputKind {
    match output {
        OutputKind::Lcov { path } => OutputKind::Lcov {
            path: path
                .replace("{workspace_slug}", slug)
                .replace("{workspace}", workspace),
        },
        other => other.clone(),
    }
}

/// Joint deux chemins relatifs à la racine en omettant les composants vides et `.`.
fn join_rel(cwd: &str, workspace: &str) -> String {
    [cwd, workspace]
        .into_iter()
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| part.trim_matches('/'))
        .collect::<Vec<_>>()
        .join("/")
}

/// Champ `workspaces` du `package.json` : tableau de motifs, ou objet `{ packages: [...] }` ;
/// `Ok(None)` = projet sans workspaces. Motifs littéraux repris tels quels, globs (au moins
/// un `*`/`?`) développés sur les répertoires existants.
async fn read_npm_workspaces(manifest: &Path) -> Result<Option<Vec<String>>, String> {
    let bytes = tokio::fs::read(manifest)
        .await
        .map_err(|err| format!("package.json illisible ({}): {err}", manifest.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("package.json invalide ({}): {err}", manifest.display()))?;
    let entries: Vec<&serde_json::Value> = match value.get("workspaces") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(serde_json::Value::Object(object)) => match object.get("packages") {
            Some(serde_json::Value::Array(items)) => items.iter().collect(),
            _ => return Ok(None),
        },
        Some(_) => return Err("champ workspaces ni tableau ni {packages}".to_string()),
    };
    let base = manifest.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut workspaces: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in entries {
        let pattern = entry
            .as_str()
            .ok_or_else(|| "entrée de workspaces non textuelle".to_string())?;
        let candidates: Vec<String> = if pattern.contains(['*', '?']) {
            expand_glob_dirs(&base, pattern).await
        } else {
            vec![pattern.to_string()]
        };
        for candidate in candidates {
            if seen.insert(candidate.clone()) {
                workspaces.push(candidate);
            }
        }
    }
    Ok(Some(workspaces))
}

/// Développe un glob de répertoires (`packages/*`) segment par segment, sur les
/// sous-répertoires existants, triés.
async fn expand_glob_dirs(base: &Path, pattern: &str) -> Vec<String> {
    let mut current: Vec<String> = vec![String::new()];
    for segment in pattern.split('/') {
        if segment.is_empty() {
            continue;
        }
        let mut next: Vec<String> = Vec::new();
        for prefix in &current {
            let dir = if prefix.is_empty() {
                base.to_path_buf()
            } else {
                base.join(prefix)
            };
            let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
                continue;
            };
            let mut names: Vec<String> = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                if entry
                    .file_type()
                    .await
                    .is_ok_and(|file_type| file_type.is_dir())
                {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
            names.sort();
            for name in names {
                if segment_match(segment, &name) {
                    next.push(if prefix.is_empty() {
                        name
                    } else {
                        format!("{prefix}/{name}")
                    });
                }
            }
        }
        current = next;
    }
    current
        .into_iter()
        .filter(|path| !path.is_empty())
        .collect()
}

// --------------------------------------------------------------------- glob (Files, npm)

/// Match de glob sur un chemin : `**` = zéro ou plusieurs segments, `*`/`?` dans un
/// segment, le reste littéral.
fn glob_match(pattern: &str, text: &str) -> bool {
    glob_segments(
        &pattern.split('/').collect::<Vec<_>>(),
        &text.split('/').collect::<Vec<_>>(),
    )
}

fn glob_segments<'a>(p: &[&'a str], t: &[&'a str]) -> bool {
    let Some((first, rest)) = p.split_first() else {
        return t.is_empty();
    };
    if *first == "**" {
        // zéro ou plusieurs segments
        glob_segments(rest, t) || (!t.is_empty() && glob_segments(p, &t[1..]))
    } else {
        let Some((head, tail)) = t.split_first() else {
            return false;
        };
        segment_match(first, head) && glob_segments(rest, tail)
    }
}

/// Match de glob dans un segment (aucun `/`).
fn segment_match(pattern: &str, text: &str) -> bool {
    seg_match_chars(
        &pattern.chars().collect::<Vec<_>>(),
        &text.chars().collect::<Vec<_>>(),
    )
}

fn seg_match_chars(p: &[char], t: &[char]) -> bool {
    let Some((first, rest)) = p.split_first() else {
        return t.is_empty();
    };
    match first {
        '*' => seg_match_chars(rest, t) || (!t.is_empty() && seg_match_chars(p, &t[1..])),
        '?' => !t.is_empty() && seg_match_chars(rest, &t[1..]),
        letter => !t.is_empty() && t.first() == Some(letter) && seg_match_chars(rest, &t[1..]),
    }
}

// -------------------------------------------------------------------------- ordonnancement

/// Statistiques terminales d'un créneau pour l'attente des préalables.
async fn execute_all(ctx: &Arc<Ctx>, slots: &[Slot], max_parallel: usize) -> Vec<SlotOutcome> {
    let statuses: Arc<Vec<AtomicU8>> = Arc::new(
        slots
            .iter()
            .map(|slot| {
                AtomicU8::new(match slot {
                    Slot::Ready(_) => ST_PENDING,
                    Slot::Done(result) => status_code(&result.status),
                })
            })
            .collect(),
    );
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    for (index, slot) in slots.iter().enumerate() {
        let id = match slot {
            Slot::Ready(ready) => ready.id.clone(),
            Slot::Done(result) => result.id.clone(),
        };
        by_id.insert(id, index);
    }
    let handles = spawn_jobs(ctx, slots, &by_id, &statuses, max_parallel);
    collect_outcomes(ctx, slots, handles).await
}

/// Lance une tâche tokio par vérification prête (verrou de groupe partagé par clé).
fn spawn_jobs(
    ctx: &Arc<Ctx>,
    slots: &[Slot],
    by_id: &BTreeMap<String, usize>,
    statuses: &Arc<Vec<AtomicU8>>,
    max_parallel: usize,
) -> Vec<Option<tokio::task::JoinHandle<Outcome>>> {
    let mut flags: BTreeMap<String, Arc<AtomicBool>> = BTreeMap::new();
    let inflight = Arc::new(AtomicUsize::new(0));
    let mut handles: Vec<Option<tokio::task::JoinHandle<Outcome>>> =
        (0..slots.len()).map(|_| None).collect();
    for (index, slot) in slots.iter().enumerate() {
        let Slot::Ready(ready) = slot else { continue };
        let dep = ready
            .after
            .as_deref()
            .and_then(|after| resolve_dep(&ready.id, after, by_id, index));
        let flag = Arc::clone(
            flags
                .entry(ready.group.clone())
                .or_insert_with(|| Arc::new(AtomicBool::new(false))),
        );
        let job = Job {
            ready: Ready {
                slot: index,
                id: ready.id.clone(),
                kind: ready.kind.clone(),
                argv: ready.argv.clone(),
                cwd: ready.cwd.clone(),
                prefix: ready.prefix.clone(),
                output: ready.output.clone(),
                group: ready.group.clone(),
                after: ready.after.clone(),
            },
            dep,
            after_id: ready.after.clone(),
            flag,
        };
        handles[index] = Some(tokio::spawn(run_job(
            Arc::clone(ctx),
            job,
            Arc::clone(statuses),
            Arc::clone(&inflight),
            max_parallel,
        )));
    }
    handles
}

/// Récolte les tâches dans l'ordre des créneaux reçus.
async fn collect_outcomes(
    ctx: &Arc<Ctx>,
    slots: &[Slot],
    mut handles: Vec<Option<tokio::task::JoinHandle<Outcome>>>,
) -> Vec<SlotOutcome> {
    let mut outcomes: Vec<Option<SlotOutcome>> = (0..slots.len()).map(|_| None).collect();
    for (index, slot) in slots.iter().enumerate() {
        match (slot, handles[index].take()) {
            (Slot::Done(result), _) => outcomes[index] = Some(SlotOutcome::solo(result.clone())),
            (Slot::Ready(ready), Some(handle)) => {
                outcomes[index] = Some(match handle.await {
                    Ok(outcome) => SlotOutcome {
                        result: outcome.result,
                        issues: outcome.issues,
                        lcov_text: outcome.lcov_text,
                    },
                    Err(err) => {
                        tracing::error!(id = %ready.id, error = %err, "tâche d'exécution empoisonnée");
                        SlotOutcome::solo(plain_result(
                            &ready.id,
                            &ready.kind,
                            CheckStatus::Errored,
                            &format!("tâche empoisonnée : {err}"),
                            ctx.tail_bytes,
                        ))
                    }
                });
            }
            (Slot::Ready(ready), None) => {
                outcomes[index] = Some(SlotOutcome::solo(plain_result(
                    &ready.id,
                    &ready.kind,
                    CheckStatus::Errored,
                    "vérification non planifiée",
                    ctx.tail_bytes,
                )));
            }
        }
    }
    outcomes
        .into_iter()
        .map(|outcome| {
            outcome.unwrap_or_else(|| {
                SlotOutcome::solo(CheckResult {
                    id: "internal".to_string(),
                    kind: CheckKind::Lint,
                    status: CheckStatus::Errored,
                    exit_code: None,
                    duration_ms: 0,
                    output_tail: "fente de rapport manquante".to_string(),
                })
            })
        })
        .collect()
}

/// Résout l'id d'un préalable : correspondance exacte, puis `<après>:<workspace>` pour les
/// vérifications issues d'une expansion npm.
fn resolve_dep(
    id: &str,
    after: &str,
    by_id: &BTreeMap<String, usize>,
    self_index: usize,
) -> Option<usize> {
    if let Some(index) = by_id.get(after) {
        return Some(*index).filter(|index| *index != self_index);
    }
    let workspace = id.split_once(':').map(|(_, workspace)| workspace)?;
    by_id
        .get(&format!("{after}:{workspace}"))
        .copied()
        .filter(|index| *index != self_index)
}

/// Tâche d'une vérification : attend le préalable, prend le groupe et un jeton de
/// concurrence, exécute, libère.
async fn run_job(
    ctx: Arc<Ctx>,
    job: Job,
    statuses: Arc<Vec<AtomicU8>>,
    inflight: Arc<AtomicUsize>,
    max_parallel: usize,
) -> Outcome {
    let slot = job.ready.slot;
    if let Some(dep) = job.dep {
        let dep_status = loop {
            let code = statuses[dep].load(Ordering::Acquire);
            if code >= ST_PASSED {
                break code;
            }
            tokio::time::sleep(POLL).await;
        };
        if dep_status == ST_ERRORED {
            let outcome = Outcome::solo(plain_result(
                &job.ready.id,
                &job.ready.kind,
                CheckStatus::Errored,
                &format!(
                    "préalable '{}' en erreur : vérification non lancée",
                    job.after_id.unwrap_or_default()
                ),
                ctx.tail_bytes,
            ));
            statuses[slot].store(ST_ERRORED, Ordering::Release);
            return outcome;
        }
    }
    acquire(&job.flag, &inflight, max_parallel).await;
    let outcome = execute(&ctx, &job.ready).await;
    job.flag.store(false, Ordering::Release);
    inflight.fetch_sub(1, Ordering::Release);
    statuses[slot].store(status_code(&outcome.result.status), Ordering::Release);
    outcome
}

/// Prend le verrou du groupe de sérialisation puis un jeton de concurrence (≤ `max_parallel`).
async fn acquire(flag: &Arc<AtomicBool>, inflight: &Arc<AtomicUsize>, max_parallel: usize) {
    loop {
        while flag
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            tokio::time::sleep(POLL).await;
        }
        let current = inflight.load(Ordering::Acquire);
        if current < max_parallel
            && inflight
                .compare_exchange(
                    current,
                    current.saturating_add(1),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        {
            return;
        }
        flag.store(false, Ordering::Release);
        tokio::time::sleep(POLL).await;
    }
}

// --------------------------------------------------------------------------- exécution

/// Résultat brut du spawn/attente d'une commande.
struct Exec {
    status: CheckStatus,
    exit_code: Option<i32>,
    /// Raison d'`errored` (spawn, attente, timeout) le cas échéant.
    reason: Option<String>,
}

impl Exec {
    fn error(reason: String) -> Self {
        Self {
            status: CheckStatus::Errored,
            exit_code: None,
            reason: Some(reason),
        }
    }
}

/// Résultat d'une fente : résultat + issues + texte lcov éventuel.
struct SlotOutcome {
    result: CheckResult,
    issues: Vec<Issue>,
    lcov_text: Option<String>,
}

impl SlotOutcome {
    fn solo(result: CheckResult) -> Self {
        Self {
            result,
            issues: Vec::new(),
            lcov_text: None,
        }
    }
}

impl Outcome {
    fn solo(result: CheckResult) -> Self {
        Self {
            result,
            issues: Vec::new(),
            lcov_text: None,
        }
    }
}

/// Exécute une vérification prête : préparation argv, spawn, sortie dans `out_dir`,
/// interprétation selon `output`.
async fn execute(ctx: &Arc<Ctx>, ready: &Ready) -> Outcome {
    let argv = match prepare_argv(ctx, ready).await {
        Ok(argv) => argv,
        Err(reason) => {
            return Outcome::solo(plain_result(
                &ready.id,
                &ready.kind,
                CheckStatus::Errored,
                &reason,
                ctx.tail_bytes,
            ));
        }
    };
    let stem = sanitize_id(&ready.id);
    let stdout_path = ctx.out_dir.join(format!("{stem}.stdout"));
    let stderr_path = ctx.out_dir.join(format!("{stem}.stderr"));
    let started = tokio::time::Instant::now();
    let exec = spawn_and_wait(ctx, ready, &argv, &stdout_path, &stderr_path).await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut tail = read_tail(&stderr_path, ctx.tail_bytes)
        .await
        .unwrap_or_default();
    let mut status = exec.status;
    if let Some(reason) = &exec.reason {
        tail = if tail.is_empty() {
            reason.clone()
        } else {
            format!("{reason}\n{tail}")
        };
    }
    if matches!(ready.output, OutputKind::None) && tail.is_empty() {
        tail = read_tail(&stdout_path, ctx.tail_bytes)
            .await
            .unwrap_or_default();
    }
    let (issues, lcov_text, escalate) = if status == CheckStatus::Errored {
        (Vec::new(), None, None)
    } else {
        interpret_output(ctx, ready, &stdout_path, &status).await
    };
    if let Some(reason) = escalate {
        status = CheckStatus::Errored;
        if tail.is_empty() {
            tail = reason;
        }
    }
    Outcome {
        result: CheckResult {
            id: ready.id.clone(),
            kind: ready.kind.clone(),
            status,
            exit_code: exec.exit_code,
            duration_ms,
            output_tail: cap_bytes(&tail, ctx.tail_bytes),
        },
        issues,
        lcov_text,
    }
}

/// Remplace `{vitest_version}` (lu dans `node_modules`), puis `{out_dir}` ; préfixe
/// l'argv de `nice -n <n>` si demandé.
async fn prepare_argv(ctx: &Arc<Ctx>, ready: &Ready) -> Result<Vec<String>, String> {
    let mut argv = ready.argv.clone();
    if argv.iter().any(|arg| arg.contains("{vitest_version}")) {
        let version = vitest_version(ctx, &ready.prefix).await.ok_or_else(|| {
            format!(
                "version de vitest introuvable dans node_modules (fournisseur @vitest/coverage-v8 impossible à épingler) pour '{}'",
                ready.prefix
            )
        })?;
        for arg in &mut argv {
            if arg.contains("{vitest_version}") {
                *arg = arg.replace("{vitest_version}", &version);
            }
        }
    }
    let out_dir = ctx.out_dir.display().to_string();
    for arg in &mut argv {
        if arg.contains(OUT_DIR_TOKEN) {
            *arg = arg.replace(OUT_DIR_TOKEN, &out_dir);
        }
    }
    Ok(match ctx.niceness {
        Some(niceness) => {
            let mut wrapped = vec!["nice".to_string(), "-n".to_string(), niceness.to_string()];
            wrapped.extend(argv);
            wrapped
        }
        None => argv,
    })
}

async fn spawn_and_wait(
    ctx: &Arc<Ctx>,
    ready: &Ready,
    argv: &[String],
    stdout_path: &Path,
    stderr_path: &Path,
) -> Exec {
    let Some((program, rest)) = argv.split_first() else {
        return Exec::error("argv vide".to_string());
    };
    let stdout_file = match std::fs::File::create(stdout_path) {
        Ok(file) => file,
        Err(source) => {
            return Exec::error(
                HealthError::ReportIo {
                    path: stdout_path.display().to_string(),
                    source,
                }
                .to_string(),
            );
        }
    };
    let stderr_file = match std::fs::File::create(stderr_path) {
        Ok(file) => file,
        Err(source) => {
            return Exec::error(
                HealthError::ReportIo {
                    path: stderr_path.display().to_string(),
                    source,
                }
                .to_string(),
            );
        }
    };
    let mut cmd = TokioCommand::new(program);
    cmd.args(rest)
        .current_dir(ctx.root.join(&ready.cwd))
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .kill_on_drop(true);
    // Groupe de processus dédié : le timeout tuera le GROUPE entier (régression R8).
    cmd.process_group(0);
    // Environnement hérité conservé : les ajouts sont ADDITIFS.
    for (key, value) in &ctx.env {
        cmd.env(key, value);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(source) => {
            return Exec::error(
                HealthError::Spawn {
                    id: ready.id.clone(),
                    source,
                }
                .to_string(),
            );
        }
    };
    match tokio::time::timeout(ctx.check_timeout, child.wait()).await {
        Ok(Ok(status)) => Exec {
            status: if status.success() {
                CheckStatus::Passed
            } else {
                CheckStatus::Failed
            },
            exit_code: status.code(),
            reason: None,
        },
        Ok(Err(source)) => Exec::error(source.to_string()),
        Err(_) => {
            kill_process_group(child.id()).await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            Exec::error(
                HealthError::Timeout {
                    id: ready.id.clone(),
                    secs: ctx.check_timeout.as_secs(),
                }
                .to_string(),
            )
        }
    }
}

/// Tue tout le groupe de processus (`kill -KILL -<pgid>` en argv, jamais un shell, jamais
/// de libc — `libc` n'est pas dans les dépendances autorisées par `lib.sdd`).
async fn kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    let target = format!("-{pid}");
    if let Err(source) = TokioCommand::new("kill")
        .args(["-KILL", &target])
        .status()
        .await
    {
        tracing::warn!(pid, error = %source, "groupe de processus non tué");
    }
}

/// Version de vitest lue dans `node_modules/vitest/package.json` du workspace, puis de la
/// racine.
async fn vitest_version(ctx: &Arc<Ctx>, prefix: &str) -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if !prefix.is_empty() {
        candidates.push(ctx.root.join(prefix));
    }
    candidates.push(ctx.root.clone());
    for dir in candidates {
        let manifest = dir.join("node_modules").join("vitest").join("package.json");
        let Ok(bytes) = tokio::fs::read(&manifest).await else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(version) = value.get("version").and_then(serde_json::Value::as_str) {
            return Some(version.to_string());
        }
    }
    None
}

/// Interprète la sortie structurée : issues, texte lcov, escalade en `errored`.
async fn interpret_output(
    ctx: &Arc<Ctx>,
    ready: &Ready,
    stdout_path: &Path,
    status: &CheckStatus,
) -> (Vec<Issue>, Option<String>, Option<String>) {
    match &ready.output {
        OutputKind::None => (Vec::new(), None, None),
        OutputKind::CargoMessages => {
            let stdout = read_lossy(stdout_path).await;
            (
                parse_cargo_messages(&stdout, &ctx.root, &ready.id),
                None,
                None,
            )
        }
        OutputKind::TscText => {
            let stdout = read_lossy(stdout_path).await;
            (
                parse_tsc_text(&stdout, &ready.prefix, &ready.id),
                None,
                None,
            )
        }
        OutputKind::RuffJson => {
            let stdout = read_lossy(stdout_path).await;
            match parse_ruff_json(&stdout, &ctx.root, &ready.id) {
                Ok(issues) => (issues, None, None),
                Err(err) => (Vec::new(), None, Some(err.to_string())),
            }
        }
        OutputKind::HadolintJson => {
            let stdout = read_lossy(stdout_path).await;
            match parse_hadolint_json(&stdout, &ctx.root, &ready.id) {
                Ok(issues) => (issues, None, None),
                Err(err) => (Vec::new(), None, Some(err.to_string())),
            }
        }
        OutputKind::Lcov { path } => {
            let lcov_path = ctx.out_dir.join(path);
            let text = read_lossy(&lcov_path).await;
            if text.trim().is_empty() {
                return if *status == CheckStatus::Passed {
                    (
                        Vec::new(),
                        None,
                        Some("couverture exigée non produite (lcov absent ou vide)".to_string()),
                    )
                } else {
                    (Vec::new(), None, None)
                };
            }
            let absolute = absolutize_sf(&text, &ctx.root.join(&ready.prefix));
            match parse_lcov(&absolute, &ctx.root, "") {
                Ok(report) if report.files.is_empty() => {
                    if *status == CheckStatus::Passed {
                        (
                            Vec::new(),
                            None,
                            Some(
                                "couverture exigée vide (aucun fichier du dépôt instrumenté)"
                                    .to_string(),
                            ),
                        )
                    } else {
                        (Vec::new(), None, None)
                    }
                }
                Ok(_) => (Vec::new(), Some(absolute), None),
                Err(err) => (Vec::new(), None, Some(err.to_string())),
            }
        }
    }
}

/// Rend absolus (sous le répertoire d'exécution) les chemins `SF:` relatifs d'un lcov, pour
/// que la fusion multi-vérifications se fasse par une seule passe de `parse_lcov`
/// (même règle de fusion que `./lcov.rs`).
fn absolutize_sf(text: &str, exec_dir: &Path) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if let Some(raw) = line.strip_prefix("SF:") {
            let raw = raw.trim();
            if !raw.starts_with('/') {
                let absolute = exec_dir.join(raw);
                out.push_str("SF:");
                out.push_str(&absolute.display().to_string());
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Fusionne les lcov produits par plusieurs vérifications (passée unique : addition
/// ligne à ligne des enregistrements du même fichier).
fn merge_lcov(texts: &[String], root: &Path) -> Option<CoverageReport> {
    if texts.is_empty() {
        return None;
    }
    let merged = texts.join("\n");
    match parse_lcov(&merged, root, "") {
        Ok(report) if report.files.is_empty() => None,
        Ok(report) => Some(report),
        Err(err) => {
            tracing::error!(error = %err, "lcov fusionné invalide");
            None
        }
    }
}

/// Calcule `new_code` : merge-base, diff du worktree, restriction de couverture/issues.
async fn compute_new_code(
    root: &Path,
    base: &str,
    coverage: Option<&CoverageReport>,
    issues: &[Issue],
) -> Result<NewCode, String> {
    let merge_base = run_git(root, &["merge-base", base, "HEAD"])
        .await
        .map_err(|err| err.to_string())?;
    let diff = run_git(
        root,
        &[
            "-c",
            "core.quotepath=off",
            "diff",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            &merge_base,
            "--",
        ],
    )
    .await
    .map_err(|err| err.to_string())?;
    let changed: ChangedLines = parse_changed_lines(&diff).map_err(|err| err.to_string())?;
    Ok(NewCode {
        base: base.to_string(),
        coverage: coverage.map_or(CoverageReport { files: Vec::new() }, |report| {
            restrict_coverage(report, &changed)
        }),
        issues: restrict_issues(issues, &changed),
    })
}

// ------------------------------------------------------------------------------- divers

async fn run_git(root: &Path, args: &[&str]) -> Result<String, HealthError> {
    let output = TokioCommand::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|source| HealthError::Git {
            reason: source.to_string(),
        })?;
    if !output.status.success() {
        return Err(HealthError::Git {
            reason: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Résultat sans exécution (expansion en échec, skip, empoisonnement).
fn plain_result(
    id: &str,
    kind: &CheckKind,
    status: CheckStatus,
    reason: &str,
    tail_bytes: usize,
) -> CheckResult {
    CheckResult {
        id: id.to_string(),
        kind: kind.clone(),
        status,
        exit_code: None,
        duration_ms: 0,
        output_tail: cap_bytes(reason, tail_bytes),
    }
}

/// Nom de fichier sûr dérivé d'un id de vérification.
fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Lecture entière d'un fichier en UTF-8 tolérant ; fichier absent = vide.
async fn read_lossy(path: &Path) -> String {
    match tokio::fs::read(path).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

/// Fin de fichier bornée à `max_bytes` (UTF-8 tolérant).
async fn read_tail(path: &Path, max_bytes: usize) -> Result<String, std::io::Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    let skip = len.saturating_sub(u64::try_from(max_bytes).unwrap_or(u64::MAX));
    file.seek(std::io::SeekFrom::Start(skip)).await?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).await?;
    Ok(cap_bytes(&String::from_utf8_lossy(&buffer), max_bytes))
}

/// Gardent au plus `max_bytes` des derniers octets, sur une frontière de caractère.
fn cap_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start = start.saturating_add(1);
    }
    text[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::{Expand, OutputKind};
    use crate::report::{CheckKind, CheckStatus, Severity};
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Répertoire de fixture unique `<tmp>/vanyline-health-<pid>-<n>` ; sous-répertoires
    /// `repo` (racine du dépôt) et `out` (répertoire de sortie) créés.
    struct Fixture {
        root: PathBuf,
        out: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let n = FIXTURE_COUNTER.fetch_add(1, Ordering::SeqCst);
            let base = std::env::temp_dir().join(format!(
                "opencode/vanyline-health-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            let root = base.join("repo");
            let out = base.join("out");
            std::fs::create_dir_all(&root).expect("fixture root");
            std::fs::create_dir_all(&out).expect("fixture out");
            Self { root, out }
        }
        fn cleanup(&self) {
            let base = self.root.parent().expect("base fixture");
            let _ = std::fs::remove_dir_all(base);
        }
    }

    fn options(fx: &Fixture) -> RunOptions {
        RunOptions {
            root: fx.root.clone(),
            out_dir: fx.out.clone(),
            base: None,
            env: Vec::new(),
            check_timeout: Duration::from_secs(30),
            tail_bytes: crate::runner::DEFAULT_TAIL_BYTES,
            unchecked_languages: Vec::new(),
            max_parallel: 4,
            preliminary_issues: Vec::new(),
            niceness: None,
        }
    }

    /// Vérification de fixture : son propre groupe de sérialisation (indépendante par
    /// défaut), sans shell dans les presets — `sh` n'apparaît que dans ces fixtures.
    fn probe(id: &str, argv: &[&str]) -> Check {
        Check {
            id: id.to_string(),
            kind: CheckKind::Tests,
            argv: argv.iter().map(|a| (*a).to_string()).collect(),
            cwd: String::new(),
            output: OutputKind::None,
            expand: None,
            serial_group: Some(id.to_string()),
            after: None,
        }
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git disponible");
        assert!(
            output.status.success(),
            "git {args:?} : {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo(root: &Path) {
        git(root, &["init", "-q", "-b", "main"]);
    }

    fn result<'a>(
        report: &'a crate::report::HealthReport,
        id: &str,
    ) -> &'a crate::report::CheckResult {
        report
            .checks
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| {
                panic!(
                    "check {id} absent : {:?}",
                    report.checks.iter().map(|c| &c.id).collect::<Vec<_>>()
                )
            })
    }

    /// Scenario `tout_vert`.
    #[tokio::test]
    async fn tout_vert() {
        let fx = Fixture::new("vert");
        let opts = options(&fx);
        let report = run_checks(
            &opts,
            &[probe("a", &["true"]), probe("b", &["sh", "-c", "exit 0"])],
        )
        .await
        .expect("rapport rendu");
        assert_eq!(result(&report, "a").status, CheckStatus::Passed);
        assert_eq!(result(&report, "b").status, CheckStatus::Passed);
        assert_eq!(report.status(), CheckStatus::Passed);
        assert!(
            !report.checks.iter().any(|c| c.id == "lsp"),
            "sans issues préliminaires, aucun résultat lsp"
        );
        fx.cleanup();
    }

    /// Scenario `echec_nest_pas_erreur`.
    #[tokio::test]
    async fn echec_nest_pas_erreur() {
        let fx = Fixture::new("echec");
        let opts = options(&fx);
        let report = run_checks(&opts, &[probe("f", &["false"]), probe("p", &["true"])])
            .await
            .expect("un échec n'est pas une erreur de la fonction");
        let failed = result(&report, "f");
        assert_eq!(failed.status, CheckStatus::Failed);
        assert_eq!(failed.exit_code, Some(1));
        assert_eq!(result(&report, "p").status, CheckStatus::Passed);
        fx.cleanup();
    }

    /// Scenario `spawn_impossible`.
    #[tokio::test]
    async fn spawn_impossible() {
        let fx = Fixture::new("spawn");
        let opts = options(&fx);
        let report = run_checks(
            &opts,
            &[
                probe("nope", &["/nonexistent-vnl/zzz"]),
                probe("p", &["true"]),
            ],
        )
        .await
        .expect("rapport rendu");
        assert_eq!(result(&report, "nope").status, CheckStatus::Errored);
        assert_eq!(
            result(&report, "p").status,
            CheckStatus::Passed,
            "les suivants s'exécutent"
        );
        fx.cleanup();
    }

    /// Scenario `timeout_tue_le_groupe` — ni le parent ni son descendant d'arrière-plan ne
    /// survit (régression R8).
    #[tokio::test]
    async fn timeout_tue_le_groupe() {
        let fx = Fixture::new("timeout");
        let mut opts = options(&fx);
        opts.check_timeout = Duration::from_millis(300);
        let pids = fx.out.join("pids");
        let script = format!("echo $$ > {pids:?}; sleep 300 & echo $! >> {pids:?}; wait");
        let report = run_checks(&opts, &[probe("t", &["sh", "-c", &script])])
            .await
            .expect("rapport rendu");
        assert_eq!(result(&report, "t").status, CheckStatus::Errored);
        let content = std::fs::read_to_string(&pids).expect("pids écrits");
        let pids: Vec<u32> = content
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        assert_eq!(
            pids.len(),
            2,
            "parent + descendant attendus, lu : {content}"
        );
        for pid in pids {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .expect("kill disponible")
                .success();
            assert!(!alive, "le processus {pid} du groupe a survécu au timeout");
        }
        fx.cleanup();
    }

    /// Scenario `sortie_dans_out_dir_pas_dans_root`.
    #[tokio::test]
    async fn sortie_dans_out_dir_pas_dans_root() {
        let fx = Fixture::new("iodir");
        let opts = options(&fx);
        let report = run_checks(
            &opts,
            &[probe(
                "alpha",
                &["sh", "-c", "echo sortie-std ; echo sortie-err 1>&2"],
            )],
        )
        .await
        .expect("rapport rendu");
        assert_eq!(result(&report, "alpha").status, CheckStatus::Passed);
        let root_entries: Vec<_> = std::fs::read_dir(&fx.root).expect("root lisible").collect();
        assert!(root_entries.is_empty(), "root doit rester inchangé");
        assert!(fx.out.join("alpha.stdout").is_file(), "stdout dans out_dir");
        assert!(fx.out.join("alpha.stderr").is_file(), "stderr dans out_dir");
        fx.cleanup();
    }

    /// Scenario `lcov_vide_sur_succes_est_erreur`.
    #[tokio::test]
    async fn lcov_vide_sur_succes_est_erreur() {
        let fx = Fixture::new("lcovvide");
        let opts = options(&fx);
        let mut lcov = probe("cov", &["true"]);
        lcov.kind = CheckKind::Coverage;
        lcov.output = OutputKind::Lcov {
            path: "absent.lcov".to_string(),
        };
        let report = run_checks(&opts, &[lcov]).await.expect("rapport rendu");
        assert_eq!(result(&report, "cov").status, CheckStatus::Errored);
        assert!(
            report.coverage.is_none(),
            "jamais « 0 % » ni « rien » : coverage None"
        );
        fx.cleanup();
    }

    /// Scenario `issues_recuperees` — un warning clippy en messages cargo remonte au rapport.
    #[tokio::test]
    async fn issues_recuperees() {
        let fx = Fixture::new("issues");
        let message = serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "level": "warning",
                "message": "used `unwrap()`",
                "code": { "code": "clippy::unwrap_used" },
                "spans": [{
                    "file_name": format!("{}/src/a.rs", fx.root.display()),
                    "line_start": 12, "column_start": 5, "is_primary": true
                }]
            }
        });
        let fixture = fx.out.join("msg.json");
        std::fs::write(&fixture, format!("{message}\n")).expect("fixture écrite");
        let opts = options(&fx);
        let mut clippy = probe("clippy-probe", &["cat", &fixture.display().to_string()]);
        clippy.kind = CheckKind::Lint;
        clippy.output = OutputKind::CargoMessages;
        let report = run_checks(&opts, &[clippy]).await.expect("rapport rendu");
        assert_eq!(result(&report, "clippy-probe").status, CheckStatus::Passed);
        assert_eq!(report.issues.len(), 1, "issues : {:?}", report.issues);
        let issue = &report.issues[0];
        assert_eq!(issue.check_id, "clippy-probe");
        assert_eq!(issue.path, "src/a.rs");
        assert_eq!(issue.line, 12);
        assert_eq!(issue.severity, Severity::Warning);
        assert_eq!(issue.code.as_deref(), Some("clippy::unwrap_used"));
        fx.cleanup();
    }

    /// Scenario `tail_borne`.
    #[tokio::test]
    async fn tail_borne() {
        use std::io::Write as _;
        let fx = Fixture::new("tail");
        let mut err_fixture = std::fs::File::create(fx.out.join("big.err")).expect("fixture");
        let body = (100_usize)
            .saturating_mul(1024)
            .saturating_sub(b"ENDMARK".len());
        err_fixture.write_all(&vec![b'x'; body]).expect("corps");
        err_fixture.write_all(b"ENDMARK").expect("marqueur");
        drop(err_fixture);
        let mut opts = options(&fx);
        opts.tail_bytes = 4096;
        let script = format!(
            "cat {:?} 1>&2",
            fx.out.join("big.err").display().to_string()
        );
        let report = run_checks(&opts, &[probe("spout", &["sh", "-c", &script])])
            .await
            .expect("rapport rendu");
        let tail = &result(&report, "spout").output_tail;
        assert!(tail.len() <= 4096, "tail de {} octets", tail.len());
        assert!(
            tail.ends_with("ENDMARK"),
            "doit se terminer par la fin du stderr"
        );
        fx.cleanup();
    }

    /// Scenario `base_invalide_refusee` — refus AVANT toute exécution de commande.
    #[tokio::test]
    async fn base_invalide_refusee() {
        let fx = Fixture::new("badebase");
        let mut opts = options(&fx);
        opts.base = Some("--output=x".to_string());
        let marker = fx.root.join("marker");
        let script = format!("touch {:?}", marker.display().to_string());
        let err = run_checks(&opts, &[probe("m", &["sh", "-c", &script])])
            .await
            .expect_err("base invalide refusée");
        assert!(
            matches!(err, crate::error::HealthError::InvalidRef { .. }),
            "attendu InvalidRef, obtenu {err:?}"
        );
        assert!(
            !marker.exists(),
            "aucune commande ne doit avoir été exécutée"
        );
        fx.cleanup();
    }

    /// Scenario `code_neuf` — couverture restreinte aux lignes du diff worktree↔merge-base.
    #[tokio::test]
    async fn code_neuf() {
        let fx = Fixture::new("neuf");
        init_repo(&fx.root);
        std::fs::write(fx.root.join("a.rs"), "ligne 1\n").expect("a.rs v1");
        git(&fx.root, &["add", "-A"]);
        git(&fx.root, &["commit", "-q", "-m", "v1"]);
        let base = git(&fx.root, &["rev-parse", "HEAD"]);
        std::fs::write(fx.root.join("a.rs"), "ligne 1\nligne 2\nligne 3\n").expect("a.rs v2");
        git(&fx.root, &["add", "-A"]);
        git(&fx.root, &["commit", "-q", "-m", "v2"]);
        let head = git(&fx.root, &["rev-parse", "HEAD"]);
        // lcov : ligne 1 couverte, 2 non couverte, 3 couverte (chemins absolus sous root).
        let lcov_fixture = fx.out.join("data.lcov");
        std::fs::write(
            &lcov_fixture,
            format!(
                "SF:{}/a.rs\nDA:1,1\nDA:2,0\nDA:3,1\nend_of_record\n",
                fx.root.display()
            ),
        )
        .expect("lcov fixture");
        let mut opts = options(&fx);
        opts.base = Some(base.clone());
        let mut cov = probe(
            "cov",
            &[
                "cp",
                &lcov_fixture.display().to_string(),
                "{out_dir}/cov.lcov",
            ],
        );
        cov.kind = CheckKind::Coverage;
        cov.output = OutputKind::Lcov {
            path: "cov.lcov".to_string(),
        };
        let report = run_checks(&opts, &[cov]).await.expect("rapport rendu");
        assert_eq!(result(&report, "cov").status, CheckStatus::Passed);
        let full = report
            .coverage
            .as_ref()
            .expect("couverture complète produite");
        assert_eq!(full.files[0].lines_found, 3);
        let new_code = report.new_code.as_ref().expect("new_code calculé");
        assert_eq!(new_code.base, base);
        assert_eq!(new_code.coverage.files.len(), 1);
        let file = &new_code.coverage.files[0];
        assert_eq!(file.path, "a.rs");
        assert_eq!(
            file.lines_found, 2,
            "seules les lignes 2-3 (ajoutées) comptent"
        );
        assert_eq!(file.lines_hit, 1);
        assert_eq!(file.uncovered.len(), 1);
        assert_eq!(file.uncovered[0].start, 2);
        assert_eq!(file.uncovered[0].end, 2);
        assert_eq!(
            report.revision.as_deref(),
            Some(head.as_str()),
            "revision = HEAD"
        );
        fx.cleanup();
    }

    /// Scenario `code_neuf_inclut_le_non_committe` — le diff est l'ÉTAT DU WORKTREE contre
    /// le merge-base : les lignes d'une modification non commitée d'un fichier suivi sont
    /// nouvelles au même titre que celles du commit (`Handles`, l.86).
    #[tokio::test]
    async fn code_neuf_inclut_le_non_committe() {
        let fx = Fixture::new("neufwc");
        init_repo(&fx.root);
        std::fs::write(fx.root.join("a.rs"), "l1\n").expect("a.rs v1");
        git(&fx.root, &["add", "-A"]);
        git(&fx.root, &["commit", "-q", "-m", "v1"]);
        let base = git(&fx.root, &["rev-parse", "HEAD"]);
        // Lignes A (2-3) : commit v2 au-dessus de la base.
        std::fs::write(fx.root.join("a.rs"), "l1\nl2\nl3\n").expect("a.rs v2");
        git(&fx.root, &["add", "-A"]);
        git(&fx.root, &["commit", "-q", "-m", "v2"]);
        // Lignes B (4-5) : MODIFICATION NON COMMITÉE du même fichier suivi (jamais
        // `git add` : le worktree porte la modification, l'index non).
        std::fs::write(fx.root.join("a.rs"), "l1\nl2\nl3\nl4\nl5\n").expect("a.rs v3 non commitée");
        // lcov : 1 et 2 couvertes, 3 non couverte, 4 couverte, 5 non couverte
        // (chemins absolus sous root).
        let lcov_fixture = fx.out.join("data.lcov");
        std::fs::write(
            &lcov_fixture,
            format!(
                "SF:{}/a.rs\nDA:1,1\nDA:2,1\nDA:3,0\nDA:4,1\nDA:5,0\nend_of_record\n",
                fx.root.display()
            ),
        )
        .expect("lcov fixture");
        let mut opts = options(&fx);
        opts.base = Some(base.clone());
        let mut cov = probe(
            "cov",
            &[
                "cp",
                &lcov_fixture.display().to_string(),
                "{out_dir}/cov.lcov",
            ],
        );
        cov.kind = CheckKind::Coverage;
        cov.output = OutputKind::Lcov {
            path: "cov.lcov".to_string(),
        };
        let report = run_checks(&opts, &[cov]).await.expect("rapport rendu");
        assert_eq!(result(&report, "cov").status, CheckStatus::Passed);
        let new_code = report.new_code.as_ref().expect("new_code calculé");
        assert_eq!(new_code.base, base);
        assert_eq!(new_code.coverage.files.len(), 1);
        let file = &new_code.coverage.files[0];
        assert_eq!(file.path, "a.rs");
        // Assertion distincte pour A : si le diff s'arrêtait à HEAD (two-dot
        // merge-base..HEAD), lines_found serait 2 et ce premier assert passerait — c'est
        // le second qui doit dire si seule A est couverte.
        assert_eq!(
            file.lines_found, 4,
            "lignes A (2-3) ET B (4-5) doivent être comptées nouvelles, obtenu {}",
            file.lines_found
        );
        assert_eq!(
            file.lines_hit, 2,
            "seules les lignes couvertes parmi A∪B comptent (2 et 4 touchées)"
        );
        // Assertion distincte pour B : le diff en two-dot révs (merge-base..HEAD)
        // ignorerait les lignes 4-5 → uncovered = [{2,2}] et cet assert ROUGE.
        assert_eq!(
            file.uncovered,
            vec![
                crate::report::LineRange { start: 3, end: 3 },
                crate::report::LineRange { start: 5, end: 5 },
            ],
            "les lignes non commitées B (4-5) doivent être dans le périmètre, \
             avec la 3 (A non couverte)"
        );
        fx.cleanup();
    }

    /// Scenario `echec_du_diff`.
    #[tokio::test]
    async fn echec_du_diff() {
        let fx = Fixture::new("diffko");
        init_repo(&fx.root);
        std::fs::write(fx.root.join("a"), "x\n").expect("a");
        git(&fx.root, &["add", "-A"]);
        git(&fx.root, &["commit", "-q", "-m", "c1"]);
        let mut opts = options(&fx);
        opts.base = Some(format!("{:040x}", 0xdeadu32));
        let report = run_checks(&opts, &[probe("p", &["true"])])
            .await
            .expect("rapport rendu malgré l'échec du diff");
        let new_code = result(&report, "new-code");
        assert_eq!(new_code.status, CheckStatus::Errored);
        assert_eq!(new_code.kind, CheckKind::NewCode);
        assert!(report.new_code.is_none());
        assert_eq!(
            result(&report, "p").status,
            CheckStatus::Passed,
            "les autres sont intacts"
        );
        fx.cleanup();
    }

    /// Scenario `environnement_additif`.
    #[tokio::test]
    async fn environnement_additif() {
        let fx = Fixture::new("env");
        let mut opts = options(&fx);
        opts.env = vec![("FOO".to_string(), "bar".to_string())];
        let report = run_checks(
            &opts,
            &[probe(
                "e",
                &["sh", "-c", "printf 'F=%s P=%s\\n' \"$FOO\" \"$PATH\""],
            )],
        )
        .await
        .expect("rapport rendu");
        let tail = &result(&report, "e").output_tail;
        assert!(tail.starts_with("F=bar P="), "FOO additif, obtenu {tail:?}");
        assert!(
            !tail.trim_end().ends_with("P="),
            "PATH hérité doit être non vide, obtenu {tail:?}"
        );
        assert_eq!(result(&report, "e").status, CheckStatus::Passed);
        fx.cleanup();
    }

    /// Scenario `expansion_par_workspace_npm` — jetons `{workspace}`/`{workspace_slug}` et
    /// ids `<id>:<workspace>`.
    #[tokio::test]
    async fn expansion_par_workspace_npm() {
        let fx = Fixture::new("npmws");
        std::fs::write(
            fx.root.join("package.json"),
            r#"{"name":"racine","workspaces":["frontend","packages/*"]}"#,
        )
        .expect("package.json");
        for ws in ["frontend", "packages/ui", "packages/protocol"] {
            std::fs::create_dir_all(fx.root.join(ws)).expect("ws");
            std::fs::write(
                fx.root.join(ws).join("package.json"),
                format!(r#"{{"name":"{ws}"}}"#),
            )
            .expect("ws package.json");
        }
        let opts = options(&fx);
        let mut expand_probe = probe(
            "ts-check",
            &[
                "sh",
                "-c",
                "printf 'WS=%s SLUG=%s\\n' \"$0\" \"$1\"",
                "--workspace={workspace}",
                "{workspace_slug}",
            ],
        );
        expand_probe.expand = Some(Expand::NpmWorkspaces);
        let report = run_checks(&opts, &[expand_probe])
            .await
            .expect("rapport rendu");
        assert_eq!(
            report.checks.len(),
            3,
            "{:?}",
            report.checks.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        for (id, ws, slug) in [
            ("ts-check:frontend", "frontend", "frontend"),
            ("ts-check:packages/ui", "packages/ui", "packages-ui"),
            (
                "ts-check:packages/protocol",
                "packages/protocol",
                "packages-protocol",
            ),
        ] {
            let r = result(&report, id);
            assert_eq!(r.status, CheckStatus::Passed, "{id} : {}", r.output_tail);
            assert_eq!(
                r.output_tail.trim_end(),
                format!("WS=--workspace={ws} SLUG={slug}"),
                "argv développé attendu pour {id}"
            );
        }
        fx.cleanup();
    }

    /// Scenario `projet_sans_workspaces` — workspace unique `.`, argument
    /// `--workspace={workspace}` retiré.
    #[tokio::test]
    async fn projet_sans_workspaces() {
        let fx = Fixture::new("nows");
        std::fs::write(fx.root.join("package.json"), r#"{"name":"solo"}"#).expect("package.json");
        let opts = options(&fx);
        let mut expand_probe = probe(
            "probe",
            &[
                "sh",
                "-c",
                "printf 'A=[%s|%s]\\n' \"$0\" \"$1\"",
                "--workspace={workspace}",
            ],
        );
        expand_probe.expand = Some(Expand::NpmWorkspaces);
        let report = run_checks(&opts, &[expand_probe])
            .await
            .expect("rapport rendu");
        assert_eq!(report.checks.len(), 1);
        assert!(
            report.checks[0].id.starts_with("probe"),
            "id : {}",
            report.checks[0].id
        );
        // L'argument `--workspace=.` est retiré : `$0` reste le nom du shell, `$1` vide.
        assert!(
            !report.checks[0].output_tail.contains("--workspace"),
            "argument --workspace retiré, obtenu {:?}",
            report.checks[0].output_tail
        );
        assert_eq!(report.checks[0].output_tail.trim_end(), "A=[sh|]");
        fx.cleanup();
    }

    /// Scenario `aucun_dockerfile_suivi` — `skipped`, aucune commande lancée.
    #[tokio::test]
    async fn aucun_dockerfile_suivi() {
        let fx = Fixture::new("nodocker");
        init_repo(&fx.root);
        std::fs::write(fx.root.join("a"), "x\n").expect("a");
        git(&fx.root, &["add", "-A"]);
        git(&fx.root, &["commit", "-q", "-m", "c1"]);
        let opts = options(&fx);
        let dockerfile = crate::checks::preset_checks("dockerfile")
            .into_iter()
            .next()
            .expect("preset dockerfile");
        let report = run_checks(&opts, &[dockerfile])
            .await
            .expect("rapport rendu");
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.checks[0].id, "docker-hadolint");
        assert_eq!(report.checks[0].status, CheckStatus::Skipped);
        assert_eq!(
            report.status(),
            CheckStatus::Passed,
            "skipped n'altère pas status()"
        );
        fx.cleanup();
    }

    /// Test additionnel (clause Handles) : `Files` développé sur les fichiers suivis.
    #[tokio::test]
    async fn files_developpe_les_globs() {
        let fx = Fixture::new("files");
        init_repo(&fx.root);
        std::fs::write(fx.root.join("Dockerfile"), "FROM x\n").expect("Dockerfile");
        std::fs::create_dir_all(fx.root.join("img")).expect("img");
        std::fs::write(fx.root.join("img").join("Dockerfile.prod"), "FROM y\n").expect("prod");
        git(&fx.root, &["add", "-A"]);
        git(&fx.root, &["commit", "-q", "-m", "c1"]);
        let opts = options(&fx);
        let mut docker_probe = probe(
            "dock",
            &[
                "sh",
                "-c",
                "for f do printf 'F=%s\\n' \"$f\"; done",
                "x",
                "{files}",
            ],
        );
        docker_probe.kind = CheckKind::Lint;
        docker_probe.output = OutputKind::None;
        docker_probe.expand = Some(Expand::Files {
            globs: ["Dockerfile", "**/Dockerfile.*"]
                .iter()
                .map(|g| (*g).to_string())
                .collect(),
        });
        let report = run_checks(&opts, &[docker_probe])
            .await
            .expect("rapport rendu");
        let r = result(&report, "dock");
        assert_eq!(r.status, CheckStatus::Passed, "tail : {}", r.output_tail);
        assert!(
            r.output_tail.contains("F=Dockerfile\n"),
            "obtenu {:?}",
            r.output_tail
        );
        assert!(
            r.output_tail.contains("F=img/Dockerfile.prod"),
            "obtenu {:?}",
            r.output_tail
        );
        fx.cleanup();
    }

    /// Scenario `defauts_trouves_par_ruff_sont_failed_pas_errored`.
    #[tokio::test]
    async fn defauts_trouves_par_ruff_sont_failed_pas_errored() {
        let fx = Fixture::new("ruff");
        let diagnostics = serde_json::json!([{
            "code": "F401",
            "message": "'os' imported but unused",
            "filename": format!("{}/a.py", fx.root.display()),
            "location": { "row": 3, "column": 2 }
        }]);
        let fixture = fx.out.join("ruff.json");
        std::fs::write(&fixture, format!("{diagnostics}\n")).expect("fixture ruff");
        let opts = options(&fx);
        let script = format!("cat {:?}; exit 1", fixture.display().to_string());
        let mut ruff = probe("py-ruff", &["sh", "-c", &script]);
        ruff.kind = CheckKind::Lint;
        ruff.output = OutputKind::RuffJson;
        let report = run_checks(&opts, &[ruff]).await.expect("rapport rendu");
        assert_eq!(result(&report, "py-ruff").status, CheckStatus::Failed);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].check_id, "py-ruff");
        assert_eq!(report.issues[0].path, "a.py");
        assert_eq!(report.issues[0].line, 3);
        fx.cleanup();
    }

    /// Test additionnel (clause Handles) : `RuffJson` illisible ⇒ `errored`.
    #[tokio::test]
    async fn ruff_illisible_est_errored() {
        let fx = Fixture::new("ruffko");
        let opts = options(&fx);
        let mut ruff = probe("py-ruff", &["sh", "-c", "echo 'ceci n est pas du json'"]);
        ruff.kind = CheckKind::Lint;
        ruff.output = OutputKind::RuffJson;
        let report = run_checks(&opts, &[ruff]).await.expect("rapport rendu");
        assert_eq!(result(&report, "py-ruff").status, CheckStatus::Errored);
        fx.cleanup();
    }

    /// Scenario `langages_non_verifies_recopies`.
    #[tokio::test]
    async fn langages_non_verifies_recopies() {
        let fx = Fixture::new("unchecked");
        let mut opts = options(&fx);
        opts.unchecked_languages = vec!["go".to_string()];
        let report = run_checks(&opts, &[probe("p", &["true"])])
            .await
            .expect("rapport rendu");
        assert_eq!(report.unchecked_languages, vec!["go".to_string()]);
        assert_eq!(report.status(), CheckStatus::Passed);
        fx.cleanup();
    }

    /// Scenario `toutes_les_verifications_sont_rapportees_malgre_un_echec`.
    #[tokio::test]
    async fn toutes_les_verifications_sont_rapportees_malgre_un_echec() {
        let fx = Fixture::new("allfail");
        let opts = options(&fx);
        let report = run_checks(
            &opts,
            &[
                probe("first", &["false"]),
                probe("second", &["sh", "-c", "echo second"]),
                probe("third", &["true"]),
            ],
        )
        .await
        .expect("rapport rendu");
        let ids: Vec<&str> = report.checks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second", "third"], "ordre reçu conservé");
        assert_eq!(report.checks[0].status, CheckStatus::Failed);
        assert_eq!(report.checks[1].status, CheckStatus::Passed);
        assert_eq!(report.checks[2].status, CheckStatus::Passed);
        assert_eq!(report.status(), CheckStatus::Failed);
        fx.cleanup();
    }

    /// Scenario `parallelisme_borne` — jamais plus de `max_parallel` simultanées, durée
    /// proche de deux vagues d'une seconde.
    #[tokio::test]
    async fn parallelisme_borne() {
        let fx = Fixture::new("par");
        let conc = fx.out.join("conc");
        std::fs::create_dir_all(&conc).expect("conc");
        let mut opts = options(&fx);
        opts.max_parallel = 2;
        let mut checks = Vec::new();
        for i in 0..4_u32 {
            let s_file = conc.join(format!("s{i}"));
            let e_file = conc.join(format!("e{i}"));
            let c_file = conc.join(format!("c{i}"));
            // S'annonce (s{i}), compte les en-cours (s − e), puis travaille 1 s et
            // se retire (e{i}).
            let script = format!(
                "touch {s_file:?}; \
                 n=$(( $(ls -1 {conc:?} | grep -c '^s') - $(ls -1 {conc:?} | grep -c '^e') )); \
                 printf '%s\\n' \"$n\" > {c_file:?}; sleep 1; touch {e_file:?}"
            );
            checks.push(probe(&format!("p{i}"), &["sh", "-c", &script]));
        }
        let start = std::time::Instant::now();
        let report = run_checks(&opts, &checks).await.expect("rapport rendu");
        let elapsed = start.elapsed();
        for c in &report.checks {
            assert_eq!(
                c.status,
                CheckStatus::Passed,
                "{} : {}",
                c.id,
                c.output_tail
            );
        }
        for i in 0..4_u32 {
            let observed: u32 = std::fs::read_to_string(conc.join(format!("c{i}")))
                .expect("compteur lu")
                .trim()
                .parse()
                .expect("entier");
            assert!(observed <= 2, "pic de concurrence {observed} > 2 (p{i})");
        }
        assert!(
            elapsed >= std::time::Duration::from_millis(1900)
                && elapsed < std::time::Duration::from_secs(8),
            "durée attendue proche de 2 s, obtenu {elapsed:?}"
        );
        fx.cleanup();
    }

    /// Scenario `meme_repertoire_de_build_jamais_en_parallele` — groupe implicite
    /// `argv[0]`+`cwd` (aucun `serial_group` déclaré), `max_parallel = 4`.
    #[tokio::test]
    async fn meme_repertoire_de_build_jamais_en_parallele() {
        let fx = Fixture::new("groupe");
        let log = fx.out.join("log");
        let mut opts = options(&fx);
        opts.max_parallel = 4;
        let mut a = probe("a", &["true"]);
        a.serial_group = None;
        a.argv = vec![
            "sh".into(),
            "-c".into(),
            format!("printf 'a1\\n' >> {log:?}; sleep 0.6; printf 'a2\\n' >> {log:?}"),
        ];
        let mut b = probe("b", &["true"]);
        b.serial_group = None;
        b.argv = vec![
            "sh".into(),
            "-c".into(),
            format!("printf 'b1\\n' >> {log:?}; sleep 0.6; printf 'b2\\n' >> {log:?}"),
        ];
        let report = run_checks(&opts, &[a, b]).await.expect("rapport rendu");
        assert_eq!(report.status(), CheckStatus::Passed);
        let content = std::fs::read_to_string(&log).expect("log écrit");
        assert!(
            content == "a1\na2\nb1\nb2\n" || content == "b1\nb2\na1\na2\n",
            "exécutions entrelacées : {content:?}"
        );
        fx.cleanup();
    }

    /// Test additionnel (clause Handles) : `after` — la dépendance attend le préalable.
    #[tokio::test]
    async fn after_attend_le_prealable() {
        let fx = Fixture::new("apres");
        let log = fx.out.join("log");
        let mut opts = options(&fx);
        opts.max_parallel = 4;
        let a = probe(
            "a",
            &["sh", "-c", &format!("sleep 0.4; printf 'a\\n' >> {log:?}")],
        );
        let mut b = probe("b", &["sh", "-c", &format!("printf 'b\\n' >> {log:?}")]);
        b.after = Some("a".to_string());
        let report = run_checks(&opts, &[a, b]).await.expect("rapport rendu");
        assert_eq!(report.status(), CheckStatus::Passed);
        let content = std::fs::read_to_string(&log).expect("log écrit");
        assert_eq!(content, "a\nb\n", "b doit s'exécuter après a");
        fx.cleanup();
    }

    /// Scenario `prealable_errore_erore_le_dependant` — un préalable `errored` rend le
    /// dépendant `errored` en cascade (jamais `skipped`, réservé à « rien à vérifier »),
    /// sans le lancer ; la raison est consignée dans `output_tail` et un check indépendant
    /// passe quand même.
    #[tokio::test]
    async fn prealable_errore_erore_le_dependant() {
        let fx = Fixture::new("cascade");
        let marker = fx.out.join("b_ran");
        let opts = options(&fx);
        let a = probe("a", &["programme-inexistant-vnl-cascade"]);
        let mut b = probe("b", &["cp", "/etc/hostname", &marker.display().to_string()]);
        b.after = Some("a".to_string());
        let c = probe("c", &["true"]);
        let report = run_checks(&opts, &[a, b, c]).await.expect("rapport rendu");
        assert_eq!(
            result(&report, "a").status,
            CheckStatus::Errored,
            "préalable : spawn impossible ⇒ errored"
        );
        let rb = result(&report, "b");
        assert_eq!(
            rb.status,
            CheckStatus::Errored,
            "un préalable mort cascade en errored, jamais skipped"
        );
        assert!(
            rb.output_tail.contains("préalable"),
            "la raison est consignée dans output_tail : {:?}",
            rb.output_tail
        );
        assert!(!marker.exists(), "b ne doit pas avoir été lancé");
        assert_eq!(
            result(&report, "c").status,
            CheckStatus::Passed,
            "un check indépendant passe quand même"
        );
        fx.cleanup();
    }

    /// Test additionnel (checks.sdd Returns) : `{vitest_version}` introuvable ⇒ le
    /// provider est `errored` sans exécution.
    #[tokio::test]
    async fn vitest_version_absente_erore_le_provider() {
        let fx = Fixture::new("vitest");
        std::fs::write(
            fx.root.join("package.json"),
            r#"{"name":"racine","workspaces":["frontend"]}"#,
        )
        .expect("package.json");
        std::fs::create_dir_all(fx.root.join("frontend")).expect("frontend");
        let opts = options(&fx);
        let mut provider = probe(
            "ts-coverage-provider",
            &["npm", "true", "@vitest/coverage-v8@{vitest_version}"],
        );
        provider.expand = Some(Expand::NpmWorkspaces);
        let report = run_checks(&opts, &[provider]).await.expect("rapport rendu");
        let r = result(&report, "ts-coverage-provider:frontend");
        assert_eq!(
            r.status,
            CheckStatus::Errored,
            "vitest introuvable ⇒ errored"
        );
        assert!(
            r.output_tail.contains("vitest"),
            "la raison mentionne vitest : {:?}",
            r.output_tail
        );
        fx.cleanup();
    }

    /// Test additionnel (clause Handles) : lcov multiples fusionnés (même règle que lcov).
    #[tokio::test]
    async fn lcov_multiples_fusionnes() {
        let fx = Fixture::new("lcovmerge");
        let f1 = fx.out.join("d1.lcov");
        let f2 = fx.out.join("d2.lcov");
        std::fs::write(
            &f1,
            format!(
                "SF:{}/a.rs\nDA:1,1\nDA:2,0\nend_of_record\n",
                fx.root.display()
            ),
        )
        .expect("d1");
        std::fs::write(
            &f2,
            format!(
                "SF:{}/a.rs\nDA:3,1\nend_of_record\nSF:{}/b.rs\nDA:1,1\nend_of_record\n",
                fx.root.display(),
                fx.root.display()
            ),
        )
        .expect("d2");
        let opts = options(&fx);
        let mut c1 = probe(
            "c1",
            &["cp", &f1.display().to_string(), "{out_dir}/r1.lcov"],
        );
        c1.kind = CheckKind::Coverage;
        c1.output = OutputKind::Lcov {
            path: "r1.lcov".to_string(),
        };
        let mut c2 = probe(
            "c2",
            &["cp", &f2.display().to_string(), "{out_dir}/r2.lcov"],
        );
        c2.kind = CheckKind::Coverage;
        c2.output = OutputKind::Lcov {
            path: "r2.lcov".to_string(),
        };
        let report = run_checks(&opts, &[c1, c2]).await.expect("rapport rendu");
        let coverage = report.coverage.as_ref().expect("fusion produite");
        assert_eq!(coverage.files.len(), 2);
        let a = coverage
            .files
            .iter()
            .find(|f| f.path == "a.rs")
            .expect("a.rs");
        assert_eq!(a.lines_found, 3, "les DA du même fichier sont additionnés");
        assert_eq!(a.lines_hit, 2);
        fx.cleanup();
    }

    /// Scenario `diagnostics_lsp_preliminaires`.
    #[tokio::test]
    async fn diagnostics_lsp_preliminaires() {
        let fx = Fixture::new("lsp");
        let mut opts = options(&fx);
        opts.preliminary_issues = vec![Issue {
            check_id: "rust-analyzer".to_string(),
            path: "src/a.rs".to_string(),
            line: 7,
            column: 1,
            severity: Severity::Error,
            code: None,
            message: "expected type".to_string(),
        }];
        let report = run_checks(&opts, &[probe("p", &["true"])])
            .await
            .expect("rapport rendu");
        let lsp = result(&report, "lsp");
        assert_eq!(lsp.status, CheckStatus::Failed, "au moins une issue error");
        assert_eq!(lsp.kind, CheckKind::Lint);
        assert_eq!(lsp.duration_ms, 0);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].check_id, "rust-analyzer");
        assert_eq!(result(&report, "p").status, CheckStatus::Passed);
        assert_eq!(report.status(), CheckStatus::Failed);
        fx.cleanup();
    }

    /// Test additionnel (clause Handles) : `preliminary_issues` sans issue `error` ⇒ `lsp`
    /// `passed`.
    #[tokio::test]
    async fn lsp_sans_erreur_est_passed() {
        let fx = Fixture::new("lspwarn");
        let mut opts = options(&fx);
        opts.preliminary_issues = vec![Issue {
            check_id: "rust-analyzer".to_string(),
            path: "src/a.rs".to_string(),
            line: 7,
            column: 1,
            severity: Severity::Warning,
            code: None,
            message: "unused".to_string(),
        }];
        let report = run_checks(&opts, &[probe("p", &["true"])])
            .await
            .expect("rapport rendu");
        assert_eq!(result(&report, "lsp").status, CheckStatus::Passed);
        assert_eq!(report.status(), CheckStatus::Passed);
        fx.cleanup();
    }

    /// Test additionnel (clause Handles) : `niceness Some(10)` ⇒ les commandes tournent
    /// sous `nice -n 10` ; `None` ⇒ priorité normale.
    #[tokio::test]
    async fn niceness_wraps_command() {
        let fx = Fixture::new("nice");
        let mut opts = options(&fx);
        // `cut -d ' ' -f 19 /proc/self/stat` : champ « nice » du processus.
        let argv = ["sh", "-c", "cut -d ' ' -f 19 /proc/self/stat"];
        opts.niceness = Some(10);
        let report = run_checks(&opts, &[probe("n", &argv)])
            .await
            .expect("rapport rendu");
        assert_eq!(result(&report, "n").output_tail.trim(), "10");
        let mut opts = options(&fx);
        opts.niceness = None;
        let report = run_checks(&opts, &[probe("n", &argv)])
            .await
            .expect("rapport rendu");
        assert_eq!(result(&report, "n").output_tail.trim(), "0");
        fx.cleanup();
    }
}
