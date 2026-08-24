//! `GET /git/status` — parse de `git status --porcelain=v2 --branch`
//! exécuté dans `VNL_SANDBOX_ROOT`.

use std::fs;
use std::path::Path as StdPath;
use std::process::Command;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::AppState;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("VNL-SBX-004: git {args:?} failed with {status}: {stderr}")]
    CommandFailed {
        args: Vec<String>,
        status: String,
        stderr: String,
    },

    #[error("VNL-SBX-005: could not parse git status output at line {line_no}: {line:?}")]
    ParseFailed { line_no: usize, line: String },

    #[error("VNL-SBX-006: cannot determine unpushed commits: HEAD is detached")]
    DetachedHead,

    #[error("VNL-SBX-007: git diff failed for {path}: {status}: {stderr}")]
    DiffFailed {
        path: String,
        status: String,
        stderr: String,
    },

    #[error("VNL-SBX-008: nothing staged to commit")]
    EmptyCommit,

    #[error("VNL-SBX-011: checkout refused: working tree is dirty (branch: {branch})")]
    CheckoutRefused { branch: String },

    #[error("VNL-SBX-009: push rejected (non fast-forward): {stderr}")]
    PushRejected { stderr: String },

    #[error("VNL-SBX-010: git write failed (no credentials): {stderr}")]
    GitWriteFailed { stderr: String },

    #[error("VNL-SBX-015: path escapes sandbox root: {path}")]
    InvalidPath { path: String },

    #[error("VNL-SBX-016: ssh key provisioning failed: {stderr}")]
    SshKeyFailed { stderr: String },

    #[error("VNL-SBX-012: merge not started: {stderr}")]
    MergeNotStarted { stderr: String },

    #[error("VNL-SBX-013: no merge in progress to abort")]
    NoMergeInProgress,

    #[error("VNL-SBX-014: invalid ref/name {value:?}: must not start with '-'")]
    InvalidRef { value: String },
}

impl IntoResponse for GitError {
    fn into_response(self) -> Response {
        // Statut par défaut inchangé (500) pour toutes les variantes
        // préexistantes — hors périmètre de ce fix. InvalidRef est neuve et
        // correspond à une entrée utilisateur rejetée, donc 400.
        let status = match &self {
            GitError::InvalidRef { .. } => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({ "error": self.to_string() })),
        )
            .into_response()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub state: FileState,
    pub staged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GitStatus {
    pub branch: String,
    pub files: Vec<FileEntry>,
    pub clean: bool,
}

// ── Types pour /git/diff ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DiffParams {
    pub path: String, // requis ; manquant → 400 automatique par axum
    #[serde(default)]
    pub staged: Option<bool>, // absent/false → diff working tree ; true → diff index
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiffResponse {
    pub path: String, // le chemin utilisateur brut (celui reçu dans DiffParams)
    pub diff: String, // patch unifié texte (stdout de git diff)
}

// ── Types pour /git/stage / /git/unstage ────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct StageRequest {
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UnstageRequest {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OkResponse {
    pub ok: bool,
}

// ── Types pour /git/commit ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CommitRequest {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommitResponse {
    pub sha: String,   // SHA complet (40 hex), PAS tronqué à 7
    pub title: String, // première ligne du message
}

// ── Types pour /git/push ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PushResponse {
    pub ok: bool,    // true si succès
    pub pushed: u32, // nombre de commits poussés par cette opération
}

/// Classification du stderr d'un `git push` échoué.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushErrorKind {
    Rejected,   // rejet non fast-forward → PushRejected
    AuthFailed, // pas de credentials write → GitWriteFailed
    Other,      // tout autre échec → CommandFailed
}

#[derive(Debug, Deserialize)]
pub struct LogParams {
    pub limit: Option<u32>,
    pub all: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub title: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogResponse {
    pub branch: String,
    pub commits: Vec<LogCommit>,
    pub truncated: bool,
}

// ── Types pour /git/branches ───────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BranchEntry {
    pub name: String,             // nom court : "main", "origin/main", ...
    pub is_remote: bool,          // true si ref sous refs/remotes/
    pub upstream: Option<String>, // upstream:short ; None si pas d'upstream
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BranchesResponse {
    pub current: String, // `git rev-parse --abbrev-ref HEAD` (brut ; "HEAD" si détaché)
    pub merging: bool,   // présence de `.git/MERGE_HEAD`
    pub branches: Vec<BranchEntry>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBranchRequest {
    pub name: String,
    pub from: Option<String>, // ref de départ (branche locale ou remote) ; None → HEAD
}

#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    pub branch: String,
}

// ── Types pour /git/ssh-key ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SshKeyStatus {
    pub exists: bool,               // le fichier PRIVÉ existe
    pub public_key: Option<String>, // contenu trimé de `<priv>.pub` si lisible, sinon None
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SshKeyResponse {
    pub public_key: String, // contenu trimé de `<priv>.pub`
}

// ── Types pour /git/merge ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergeResponse {
    pub conflicted: bool, // true = conflit (résultat normal, réponse 200)
    pub sha: Option<String>, // SHA complet (40 hex) du merge auto-commité ;
                          // présenté seulement si conflicted=false
}

/// Chemins de la clé SSH sous `home` : (privée, publique).
/// `home/.ssh/id_ed25519` et `home/.ssh/id_ed25519.pub`.
pub fn ssh_key_paths(home: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let priv_path = home.join(".ssh").join("id_ed25519");
    let pub_path = priv_path.with_extension("pub");
    (priv_path, pub_path)
}

/// HOME du pod sandbox : `std::env::var("HOME")` si défini et non vide,
/// sinon `/home/vanyline` (repli — le pod Sandbox monte le PVC Owner à ce
/// chemin, cf. design section "Provisioning SSH"). Dépend de l'env.
pub fn ssh_home() -> std::path::PathBuf {
    std::env::var("HOME").map_or_else(
        |_| std::path::PathBuf::from("/home/vanyline"),
        |v| {
            if v.is_empty() {
                std::path::PathBuf::from("/home/vanyline")
            } else {
                std::path::PathBuf::from(v)
            }
        },
    )
}

/// Lit le contenu du fichier public (`.pub`) et le trim. Retourne `None` si le
/// fichier n'existe pas ou ne peut pas être lu.
fn read_public_key(priv_path: &std::path::Path) -> Option<String> {
    let pub_path = priv_path.with_extension("pub");
    fs::read_to_string(&pub_path)
        .map(|s| s.trim().to_string())
        .ok()
}

/// Une seule lettre de colonne porcelain v2 -> `FileState`. `'.'` (pas de
/// changement sur cette colonne) est géré par l'appelant, jamais passé ici.
/// `T` (typechange) est traité comme `Modified`, `C` (copie) comme
/// `Renamed` — le schéma JSON de cette feature n'a pas d'état dédié pour
/// ces deux cas (design `docs/features/ws11-sandbox-git.md`).
fn map_state_char(c: char) -> Option<FileState> {
    match c {
        'A' => Some(FileState::Added),
        'D' => Some(FileState::Deleted),
        'M' | 'T' => Some(FileState::Modified),
        'R' | 'C' => Some(FileState::Renamed),
        'U' => Some(FileState::Conflicted),
        _ => None,
    }
}

/// Parse la sortie de `git status --porcelain=v2 --branch`. Fonction pure,
/// aucune I/O — testable directement sur des chaînes de fixture.
pub fn parse_status(output: &str) -> Result<GitStatus, GitError> {
    let mut branch: Option<String> = None;
    let mut files = Vec::new();

    for (idx, line) in output.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_no = idx + 1;

        if let Some(rest) = line.strip_prefix("# branch.head ") {
            branch = Some(rest.to_string());
            continue;
        }
        if line.starts_with("# branch.") {
            // branch.oid / branch.upstream / branch.ab — pas nécessaires ici.
            continue;
        }

        let mut parts = line.splitn(2, ' ');
        let tag = parts.next().unwrap_or("");
        let remainder = parts.next().unwrap_or("");

        match tag {
            "1" => files.push(parse_ordinary(remainder, line_no, line)?),
            "2" => files.push(parse_rename(remainder, line_no, line)?),
            "u" => files.push(parse_unmerged(remainder, line_no, line)?),
            "?" => files.push(FileEntry {
                path: remainder.to_string(),
                state: FileState::Untracked,
                staged: false,
            }),
            "!" => continue, // entrées ignorées — --ignored non demandé, ignoré par prudence
            _ => {
                return Err(GitError::ParseFailed {
                    line_no,
                    line: line.to_string(),
                });
            }
        }
    }

    let branch = branch.ok_or_else(|| GitError::ParseFailed {
        line_no: 0,
        line: "missing '# branch.head' header in git status output".to_string(),
    })?;
    let clean = files.is_empty();

    Ok(GitStatus {
        branch,
        files,
        clean,
    })
}

