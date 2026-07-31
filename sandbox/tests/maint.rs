use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

use vanyline_sandbox::maint;

/// Creates a source git repo with an initial commit in `dir`, returning its
/// path. Each step via `std::process::Command`; panics with stderr if any
/// step fails.
fn make_source_repo(dir: &Path) -> PathBuf {
    let dir = dir.to_path_buf();
    // `git init` requires the working directory to exist.
    std::fs::create_dir_all(&dir).expect("create dir");

    // `git init`
    assert!(
        Command::new("git")
            .args(["init"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    // Write a file
    std::fs::write(dir.join("README.md"), "# test\n").expect("write README.md");

    // `git add`
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success(),
        "git add failed"
    );

    // `git commit`
    let output = Command::new("git")
        .args([
            "-c",
            "user.email=test@test",
            "-c",
            "user.name=test",
            "commit",
            "-m",
            "init",
        ])
        .current_dir(&dir)
        .output()
        .expect("git commit failed");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    dir
}

// ===== init_creates_bare_and_caches =====
#[test]
fn init_creates_bare_and_caches() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let caches = vec!["cargo".into(), "pnpm".into()];
    maint::run_init(&ws, src.to_str().unwrap(), &caches).unwrap();

    // repo.git exists and is a bare repo (contains HEAD)
    assert!(ws.join("repo.git").exists());
    assert!(ws.join("repo.git").join("HEAD").exists());

    // Cache directories exist
    assert!(ws.join("cache").join("cargo").exists());
    assert!(ws.join("cache").join("pnpm-store").exists());
}

// ===== init_is_idempotent =====
#[test]
fn init_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let caches = vec!["cargo".into()];
    maint::run_init(&ws, src.to_str().unwrap(), &caches).unwrap();

    // Place a marker file inside repo.git.
    std::fs::write(ws.join("repo.git").join("MARKER"), "keep").unwrap();

    // Call init again.
    maint::run_init(&ws, src.to_str().unwrap(), &caches).unwrap();

    // Marker still exists — clone was NOT redone.
    assert!(ws.join("repo.git").join("MARKER").exists());
    assert!(ws.join("cache").join("cargo").exists());
}

// ===== init_bad_repo_fails =====
#[test]
fn init_bad_repo_fails() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let fake_repo = tmp.path().join("nonexistent").to_str().unwrap().to_string();

    let err = maint::run_init(&ws, &fake_repo, &[]).unwrap_err();
    match &err {
        maint::MaintError::GitFailed { stderr, .. } => {
            assert!(
                err.to_string().contains("VNL-MAINT-004"),
                "error should contain VNL-MAINT-004: {}",
                err
            );
            assert!(!stderr.is_empty(), "stderr should not be empty: {stderr}");
        }
        _ => panic!("expected GitFailed, got {err:?}"),
    }
    assert!(
        !ws.join("repo.git").exists(),
        "repo.git should not exist after failed init"
    );
}

// ===== fetch_ok =====
#[test]
fn fetch_ok() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();
    maint::run_fetch(&ws).unwrap();
}

// ===== fetch_without_bare_fails =====
#[test]
fn fetch_without_bare_fails() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let err = maint::run_fetch(&ws).unwrap_err();
    assert!(
        matches!(err, maint::MaintError::GitFailed { .. }),
        "expected GitFailed, got {err:?}"
    );
}

// ===== purge_removes_everything =====
#[test]
fn purge_removes_everything() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &["cargo".into(), "pnpm".into()]).unwrap();

    // Also create worktrees/<sandbox>/ directories.
    std::fs::create_dir_all(ws.join("worktrees").join("demo-branch")).unwrap();

    maint::run_purge(&ws).unwrap();

    assert!(!ws.join("repo.git").exists(), "repo.git should be removed");
    assert!(
        !ws.join("worktrees").exists(),
        "worktrees should be removed"
    );
    assert!(!ws.join("cache").exists(), "cache should be removed");
}

// ===== purge_on_empty_workspace_ok =====
#[test]
fn purge_on_empty_workspace_ok() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_purge(&ws).unwrap();
}
