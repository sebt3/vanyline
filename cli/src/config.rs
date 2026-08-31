use std::path::PathBuf;

use vanyline_cfgstore::layers::{Layers, load_config_layer};
use vanyline_lib::VnyError;

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

/// Écrit `defaults.agent = name` dans le `config.yaml` de la couche globale
/// de `layers` (jamais la couche workspace). Relit l'existant (couche
/// absente -> `RawConfigFile::default()`), mute uniquement `defaults.agent`,
/// réécrit le fichier en entier — providers/models/mcp/autres clés
/// `defaults` sont préservées EN CONTENU mais pas en formatting (pas de
/// commentaires, ordre des clés = ordre `BTreeMap`, cf. `yaml_serde`).
/// Crée `layers.global_dir` s'il n'existe pas encore.
#[allow(dead_code)]
pub fn set_default_agent(layers: &Layers, name: &str) -> Result<(), VnyError> {
    let mut raw = load_config_layer(&layers.global_dir)?;
    // CfgStoreError -> VnyError via From (0a)
    raw.defaults.insert(
        "agent".to_string(),
        yaml_serde::Value::String(name.to_string()),
    );
    std::fs::create_dir_all(&layers.global_dir).map_err(VnyError::from)?;
    let path = layers.global_dir.join("config.yaml");
    let content = yaml_serde::to_string(&raw).map_err(|e| {
        VnyError::ConfigError(format!("Failed to serialize {}: {}", path.display(), e))
    })?;
    std::fs::write(&path, content).map_err(VnyError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use vanyline_cfgstore::layers::{
        config_entry_source, file_entry_source, list_layer_files, list_layer_skill_dirs,
        load_config_layer, merge_config_layers, merge_layer_files, skill_entry_source,
    };

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

    // --- set_default_agent ---

    #[test]
    fn set_default_agent_writes_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        set_default_agent(&layers, "build").unwrap();
        let raw = load_config_layer(&layers.global_dir).unwrap();
        assert_eq!(
            raw.defaults.get("agent"),
            Some(&yaml_serde::Value::String("build".into()))
        );
    }

    #[test]
    fn set_default_agent_preserves_existing_content() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "providers:\n  strix:\n    type: openai-compatible\n    endpoint: http://localhost\nmodels:\n  qwen-code:\n    provider: ollama\n    model: qwen2.5\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        set_default_agent(&layers, "build").unwrap();
        let raw = load_config_layer(&layers.global_dir).unwrap();
        assert!(raw.providers.contains_key("strix"));
        assert!(raw.models.contains_key("qwen-code"));
        assert_eq!(
            raw.defaults.get("agent"),
            Some(&yaml_serde::Value::String("build".into()))
        );
    }

    #[test]
    fn set_default_agent_overwrites_existing_default() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.yaml");
        std::fs::write(&config_path, "defaults:\n  agent: old\n").unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        set_default_agent(&layers, "new").unwrap();
        let raw = load_config_layer(&layers.global_dir).unwrap();
        let agent = raw.defaults.get("agent").unwrap();
        assert_eq!(agent.as_str().unwrap(), "new");
        // Ensure there's only one entry for "agent" (the BTreeMap should have exactly one)
        let agent_count = raw
            .defaults
            .values()
            .filter(|v| {
                if let Some(s) = v.as_str() {
                    s == "new"
                } else {
                    false
                }
            })
            .count();
        assert_eq!(agent_count, 1);
    }

    #[test]
    fn set_default_agent_creates_missing_global_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let non_existent = tmp.path().join("does-not-exist-yet");
        assert!(!non_existent.exists());
        let layers = Layers {
            global_dir: non_existent.clone(),
            workspace_dir: None,
        };
        set_default_agent(&layers, "build").unwrap();
        assert!(non_existent.is_dir());
        let raw = load_config_layer(&layers.global_dir).unwrap();
        assert_eq!(
            raw.defaults.get("agent"),
            Some(&yaml_serde::Value::String("build".into()))
        );
    }
}