/// `<XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>` (ligne type `1`, déjà
/// dépouillée de son `"1 "` initial).
fn parse_ordinary(remainder: &str, line_no: usize, full_line: &str) -> Result<FileEntry, GitError> {
    let fields: Vec<&str> = remainder.splitn(8, ' ').collect();
    match fields.as_slice() {
        [xy, _sub, _mh, _mi, _mw, _hh, _hi, path] => entry_from_xy(xy, path, line_no, full_line),
        _ => Err(GitError::ParseFailed {
            line_no,
            line: full_line.to_string(),
        }),
    }
}

/// `<XY> <sub> <mH> <mI> <mW> <hH> <hI> <Xscore> <path>\t<origPath>`
/// (ligne type `2`). `origPath` est jeté — le schéma de réponse n'a que
/// `path`.
fn parse_rename(remainder: &str, line_no: usize, full_line: &str) -> Result<FileEntry, GitError> {
    let fields: Vec<&str> = remainder.splitn(9, ' ').collect();
    match fields.as_slice() {
        [xy, _sub, _mh, _mi, _mw, _hh, _hi, _score, path_and_orig] => {
            let path = path_and_orig.split('\t').next().unwrap_or(path_and_orig);
            entry_from_xy(xy, path, line_no, full_line)
        }
        _ => Err(GitError::ParseFailed {
            line_no,
            line: full_line.to_string(),
        }),
    }
}

/// `<XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>` (ligne type `u`
/// — conflit). Toujours `Conflicted`/`staged: true` quelle que soit la
/// combinaison de lettres (AA, UU, DD...).
fn parse_unmerged(remainder: &str, line_no: usize, full_line: &str) -> Result<FileEntry, GitError> {
    let fields: Vec<&str> = remainder.splitn(10, ' ').collect();
    match fields.as_slice() {
        [_xy, _sub, _m1, _m2, _m3, _mw, _h1, _h2, _h3, path] => Ok(FileEntry {
            path: path.to_string(),
            state: FileState::Conflicted,
            staged: true,
        }),
        _ => Err(GitError::ParseFailed {
            line_no,
            line: full_line.to_string(),
        }),
    }
}

/// Mapping X/Y -> `FileEntry` partagé par les lignes ordinaires et de
/// renommage. `X` (colonne index/staged) l'emporte si différent de `.` ;
/// repli sur `Y` (colonne worktree) sinon.
fn entry_from_xy(
    xy: &str,
    path: &str,
    line_no: usize,
    full_line: &str,
) -> Result<FileEntry, GitError> {
    let mut chars = xy.chars();
    let (x, y) = match (chars.next(), chars.next()) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            return Err(GitError::ParseFailed {
                line_no,
                line: full_line.to_string(),
            });
        }
    };

    let (effective, staged) = if x != '.' { (x, true) } else { (y, false) };
    let state = map_state_char(effective).ok_or_else(|| GitError::ParseFailed {
        line_no,
        line: full_line.to_string(),
    })?;

    Ok(FileEntry {
        path: path.to_string(),
        state,
        staged,
    })
}

/// Exécute `git -C <sandbox_root> status --porcelain=v2 --branch` et parse
/// le résultat. Le `Command` bloquant tourne hors de l'executor tokio.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn run_status(sandbox_root: &StdPath) -> Result<GitStatus, GitError> {
    let root = sandbox_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let root_str = root.to_string_lossy().into_owned();
        let args: Vec<String> = [
            "-C".to_string(),
            root_str,
            "status".to_string(),
            "--porcelain=v2".to_string(),
            "--branch".to_string(),
        ]
        .into_iter()
        .collect();
        let output =
            Command::new("git")
                .args(&args)
                .output()
                .map_err(|e| GitError::CommandFailed {
                    args: args.clone(),
                    status: e.to_string(),
                    stderr: String::new(),
                })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(GitError::CommandFailed {
                args,
                status: output.status.to_string(),
                stderr,
            });
        }

        parse_status(&String::from_utf8_lossy(&output.stdout))
    })
    .await
    .expect("run_status blocking task panicked")
}

pub async fn handle_status(State(state): State<AppState>) -> Result<Json<GitStatus>, GitError> {
    run_status(&state.config.sandbox_root).await.map(Json)
}

// ── GET /git/unpushed ──────────────────────────────────────────────────────

