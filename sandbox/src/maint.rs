//! Logique de l'utilitaire `vanyline-maint` : maintenance des workspaces git
//! des Projects (clone bare, fetch, purge). Invoqué par les Jobs du controller
//! avec des arguments en argv — jamais via un shell.

use std::path::Path;
use std::process::Command;

/// Répertoire (relatif à la racine du workspace) du clone bare.
/// Doit rester identique à `controller/src/project.rs::bare_repo_path`.
pub const BARE_REPO_DIR: &str = "repo.git";

/// Répertoire (relatif) des worktrees. Utilisé par `checkout`/`remove`
/// (tâche suivante) et par `purge` (cette tâche).
pub const WORKTREES_DIR: &str = "worktrees";

/// Répertoire (relatif) racine des caches.
pub const CACHE_DIR: &str = "cache";

/// Nom du répertoire de cache pour un identifiant donné.
/// Doit rester identique à `controller/src/project.rs::cache_dir_name`.
pub fn cache_dir_name(cache: &str) -> String {
    match cache {
        "pnpm" => "pnpm-store".to_string(),
        other => other.to_string(),
    }
}

/// Chemin (relatif) du répertoire de cache : `cache/<cache_dir_name>`.
/// Doit rester identique à `controller/src/project.rs::cache_path`.
pub fn cache_path(cache: &str) -> String {
    format!("cache/{}", cache_dir_name(cache))
}

#[derive(Debug, thiserror::Error)]
pub enum MaintError {
    #[error("VNL-MAINT-001: invalid branch name '{name}': {reason}")]
    InvalidBranch { name: String, reason: String },

    #[error("VNL-MAINT-002: invalid repo url '{url}': {reason}")]
    InvalidRepo { url: String, reason: String },

    #[error("VNL-MAINT-003: invalid sandbox name '{name}': {reason}")]
    InvalidSandboxName { name: String, reason: String },

    #[error("VNL-MAINT-004: git {args:?} failed with {status}: {stderr}")]
    GitFailed {
        args: Vec<String>,
        status: String,
        stderr: String,
    },

