//! `GET /git/status` — parse de `git status --porcelain=v2 --branch`
//! exécuté dans `VNL_SANDBOX_ROOT`.

use std::path::Path;
use std::process::Command;

use axum::{
    Json,
    extract::{State, Query},
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

    #[error("VNL-SBX-015: path escapes sandbox root: {path}")]
    InvalidPath { path: String },
}

impl IntoResponse for GitError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
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
    pub path: String,          // requis ; manquant → 400 automatique par axum
    #[serde(default)]
    pub staged: Option<bool>,  // absent/false → diff working tree ; true → diff index
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiffResponse {
    pub path: String,   // le chemin utilisateur brut (celui reçu dans DiffParams)
    pub diff: String,   // patch unifié texte (stdout de git diff)
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
    pub sha: String,    // SHA complet (40 hex), PAS tronqué à 7
    pub title: String,  // première ligne du message
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
pub async fn run_status(sandbox_root: &Path) -> Result<GitStatus, GitError> {
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
pub async fn run_unpushed(sandbox_root: &Path) -> Result<UnpushedStatus, GitError> {
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
    let mut args = vec!["restore".to_string(), "--staged".to_string(), "--".to_string()];
    for p in paths {
        args.push(p.clone());
    }
    args
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
        GitError::InvalidPath { path: params.path.clone() }
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
        Ok(stdout) => Ok(Json(DiffResponse {
            path,
            diff: stdout,
        })),
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
    let has_staged =
        tokio::task::spawn_blocking(move || {
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
    let log_output =
        tokio::task::spawn_blocking(move || {
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

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn clean_output() -> &'static str {
        "# branch.oid dce41965c9aa085042cef737c1eaa4141a055b5a\n\
         # branch.head main\n"
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
        assert_eq!(
            diff_args("a/b.txt", false),
            vec!["diff", "--", "a/b.txt"]
        );
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
            ("dce41965c9aa085042cef737c1eaa4141a055b5a".to_string(), "Initial commit".to_string())
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
}
