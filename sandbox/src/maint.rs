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

    #[error("VNL-MAINT-006: k8s patch status project '{project}' failed: {message}")]
    K8sPatch { project: String, message: String },
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
///
/// Pose ensuite la refspec de fetch (idempotent, appliqué que le clone
/// vienne d'être fait ou préexistait déjà).
#[allow(clippy::unwrap_used)] // chemin construit en interne a partir d un nom deja valide (MaintError valide les entrees en amont), toujours UTF-8 dans ce deploiement
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

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(MaintError::GitFailed {
                args: vec![
                    "clone".into(),
                    "--bare".into(),
                    "--".into(),
                    repo.to_string(),
                    bare_path.to_string_lossy().to_string(),
                ],
                status: output.status.to_string(),
                stderr,
            });
        }
    }

    // Idempotent : appliqué que le clone vienne d'être fait ou préexistait déjà.
    set_fetch_refspec(&bare_path)
}

/// `fetch` : `git --git-dir=<workspace>/repo.git fetch --prune`.
#[allow(clippy::unwrap_used)] // chemin construit en interne a partir d un nom deja valide (MaintError valide les entrees en amont), toujours UTF-8 dans ce deploiement
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

/// Chemin (relatif au workspace) du worktree d'une sandbox donnée.
/// Doit rester identique à `controller/src/project.rs::worktree_path`.
pub fn worktree_path(sandbox_name: &str) -> String {
    format!("worktrees/{sandbox_name}")
}

/// `checkout` : crée le worktree `worktrees/<sandbox>` pour `branch` s'il
/// n'existe pas déjà (idempotent). Valide `sandbox` (validate_sandbox_name),
/// `branch` (validate_branch) et, si fourni et non vide, `default_branch`
/// (validate_branch) AVANT toute action. `default_branch: None` ou `Some("")`
/// => résolution via symbolic-ref, repli "main" (voir Contexte).
pub fn run_checkout(
    workspace: &Path,
    sandbox: &str,
    branch: &str,
    default_branch: Option<&str>,
) -> Result<(), MaintError> {
    validate_sandbox_name(sandbox)?;
    validate_branch(branch)?;
    if let Some(db) = default_branch
        && !db.is_empty()
    {
        validate_branch(db)?;
    }

    let wt = worktree_path(sandbox);
    if workspace.join(&wt).exists() {
        // Already checked out — idempotent.
        return Ok(());
    }

    // Resolve the default branch.
    let default = if let Some(db) = default_branch {
        if db.is_empty() {
            // Resolve via symbolic-ref, fall back to "main".
            resolve_default_branch(workspace).unwrap_or_else(|_| "main".into())
        } else {
            db.to_string()
        }
    } else {
        // Resolve via symbolic-ref, fall back to "main".
        resolve_default_branch(workspace).unwrap_or_else(|_| "main".into())
    };

    let bare_git_dir = BARE_REPO_DIR;

    // Check if the branch exists in the bare repo.
    let show_ref = Command::new("git")
        .args([
            "--git-dir",
            bare_git_dir,
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(workspace)
        .output()
        .map_err(|e| MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                bare_git_dir.into(),
                "show-ref".into(),
                "--verify".into(),
                "--quiet".into(),
                format!("refs/heads/{branch}"),
            ],
            status: e.to_string(),
            stderr: String::new(),
        })?;

    let git_worktree_add = |args: &[&str]| -> Result<(), MaintError> {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .map_err(|e| MaintError::GitFailed {
                args: args.iter().cloned().map(String::from).collect(),
                status: e.to_string(),
                stderr: String::new(),
            })?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(MaintError::GitFailed {
                args: args.iter().cloned().map(String::from).collect(),
                status: output.status.to_string(),
                stderr,
            })
        }
    };

    if show_ref.status.success() {
        // Branch exists in bare repo.
        git_worktree_add(&["--git-dir", bare_git_dir, "worktree", "add", &wt, branch])?;
    } else {
        // Branch doesn't exist — create it from the default.
        git_worktree_add(&[
            "--git-dir",
            bare_git_dir,
            "worktree",
            "add",
            "-b",
            branch,
            &wt,
            &default,
        ])?;
    }

    Ok(())
}

/// Resolve the current branch via `symbolic-ref --short HEAD`.
/// Returns "main" on failure (same fallback as the controller scripts).
fn resolve_default_branch(workspace: &Path) -> Result<String, MaintError> {
    let output = Command::new("git")
        .args([
            "--git-dir",
            BARE_REPO_DIR,
            "symbolic-ref",
            "--short",
            "HEAD",
        ])
        .current_dir(workspace)
        .output()
        .map_err(|e| MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                BARE_REPO_DIR.into(),
                "symbolic-ref".into(),
                "--short".into(),
                "HEAD".into(),
            ],
            status: e.to_string(),
            stderr: String::new(),
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                BARE_REPO_DIR.into(),
                "symbolic-ref".into(),
                "--short".into(),
                "HEAD".into(),
            ],
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