    #[error("VNL-MAINT-005: io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Valide un nom de branche via `git check-ref-format --branch <name>`
/// (subprocess en argv). Rejette AVANT l'appel git : chaîne vide, chaîne
/// commençant par `-`, chaîne contenant un caractère de contrôle (dont `\n`).
/// Utilisée par `checkout` dans la tâche suivante ; livrée et testée ici.
pub fn validate_branch(name: &str) -> Result<(), MaintError> {
    if name.is_empty() {
        return Err(MaintError::InvalidBranch {
            name: name.to_string(),
            reason: "branch name is empty".into(),
        });
    }
    if name.starts_with('-') {
        return Err(MaintError::InvalidBranch {
            name: name.to_string(),
            reason: "branch name starts with '-'".into(),
        });
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(MaintError::InvalidBranch {
            name: name.to_string(),
            reason: "branch name contains control characters".into(),
        });
    }
    let output = Command::new("git")
        .args(["check-ref-format", "--branch", name])
        .output()
        .map_err(|e| MaintError::InvalidBranch {
            name: name.to_string(),
            reason: format!("git check-ref-format failed: {e}"),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(MaintError::InvalidBranch {
            name: name.to_string(),
            reason: stderr.trim().to_string(),
        })
    }
}

/// Valide une URL/chemin de repo git plausible. Règles : non vide, ne commence
/// pas par `-`, aucun caractère de contrôle. (Pas de parsing d'URL complet —
/// git accepte des URLs, des chemins et des formes scp-like ; on ferme juste
/// l'injection d'argument et les entrées malformées.)
pub fn validate_repo(url: &str) -> Result<(), MaintError> {
    if url.is_empty() {
        return Err(MaintError::InvalidRepo {
            url: url.to_string(),
            reason: "repo url is empty".into(),
        });
    }
    if url.starts_with('-') {
        return Err(MaintError::InvalidRepo {
            url: url.to_string(),
            reason: "repo url starts with '-'".into(),
        });
    }
    if url.chars().any(|c| c.is_control()) {
        return Err(MaintError::InvalidRepo {
            url: url.to_string(),
            reason: "repo url contains control characters".into(),
        });
    }
    Ok(())
}

/// Valide un nom de sandbox comme composant de chemin sûr : non vide,
/// uniquement `[A-Za-z0-9._-]`, différent de `.` et `..`, ne commence pas
/// par `-`. (Le nom entre dans `worktrees/<name>` — ferme le path traversal.)
/// Utilisée par `checkout`/`remove` dans la tâche suivante ; livrée et testée ici.
pub fn validate_sandbox_name(name: &str) -> Result<(), MaintError> {
    if name.is_empty() {
        return Err(MaintError::InvalidSandboxName {
            name: name.to_string(),
            reason: "sandbox name is empty".into(),
        });
    }
    if name.starts_with('-') {
        return Err(MaintError::InvalidSandboxName {
            name: name.to_string(),
            reason: "sandbox name starts with '-'".into(),
        });
    }
    if name == "." || name == ".." {
        return Err(MaintError::InvalidSandboxName {
            name: name.to_string(),
            reason: format!("sandbox name is '{name}', which is a path traversal"),
        });
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(MaintError::InvalidSandboxName {
            name: name.to_string(),
            reason: "sandbox name contains invalid characters (only A-Za-z0-9._-)".into(),
        });
    }
    Ok(())
}

/// `init` : crée `<workspace>/cache/<dir>` pour chaque cache (mkdir -p,
/// toujours, même si le clone existe déjà), puis clone bare le repo vers
/// `<workspace>/repo.git` si ce répertoire n'existe pas déjà.
/// Invocation git : `git clone --bare -- <repo> <workspace>/repo.git`
/// (noter le `--` avant les arguments positionnels).
pub fn run_init(workspace: &Path, repo: &str, caches: &[String]) -> Result<(), MaintError> {
    // Create all cache directories (always, even if clone already exists).
    for cache in caches {
        let dir = workspace.join(cache_path(cache));
        std::fs::create_dir_all(&dir).map_err(|e| MaintError::Io {
            path: dir.to_string_lossy().to_string(),
            source: e,
        })?;
    }

    // Clone bare only if repo.git doesn't already exist.
    let bare_path = workspace.join(BARE_REPO_DIR);
    if !bare_path.exists() {
        let output = Command::new("git")
            .args(["clone", "--bare", "--", repo, bare_path.to_str().unwrap()])
            .output()
            .map_err(|e| MaintError::GitFailed {
                args: vec![
                    "clone".into(),
                    "--bare".into(),
                    "--".into(),
                    repo.to_string(),
                    bare_path.to_string_lossy().to_string(),
                ],
                status: e.to_string(),
                stderr: String::new(),
            })?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(MaintError::GitFailed {
                args: vec![
                    "clone".into(),
                    "--bare".into(),
                    "--".into(),
                    repo.to_string(),
                    bare_path.to_string_lossy().to_string(),
                ],
                status: output.status.to_string(),
                stderr,
            })
        }
    } else {
        // Already cloned, skip — idempotent.
        Ok(())
    }
}

/// `fetch` : `git --git-dir=<workspace>/repo.git fetch --prune`.
pub fn run_fetch(workspace: &Path) -> Result<(), MaintError> {
    let git_dir = workspace.join(BARE_REPO_DIR);
    let output = Command::new("git")
        .args(["--git-dir", git_dir.to_str().unwrap(), "fetch", "--prune"])
        .output()
        .map_err(|e| MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                git_dir.to_string_lossy().to_string(),
                "fetch".into(),
                "--prune".into(),
            ],
            status: e.to_string(),
            stderr: String::new(),
        })?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                git_dir.to_string_lossy().to_string(),
                "fetch".into(),
                "--prune".into(),
            ],
            status: output.status.to_string(),
            stderr,
        })
    }
}

