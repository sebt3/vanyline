use std::path::PathBuf;

use vanyline_cfgstore::layers::Layers;

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
    base.join("vanyline")
}

pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("~/.local/share"));
    base.join("vanyline")
}

// tooling.sdd D6a — config.sdd §Raises/@Returns : échec de création ⇒ stderr
// commençant par `Failed to create config dir: ` suivi de l'erreur d'I/O, puis
// exit(1) (Scenario dédié). La question de la FORME (tracing/propagation,
// survie du code 1) reste le [?] ouvert config.sdd Tasks l.205 — non tranchée
// ici.
#[allow(clippy::print_stderr)]
pub fn ensure_config_dir() {
    std::fs::create_dir_all(config_dir()).unwrap_or_else(|e| {
        eprintln!("Failed to create config dir: {e}");
        std::process::exit(1);
    });
}

// tooling.sdd D6a — config.sdd §Raises : @ensure_data_dir en échec, même motif
// avec le préfixe `Failed to create data dir: ` + exit(1) (Scenario dédié).
// Cf. [?] config.sdd Tasks l.205 pour la forme.
#[allow(clippy::print_stderr)]
pub fn ensure_data_dir() {
    std::fs::create_dir_all(data_dir().join("conversations")).unwrap_or_else(|e| {
        eprintln!("Failed to create data dir: {e}");
        std::process::exit(1);
    });
}

#[allow(dead_code)]
/// Remonte l'arborescence depuis `start` (doit être un chemin absolu) jusqu'à
/// trouver un répertoire contenant `.vanyline/` (dossier) OU `.git` (dossier
/// OU fichier — cas des worktrees git). Le premier trouvé, quel que soit le
/// marqueur, fixe la racine. `None` si aucun marqueur jusqu'à la racine du
/// système de fichiers.
pub fn discover_workspace_root(start: &std::path::Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".vanyline").is_dir() || current.join(".git").exists() {
            return Some(current);
        }
        let parent = current.parent()?;
        current = parent.to_path_buf();
    }
}

pub fn discover_layers(start: &std::path::Path) -> Layers {
    Layers {
        global_dir: config_dir(),
        workspace_dir: discover_workspace_root(start).map(|root| root.join(".vanyline")),
    }
}

/// Namespace par défaut configuré dans `config.yaml` (clé `defaults.namespace`,
/// fusionnée sur les deux couches comme le reste de `defaults` — cf.
/// `merge_config_layers`). `None` si absent des deux couches, ou si la
/// valeur n'est pas une chaîne. Ne consulte JAMAIS le kubeconfig — c'est
/// `VnlK8sClient::discover` qui s'en charge en dernier recours.
pub fn configured_namespace(layers: &Layers) -> Option<String> {
    layers.load_merged_config().ok().and_then(|raw| {
        raw.defaults
            .get("namespace")
            .and_then(|v| v.as_str().map(String::from))
    })
}

/// Nom de la sandbox toolbox configuree par defaut (`defaults.toolbox`
/// du `config.yaml` fusionne). Meme mecanique que `configured_namespace`.
pub fn configured_toolbox(layers: &Layers) -> Option<String> {
    layers.load_merged_config().ok().and_then(|raw| {
        raw.defaults
            .get("toolbox")
            .and_then(|v| v.as_str().map(String::from))
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    // --- discover_workspace_root ---

    #[test]
    fn finds_vanyline_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".vanyline")).unwrap();
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        let result = discover_workspace_root(&root.join("sub/deep")).unwrap();
        assert_eq!(result, root);
    }

    #[test]
    fn finds_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let result = discover_workspace_root(&root.join("sub")).unwrap();
        assert_eq!(result, root);
    }

    #[test]
    fn finds_git_file_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut f = std::fs::File::create(root.join(".git")).unwrap();
        f.write_all(b"gitdir: /elsewhere").unwrap();
        drop(f);
        let result = discover_workspace_root(root).unwrap();
        assert_eq!(result, root);
    }

    #[test]
    fn closest_marker_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("sub/.vanyline")).unwrap();
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        let result = discover_workspace_root(&root.join("sub/deep")).unwrap();
        assert_eq!(result, root.join("sub"));
    }

    #[test]
    fn no_marker_returns_none() {
        // Use /tmp to avoid crossing the repo's own .git
        let tmp = tempfile::tempdir_in("/tmp").unwrap();
        let subdir = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&subdir).unwrap();
        let result = discover_workspace_root(&subdir);
        assert!(result.is_none());
    }

    // --- configured_namespace / configured_toolbox (couches temporaires, sans env) ---

    fn layers_with(
        global: Option<&str>,
        workspace: Option<&str>,
    ) -> (tempfile::TempDir, tempfile::TempDir, Layers) {
        let g = tempfile::tempdir().unwrap();
        let w = tempfile::tempdir().unwrap();
        if let Some(c) = global {
            std::fs::write(g.path().join("config.yaml"), c).unwrap();
        }
        if let Some(c) = workspace {
            std::fs::write(w.path().join("config.yaml"), c).unwrap();
        }
        let layers = Layers {
            global_dir: g.path().to_path_buf(),
            workspace_dir: workspace.map(|_| w.path().to_path_buf()),
        };
        (g, w, layers)
    }

    // Scenario: configured_namespace lit la valeur de la couche globale
    #[test]
    fn configured_namespace_lit_la_valeur_de_la_couche_globale() {
        let (_g, _w, layers) = layers_with(Some("defaults:\n  namespace: prod\n"), None);
        assert_eq!(configured_namespace(&layers).as_deref(), Some("prod"));
    }

    // Scenario: la couche workspace ecrase defaults.namespace
    #[test]
    fn la_couche_workspace_ecrase_defaults_namespace() {
        let (_g, _w, layers) = layers_with(
            Some("defaults:\n  namespace: prod\n"),
            Some("defaults:\n  namespace: dev\n"),
        );
        assert_eq!(configured_namespace(&layers).as_deref(), Some("dev"));
    }

    // Scenario: configured_namespace ignore une valeur non textuelle
    #[test]
    fn configured_namespace_ignore_une_valeur_non_textuelle() {
        let (_g, _w, layers) = layers_with(Some("defaults:\n  namespace: 3\n"), None);
        assert_eq!(configured_namespace(&layers), None);
    }

    // Scenario: configured_namespace retourne None si une couche est invalide
    #[test]
    fn configured_namespace_retourne_none_si_une_couche_est_invalide() {
        let (_g, _w, layers) = layers_with(
            Some("defaults:\n  namespace: prod\n"),
            Some("defaults: [unclosed"),
        );
        assert_eq!(configured_namespace(&layers), None);
    }

    // Scenario: configured_toolbox applique la meme fusion que configured_namespace
    #[test]
    fn configured_toolbox_applique_la_meme_fusion_que_configured_namespace() {
        let (_g, _w, layers) = layers_with(
            Some("defaults:\n  toolbox: base\n"),
            Some("defaults:\n  toolbox: labs\n"),
        );
        assert_eq!(configured_toolbox(&layers).as_deref(), Some("labs"));
        let sans_workspace = Layers {
            global_dir: layers.global_dir.clone(),
            workspace_dir: None,
        };
        assert_eq!(configured_toolbox(&sans_workspace).as_deref(), Some("base"));
    }

    // Scenario: defaults absents de toute couche donnent None
    #[test]
    fn defaults_absents_de_toute_couche_donnent_none() {
        let (_g, _w, layers) = layers_with(None, None);
        assert_eq!(configured_namespace(&layers), None);
        assert_eq!(configured_toolbox(&layers), None);
    }
}
