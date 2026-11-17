//! Tests d'usage SANS TERMINAL du mode interactif (`../chat.sdd` §Must « REPL
//! INTERACTIF = TUI ratatui ») sur le binaire réel : le mode sans sous-commande
//! exige que stdin ET stdout soient des terminaux ; sinon message exact et
//! sortie code 2 (le one-shot `run <message>` reste le mode scriptable,
//! inchangé). XDG isolé par tempdir injecté au child uniquement (convention de
//! `rpc_stdio_smoke.rs` : la config de la machine n'est jamais lue ni touchée).

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

/// Le one-shot reste le mode scriptable : `run <message>` sans terminal ne
/// rend JAMAIS le message d'usage interactif (le chemin interactif n'est pas
/// emprunté). Config isolée sans agent → terminaison `No agent specified`
/// code 1 (`chat.sdd` §Raises), ce qui prouve que le dispatch est passé par
/// `run_one_shot`.
#[ignore = "run : lot 3 (provisoire : run renvoie une erreur)"]
#[test]
fn one_shot_sans_terminal_ne_rend_pas_le_message_interactif() {
    let home = tempdir().expect("tempdir XDG");
    let ws = tempdir().expect("tempdir cwd");
    let out = Command::new(env!("CARGO_BIN_EXE_vanyline"))
        .args(["run", "bonjour"])
        .env("XDG_CONFIG_HOME", home.path())
        .env("XDG_DATA_HOME", home.path())
        .current_dir(ws.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de vanyline run")
        .wait_with_output()
        .expect("attente de vanyline run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("needs a terminal"),
        "le chemin interactif ne doit pas être emprunté par le one-shot: {stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "config isolée sans agent : code 1 (chemin one-shot) ; stderr: {stderr}"
    );
    assert!(
        stderr.contains("No agent specified"),
        "terminaison one-shot attendue, obtenue: {stderr}"
    );
}