/// `purge` : supprime récursivement `repo.git`, `worktrees` et `cache` sous
/// `workspace`. `std::fs::remove_dir_all` ; un `NotFound` est ignoré (succès).
pub fn run_purge(workspace: &Path) -> Result<(), MaintError> {
    for dir in [BARE_REPO_DIR, WORKTREES_DIR, CACHE_DIR] {
        let path = workspace.join(dir);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(MaintError::Io {
                    path: path.to_string_lossy().to_string(),
                    source: e,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. layout_constants — anti-dérive avec le controller.
    #[test]
    fn layout_constants() {
        assert_eq!(BARE_REPO_DIR, "repo.git");
        assert_eq!(WORKTREES_DIR, "worktrees");
        assert_eq!(CACHE_DIR, "cache");
    }

    // 2. cache_dir_name_mapping.
    #[test]
    fn cache_dir_name_mapping() {
        assert_eq!(cache_dir_name("cargo"), "cargo");
        assert_eq!(cache_dir_name("pnpm"), "pnpm-store");
        assert_eq!(cache_dir_name("custom"), "custom");
    }

    // 3. cache_path_uses_mapping.
    #[test]
    fn cache_path_uses_mapping() {
        assert_eq!(cache_path("pnpm"), "cache/pnpm-store");
        assert_eq!(cache_path("cargo"), "cache/cargo");
    }

    // ===== validate_branch =====

    #[test]
    fn validate_branch_ok() {
        validate_branch("main").unwrap();
        validate_branch("feat/x").unwrap();
        validate_branch("user/topic-1.2").unwrap();
    }

    #[test]
    fn validate_branch_rejects_empty() {
        let err = validate_branch("").unwrap_err();
        assert!(matches!(err, MaintError::InvalidBranch { .. }));
    }

    #[test]
    fn validate_branch_rejects_leading_dash() {
        let err = validate_branch("-oops").unwrap_err();
        assert!(matches!(err, MaintError::InvalidBranch { .. }));
    }

    #[test]
    fn validate_branch_rejects_control_chars() {
        let err = validate_branch("a\nb").unwrap_err();
        assert!(matches!(err, MaintError::InvalidBranch { .. }));
    }

    #[test]
    fn validate_branch_rejects_git_invalid() {
        validate_branch("a..b").expect_err("git rejects double-dot");
        validate_branch("a b").expect_err("git rejects space");
        validate_branch("end/").expect_err("git rejects trailing slash");
    }

    // ===== validate_repo =====

    #[test]
    fn validate_repo_ok() {
        validate_repo("https://github.com/o/r.git").unwrap();
        validate_repo("git@github.com:o/r.git").unwrap();
        validate_repo("/srv/git/repo.git").unwrap();
    }

    #[test]
    fn validate_repo_rejects() {
        let err = validate_repo("").unwrap_err();
        assert!(matches!(err, MaintError::InvalidRepo { .. }));

        let err = validate_repo("-flag").unwrap_err();
        assert!(matches!(err, MaintError::InvalidRepo { .. }));

        let err = validate_repo("a\nb").unwrap_err();
        assert!(matches!(err, MaintError::InvalidRepo { .. }));
    }

    // ===== validate_sandbox_name =====

    #[test]
    fn validate_sandbox_name_ok() {
        validate_sandbox_name("demo-branch").unwrap();
        validate_sandbox_name("sb1").unwrap();
        validate_sandbox_name("a.b_c").unwrap();
    }

    #[test]
    fn validate_sandbox_name_rejects() {
        validate_sandbox_name("").expect_err("empty");
        validate_sandbox_name("a/b").expect_err("slash");
        validate_sandbox_name("..").expect_err("parent");
        validate_sandbox_name(".").expect_err("self");
        validate_sandbox_name("-x").expect_err("leading dash");
        validate_sandbox_name("a b").expect_err("space");
    }
}