/// `remove` : retire le worktree `worktrees/<sandbox>` (worktree remove
/// --force, repli rm -rf) puis `worktree prune`. Valide `sandbox` d'abord.
pub fn run_remove(workspace: &Path, sandbox: &str) -> Result<(), MaintError> {
    validate_sandbox_name(sandbox)?;

    let wt = worktree_path(sandbox);
    let bare_git_dir = BARE_REPO_DIR;

    // First attempt: git worktree remove --force.
    // A non-zero exit is NOT an error — it's a logic branch (incoherent state).
    let remove_result = Command::new("git")
        .args([
            "--git-dir",
            bare_git_dir,
            "worktree",
            "remove",
            "--force",
            &wt,
        ])
        .current_dir(workspace)
        .output(); // Result<Output, io::Error> — we handle both variants below.

    // Fallback: rm -rf the worktree directory.
    // A non-zero exit is NOT an error — it's a logic branch (incoherent state).
    // A spawn error also triggers the fallback.
    if !matches!(remove_result, Ok(output) if output.status.success()) {
        let _ = std::fs::remove_dir_all(workspace.join(&wt));
    }

    // `worktree prune` must succeed — its failure is an error.
    let prune_output = Command::new("git")
        .args(["--git-dir", bare_git_dir, "worktree", "prune"])
        .current_dir(workspace)
        .output()
        .map_err(|e| MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                bare_git_dir.into(),
                "worktree".into(),
                "prune".into(),
            ],
            status: e.to_string(),
            stderr: String::new(),
        })?;

    if prune_output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&prune_output.stderr).to_string();
        Err(MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                bare_git_dir.into(),
                "worktree".into(),
                "prune".into(),
            ],
            status: prune_output.status.to_string(),
            stderr,
        })
    }
}

/// Nom de fichier marquant la présence de Rust — racine ou sous-chemin
/// (membre de workspace Cargo).
const RUST_MARKER: &str = "Cargo.toml";

/// Noms de fichiers marquant la présence de JS/TS — racine uniquement.
const JS_TS_MARKERS: [&str; 2] = ["package.json", "tsconfig.json"];

