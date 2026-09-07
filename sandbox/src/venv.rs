//! Activation du `.venv/` de workspace (feature python-support).
//!
//! Overlay d'environnement appliqué aux process SHELL uniquement (terminal PTY,
//! `execute_command`) — jamais au LSP (pyright auto-découvre `<workspace>/.venv`).
//! Marqueur : `<sandbox_root>/.venv/pyvenv.cfg` en tant que FICHIER (`is_file()`,
//! pas juste le dossier). Pas de walk-up : seul `<sandbox_root>/.venv` est
//! reconnu (convention `python -m venv .venv` / uv / poetry-in-project ; venv
//! niché = activation manuelle, périmètre design). Recalculé à CHAQUE appel —
//! jamais mis en cache : un `.venv` créé en cours de session est vu par la
//! prochaine commande / le prochain terminal.

use std::path::Path;

/// Overlay `[(clé, valeur)]` : `VIRTUAL_ENV` + `PATH` préfixé de
/// `<sandbox_root>/.venv/bin` (sur le `PATH` courant du process). Vide si le
/// marqueur absent. `sandbox_root` : config du process, jamais une entrée
/// utilisateur.
pub fn venv_overlay(sandbox_root: &Path) -> Vec<(String, String)> {
    let venv = sandbox_root.join(".venv");
    if !venv.join("pyvenv.cfg").is_file() {
        return Vec::new();
    }
    let path = format!(
        "{}:{}",
        venv.join("bin").to_string_lossy(),
        std::env::var("PATH").unwrap_or_default()
    );
    vec![
        (
            "VIRTUAL_ENV".to_string(),
            venv.to_string_lossy().into_owned(),
        ),
        ("PATH".to_string(), path),
    ]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    // Test 1 : tmpdir vide ⟹ aucun overlay.
    #[test]
    fn venv_overlay_absent() {
        let tmpdir = tempfile::tempdir().unwrap();
        assert_eq!(
            venv_overlay(tmpdir.path()),
            Vec::<(String, String)>::new(),
            "dossier vide : pas de .venv, pas d'overlay"
        );
    }

    // Test 2 : `.venv/` créé comme dossier seul ⟹ aucun overlay — le marqueur
    // est le FICHIER `pyvenv.cfg`, pas le dossier `.venv`.
    #[test]
    fn venv_overlay_dossier_sans_marqueur() {
        let tmpdir = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmpdir.path().join(".venv")).unwrap();
        assert_eq!(
            venv_overlay(tmpdir.path()),
            Vec::<(String, String)>::new(),
            "dossier .venv sans pyvenv.cfg : pas d'overlay"
        );
    }

    // Test 3 : `pyvenv.cfg` présent mais comme DOSSIER ⟹ aucun overlay —
    // le test du marqueur est `is_file()`, pas `exists()`.
    #[test]
    fn venv_overlay_marqueur_est_un_dossier() {
        let tmpdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".venv").join("pyvenv.cfg")).unwrap();
        assert_eq!(
            venv_overlay(tmpdir.path()),
            Vec::<(String, String)>::new(),
            "pyvenv.cfg dossier : pas d'overlay (is_file(), pas exists())"
        );
    }

    // Test 4 : marqueur fichier ⟹ exactement les deux paires VIRTUAL_ENV +
    // PATH (égalité exacte, valeur PATH recalculée à l'identique dans le test).
    #[test]
    fn venv_overlay_present() {
        let tmpdir = tempfile::tempdir().unwrap();
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\nversion = 3.12\n").unwrap();

        let expected_path = format!(
            "{}:{}",
            venv.join("bin").to_string_lossy(),
            std::env::var("PATH").unwrap_or_default()
        );
        assert_eq!(
            venv_overlay(tmpdir.path()),
            vec![
                (
                    "VIRTUAL_ENV".to_string(),
                    venv.to_string_lossy().into_owned()
                ),
                ("PATH".to_string(), expected_path),
            ],
            "exactement VIRTUAL_ENV + PATH préfixé de .venv/bin, dans cet ordre"
        );
    }

    // Test 5 : aucun cache — venv créé entre les deux appels ⟹ vu au second.
    #[test]
    fn venv_overlay_recalcule_a_chaque_appel() {
        let tmpdir = tempfile::tempdir().unwrap();
        assert!(
            venv_overlay(tmpdir.path()).is_empty(),
            "premier appel : .venv pas encore créé"
        );
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert_eq!(
            venv_overlay(tmpdir.path()).len(),
            2,
            "second appel : .venv créé entre-temps doit être vu (jamais de cache)"
        );
    }
}