const UNPUSHED_MAX_COMMITS: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommitEntry {
    pub sha: String,
    pub title: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnpushedStatus {
    pub branch: String,
    pub upstream: Option<String>,
    pub commits: Vec<CommitEntry>,
    pub truncated: bool,
}

/// Exécute une commande git et retourne stdout. Erreur `CommandFailed`
/// (VNL-SBX-004) si le process ne peut pas être lancé ou retourne un code
/// non nul. Doublon volontaire avec la logique inline de `run_status`
/// (tâche 02, déjà mergée) — ne pas refactorer `run_status` pour réutiliser
/// ce helper, ça toucherait du code déjà testé hors du périmètre de cette
/// tâche.
fn run_git(args: &[&str]) -> Result<String, GitError> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| GitError::CommandFailed {
            args: args.iter().map(|s| s.to_string()).collect(),
            status: e.to_string(),
            stderr: String::new(),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(GitError::CommandFailed {
            args: args.iter().map(|s| s.to_string()).collect(),
            status: output.status.to_string(),
            stderr,
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `git diff [--cached] --quiet` : propre (0) → `Ok(false)`, sale (1) →
/// `Ok(true)`, tout autre code de sortie ou échec de spawn → `Err`
/// (ne jamais retomber silencieusement sur "propre" quand on n'a pas pu
/// vérifier — cf. `handle_checkout`).
fn is_dirty(root: &StdPath, staged: bool) -> Result<bool, GitError> {
    let mut args = vec!["-C".to_string(), root.to_string_lossy().into_owned()];
    args.push("diff".to_string());
    if staged {
        args.push("--cached".to_string());
    }
    args.push("--quiet".to_string());
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let output = Command::new("git")
        .args(&refs)
        .output()
        .map_err(|e| GitError::CommandFailed {
            args: args.clone(),
            status: e.to_string(),
            stderr: String::new(),
        })?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(GitError::CommandFailed {
            args,
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }),
    }
}

/// Rejette une valeur qui commencerait par `-` avant qu'elle ne devienne un
/// argument positionnel de commande git (branche, nom, point de départ) —
/// sans ce garde-fou, une valeur comme `--abort` passée à `git merge`
/// (merge_args n'a pas de séparateur `--`, comme checkout_args/
/// create_branch_args/delete_branch_args) serait interprétée comme un flag
/// plutôt qu'une ref, avec des effets de bord (ex. annuler un merge en
/// cours au lieu d'en démarrer un). Ajouter `--` partout ne suffit pas :
/// `git checkout -- <x>` change le sens de `<x>` (pathspec, pas branche) —
/// la validation en amont est le fix uniforme.
fn reject_leading_dash(value: &str) -> Result<(), GitError> {
    if value.starts_with('-') {
        return Err(GitError::InvalidRef {
            value: value.to_string(),
        });
    }
    Ok(())
}

/// `true` si `refname` existe (`git show-ref --verify --quiet`).
fn ref_exists(worktree_root: &str, refname: &str) -> Result<bool, GitError> {
    let status = Command::new("git")
        .args([
            "-C",
            worktree_root,
            "show-ref",
            "--verify",
            "--quiet",
            refname,
        ])
        .status()
        .map_err(|e| GitError::CommandFailed {
            args: vec![
                "show-ref".into(),
                "--verify".into(),
                "--quiet".into(),
                refname.into(),
            ],
            status: e.to_string(),
            stderr: String::new(),
        })?;
    Ok(status.success())
}

/// Résout la "branche par défaut" du remote via le HEAD symbolique du
/// dépôt bare lui-même (pas celui du worktree) — cf. section Contexte.
/// Ne retourne jamais d'erreur : repli sur `"main"` (même convention que
/// `vanyline-maint`).
fn resolve_default_branch(worktree_root: &str) -> String {
    let Ok(common_dir) = run_git(&["-C", worktree_root, "rev-parse", "--git-common-dir"]) else {
        return "main".to_string();
    };
    let common_dir = common_dir.trim();
    match run_git(&["--git-dir", common_dir, "symbolic-ref", "--short", "HEAD"]) {
        Ok(d) => d.trim().to_string(),
        Err(_) => "main".to_string(),
    }
}

/// Parse la sortie de `git log <range> --pretty=format:"%H\x1f%s\x1f%an\x1f%aI"`
/// — une ligne par commit. Fonction pure — testable directement sur des
/// chaînes de fixture. Le SHA est tronqué à 7 caractères (convention fixe,
/// pas le `--abbrev` variable de git).
fn parse_commits(output: &str) -> Result<Vec<CommitEntry>, GitError> {
    let mut commits = Vec::new();
    for (idx, line) in output.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_no = idx + 1;
        let fields: Vec<&str> = line.split('\u{1f}').collect();
        match fields.as_slice() {
            [sha, title, author, date] => {
                commits.push(CommitEntry {
                    sha: sha.chars().take(7).collect(),
                    title: title.to_string(),
                    author: author.to_string(),
                    date: date.to_string(),
                });
            }
            _ => {
                return Err(GitError::ParseFailed {
                    line_no,
                    line: line.to_string(),
                });
            }
        }
    }
    Ok(commits)
}

/// Exécute la comparaison et retourne le résultat. `sandbox_root` est un
/// worktree normal (pas bare) — `VNL_SANDBOX_ROOT`.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn run_unpushed(sandbox_root: &StdPath) -> Result<UnpushedStatus, GitError> {
    let root = sandbox_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let root_str = root.to_string_lossy().into_owned();

        let branch = run_git(&["-C", &root_str, "rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .to_string();
        if branch == "HEAD" {
            return Err(GitError::DetachedHead);
        }

        let origin_branch_ref = format!("refs/remotes/origin/{branch}");
        let (upstream, compare_ref) = if ref_exists(&root_str, &origin_branch_ref)? {
            let up = format!("origin/{branch}");
            (Some(up.clone()), up)
        } else {
            let default = resolve_default_branch(&root_str);
            (None, format!("origin/{default}"))
        };

        let range = format!("{compare_ref}..HEAD");
        let max_count_arg = format!("--max-count={}", UNPUSHED_MAX_COMMITS + 1);
        let log_output = run_git(&[
            "-C",
            &root_str,
            "log",
            &range,
            &max_count_arg,
            "--pretty=format:%H\u{1f}%s\u{1f}%an\u{1f}%aI",
        ])?;

        let mut commits = parse_commits(&log_output)?;
        let truncated = commits.len() > UNPUSHED_MAX_COMMITS;
        commits.truncate(UNPUSHED_MAX_COMMITS);

        Ok(UnpushedStatus {
            branch,
            upstream,
            commits,
            truncated,
        })
    })
    .await
    .expect("run_unpushed blocking task panicked")
}

pub async fn handle_unpushed(
    State(state): State<AppState>,
) -> Result<Json<UnpushedStatus>, GitError> {
    run_unpushed(&state.config.sandbox_root).await.map(Json)
}

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// Args git pour `git diff [--staged] -- <path>` (sans `-C` — ajouté par
/// l'appelant). `path` est déjà le chemin relatif confiné.
fn diff_args(path: &str, staged: bool) -> Vec<String> {
    let mut args = vec!["diff".to_string()];
    if staged {
        args.push("--staged".to_string());
    }
    args.push("--".to_string());
    args.push(path.to_string());
    args
}

/// Args git pour `git add -- <paths...>` — chemins relatifs déjà confinés.
fn stage_args(paths: &[String]) -> Vec<String> {
    let mut args = vec!["add".to_string(), "--".to_string()];
    for p in paths {
        args.push(p.clone());
    }
    args
}

/// Args git pour `git restore --staged -- <paths...>`.
fn unstage_args(paths: &[String]) -> Vec<String> {
    let mut args = vec![
        "restore".to_string(),
        "--staged".to_string(),
        "--".to_string(),
    ];
    for p in paths {
        args.push(p.clone());
    }
    args
}

/// Args git pour `git merge <branch>` (sans `-C` — ajouté par l'appelant).
fn merge_args(branch: &str) -> Vec<String> {
    vec!["merge".to_string(), branch.to_string()]
}

/// Args git pour `git merge --abort`.
fn merge_abort_args() -> Vec<String> {
    vec!["merge".to_string(), "--abort".to_string()]
}

/// Parse la sortie de `git log -1 --pretty=format:%H%x1f%s` →
/// `(sha_complet, titre)`. Une ligne attendue ; `\u{1f}` sépare sha et titre.
/// Sortie vide → ParseFailed { line_no: 0, line: "empty output" }.
fn parse_commit_output(output: &str) -> Result<(String, String), GitError> {
    if output.is_empty() {
        return Err(GitError::ParseFailed {
            line_no: 0,
            line: "empty output".to_string(),
        });
    }
    // git log -… --quiet ne génère aucune ligne si l'index est vide ;
    // ici on est dans la branche où un commit existe.
    let line = output.lines().next().unwrap_or("");
    let fields: Vec<&str> = line.split('\u{1f}').collect();
    match fields.as_slice() {
        [sha, title] => Ok((sha.to_string(), title.to_string())),
        _ => Err(GitError::ParseFailed {
            line_no: 1,
            line: line.to_string(),
        }),
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /git/diff — diff working tree ou index pour un chemin.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_diff(
    State(state): State<AppState>,
    Query(params): Query<DiffParams>,
) -> Result<Json<DiffResponse>, GitError> {
    use crate::tools_impl::confine_path;

    let confined = confine_path(&state.config.sandbox_root, &params.path).map_err(|_| {
        GitError::InvalidPath {
            path: params.path.clone(),
        }
    })?;
    let rel = confined
        .strip_prefix(&state.config.sandbox_root)
        .map_err(|_e| GitError::InvalidPath {
            path: confined.to_string_lossy().into_owned(),
        })?
        .to_string_lossy()
        .into_owned();

    let path = params.path;
    let staged = params.staged == Some(true);
    let root = state.config.sandbox_root.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        let args: Vec<String> = ["-C".to_string(), root.to_string_lossy().into_owned()]
            .into_iter()
            .chain(diff_args(&rel, staged))
            .collect();
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_diff blocking task panicked");

    match result {
        Ok(stdout) => Ok(Json(DiffResponse { path, diff: stdout })),
        Err(GitError::CommandFailed {
            args: _,
            status,
            stderr,
        }) => Err(GitError::DiffFailed {
            path,
            status,
            stderr,
        }),
        Err(e) => Err(e),
    }
}

/// POST /git/stage — ajouter des chemins à l'index.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_stage(
    State(state): State<AppState>,
    Json(body): Json<StageRequest>,
) -> Result<Json<OkResponse>, GitError> {
    if body.paths.is_empty() {
        return Ok(Json(OkResponse { ok: true }));
    }

    let root = state.config.sandbox_root.to_path_buf();
    let confined_paths = body
        .paths
        .iter()
        .map(|p| {
            crate::tools_impl::confine_path(&root, p)
                .map_err(|_| GitError::InvalidPath { path: p.clone() })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let rels: Vec<String> = confined_paths
        .into_iter()
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    let root_ref = root.clone();
    tokio::task::spawn_blocking(move || {
        let args: Vec<String> = ["-C".to_string(), root_ref.to_string_lossy().into_owned()]
            .into_iter()
            .chain(stage_args(&rels))
            .collect();
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_stage blocking task panicked")?;

    Ok(Json(OkResponse { ok: true }))
}

/// POST /git/unstage — retirer des chemins de l'index.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_unstage(
    State(state): State<AppState>,
    Json(body): Json<UnstageRequest>,
) -> Result<Json<OkResponse>, GitError> {
    if body.paths.is_empty() {
        return Ok(Json(OkResponse { ok: true }));
    }

    let root = state.config.sandbox_root.to_path_buf();
    let confined_paths = body
        .paths
        .iter()
        .map(|p| {
            crate::tools_impl::confine_path(&root, p)
                .map_err(|_| GitError::InvalidPath { path: p.clone() })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let rels: Vec<String> = confined_paths
        .into_iter()
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    let root_ref = root.clone();
    tokio::task::spawn_blocking(move || {
        let args: Vec<String> = ["-C".to_string(), root_ref.to_string_lossy().into_owned()]
            .into_iter()
            .chain(unstage_args(&rels))
            .collect();
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_unstage blocking task panicked")?;

    Ok(Json(OkResponse { ok: true }))
}

/// POST /git/commit — commit les changements stagés.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_commit(
    State(state): State<AppState>,
    Json(body): Json<CommitRequest>,
) -> Result<Json<CommitResponse>, GitError> {
    // Clone root (used in three separate blocking tasks).
    let root = state.config.sandbox_root.to_path_buf();
    let message = body.message.clone();

    // 1. Vérifier s'il y a des changements stagés :
    // `git diff --cached --quiet` → code 0 = propre (Ok ""),
    // code 1 = des changes (Err CommandFailed).
    // On s'intéresse à l'exit code.
    let root_ref = root.clone();
    let has_staged = tokio::task::spawn_blocking(move || {
        let args: Vec<String> = [
            "-C".to_string(),
            root_ref.to_string_lossy().into_owned(),
            "diff".to_string(),
            "--cached".to_string(),
            "--quiet".to_string(),
        ]
        .into_iter()
        .collect();
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_commit check blocking task panicked");
    if has_staged.is_ok() {
        // git diff --cached --quiet est revenu avec code 0 : rien de stagé.
        return Err(GitError::EmptyCommit);
    }

    // 2. Commit : `git -C root commit -m <message>` (pas de `-a` implicite).
    let root_ref = root.clone();
    let msg = message.clone();
    tokio::task::spawn_blocking(move || {
        let args: Vec<String> = [
            "-C".to_string(),
            root_ref.to_string_lossy().into_owned(),
            "commit".to_string(),
            "-m".to_string(),
            msg,
        ]
        .into_iter()
        .collect();
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_commit commit blocking task panicked")?;

    // 3. `git -C root log -1 --pretty=format:%H%x1f%s` → parse_commit_output.
    let root_ref = root.clone();
    let log_output = tokio::task::spawn_blocking(move || {
        let args: Vec<String> = [
            "-C".to_string(),
            root_ref.to_string_lossy().into_owned(),
            "log".to_string(),
            "-1".to_string(),
            "--pretty=format:%H\u{1f}%s".to_string(),
        ]
        .into_iter()
        .collect();
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_commit log blocking task panicked")?;

    let (sha, title) = parse_commit_output(&log_output)?;

    Ok(Json(CommitResponse { sha, title }))
}

// ── Helpers purs (branches) ────────────────────────────────────────────────

/// Args git pour `git for-each-ref --format='%(refname)%09%(upstream:short)' refs/heads refs/remotes`.
fn branch_list_args() -> Vec<String> {
    vec![
        "for-each-ref".to_string(),
        "--format=%(refname)%09%(upstream:short)".to_string(),
        "refs/heads".to_string(),
        "refs/remotes".to_string(),
    ]
}

/// Args git pour `git branch <name> [<from>]`.
fn create_branch_args(name: &str, from: Option<&str>) -> Vec<String> {
    match from {
        Some(f) => vec!["branch".to_string(), name.to_string(), f.to_string()],
        None => vec!["branch".to_string(), name.to_string()],
    }
}

/// Args git pour `git checkout <branch>`.
fn checkout_args(branch: &str) -> Vec<String> {
    vec!["checkout".to_string(), branch.to_string()]
}

/// Args git pour `git branch -D <name>` (suppression forcée — choix du design,
/// pas de confirmation côté serveur).
fn delete_branch_args(name: &str) -> Vec<String> {
    vec!["branch".to_string(), "-D".to_string(), name.to_string()]
}

/// Parse la sortie de `git for-each-ref --format='%(refname)%09%(upstream:short)'`
/// `refs/heads refs/remotes` → `Vec<BranchEntry>`.
/// Une ligne = `refname\tupstream` (upstream vide si pas d'upstream).
/// - `refname` est COMPLET ("refs/heads/main", "refs/remotes/origin/main") :
///   `name` = refname sans le préfixe (`refs/heads/` ou `refs/remotes/`),
///   `is_remote` = refname commence par `refs/remotes/`.
/// - `upstream` vide → None.
///
/// Ligne sans `\t` → `ParseFailed { line_no, line }`.
/// Sortie vide → `Ok(vec![])`.
fn parse_branches(output: &str) -> Result<Vec<BranchEntry>, GitError> {
    if output.is_empty() {
        return Ok(Vec::new());
    }

    let mut branches = Vec::new();
    for (idx, line) in output.lines().enumerate() {
        let line_no = idx + 1;
        let parts: Vec<&str> = line.split('\t').collect();
        match parts.as_slice() {
            [_refname] => {
                return Err(GitError::ParseFailed {
                    line_no,
                    line: line.to_string(),
                });
            }
            [refname, upstream] => {
                let name = if refname.starts_with("refs/remotes/") {
                    refname
                        .strip_prefix("refs/remotes/")
                        .unwrap_or(refname)
                        .to_string()
                } else if refname.starts_with("refs/heads/") {
                    refname
                        .strip_prefix("refs/heads/")
                        .unwrap_or(refname)
                        .to_string()
                } else {
                    refname.to_string()
                };
                // upstream:short de git peut être refs/remotes/origin/main ;
                // on abrége en stripant refs/ (→ origin/main).
                let upstream_short = if upstream.starts_with("refs/remotes/") {
                    upstream
                        .strip_prefix("refs/remotes/")
                        .unwrap_or(upstream)
                        .to_string()
                } else if upstream.starts_with("refs/heads/") {
                    upstream
                        .strip_prefix("refs/heads/")
                        .unwrap_or(upstream)
                        .to_string()
                } else {
                    upstream.to_string()
                };
                branches.push(BranchEntry {
                    name,
                    is_remote: refname.starts_with("refs/remotes/"),
                    upstream: if upstream.is_empty() {
                        None
                    } else {
                        Some(upstream_short)
                    },
                });
            }
            _ => {
                return Err(GitError::ParseFailed {
                    line_no,
                    line: line.to_string(),
                });
            }
        }
    }
    Ok(branches)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /git/branches — liste des branches et statut de fusion.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_branches(
    State(state): State<AppState>,
) -> Result<Json<BranchesResponse>, GitError> {
    let root = state.config.sandbox_root.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        // 1. `git -C root rev-parse --abbrev-ref HEAD` → current
        let current = run_git(&[
            "-C",
            &root.to_string_lossy(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])?;
        let current = current.trim().to_string();

        // 2. merging = `git rev-parse --verify MERGE_HEAD` réussit → true
        let merging = Command::new("git")
            .args([
                "-C",
                &root.to_string_lossy(),
                "rev-parse",
                "--verify",
                "MERGE_HEAD",
            ])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);

        // 3. `git for-each-ref` → parse
        let list_args = branch_list_args();
        let list_args_prefixed: Vec<String> =
            ["-C".to_string(), root.to_string_lossy().into_owned()]
                .into_iter()
                .chain(list_args)
                .collect();
        let list_refs_prefixed: Vec<&str> = list_args_prefixed.iter().map(|s| s.as_str()).collect();
        let output = run_git(&list_refs_prefixed)?;
        let branches = parse_branches(&output)?;

        Ok((current, merging, branches))
    })
    .await
    .expect("handle_branches blocking task panicked");

    let (current, merging, branches) = result?;
    Ok(Json(BranchesResponse {
        current,
        merging,
        branches,
    }))
}

/// POST /git/branches — créer une nouvelle branche.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_create_branch(
    State(state): State<AppState>,
    Json(body): Json<CreateBranchRequest>,
) -> Result<Json<OkResponse>, GitError> {
    reject_leading_dash(&body.name)?;
    if let Some(from) = &body.from {
        reject_leading_dash(from)?;
    }
    let root = state.config.sandbox_root.to_path_buf();
    let name = body.name.clone();
    let from = body.from.clone();
    tokio::task::spawn_blocking(move || {
        let args = create_branch_args(&name, from.as_deref());
        let args_prefixed: Vec<String> = ["-C".to_string(), root.to_string_lossy().into_owned()]
            .into_iter()
            .chain(args)
            .collect();
        let refs: Vec<&str> = args_prefixed.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_create_branch blocking task panicked")?;

    Ok(Json(OkResponse { ok: true }))
}

/// POST /git/checkout — changer de branche avec vérification dirty tree.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_checkout(
    State(state): State<AppState>,
    Json(body): Json<CheckoutRequest>,
) -> Result<Json<OkResponse>, GitError> {
    reject_leading_dash(&body.branch)?;
    let branch = body.branch.clone();
    let root = state.config.sandbox_root.to_path_buf();

    // 1. Vérifier si le working tree est sale (avant checkout). `git diff
    // --quiet` : code 0 = propre, code 1 = sale, tout le reste (spawn
    // impossible, dépôt corrompu...) est une VRAIE erreur — surtout ne pas
    // la traiter comme "propre" (ça défairait le refus strict voulu par le
    // design : mieux vaut échouer bruyamment que laisser passer un
    // checkout sur un état qu'on n'a pas pu vérifier).
    let root_ref = root.clone();
    let dirty = tokio::task::spawn_blocking(move || -> Result<bool, GitError> {
        let dirty_worktree = is_dirty(&root_ref, false)?;
        let dirty_staged = is_dirty(&root_ref, true)?;
        Ok(dirty_worktree || dirty_staged)
    })
    .await
    .expect("handle_checkout dirty-check panicked")?;

    if dirty {
        return Err(GitError::CheckoutRefused { branch });
    }

    // 2. Aucun changement tracked/stagés : checkout direct.
    let root_ref = root.clone();
    let branch_ref = branch.clone();
    let result = tokio::task::spawn_blocking(move || {
        let args = checkout_args(&branch_ref);
        let root_owned: String = root_ref.to_string_lossy().into();
        let mut args_prefixed: Vec<String> = vec!["-C".to_string(), root_owned];
        args_prefixed.extend(args);
        let refs: Vec<&str> = args_prefixed.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_checkout blocking task panicked");

    result.map(|_| Json(OkResponse { ok: true }))
}

/// DELETE /git/branches/{name} — supprimer une branche (force).
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_delete_branch(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<OkResponse>, GitError> {
    reject_leading_dash(&name)?;
    let root = state.config.sandbox_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let args = delete_branch_args(&name);
        let args_prefixed: Vec<String> = ["-C".to_string(), root.to_string_lossy().into_owned()]
            .into_iter()
            .chain(args)
            .collect();
        let refs: Vec<&str> = args_prefixed.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_delete_branch blocking task panicked")?;

    Ok(Json(OkResponse { ok: true }))
}

// ── Helpers purs (push) ────────────────────────────────────────────────────

/// Args git pour `git push origin <refspec>` (refspec = nom court, ex "main").
fn push_args(refspec: &str) -> Vec<String> {
    vec![
        "push".to_string(),
        "origin".to_string(),
        refspec.to_string(),
    ]
}

/// Args git pour compter les commits à pousser.
/// `Some(refspec)` → `rev-list --count <refspec>..HEAD` (delta local/upstream) ;
/// `None` → `rev-list --count HEAD` (aucune ref cible côté remote : tous les commits).
fn count_pushed_args(refspec: Option<&str>) -> Vec<String> {
    match refspec {
        Some(r) => vec![
            "rev-list".to_string(),
            "--count".to_string(),
            format!("{r}..HEAD"),
        ],
        None => vec![
            "rev-list".to_string(),
            "--count".to_string(),
            "HEAD".to_string(),
        ],
    }
}

/// Args git pour `git log [--all] --max-count=<n+1> --pretty=format:...`
/// (sans `-C` — ajouté par l'appelant).
/// Format : `%H%x1f%P%x1f%D%x1f%s%x1f%an%x1f%aI` (6 champs séparés par \u{1f}).
/// `limit+1` pour détecter la troncature.
pub fn log_args(limit: u32, all: bool) -> Vec<String> {
    let mut args = vec![
        "log".to_string(),
        format!("--max-count={}", limit + 1),
        "--pretty=format:%H\u{1f}%P\u{1f}%D\u{1f}%s\u{1f}%an\u{1f}%aI".to_string(),
    ];
    if all {
        args.insert(1, "--all".to_string());
    }
    args
}

/// Parse UNE ligne de log git (6 champs séparés par \u{1f}).
/// Format attendu : `sha\u{1f}parents\u{1f}refs\u{1f}title\u{1f}author\u{1f}date`.
/// - `sha` : SHA complet (40 hex).
/// - `parents` : shas complets séparés par des espaces ; champ vide → vec![]
/// - `refs` : `%D` brut — split sur `", "` ; champ vide → vec![]
/// - `title` : `%s`
/// - `author` : `%an`
/// - `date` : `%aI` (strict ISO 8601)
///   Moins de 6 champs → `ParseFailed { line_no, line }`.
pub fn parse_log_line(line: &str, line_no: usize, full: &str) -> Result<LogCommit, GitError> {
    let fields: Vec<&str> = line.split('\u{1f}').collect();
    match fields.as_slice() {
        [sha, parents, refs, title, author, date] => {
            let parents_vec: Vec<String> = if parents.is_empty() {
                Vec::new()
            } else {
                parents
                    .split(' ')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect()
            };
            let refs_vec: Vec<String> = if refs.is_empty() {
                Vec::new()
            } else {
                refs.split(", ").map(|s| s.to_string()).collect()
            };
            Ok(LogCommit {
                sha: sha.to_string(),
                parents: parents_vec,
                refs: refs_vec,
                title: title.to_string(),
                author: author.to_string(),
                date: date.to_string(),
            })
        }
        _ => Err(GitError::ParseFailed {
            line_no,
            line: full.to_string(),
        }),
    }
}

/// Parse la sortie de `git log` (une ligne par commit) → Vec<LogCommit>.
/// Ligne vide → ignorée. Sortie vide → `Ok(vec![])`.
pub fn parse_log(output: &str) -> Result<Vec<LogCommit>, GitError> {
    let mut commits = Vec::new();
    for (idx, line) in output.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_no = idx + 1;
        commits.push(parse_log_line(line, line_no, output)?);
    }
    Ok(commits)
}

/// Parse la sortie de `git rev-list --count` → u32. Trim avant parse.
/// Entrée non numérique → ParseFailed { line_no: 1, line }.
fn parse_count(output: &str) -> Result<u32, GitError> {
    let trimmed = output.trim();
    trimmed.parse::<u32>().map_err(|_| GitError::ParseFailed {
        line_no: 1,
        line: trimmed.to_string(),
    })
}

/// Classification du stderr d'un `git push` échoué (heuristique).
pub fn classify_push_stderr(stderr: &str) -> PushErrorKind {
    if stderr.contains("rejected") || stderr.contains("non-fast-forward") {
        PushErrorKind::Rejected
    } else if stderr.contains("Authentication failed")
        || stderr.contains("Permission denied")
        || stderr.contains("Could not read from remote repository")
        || stderr.contains("Host key verification failed")
        || stderr.contains("failed to authenticate")
        || stderr.contains("unable to get credential")
    {
        PushErrorKind::AuthFailed
    } else {
        PushErrorKind::Other
    }
}

// ── Handler ──────────────────────────────────────────────────────────────────

/// POST /git/push — pousser la branche courante vers le remote origin.
///
/// Pattern shared avec les autres handlers du module :
/// `tokio::task::spawn_blocking` + `run_git`/`ref_exists`.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_push(State(state): State<AppState>) -> Result<Json<PushResponse>, GitError> {
    let root = state.config.sandbox_root.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        let root_str = root.to_string_lossy().into_owned();

        // 1. `git -C root rev-parse --abbrev-ref HEAD` → branch
        let branch = run_git(&["-C", &root_str, "rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .to_string();
        if branch == "HEAD" {
            return Err(GitError::DetachedHead);
        }

        // 2. Resolution de la cible comme /git/unpushed
        let origin_branch_ref = format!("refs/remotes/origin/{branch}");
        let target = if ref_exists(&root_str, &origin_branch_ref)? {
            branch.clone()
        } else {
            resolve_default_branch(&root_str)
        };

        // 3. pushed (AVANT push) — compter les commits a pousser.
        let origin_target_ref = format!("refs/remotes/origin/{target}");
        let pushed = if ref_exists(&root_str, &origin_target_ref)? {
            let count_args = count_pushed_args(Some(&format!("origin/{target}")));
            // Build: -C <root> rev-list --count origin/<target>..HEAD
            let full: Vec<String> = ["-C".to_string(), root_str.to_string()]
                .into_iter()
                .chain(count_args)
                .collect();
            let full_refs: Vec<&str> = full.iter().map(|s| s.as_str()).collect();
            let output = run_git(&full_refs)?;
            parse_count(&output)?
        } else {
            let count_args = count_pushed_args(None);
            let full: Vec<String> = ["-C".to_string(), root_str.to_string()]
                .into_iter()
                .chain(count_args)
                .collect();
            let full_refs: Vec<&str> = full.iter().map(|s| s.as_str()).collect();
            let output = run_git(&full_refs)?;
            parse_count(&output)?
        };

        // 4. git -C root push origin <target>
        let push_args_list = push_args(&target);
        let mut push_cmd = vec!["-C".to_string(), root_str.to_string()];
        push_cmd.extend(push_args_list);
        let push_refs: Vec<&str> = push_cmd.iter().map(|s| s.as_str()).collect();
        match run_git(&push_refs) {
            Ok(_) => {}
            Err(GitError::CommandFailed {
                args,
                status,
                stderr,
            }) => {
                let kind = classify_push_stderr(&stderr);
                return Err(match kind {
                    PushErrorKind::Rejected => GitError::PushRejected { stderr },
                    PushErrorKind::AuthFailed => GitError::GitWriteFailed { stderr },
                    PushErrorKind::Other => GitError::CommandFailed {
                        args,
                        status,
                        stderr,
                    },
                });
            }
            Err(e) => return Err(e),
        }

        // 5. Success
        Ok::<_, GitError>(PushResponse { ok: true, pushed })
    })
    .await
    .expect("handle_push blocking task panicked");

    result.map(Json)
}

/// GET /git/log — historique de la branche courante (ou `--all`).
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_log(
    State(state): State<AppState>,
    Query(params): Query<LogParams>,
) -> Result<Json<LogResponse>, GitError> {
    let root = state.config.sandbox_root.to_path_buf();
    let limit = params.limit.unwrap_or(100);
    let all = params.all == Some(true);

    let result = tokio::task::spawn_blocking(move || {
        let root_str = root.to_string_lossy().into_owned();

        // 1. Branch : `git -C root rev-parse --abbrev-ref HEAD`
        let branch = run_git(&["-C", &root_str, "rev-parse", "--abbrev-ref", "HEAD"])?;
        let branch = branch.trim().to_string();

        // 2. Git log : `--all` si demandé, `--max-count=limit+1` pour détecter troncature
        let args = log_args(limit, all);
        let args_prefixed: Vec<String> = ["-C".to_string(), root_str]
            .into_iter()
            .chain(args)
            .collect();
        let refs: Vec<&str> = args_prefixed.iter().map(|s| s.as_str()).collect();
        let output = run_git(&refs)?;

        // 3. Parse : `parse_log` → Vec<LogCommit>
        let mut commits = parse_log(&output)?;

        // 4. Détection troncature et truncation
        let truncated = commits.len() > limit as usize;
        commits.truncate(limit as usize);

        Ok((branch, commits, truncated))
    })
    .await
    .expect("handle_log blocking task panicked");

    let (branch, commits, truncated) = result?;
    Ok(Json(LogResponse {
        branch,
        commits,
        truncated,
    }))
}

// ── SSH key endpoints ──────────────────────────────────────────────────────

/// GET /git/ssh-key — check the SSH key status (idempotent, never errors if
/// the key does not exist — that is a normal state).
pub async fn handle_ssh_key_status(
    _state: State<AppState>,
) -> Result<Json<SshKeyStatus>, GitError> {
    let home = ssh_home();
    let (priv_path, pub_path) = ssh_key_paths(&home);
    let exists = priv_path.is_file();
    let public_key = if exists {
        read_public_key(&priv_path)
    } else {
        // Try to read anyway — the .pub might exist without the private key.
        fs::read_to_string(&pub_path)
            .ok()
            .map(|s| s.trim().to_string())
    };
    Ok(Json(SshKeyStatus { exists, public_key }))
}

/// POST /git/ssh-key — generate an ed25519 SSH keypair, idempotent.
///
/// If the private key already exists, reads and returns the public key without
/// regenerating. Otherwise creates `~/.ssh/id_ed25519` via `ssh-keygen`.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_ssh_key_create(
    _state: State<AppState>,
) -> Result<Json<SshKeyResponse>, GitError> {
    // state is unused — we always provision in the pod HOME

    let home = ssh_home();
    let (priv_path, pub_path) = ssh_key_paths(&home);

    // Idempotent: private key already exists → return current public key
    if priv_path.is_file() {
        let public_key = fs::read_to_string(&pub_path)
            .map_err(|e| GitError::SshKeyFailed {
                stderr: format!("could not read public key: {e}"),
            })?
            .trim()
            .to_string();
        return Ok(Json(SshKeyResponse { public_key }));
    }

    // Create ~/.ssh directory
    let ssh_dir = priv_path.parent().ok_or_else(|| GitError::SshKeyFailed {
        stderr: "could not determine .ssh directory parent".to_string(),
    })?;
    fs::create_dir_all(ssh_dir).map_err(|e| GitError::SshKeyFailed {
        stderr: format!("could not create .ssh directory: {e}"),
    })?;

    // Generate keypair via ssh-keygen (blocking I/O)
    let priv_abs = priv_path.to_string_lossy().into_owned();
    let result = tokio::task::spawn_blocking(move || {
        let output = Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-f", &priv_abs])
            .output()
            .map_err(|e| GitError::SshKeyFailed {
                stderr: format!("could not execute ssh-keygen: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(GitError::SshKeyFailed { stderr });
        }
        Ok(output)
    })
    .await
    .expect("handle_ssh_key_create blocking task panicked");

    _ = result?;

    // Read public key
    let public_key = fs::read_to_string(&pub_path).map_err(|e| GitError::SshKeyFailed {
        stderr: format!("could not read public key after generation: {e}"),
    })?;

    Ok(Json(SshKeyResponse {
        public_key: public_key.trim().to_string(),
    }))
}

/// POST /git/merge — lancer un merge de la branche spécifiée.
///
/// Exit code 0 → succès auto-commité (réponse 200 avec SHA).
/// Exit code 1 → conflit (réponse 200 avec `conflicted: true`, **pas une erreur HTTP**).
/// Tout autre code → `MergeNotStarted` (erreur HTTP VNL-SBX-012).
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_merge(
    State(state): State<AppState>,
    Json(body): Json<MergeRequest>,
) -> Result<Json<MergeResponse>, GitError> {
    reject_leading_dash(&body.branch)?;
    let root = state.config.sandbox_root.to_path_buf();
    let branch = body.branch.clone();

    #[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
    let result = tokio::task::spawn_blocking(move || {
        let root_str = root.to_string_lossy().into_owned();

        // 1. Exécuter `git -C root merge <branch>` et récupérer le code de sortie.
        let args = merge_args(&branch);
        let cmd_args: Vec<String> = ["-C".to_string(), root_str.clone()]
            .into_iter()
            .chain(args)
            .collect();
        let refs: Vec<&str> = cmd_args.iter().map(|s| s.as_str()).collect();
        let output =
            Command::new("git")
                .args(&refs)
                .output()
                .map_err(|e| GitError::CommandFailed {
                    args: refs.into_iter().map(|s| s.to_string()).collect(),
                    status: e.to_string(),
                    stderr: String::new(),
                })?;

        match output.status.code() {
            // 2. Exit code 0 → succès (merge auto-commité par git).
            Some(0) => {
                let sha_output = Command::new("git")
                    .args(["-C", &root_str, "rev-parse", "HEAD"])
                    .output()
                    .map_err(|e| GitError::CommandFailed {
                        args: vec!["-C".into(), root_str, "rev-parse".into(), "HEAD".into()],
                        status: e.to_string(),
                        stderr: String::new(),
                    })?;
                let sha = String::from_utf8_lossy(&sha_output.stdout)
                    .trim()
                    .to_string();
                Ok(MergeResponse {
                    conflicted: false,
                    sha: Some(sha),
                })
            }
            // 3. Exit code 1 → conflit (RÉSULTAT NORMAL, pas une erreur).
            Some(1) => Ok(MergeResponse {
                conflicted: true,
                sha: None,
            }),
            // 4. Tout autre code → merge non démarré (erreur HTTP VNL-SBX-012).
            _ => {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                Err(GitError::MergeNotStarted { stderr })
            }
        }
    })
    .await
    .expect("handle_merge blocking task panicked");

    result.map(Json)
}

/// POST /git/merge/abort — annuler un merge en cours.
///
/// Succès → réponse 200 avec `{ ok: true }`.
/// Échec (aucun merge en cours) → `NoMergeInProgress` (erreur HTTP VNL-SBX-013).
#[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
pub async fn handle_merge_abort(
    State(state): State<AppState>,
) -> Result<Json<OkResponse>, GitError> {
    let root = state.config.sandbox_root.to_path_buf();

    #[allow(clippy::expect_used)] // JoinError signifie un panic interne, pas une erreur git normale
    let result = tokio::task::spawn_blocking(move || {
        let args = merge_abort_args();
        let args_prefixed: Vec<String> = ["-C".to_string(), root.to_string_lossy().into_owned()]
            .into_iter()
            .chain(args)
            .collect();
        let refs: Vec<&str> = args_prefixed.iter().map(|s| s.as_str()).collect();
        run_git(&refs)
    })
    .await
    .expect("handle_merge_abort blocking task panicked");

    // Exécution de `run_git` → CommandFailed (non-zero exit) signifie aucun merge en cours.
    match result {
        Ok(_) => Ok(Json(OkResponse { ok: true })),
        Err(GitError::CommandFailed { .. }) => Err(GitError::NoMergeInProgress),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn clean_output() -> &'static str {
        "# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
         # branch.head main\n"
    }

    #[test]
    fn reject_leading_dash_rejects_flag_like_values() {
        assert!(reject_leading_dash("--abort").is_err());
        assert!(reject_leading_dash("-f").is_err());
        assert!(reject_leading_dash("-").is_err());
    }

    #[test]
    fn reject_leading_dash_accepts_normal_refs() {
        assert!(reject_leading_dash("feature-x").is_ok());
        assert!(reject_leading_dash("feature/x").is_ok());
        assert!(reject_leading_dash("main").is_ok());
    }

    #[test]
    fn is_dirty_clean_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Command::new("git")
            .args(["-C", &root.to_string_lossy(), "init", "-q"])
            .output()
            .unwrap();
        Command::new("git")
            .args([
                "-C",
                &root.to_string_lossy(),
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "init",
            ])
            .output()
            .unwrap();
        assert!(!is_dirty(root, false).unwrap());
        assert!(!is_dirty(root, true).unwrap());
    }

    #[test]
    fn is_dirty_modified_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Command::new("git")
            .args(["-C", &root.to_string_lossy(), "init", "-q"])
            .output()
            .unwrap();
        fs::write(root.join("a.txt"), "hello").unwrap();
        Command::new("git")
            .args(["-C", &root.to_string_lossy(), "add", "a.txt"])
            .output()
            .unwrap();
        Command::new("git")
            .args(["-C", &root.to_string_lossy(), "commit", "-q", "-m", "init"])
            .output()
            .unwrap();
        fs::write(root.join("a.txt"), "changed").unwrap();
        assert!(is_dirty(root, false).unwrap());
        assert!(!is_dirty(root, true).unwrap());
    }

    #[test]
    fn is_dirty_errors_on_non_git_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_dirty(dir.path(), false).is_err());
    }

    #[test]
    fn ssh_key_paths_nominal() {
        let (priv_path, pub_path) = ssh_key_paths(std::path::Path::new("/home/vanyline"));
        assert_eq!(
            priv_path,
            std::path::PathBuf::from("/home/vanyline/.ssh/id_ed25519")
        );
        assert_eq!(
            pub_path,
            std::path::PathBuf::from("/home/vanyline/.ssh/id_ed25519.pub")
        );
    }

    #[test]
    fn clean_repo() {
        let result = parse_status(clean_output()).unwrap();
        assert_eq!(result.branch, "main");
        assert!(result.files.is_empty());
        assert!(result.clean);
    }

    #[test]
    fn staged_added() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
# branch.head main\n\
1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 7864480e4202ea42b4c55eec46eff4cb8987a582 staged_add.txt\n\
";
        let result = parse_status(output).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "staged_add.txt");
        assert_eq!(result.files[0].state, FileState::Added);
        assert!(result.files[0].staged);
    }

    #[test]
    fn unstaged_modified() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
# branch.head main\n\
1 .M N... 100644 100644 100644 78981922613b2afb6025042ff6bd878ac1994e85 78981922613b2afb6025042ff6bd878ac1994e85 tracked.txt\n\
";
        let result = parse_status(output).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "tracked.txt");
        assert_eq!(result.files[0].state, FileState::Modified);
        assert!(!result.files[0].staged);
    }

    #[test]
    fn staged_and_unstaged_modified() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
# branch.head main\n\
1 MM N... 100644 100644 100644 76b5eb87f1cb631b7ae3229e54551d5de3530edb 5b33df4f36142872f4b5bb89fa44e08d8ad4594f tracked.txt\n\
";
        let result = parse_status(output).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "tracked.txt");
        assert_eq!(result.files[0].state, FileState::Modified);
        assert!(result.files[0].staged);
    }

    #[test]
    fn renamed() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
# branch.head main\n\
2 R. N... 100644 100644 100644 f2ad6c76f0115a6ba5b00456a849810e7ec0af20 f2ad6c76f0115a6ba5b00456a849810e7ec0af20 R100 to_rename_new.txt\tto_rename_old.txt\n\
";
        let result = parse_status(output).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "to_rename_new.txt");
        assert_eq!(result.files[0].state, FileState::Renamed);
    }

    #[test]
    fn untracked() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
# branch.head main\n\
? untracked.txt\n\
";
        let result = parse_status(output).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "untracked.txt");
        assert_eq!(result.files[0].state, FileState::Untracked);
        assert!(!result.files[0].staged);
    }

    #[test]
    fn conflicted() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
# branch.head main\n\
u AA N... 000000 100644 100644 100644 0000000000000000000000000000000000000000 223b7836fb19fdf64ba2d3cd6173c6a283141f78 f70f10e4db19068f79bc43844b49f3eece45c4e8 conflict.txt\n\
";
        let result = parse_status(output).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "conflict.txt");
        assert_eq!(result.files[0].state, FileState::Conflicted);
        assert!(result.files[0].staged);
    }

    #[test]
    fn detached_head() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