/// Liste les chemins de fichiers de l'arbre HEAD du clone bare
/// `workspace/repo.git` (`git --git-dir <bare> ls-tree -r --name-only HEAD`).
/// Chaque chemin est relatif à la racine du dépôt, séparateur `/` (format git,
/// indépendant de l'OS). Erreur `MaintError::GitFailed` si la commande échoue
/// (ex: `repo.git` absent — `detect` appelé avant `init`).
#[allow(clippy::unwrap_used)] // chemin construit en interne a partir d un nom deja valide (MaintError valide les entrees en amont), toujours UTF-8 dans ce deploiement
fn list_head_tree(workspace: &Path) -> Result<Vec<String>, MaintError> {
    let bare_path = workspace.join(BARE_REPO_DIR);
    let output = Command::new("git")
        .args([
            "--git-dir",
            bare_path.to_str().unwrap(),
            "ls-tree",
            "-r",
            "--name-only",
            "HEAD",
        ])
        .current_dir(workspace)
        .output()
        .map_err(|e| MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                bare_path.to_string_lossy().to_string(),
                "ls-tree".into(),
                "-r".into(),
                "--name-only".into(),
                "HEAD".into(),
            ],
            status: e.to_string(),
            stderr: String::new(),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(MaintError::GitFailed {
            args: vec![
                "--git-dir".into(),
                bare_path.to_string_lossy().to_string(),
                "ls-tree".into(),
                "-r".into(),
                "--name-only".into(),
                "HEAD".into(),
            ],
            status: output.status.to_string(),
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Détecte les langages utilisés à partir des marqueurs de fichiers de
/// l'arbre HEAD. Résultat dans l'ordre fixe `["rust", "js-ts"]` (filtré).
pub fn detect_languages(workspace: &Path) -> Result<Vec<String>, MaintError> {
    let paths = list_head_tree(workspace)?;
    let has_rust = paths
        .iter()
        .any(|p| p == RUST_MARKER || p.ends_with(&format!("/{RUST_MARKER}")));
    let has_js_ts = paths.iter().any(|p| JS_TS_MARKERS.contains(&p.as_str()));

    let mut languages = Vec::new();
    if has_rust {
        languages.push("rust".to_string());
    }
    if has_js_ts {
        languages.push("js-ts".to_string());
    }
    Ok(languages)
}

/// `detect` : sérialise `detect_languages` en JSON `{"languages": [...]}`.
pub fn run_detect(workspace: &Path) -> Result<String, MaintError> {
    let languages = detect_languages(workspace)?;
    Ok(serde_json::json!({ "languages": languages }).to_string())
}

/// `detect` + patch optionnel du status K8s. `project = None` : comportement
/// identique à `run_detect` (pas d'appel réseau — mode local/test, ex: `vanyline
/// sandbox` en CLI hors cluster). `project = Some(name)` : patche en plus
/// `Project.status.{languages,detectedAt}` du Project `name`, dans le
/// namespace résolu par `kube::Config::infer()` (in-cluster — le pod du Job
/// tourne avec un ServiceAccount, cf. tâche 04 pour le RBAC).
pub async fn run_detect_and_patch(
    workspace: &Path,
    project: Option<&str>,
) -> Result<String, MaintError> {
    let languages = detect_languages(workspace)?;
    let json = serde_json::json!({ "languages": languages }).to_string();

    if let Some(project_name) = project {
        patch_project_languages(project_name, &languages).await?;
    }

    Ok(json)
}

/// Merge patch ciblé `status.{languages,detectedAt}` — voir "Point
/// d'attention critique" dans le fichier de tâche : ne jamais construire ce
/// patch via `ProjectStatus { .. }`.
async fn patch_project_languages(
    project_name: &str,
    languages: &[String],
) -> Result<(), MaintError> {
    let config = kube::Config::infer()
        .await
        .map_err(|e| MaintError::K8sPatch {
            project: project_name.to_string(),
            message: format!("config: {e}"),
        })?;
    let ns = config.default_namespace.clone();
    let client = kube::Client::try_from(config).map_err(|e| MaintError::K8sPatch {
        project: project_name.to_string(),
        message: format!("client: {e}"),
    })?;

    let api: kube::Api<vanyline_crds::Project> = kube::Api::namespaced(client, &ns);
    let now = k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        k8s_openapi::jiff::Timestamp::now(),
    );
    let patch = serde_json::json!({
        "status": {
            "languages": languages,
            "detectedAt": now,
        }
    });
    api.patch_status(
        project_name,
        &kube::api::PatchParams::default(),
        &kube::api::Patch::Merge(&patch),
    )
    .await
    .map_err(|e| MaintError::K8sPatch {
        project: project_name.to_string(),
        message: e.to_string(),
    })?;
    Ok(())
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

/// Refspec de fetch posée sur le clone bare — `git clone --bare` n'en
/// configure aucune par défaut. Cible `refs/remotes/origin/*`, pas
/// `refs/heads/*` : ne doit jamais écraser les branches locales créées par
/// `checkout` (worktree add -b).
const FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";

/// Pose (ou réécrit, idempotent via `--replace-all`) la refspec de fetch
/// sur le clone bare `bare_path`.
#[allow(clippy::unwrap_used)] // chemin construit en interne a partir d un nom deja valide (MaintError valide les entrees en amont), toujours UTF-8 dans ce deploiement
fn set_fetch_refspec(bare_path: &Path) -> Result<(), MaintError> {
    let bare_str = bare_path.to_str().unwrap();
    let args = [
        "--git-dir",
        bare_str,
        "config",
        "--replace-all",
        "remote.origin.fetch",
        FETCH_REFSPEC,
    ];
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| MaintError::GitFailed {
            args: args.iter().map(|s| s.to_string()).collect(),
            status: e.to_string(),
            stderr: String::new(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(MaintError::GitFailed {
            args: args.iter().map(|s| s.to_string()).collect(),
            status: output.status.to_string(),
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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

    // ===== worktree_path =====

    #[test]
    fn worktree_path_value() {
        assert_eq!(worktree_path("sb1"), "worktrees/sb1");
    }

    // ===== run_checkout validation =====

    #[test]
    fn checkout_rejects_invalid_sandbox() {
        let result = run_checkout(Path::new("/tmp"), "a/b", "main", None);
        assert!(matches!(result, Err(MaintError::InvalidSandboxName { .. })));
    }

    #[test]
    fn checkout_rejects_invalid_branch() {
        let result = run_checkout(Path::new("/tmp"), "sb1", "a..b", None);
        assert!(matches!(result, Err(MaintError::InvalidBranch { .. })));
    }

    // ===== run_remove validation =====

    #[test]
    fn remove_rejects_invalid_sandbox() {
        let result = run_remove(Path::new("/tmp"), "..");
        assert!(matches!(result, Err(MaintError::InvalidSandboxName { .. })));
    }
}
