#![allow(clippy::unwrap_used, clippy::expect_used)]

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

    // `git init -b main` — branche par défaut déterministe.
    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
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
                "error should contain VNL-MAINT-004: {err}"
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

// ===== checkout_existing_branch =====
#[test]
fn checkout_existing_branch() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    // Init the bare repo.
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // Check out sb1 with branch "main".
    maint::run_checkout(&ws, "sb1", "main", None).unwrap();

    // worktrees/sb1 exists and contains README.md.
    assert!(ws.join("worktrees/sb1").exists());
    assert!(ws.join("worktrees/sb1/README.md").exists());
}

// ===== checkout_creates_branch_from_default =====
#[test]
fn checkout_creates_branch_from_default() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // Check out a non-existing branch — should be created from "main".
    maint::run_checkout(&ws, "sb1", "feature-x", None).unwrap();

    // worktrees/sb1 exists.
    assert!(ws.join("worktrees/sb1").exists());

    // The worktree's current branch is "feature-x".
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(ws.join("worktrees/sb1"))
        .output()
        .expect("git rev-parse failed");
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(head, "feature-x");
}

// ===== checkout_explicit_default_branch =====
#[test]
fn checkout_explicit_default_branch() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // Checkout with explicit default branch.
    maint::run_checkout(&ws, "sb1", "feature-y", Some("main")).unwrap();

    assert!(ws.join("worktrees/sb1").exists());
}

// ===== checkout_is_idempotent =====
#[test]
fn checkout_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // First checkout.
    maint::run_checkout(&ws, "sb1", "main", None).unwrap();

    // Place a marker inside the worktree.
    std::fs::write(ws.join("worktrees/sb1/MARKER"), "keep").unwrap();

    // Second checkout — should be a no-op.
    maint::run_checkout(&ws, "sb1", "main", None).unwrap();

    // Marker still exists.
    assert!(ws.join("worktrees/sb1/MARKER").exists());
}

// ===== remove_removes_worktree =====
#[test]
fn remove_removes_worktree() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // Checkout then remove.
    maint::run_checkout(&ws, "sb1", "main", None).unwrap();
    maint::run_remove(&ws, "sb1").unwrap();

    assert!(
        !ws.join("worktrees/sb1").exists(),
        "worktree should be removed"
    );
}

// ===== remove_incoherent_worktree_falls_back =====
#[test]
fn remove_incoherent_worktree_falls_back() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // Checkout then manually remove the directory to simulate incoherent state.
    maint::run_checkout(&ws, "sb1", "main", None).unwrap();
    std::fs::remove_dir_all(ws.join("worktrees/sb1")).unwrap();

    // run_remove should succeed (fallback + prune).
    maint::run_remove(&ws, "sb1").unwrap();

    // A subsequent checkout should also succeed (proves prune cleaned up).
    maint::run_checkout(&ws, "sb1", "main", None).unwrap();
    assert!(ws.join("worktrees/sb1").exists());
}

// ===== remove_nonexistent_worktree_ok =====
#[test]
fn remove_nonexistent_worktree_ok() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // No checkout — just remove a non-existent worktree.
    maint::run_remove(&ws, "never-created").unwrap();
}

// ===== init_sets_fetch_refspec =====
#[test]
fn init_sets_fetch_refspec() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // Read the fetch refspec from the bare repo.
    let output = Command::new("git")
        .args([
            "--git-dir",
            ws.join("repo.git").to_str().unwrap(),
            "config",
            "remote.origin.fetch",
        ])
        .output()
        .expect("git config failed");
    assert!(
        output.status.success(),
        "git config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(
        stdout, "+refs/heads/*:refs/remotes/origin/*",
        "fetch refspec should be set on the bare repo"
    );
}

// ===== fetch_populates_remote_tracking_refs =====
#[test]
fn fetch_populates_remote_tracking_refs() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    // Init the bare repo.
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    // Create a new branch on src AFTER init.
    assert!(
        Command::new("git")
            .args(["checkout", "-b", "feature"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git checkout -b feature failed"
    );
    let commit_output = Command::new("git")
        .args([
            "-c",
            "user.email=test@test",
            "-c",
            "user.name=test",
            "commit",
            "--allow-empty",
            "-m",
            "feature commit",
        ])
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(
        commit_output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&commit_output.stderr)
    );

    // Fetch into the workspace.
    maint::run_fetch(&ws).unwrap();

    // Verify refs/remotes/origin/feature exists in the bare repo.
    assert!(
        ws.join("repo.git/refs/remotes/origin/feature").exists(),
        "refs/remotes/origin/feature should exist after fetch (proves the refspec is working)"
    );
}

// ===== init_idempotent_refspec_not_duplicated =====
#[test]
fn init_idempotent_refspec_not_duplicated() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let caches = vec!["cargo".into()];

    // Run init twice.
    maint::run_init(&ws, src.to_str().unwrap(), &caches).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &caches).unwrap();

    // Read all values of remote.origin.fetch.
    let output = Command::new("git")
        .args([
            "--git-dir",
            ws.join("repo.git").to_str().unwrap(),
            "config",
            "--get-all",
            "remote.origin.fetch",
        ])
        .output()
        .expect("git config --get-all failed");
    assert!(
        output.status.success(),
        "git config --get-all failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "remote.origin.fetch should appear exactly once after two init calls (found {}): {}",
        lines.len(),
        stdout
    );
}

