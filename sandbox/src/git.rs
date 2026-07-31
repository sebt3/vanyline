//! `GET /git/status` — parse de `git status --porcelain=v2 --branch`
//! exécuté dans `VNL_SANDBOX_ROOT`.

use std::path::Path;
use std::process::Command;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

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
        let output = Command::new("git")
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

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
}