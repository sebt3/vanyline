//! Tests d'usage SANS TERMINAL sur le binaire réel : le mode sans sous-commande
//! affiche l'aide (code 2) ; `run` échoue en usage (code 2) avant tout appel au
//! cluster quand il manque l'Owner ou le message. Réglages isolés par `HOME`
//! temporaire : la config de la machine n'est jamais lue ni touchée.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::assert_is_empty
    )
)]

use std::process::{Command, Stdio};

use tempfile::tempdir;

/// Scenario (`main.sdd`, « sans sous-commande ») : `vanyline` seul rend l'aide
/// sur stdout et sort en code 2 ; l'ancien REPL n'existe plus, rien n'est lancé
/// sur le terminal.
#[test]
fn sans_sous_commande_affiche_l_aide_code_2() {
    let home = tempdir().expect("tempdir XDG");
    let ws = tempdir().expect("tempdir cwd");
    let out = Command::new(env!("CARGO_BIN_EXE_vanyline"))
        .env("XDG_CONFIG_HOME", home.path())
        .env("XDG_DATA_HOME", home.path())
        .current_dir(ws.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de vanyline sans sous-commande")
        .wait_with_output()
        .expect("attente de vanyline");
    assert_eq!(
        out.status.code(),
        Some(2),
        "code de sortie 2 attendu ; stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Usage"),
        "l'aide est rendue sur stdout: {stdout}"
    );
    assert!(
        stdout.contains("chat"),
        "la commande chat figure dans l'aide: {stdout}"
    );
}

/// `run` sans Owner (ni `--owner` ni `defaults.owner`) : erreur d'usage nommant les deux moyens, code 2, avant tout
/// appel au cluster (`target.sdd`, `AC-7`). Réglages isolés par `HOME` temporaire : le fichier de la machine n'est
/// jamais lu.
#[test]
fn run_sans_owner_code_2_avant_le_cluster() {
    let home = tempdir().expect("tempdir HOME");
    let ws = tempdir().expect("tempdir cwd");
    let out = Command::new(env!("CARGO_BIN_EXE_vanyline"))
        .args(["run", "bonjour", "--context", "demo"])
        .env("HOME", home.path())
        .current_dir(ws.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de vanyline run")
        .wait_with_output()
        .expect("attente de vanyline run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "code 2 attendu (usage) ; stderr: {stderr}"
    );
    assert!(stderr.contains("--owner"), "stderr: {stderr}");
    assert!(stderr.contains("defaults.owner"), "stderr: {stderr}");
    assert!(
        !stderr.contains("VNL-K8S-001"),
        "aucun appel au cluster avant l'erreur d'usage: {stderr}"
    );
}

/// `run -` avec une entrée standard vide : erreur d'usage, code 2, aucune session (`run.sdd`, scénario « message vide »).
#[test]
fn run_message_vide_sur_l_entree_code_2() {
    let home = tempdir().expect("tempdir HOME");
    let ws = tempdir().expect("tempdir cwd");
    let out = Command::new(env!("CARGO_BIN_EXE_vanyline"))
        .args(["run", "-", "--context", "demo", "--owner", "alice"])
        .env("HOME", home.path())
        .current_dir(ws.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de vanyline run -")
        .wait_with_output()
        .expect("attente de vanyline run -");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "stderr: {stderr}");
    assert!(stderr.contains("VNL-CLI-RUN-001"), "stderr: {stderr}");
}
