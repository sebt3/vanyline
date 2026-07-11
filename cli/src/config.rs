use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use vanyline_lib::VnyError;

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
    base.join("vanyline")
}

pub fn ensure_config_dir() {
    std::fs::create_dir_all(config_dir()).unwrap_or_else(|e| {
        eprintln!("Failed to create config dir: {e}");
        std::process::exit(1);
    });
    std::fs::create_dir_all(config_dir().join("conversations")).unwrap_or_else(|e| {
        eprintln!("Failed to create conversations dir: {e}");
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
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return None,
        }
    }
}

/// Les deux couches de configuration. `workspace_dir` est `Some(<racine>/.vanyline)`
/// uniquement si `discover_workspace_root` a trouvé une racine.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Layers {
    pub global_dir: PathBuf,
    pub workspace_dir: Option<PathBuf>,
}

#[allow(dead_code)]
impl Layers {
    /// `start` : répertoire depuis lequel lancer la découverte workspace
    /// (typiquement `std::env::current_dir()` — doit être absolu).
    pub fn discover(start: &std::path::Path) -> Layers {
        Layers {
            global_dir: config_dir(),
            workspace_dir: discover_workspace_root(start).map(|root| root.join(".vanyline")),
        }
    }

    /// Charge et fusionne `config.yaml` des deux couches (voir `merge_config_layers`).
    pub fn load_merged_config(&self) -> Result<RawConfigFile, VnyError> {
        let global = load_config_layer(&self.global_dir)?;
        let workspace = match &self.workspace_dir {
            Some(dir) => Some(load_config_layer(dir)?),
            None => None,
        };
        Ok(merge_config_layers(global, workspace))
    }
}

/// Représentation brute de `config.yaml` — pas encore de types du domaine
/// (`Provider`, `ModelProfile`...), ça viendra en tâche 2 (`fs-store`). Les
/// clés des 4 maps sont les noms (`providers.strix`, `models.qwen-code`...) ;
/// les valeurs restent `yaml_serde::Value` non interprétées.
#[allow(dead_code)]
#[derive(Debug, Default, Clone, Deserialize)]
pub struct RawConfigFile {
    #[serde(default)]
    pub providers: BTreeMap<String, yaml_serde::Value>,
    #[serde(default)]
    pub models: BTreeMap<String, yaml_serde::Value>,
    #[serde(default)]
    pub mcp: BTreeMap<String, yaml_serde::Value>,
    #[serde(default)]
    pub defaults: BTreeMap<String, yaml_serde::Value>,
}

/// Lit `<dir>/config.yaml`. Fichier absent -> `RawConfigFile::default()` (pas
/// une erreur — une couche peut ne pas exister). YAML invalide ->
/// `VnyError::ConfigError` avec le chemin et le message d'erreur sous-jacent
/// (même convention que le parsing JSON existant dans `config_store.rs`).
#[allow(dead_code)]
pub fn load_config_layer(dir: &std::path::Path) -> Result<RawConfigFile, VnyError> {
    let path = dir.join("config.yaml");
    match std::fs::read_to_string(&path) {
        Ok(content) => yaml_serde::from_str(&content)
            .map_err(|e| VnyError::ConfigError(format!("Failed to parse {}: {}", path.display(), e))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RawConfigFile::default()),
        Err(e) => Err(VnyError::from(e)),
    }
}

