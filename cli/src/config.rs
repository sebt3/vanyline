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

pub fn ensure_config_dir() {
    std::fs::create_dir_all(config_dir()).unwrap_or_else(|e| {
        eprintln!("Failed to create config dir: {e}");
        std::process::exit(1);
    });
}

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
}
