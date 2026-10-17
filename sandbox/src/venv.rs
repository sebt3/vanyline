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
#[must_use]
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

    // Helper : overlay attendu pour un `sandbox_root` donné (PATH recalculé
    // à l'identique, comme dans `venv_overlay_present`).
    fn expected_overlay(root: &std::path::Path) -> Vec<(String, String)> {
        let venv = root.join(".venv");
        vec![
            (
                "VIRTUAL_ENV".to_string(),
                venv.to_string_lossy().into_owned(),
            ),
            (
                "PATH".to_string(),
                format!(
                    "{}:{}",
                    venv.join("bin").to_string_lossy(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            ),
        ]
    }

    // Scenario: tmpdir_vide_overlay_vide
    #[test]
    fn tmpdir_vide_overlay_vide() {
        let tmpdir = tempfile::tempdir().unwrap();
        assert_eq!(venv_overlay(tmpdir.path()), Vec::<(String, String)>::new());
    }

    // Scenario: dossier_venv_sans_marqueur_overlay_vide
    #[test]
    fn dossier_venv_sans_marqueur_overlay_vide() {
        let tmpdir = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmpdir.path().join(".venv")).unwrap();
        assert!(venv_overlay(tmpdir.path()).is_empty());
    }

    // Scenario: pyvenv_cfg_dossier_n_est_pas_un_marqueur
    #[test]
    fn pyvenv_cfg_dossier_n_est_pas_un_marqueur() {
        let tmpdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".venv").join("pyvenv.cfg")).unwrap();
        assert!(venv_overlay(tmpdir.path()).is_empty());
    }

    // Scenario: overlay_present_exactement_virtual_env_puis_path
    #[test]
    fn overlay_present_exactement_virtual_env_puis_path() {
        let tmpdir = tempfile::tempdir().unwrap();
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert_eq!(venv_overlay(tmpdir.path()), expected_overlay(tmpdir.path()));
    }

    // Scenario: venv_cree_entre_deux_appels_vu_des_le_second
    #[test]
    fn venv_cree_entre_deux_appels_vu_des_le_second() {
        let tmpdir = tempfile::tempdir().unwrap();
        assert!(venv_overlay(tmpdir.path()).is_empty());
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert_eq!(venv_overlay(tmpdir.path()).len(), 2);
    }

    // Scenario: racine_inexistente_overlay_vide
    #[test]
    fn racine_inexistente_overlay_vide() {
        let tmpdir = tempfile::tempdir().unwrap();
        let absent = tmpdir.path().join("absent");
        assert!(venv_overlay(&absent).is_empty());
    }

    // Scenario: venv_fichier_regulier_overlay_vide
    #[test]
    fn venv_fichier_regulier_overlay_vide() {
        let tmpdir = tempfile::tempdir().unwrap();
        std::fs::write(tmpdir.path().join(".venv"), "pas un dossier").unwrap();
        assert!(venv_overlay(tmpdir.path()).is_empty());
    }

    // Scenario: pyvenv_cfg_symlink_vers_fichier_est_un_marqueur
    #[test]
    fn pyvenv_cfg_symlink_vers_fichier_est_un_marqueur() {
        let tmpdir = tempfile::tempdir().unwrap();
        let real = tmpdir.path().join("real.cfg");
        std::fs::write(&real, "home = /usr/bin\n").unwrap();
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::os::unix::fs::symlink(&real, venv.join("pyvenv.cfg")).unwrap();
        assert_eq!(venv_overlay(tmpdir.path()).len(), 2);
    }

    // Scenario: pyvenv_cfg_symlink_casse_n_est_pas_un_marqueur
    #[test]
    fn pyvenv_cfg_symlink_casse_n_est_pas_un_marqueur() {
        let tmpdir = tempfile::tempdir().unwrap();
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::os::unix::fs::symlink(
            tmpdir.path().join("inexistant.cfg"),
            venv.join("pyvenv.cfg"),
        )
        .unwrap();
        assert!(venv_overlay(tmpdir.path()).is_empty());
    }

    // Scenario: repertoire_venv_inaccessible_traite_comme_absent
    // Suppose un processus soumis au DAC (non-root sans CAP_DAC_OVERRIDE, comme en CI) :
    // dans un conteneur à capacités élevées, mode 000 n'empêche pas le stat — ignoré là-bas.
    #[test]
    #[ignore = "exige un processus soumis au DAC (pas CAP_DAC_OVERRIDE) ; haiku-questions/sandbox.md"]
    fn repertoire_venv_inaccessible_traite_comme_absent() {
        use std::os::unix::fs::PermissionsExt;
        let tmpdir = tempfile::tempdir().unwrap();
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        std::fs::set_permissions(&venv, std::fs::Permissions::from_mode(0o000)).unwrap();
        let overlay = venv_overlay(tmpdir.path());
        // Restaure les droits avant les assertions pour que tempdir puisse nettoyer.
        std::fs::set_permissions(&venv, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(overlay.is_empty());
    }

    // Scenario: bin_absent_overlay_quand_meme_pose
    #[test]
    fn bin_absent_overlay_quand_meme_pose() {
        let tmpdir = tempfile::tempdir().unwrap();
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert!(!venv.join("bin").exists());
        assert_eq!(venv_overlay(tmpdir.path()), expected_overlay(tmpdir.path()));
    }

    // Scenario: deux_appels_successifs_path_identique
    #[test]
    fn deux_appels_successifs_path_identique() {
        let tmpdir = tempfile::tempdir().unwrap();
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert_eq!(venv_overlay(tmpdir.path()), venv_overlay(tmpdir.path()));
    }

    // Scenario: marqueur_vide_de_contenu_active_overlay
    #[test]
    fn marqueur_vide_de_contenu_active_overlay() {
        let tmpdir = tempfile::tempdir().unwrap();
        let venv = tmpdir.path().join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "").unwrap();
        assert_eq!(venv_overlay(tmpdir.path()).len(), 2);
    }

    // Scenario: chemin_non_utf8_values_en_fffd
    #[test]
    fn chemin_non_utf8_values_en_fffd() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let tmpdir = tempfile::tempdir().unwrap();
        let root = tmpdir.path().join(OsStr::from_bytes(b"nv\xFF"));
        let venv = root.join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        let overlay = venv_overlay(&root);
        assert_eq!(overlay.len(), 2);
        assert!(overlay[0].1.contains('\u{FFFD}'));
        assert!(overlay[1].1.starts_with(&format!("{}/bin:", overlay[0].1)));
    }
}