/// Fusionne deux couches **clé par clé, au niveau de chaque map nommée**
/// (`providers`, `models`, `mcp`, `defaults`) : si `workspace` est `Some`, pour
/// chaque map, ses entrées sont insérées dans celles de `global` — une clé
/// présente dans les deux est intégralement remplacée par la valeur workspace
/// (pas de deep-merge intra-entrée, cf. design). `workspace: None` -> retourne
/// `global` inchangé.
#[allow(dead_code)]
pub fn merge_config_layers(global: RawConfigFile, workspace: Option<RawConfigFile>) -> RawConfigFile {
    let Some(ws) = workspace else { return global };
    let mut providers = global.providers;
    providers.extend(ws.providers);
    let mut models = global.models;
    models.extend(ws.models);
    let mut mcp = global.mcp;
    mcp.extend(ws.mcp);
    let mut defaults = global.defaults;
    defaults.extend(ws.defaults);
    RawConfigFile { providers, models, mcp, defaults }
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

    // --- load_config_layer ---

    #[test]
    fn missing_file_is_default() {
        let tmp = tempfile::tempdir().unwrap();
        let result = load_config_layer(tmp.path()).unwrap();
        assert!(result.providers.is_empty());
        assert!(result.models.is_empty());
        assert!(result.mcp.is_empty());
        assert!(result.defaults.is_empty());
    }

    #[test]
    fn parses_valid_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "\
providers:\n  strix:\n    type: openai\nmodels:\n  qwen-code:\n    max_tokens: 1024\nmcp:\n  server1:\n    url: http://localhost:8080\ndefaults:\n  agent: build\n",
        )
        .unwrap();
        let result = load_config_layer(tmp.path()).unwrap();
        assert!(result.providers.contains_key("strix"));
        assert!(result.models.contains_key("qwen-code"));
        assert!(result.mcp.contains_key("server1"));
        assert!(result.defaults.contains_key("agent"));
    }

    #[test]
    fn invalid_yaml_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "{invalid: [yaml\n").unwrap();
        let result = load_config_layer(tmp.path());
        match result {
            Err(VnyError::ConfigError(msg)) => assert!(
                msg.contains(&path.display().to_string()),
                "Error message should contain the path"
            ),
            other => panic!("Expected ConfigError, got {:?}", other),
        }
    }

    // --- merge_config_layers ---

    #[test]
    fn workspace_none_returns_global_unchanged() {
        let global = RawConfigFile {
            providers: BTreeMap::from([("strix".into(), yaml_serde::Value::String("x".into()))]),
            ..Default::default()
        };
        let result = merge_config_layers(global.clone(), None);
        assert_eq!(format!("{:?}", result), format!("{:?}", global));
    }

    #[test]
    fn disjoint_keys_union() {
        let global = RawConfigFile {
            providers: BTreeMap::from([("strix".into(), yaml_serde::Value::String("g".into()))]),
            ..Default::default()
        };
        let workspace = RawConfigFile {
            providers: BTreeMap::from([("local".into(), yaml_serde::Value::String("w".into()))]),
            ..Default::default()
        };
        let result = merge_config_layers(global, Some(workspace));
        assert!(result.providers.contains_key("strix"));
        assert!(result.providers.contains_key("local"));
        assert_eq!(result.providers.len(), 2);
    }

    #[test]
    fn colliding_key_workspace_wins_wholesale() {
        let global = RawConfigFile {
            models: BTreeMap::from([
                (
                    "qwen-code".into(),
                    yaml_serde::from_str("temperature: 0.1\nmax_tokens: 512").unwrap(),
                ),
            ]),
            ..Default::default()
        };
        let workspace = RawConfigFile {
            models: BTreeMap::from([
                (
                    "qwen-code".into(),
                    yaml_serde::from_str("temperature: 0.5").unwrap(),
                ),
            ]),
            ..Default::default()
        };
        let result = merge_config_layers(global, Some(workspace));
        // workspace value should replace entirely: no max_tokens
        let model_val = result.models.get("qwen-code").unwrap();
        let model_str = yaml_serde::to_string(model_val).unwrap();
        assert!(model_str.contains("temperature: 0.5"));
        assert!(!model_str.contains("max_tokens"));
    }

    #[test]
    fn defaults_key_by_key() {
        let global = RawConfigFile {
            defaults: BTreeMap::from([
                ("agent".into(), yaml_serde::Value::String("build".into())),
                ("autre".into(), yaml_serde::Value::String("x".into())),
            ]),
            ..Default::default()
        };
        let workspace = RawConfigFile {
            defaults: BTreeMap::from([("agent".into(), yaml_serde::Value::String("debug".into()))]),
            ..Default::default()
        };
        let result = merge_config_layers(global, Some(workspace));
        assert_eq!(result.defaults["agent"], "debug");
        assert_eq!(result.defaults["autre"], "x");
    }
}