# branch.head (detached)\n\
";
        let result = parse_status(output).unwrap();
        assert_eq!(result.branch, "(detached)");
        assert!(result.files.is_empty());
    }

    #[test]
    fn unrecognized_line_is_parse_error() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
# branch.head main\n\
X foo\n\
";
        let result = parse_status(output);
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::ParseFailed { line_no, line } => {
                assert_eq!(line_no, 3);
                assert_eq!(line, "X foo");
            }
            _ => panic!("expected ParseFailed"),
        }
    }

    #[test]
    fn missing_branch_head_is_parse_error() {
        let output = "\
# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
";
        let result = parse_status(output);
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::ParseFailed { line_no, line } => {
                assert_eq!(line_no, 0);
                assert_eq!(line, "missing '# branch.head' header in git status output");
            }
            _ => panic!("expected ParseFailed"),
        }
    }

    // ── parse_commits (unpushed) ───────────────────────────────────────────

    #[test]
    fn parse_commits_nominal() {
        let sep = '\u{1f}';
        let output = format!(
            "abcdef123456{sep}Initial commit{sep}Alice{sep}2024-01-01T10:00:00Z\n\
             fedcba987654{sep}Second commit{sep}Bob{sep}2024-01-02T14:30:00Z",
            sep = sep,
        );
        let result = parse_commits(&output).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].sha, "abcdef1");
        assert_eq!(result[0].title, "Initial commit");
        assert_eq!(result[0].author, "Alice");
        assert_eq!(result[0].date, "2024-01-01T10:00:00Z");
        assert_eq!(result[1].sha, "fedcba9");
        assert_eq!(result[1].title, "Second commit");
        assert_eq!(result[1].author, "Bob");
        assert_eq!(result[1].date, "2024-01-02T14:30:00Z");
    }

    #[test]
    fn parse_commits_empty_output() {
        let result = parse_commits("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_commits_malformed_line_is_parse_error() {
        let result = parse_commits("abc123 no separator");
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::ParseFailed { line_no, line } => {
                assert_eq!(line_no, 1);
                assert_eq!(line, "abc123 no separator");
            }
            _ => panic!("expected ParseFailed"),
        }
    }

    // ── Helpers git (diff/stage/unstage/commit) ────────────────────────────

    #[test]
    fn diff_args_staged() {
        assert_eq!(
            diff_args("a/b.txt", true),
            vec!["diff", "--staged", "--", "a/b.txt"]
        );
        assert_eq!(diff_args("a/b.txt", false), vec!["diff", "--", "a/b.txt"]);
    }

    #[test]
    fn stage_args_include_paths() {
        assert_eq!(
            stage_args(&["a.txt".to_string(), "b/c.txt".to_string()]),
            vec!["add", "--", "a.txt", "b/c.txt"]
        );
    }

    #[test]
    fn unstage_args_include_paths() {
        assert_eq!(
            unstage_args(&["a.txt".to_string()]),
            vec!["restore", "--staged", "--", "a.txt"]
        );
    }

    #[test]
    fn parse_commit_output_nominal() {
        let input = "dce41965c9aa085042cef737c1eaa4141a055b5a\u{1f}Initial commit";
        let result = parse_commit_output(input).unwrap();
        assert_eq!(
            result,
            (
                "dce41965c9aa085042cef737c1eaa4141a055b5a".to_string(),
                "Initial commit".to_string()
            )
        );
    }

    #[test]
    fn parse_commit_output_malformed() {
        let result = parse_commit_output("no separator");
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::ParseFailed { line_no, line } => {
                assert_eq!(line_no, 1);
                assert_eq!(line, "no separator");
            }
            _ => panic!("expected ParseFailed"),
        }
    }

    #[test]
    fn parse_commit_output_empty() {
        let result = parse_commit_output("");
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::ParseFailed { line_no, line } => {
                assert_eq!(line_no, 0);
                assert_eq!(line, "empty output");
            }
            _ => panic!("expected ParseFailed"),
        }
    }

    // ── Helpers branches ─────────────────────────────────────────────────────

    #[test]
    fn create_branch_args_with_from() {
        assert_eq!(
            create_branch_args("feat/x", Some("main")),
            vec!["branch", "feat/x", "main"]
        );
        assert_eq!(create_branch_args("feat/x", None), vec!["branch", "feat/x"]);
    }

    #[test]
    fn checkout_args_nominal() {
        assert_eq!(checkout_args("main"), vec!["checkout", "main"]);
    }

    #[test]
    fn delete_branch_args_force() {
        assert_eq!(delete_branch_args("feat/x"), vec!["branch", "-D", "feat/x"]);
    }

    #[test]
    fn parse_branches_nominal() {
        let input = "refs/heads/main\trefs/remotes/origin/main\nrefs/heads/feat/x\t\nrefs/remotes/origin/main\t\n";
        let result = parse_branches(input).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "main");
        assert!(!result[0].is_remote);
        assert_eq!(result[0].upstream, Some("origin/main".to_string()));
        assert_eq!(result[1].name, "feat/x");
        assert!(!result[1].is_remote);
        assert_eq!(result[1].upstream, None);
        assert_eq!(result[2].name, "origin/main");
        assert!(result[2].is_remote);
        assert_eq!(result[2].upstream, None);
    }

    #[test]
    fn parse_branches_empty() {
        let result = parse_branches("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_branches_malformed() {
        let result = parse_branches("refs/heads/main\n");
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::ParseFailed { line_no, line } => {
                assert_eq!(line_no, 1);
                assert_eq!(line, "refs/heads/main");
            }
            _ => panic!("expected ParseFailed"),
        }
    }

    // ── Helpers push ──────────────────────────────────────────────────────────

    #[test]
    fn push_args_nominal() {
        assert_eq!(push_args("main"), vec!["push", "origin", "main"]);
    }

    #[test]
    fn count_pushed_args_with_refspec() {
        let r = "origin/main";
        assert_eq!(
            count_pushed_args(Some(r)),
            vec!["rev-list", "--count", "origin/main..HEAD"]
        );
        assert_eq!(count_pushed_args(None), vec!["rev-list", "--count", "HEAD"]);
    }

    #[test]
    fn parse_count_nominal() {
        assert_eq!(parse_count("3\n").unwrap(), 3);
        assert_eq!(parse_count("0").unwrap(), 0);
    }

    #[test]
    fn parse_count_non_numeric() {
        let result = parse_count("abc");
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::ParseFailed { line_no, line } => {
                assert_eq!(line_no, 1);
                assert_eq!(line, "abc");
            }
            _ => panic!("expected ParseFailed"),
        }
    }

    #[test]
    fn classify_push_stderr_rejected() {
        assert!(matches!(
            classify_push_stderr("remote: To prevent you from losing history, non-fast-forward"),
            PushErrorKind::Rejected
        ));
    }

    #[test]
    fn classify_push_stderr_auth() {
        assert!(matches!(
            classify_push_stderr("Authentication failed"),
            PushErrorKind::AuthFailed
        ));
        assert!(matches!(
            classify_push_stderr("Permission denied"),
            PushErrorKind::AuthFailed
        ));
    }

    #[test]
    fn classify_push_stderr_other() {
        assert!(matches!(
            classify_push_stderr("fatal: bad config"),
            PushErrorKind::Other
        ));
    }

    // ── Helpers log ──────────────────────────────────────────────────────────

    #[test]
    fn log_args_all_true() {
        assert_eq!(
            log_args(100, true),
            vec![
                "log".to_string(),
                "--all".to_string(),
                "--max-count=101".to_string(),
                "--pretty=format:%H\u{1f}%P\u{1f}%D\u{1f}%s\u{1f}%an\u{1f}%aI".to_string(),
            ]
        );
        assert_eq!(
            log_args(100, false),
            vec![
                "log".to_string(),
                "--max-count=101".to_string(),
                "--pretty=format:%H\u{1f}%P\u{1f}%D\u{1f}%s\u{1f}%an\u{1f}%aI".to_string(),
            ]
        );
    }

    #[test]
    fn parse_log_nominal() {
        let sep = '\u{1f}';
        let line = format!(
            "dce41965c9aa085042cef737c1eaa4141a055b5a{sep}\
             abc1111def2222 3333aaaa4444{sep}\
             HEAD, origin/main{sep}\
             Initial commit{sep}\
             Alice{sep}\
             2024-01-01T10:00:00Z",
            sep = sep,
        );
        let result = parse_log(&line).unwrap();
        assert_eq!(result.len(), 1);
        let commit = &result[0];
        assert_eq!(commit.sha, "dce41965c9aa085042cef737c1eaa4141a055b5a");
        assert_eq!(
            commit.parents,
            vec!["abc1111def2222".to_string(), "3333aaaa4444".to_string()]
        );
        assert_eq!(
            commit.refs,
            vec!["HEAD".to_string(), "origin/main".to_string()]
        );
        assert_eq!(commit.title, "Initial commit");
        assert_eq!(commit.author, "Alice");
        assert_eq!(commit.date, "2024-01-01T10:00:00Z");
    }

    #[test]
    fn parse_log_root_commit() {
        let sep = '\u{1f}';
        let line = format!(
            "dce41965c9aa085042cef737c1eaa4141a055b5a{sep}{sep}\
             {sep}Initial commit{sep}Alice{sep}2024-01-01T10:00:00Z",
            sep = sep,
        );
        let result = parse_log(&line).unwrap();
        assert_eq!(result.len(), 1);
        let commit = &result[0];
        assert_eq!(commit.sha, "dce41965c9aa085042cef737c1eaa4141a055b5a");
        assert!(commit.parents.is_empty());
        assert!(commit.refs.is_empty());
        assert_eq!(commit.title, "Initial commit");
        assert_eq!(commit.author, "Alice");
        assert_eq!(commit.date, "2024-01-01T10:00:00Z");
    }

    #[test]
    fn parse_log_malformed() {
        let result = parse_log("abc no separator");
        assert!(result.is_err());
        match result.unwrap_err() {
            GitError::ParseFailed { line_no, line } => {
                assert_eq!(line_no, 1);
                assert_eq!(line, "abc no separator");
            }
            _ => panic!("expected ParseFailed"),
        }
    }

    #[test]
    fn parse_log_empty() {
        let result = parse_log("").unwrap();
        assert!(result.is_empty());
    }

    // ── Helpers merge ────────────────────────────────────────────────────────

    #[test]
    fn merge_args_nominal() {
        assert_eq!(merge_args("feature-x"), vec!["merge", "feature-x"]);
    }

    #[test]
    fn merge_abort_args_nominal() {
        assert_eq!(merge_abort_args(), vec!["merge", "--abort"]);
    }
}