// ===== detect_rust_only =====
#[test]
fn detect_rust_only() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    // Write Cargo.toml at root.
    std::fs::write(
        src.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();

    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git add failed"
    );
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
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(output.status.success(), "git commit failed: {}", String::from_utf8_lossy(&output.stderr));

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect(&ws).unwrap();
    assert_eq!(result, r#"{"languages":["rust"]}"#);
}

// ===== detect_js_ts_package_json =====
#[test]
fn detect_js_ts_package_json() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    std::fs::write(src.join("package.json"), r#"{"name":"app"}"#).unwrap();

    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git add failed"
    );
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
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(output.status.success(), "git commit failed: {}", String::from_utf8_lossy(&output.stderr));

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect(&ws).unwrap();
    assert_eq!(result, r#"{"languages":["js-ts"]}"#);
}

// ===== detect_js_ts_tsconfig_json =====
#[test]
fn detect_js_ts_tsconfig_json() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    std::fs::write(
        src.join("tsconfig.json"),
        r#"{"compilerOptions":{}}"#,
    )
    .unwrap();

    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git add failed"
    );
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
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(output.status.success(), "git commit failed: {}", String::from_utf8_lossy(&output.stderr));

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect(&ws).unwrap();
    assert_eq!(result, r#"{"languages":["js-ts"]}"#);
}

// ===== detect_both_rust_js_ts =====
#[test]
fn detect_both_rust_js_ts() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    std::fs::write(
        src.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(src.join("package.json"), r#"{"name":"app"}"#).unwrap();

    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git add failed"
    );
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
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(output.status.success(), "git commit failed: {}", String::from_utf8_lossy(&output.stderr));

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect(&ws).unwrap();
    assert_eq!(result, r#"{"languages":["rust","js-ts"]}"#);
}

// ===== detect_nested_cargo_toml_workspace_member =====
#[test]
fn detect_nested_cargo_toml_workspace_member() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    // Create crates/foo/Cargo.toml (nested, simulates a workspace member).
    std::fs::create_dir_all(src.join("crates/foo")).unwrap();
    std::fs::write(
        src.join("crates/foo/Cargo.toml"),
        "[package]\nname=\"foo\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();

    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git add failed"
    );
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
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(output.status.success(), "git commit failed: {}", String::from_utf8_lossy(&output.stderr));

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect(&ws).unwrap();
    assert_eq!(result, r#"{"languages":["rust"]}"#);
}

// ===== detect_nested_package_json_ignored =====
#[test]
fn detect_nested_package_json_ignored() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    // Create frontend/package.json (nested — should be ignored for JS/TS).
    std::fs::create_dir_all(src.join("frontend")).unwrap();
    std::fs::write(
        src.join("frontend/package.json"),
        r#"{"name":"web"}"#,
    )
    .unwrap();

    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git add failed"
    );
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
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(output.status.success(), "git commit failed: {}", String::from_utf8_lossy(&output.stderr));

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect(&ws).unwrap();
    assert_eq!(result, r#"{"languages":[]}"#);
}

// ===== detect_no_markers =====
#[test]
fn detect_no_markers() {
    let tmp = TempDir::new().unwrap();
    let src = make_source_repo(&tmp.path().join("src"));
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect(&ws).unwrap();
    assert_eq!(result, r#"{"languages":[]}"#);
}

// ===== detect_without_bare_fails =====
#[test]
fn detect_without_bare_fails() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let err = maint::run_detect(&ws).unwrap_err();
    assert!(
        matches!(err, maint::MaintError::GitFailed { .. }),
        "expected GitFailed, got {err:?}"
    );
}

// ===== detect_and_patch_without_project_matches_plain_detect =====
#[tokio::test]
async fn detect_and_patch_without_project_matches_plain_detect() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    std::fs::write(src.join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.1.0\"\n").unwrap();

    assert!(
        Command::new("git").args(["add", "."]).current_dir(&src).status().unwrap().success(),
        "git add failed"
    );
    let commit = Command::new("git")
        .args(["-c", "user.email=test@test", "-c", "user.name=test", "commit", "-m", "init"])
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(commit.status.success(), "git commit failed: {}", String::from_utf8_lossy(&commit.stderr));

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect_and_patch(&ws, None).await.unwrap();
    let plain = maint::run_detect(&ws).unwrap();
    assert_eq!(result, plain);
}

// ===== detect_and_patch_with_project_fails_without_cluster =====
#[tokio::test]
async fn detect_and_patch_with_project_fails_without_cluster() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&src)
            .status()
            .unwrap()
            .success(),
        "git init failed"
    );

    std::fs::write(
        src.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();

    assert!(
        Command::new("git").args(["add", "."]).current_dir(&src).status().unwrap().success(),
        "git add failed"
    );
    let commit = Command::new("git")
        .args(["-c", "user.email=test@test", "-c", "user.name=test", "commit", "-m", "init"])
        .current_dir(&src)
        .output()
        .expect("git commit failed");
    assert!(commit.status.success(), "git commit failed: {}", String::from_utf8_lossy(&commit.stderr));

    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    maint::run_init(&ws, src.to_str().unwrap(), &[]).unwrap();

    let result = maint::run_detect_and_patch(&ws, Some("whatever")).await;
    assert!(
        matches!(result, Err(maint::MaintError::K8sPatch { .. })),
        "expected K8sPatch error, got {result:?}"
    );
}
