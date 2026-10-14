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

/// Scenario (§Must chat.sdd, tâche « ALIGNEMENT TUI ») : `vanyline` sans
/// sous-commande dans un pipe (stdin et stdout non terminaux) → message exact
/// sur stderr et code de sortie 2 ; l'écran alternatif n'est jamais tenté.
#[test]
fn interactif_sans_terminal_message_exact_et_code_2() {
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
        "code de sortie 2 attendu ; stderr: {:?}, stdout: {:?}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.trim_end(),
        "vanyline: interactive chat needs a terminal; use `vanyline run <message>` for scripts",
        "message exact attendu sur stderr"
    );
}

/// Le one-shot reste le mode scriptable : `run <message>` sans terminal ne
/// rend JAMAIS le message d'usage interactif (le chemin interactif n'est pas
/// emprunté). Config isolée sans agent → terminaison `No agent specified`
/// code 1 (`chat.sdd` §Raises), ce qui prouve que le dispatch est passé par
/// `run_one_shot`.
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
