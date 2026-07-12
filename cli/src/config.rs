use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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

    /// Résout les fichiers d'extension `ext` sous `<couche>/<subdir>/` des
    /// deux couches, fusionnés par nom (workspace gagne). `subdir` : ex.
    /// `"agents"`, `"toolsets"`.
    pub fn resolve_named_files(&self, subdir: &str, ext: &str) -> Result<BTreeMap<String, PathBuf>, VnyError> {
        let global = list_layer_files(&self.global_dir.join(subdir), ext)?;
        let workspace = match &self.workspace_dir {
            Some(dir) => Some(list_layer_files(&dir.join(subdir), ext)?),
            None => None,
        };
        Ok(merge_layer_files(global, workspace))
    }

    /// Résout les `SKILL.md` des deux couches (sous `<couche>/skills/`),
    /// fusionnés par nom de répertoire (workspace gagne) via
    /// `merge_layer_files` — même mécanique que `resolve_named_files`.
    pub fn resolve_skill_files(&self) -> Result<BTreeMap<String, PathBuf>, VnyError> {
        let global = list_layer_skill_dirs(&self.global_dir.join("skills"))?;
        let workspace = match &self.workspace_dir {
            Some(dir) => Some(list_layer_skill_dirs(&dir.join("skills"))?),
            None => None,
        };
        Ok(merge_layer_files(global, workspace))
    }
}

/// Représentation brute de `config.yaml` — pas encore de types du domaine
/// (`Provider`, `ModelProfile`...), ça viendra en tâche 2 (`fs-store`). Les
/// clés des 4 maps sont les noms (`providers.strix`, `models.qwen-code`...) ;
/// les valeurs restent `yaml_serde::Value` non interprétées.
#[allow(dead_code)]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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

/// Liste les fichiers d'extension `ext` (sans le point, ex. `"md"`)
/// directement sous `dir` (non récursif — les sous-répertoires, ex.
/// `skills/<name>/`, sont ignorés), indexés par `stem` (nom de fichier sans
/// extension). `dir` absent -> map vide, PAS une erreur (une couche peut ne
/// pas avoir ce sous-répertoire).
#[allow(dead_code)]
pub fn list_layer_files(dir: &std::path::Path, ext: &str) -> Result<BTreeMap<String, PathBuf>, VnyError> {
    let mut result = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(VnyError::from(e)),
    };
    for entry in entries {
        let entry = entry.map_err(VnyError::from)?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some(ext) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                result.insert(stem.to_string(), path);
            }
        }
    }
    Ok(result)
}

/// Fusionne deux maps stem -> chemin : `workspace` (si `Some`) remplace
/// `global` à stem égal (remplacement de la valeur, comme
/// `merge_config_layers`). `None` -> `global` inchangé.
#[allow(dead_code)]
pub fn merge_layer_files(
    global: BTreeMap<String, PathBuf>,
    workspace: Option<BTreeMap<String, PathBuf>>,
) -> BTreeMap<String, PathBuf> {
    let Some(ws) = workspace else { return global };
    let mut merged = global;
    merged.extend(ws);
    merged
}

/// Liste les sous-répertoires directs de `dir` qui contiennent un fichier
/// `SKILL.md`, indexés par nom de répertoire, valeur = chemin vers ce
/// `SKILL.md` (pas le répertoire lui-même — évite de le rejoindre à chaque
/// usage). Un sous-répertoire sans `SKILL.md` est ignoré silencieusement.
/// `dir` absent -> map vide (comme `list_layer_files`).
#[allow(dead_code)]
pub fn list_layer_skill_dirs(dir: &std::path::Path) -> Result<BTreeMap<String, PathBuf>, VnyError> {
    let mut result = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(VnyError::from(e)),
    };
    for entry in entries {
        let entry = entry.map_err(VnyError::from)?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_file = path.join("SKILL.md");
        if skill_file.is_file() {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                result.insert(name.to_string(), skill_file);
            }
        }
    }
    Ok(result)
}

/// "workspace" si `name` est présent dans la map choisie (`pick`) du
/// `config.yaml` de la couche workspace de `layers`, "global" sinon — y
/// compris si `layers.workspace_dir` est `None`, ou si son `config.yaml`
/// est illisible/invalide (cette fonction n'est pas le lieu pour remonter
/// une erreur de parsing — `list_x()`/`config check` s'en chargent déjà).
pub fn config_entry_source(
    layers: &Layers,
    name: &str,
    pick: fn(&RawConfigFile) -> &BTreeMap<String, yaml_serde::Value>,
) -> &'static str {
    match &layers.workspace_dir {
        Some(dir) => match load_config_layer(dir) {
            Ok(raw) if pick(&raw).contains_key(name) => "workspace",
            _ => "global",
        },
        None => "global",
    }
}

/// "workspace" si `name` est présent parmi les fichiers d'extension `ext`
/// sous `<workspace>/<subdir>/`. Même permissivité que `config_entry_source`
/// en cas d'erreur de lecture.
pub fn file_entry_source(layers: &Layers, subdir: &str, ext: &str, name: &str) -> &'static str {
    match &layers.workspace_dir {
        Some(dir) => match list_layer_files(&dir.join(subdir), ext) {
            Ok(files) if files.contains_key(name) => "workspace",
            _ => "global",
        },
        None => "global",
    }
}

/// Équivalent de `file_entry_source` pour `skills/<name>/SKILL.md`
/// (découverte par répertoire, pas par extension — cf.
/// `list_layer_skill_dirs`).
pub fn skill_entry_source(layers: &Layers, name: &str) -> &'static str {
    match &layers.workspace_dir {
        Some(dir) => match list_layer_skill_dirs(&dir.join("skills")) {
            Ok(files) if files.contains_key(name) => "workspace",
            _ => "global",
        },
        None => "global",
    }
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
    raw.defaults
        .insert("agent".to_string(), yaml_serde::Value::String(name.to_string()));
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

    // --- list_layer_files ---

    #[test]
    fn list_layer_files_finds_matching_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let path_a = tmp.path().join("a.md");
        let path_b = tmp.path().join("b.md");
        let path_c = tmp.path().join("c.txt");
        std::fs::write(&path_a, "# A").unwrap();
        std::fs::write(&path_b, "# B").unwrap();
        std::fs::write(&path_c, "text").unwrap();
        let result = list_layer_files(tmp.path(), "md").unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("a"));
        assert!(result.contains_key("b"));
        assert!(!result.contains_key("c"));
    }

    #[test]
    fn list_layer_files_ignores_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        let path_a = tmp.path().join("a.md");
        std::fs::write(&path_a, "# A").unwrap();
        std::fs::create_dir(tmp.path().join("sub.md")).unwrap();
        let result = list_layer_files(tmp.path(), "md").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("a"));
    }

    #[test]
    fn list_layer_files_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does_not_exist");
        let result = list_layer_files(&missing, "md").unwrap();
        assert!(result.is_empty());
    }

    // --- merge_layer_files ---

    #[test]
    fn merge_layer_files_workspace_overrides() {
        let tmp_a = tempfile::tempdir().unwrap();
        let tmp_b = tempfile::tempdir().unwrap();
        let tmp_c = tempfile::tempdir().unwrap();
        let global = BTreeMap::from([("build".into(), tmp_a.path().to_path_buf())]);
        let workspace = BTreeMap::from([
            ("build".into(), tmp_b.path().to_path_buf()),
            ("debug".into(), tmp_c.path().to_path_buf()),
        ]);
        let result = merge_layer_files(global, Some(workspace));
        assert_eq!(result.len(), 2);
        assert!(
            result["build"] == tmp_b.path(),
            "build should be overridden by workspace path"
        );
        assert!(result.contains_key("debug"));
        assert!(result["debug"] == tmp_c.path());
    }

    // --- resolve_named_files ---

    #[test]
    fn resolve_named_files_merges_across_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::create_dir_all(workspace.join("agents")).unwrap();
        std::fs::write(global.join("agents").join("build.md"), "# build").unwrap();
        std::fs::write(workspace.join("agents").join("debug.md"), "# debug").unwrap();
        let layers = Layers {
            global_dir: global,
            workspace_dir: Some(workspace),
        };
        let result = layers.resolve_named_files("agents", "md").unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("build"));
        assert!(result.contains_key("debug"));
    }

    // --- list_layer_skill_dirs ---

    #[test]
    fn list_layer_skill_dirs_finds_dirs_with_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("pdf")).unwrap();
        std::fs::create_dir_all(tmp.path().join("empty-dir")).unwrap();
        std::fs::write(tmp.path().join("pdf").join("SKILL.md"), "---\ndescription: PDF\n---\n").unwrap();
        let result = list_layer_skill_dirs(tmp.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("pdf"));
        assert_eq!(
            result["pdf"],
            tmp.path().join("pdf").join("SKILL.md")
        );
    }

    #[test]
    fn list_layer_skill_dirs_ignores_plain_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("not-a-skill.md"), "---\n---\n").unwrap();
        let result = list_layer_skill_dirs(tmp.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_layer_skill_dirs_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does_not_exist");
        let result = list_layer_skill_dirs(&missing).unwrap();
        assert!(result.is_empty());
    }

    // --- resolve_skill_files ---

    #[test]
    fn resolve_skill_files_merges_across_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(global.join("skills").join("pdf")).unwrap();
        std::fs::create_dir_all(workspace.join("skills").join("csv")).unwrap();
        std::fs::write(global.join("skills").join("pdf").join("SKILL.md"), "---\n---\n").unwrap();
        std::fs::write(workspace.join("skills").join("csv").join("SKILL.md"), "---\n---\n").unwrap();
        let layers = Layers {
            global_dir: global,
            workspace_dir: Some(workspace),
        };
        let result = layers.resolve_skill_files().unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("pdf"));
        assert!(result.contains_key("csv"));
    }

    // --- config_entry_source ---

    #[test]
    fn config_entry_source_workspace_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(
            ws_dir.join("config.yaml"),
            "models:\n  qwen-code:\n    provider: ollama\n    model: qwen2\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: PathBuf::from("/nonexistent"),
            workspace_dir: Some(ws_dir),
        };
        let result = config_entry_source(&layers, "qwen-code", |r| &r.models);
        assert_eq!(result, "workspace");
    }

    #[test]
    fn config_entry_source_global_fallback_no_workspace() {
        let layers = Layers {
            global_dir: PathBuf::from("/nonexistent"),
            workspace_dir: None,
        };
        let result = config_entry_source(&layers, "any-key", |r| &r.models);
        assert_eq!(result, "global");
    }

    #[test]
    fn config_entry_source_global_fallback_absent_in_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(
            ws_dir.join("config.yaml"),
            "models:\n  other-model:\n    provider: openai\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: PathBuf::from("/nonexistent"),
            workspace_dir: Some(ws_dir),
        };
        let result = config_entry_source(&layers, "nonexistent-model", |r| &r.models);
        assert_eq!(result, "global");
    }

    // --- file_entry_source ---

    #[test]
    fn file_entry_source_workspace_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("workspace");
        let toolsets_dir = ws_dir.join("toolsets");
        std::fs::create_dir_all(&toolsets_dir).unwrap();
        std::fs::write(toolsets_dir.join("grafana.yaml"), "description: Grafana\n").unwrap();
        let layers = Layers {
            global_dir: PathBuf::from("/nonexistent"),
            workspace_dir: Some(ws_dir),
        };
        let result = file_entry_source(&layers, "toolsets", "yaml", "grafana");
        assert_eq!(result, "workspace");
    }

    #[test]
    fn file_entry_source_global_fallback_no_workspace() {
        let layers = Layers {
            global_dir: PathBuf::from("/nonexistent"),
            workspace_dir: None,
        };
        let result = file_entry_source(&layers, "toolsets", "yaml", "grafana");
        assert_eq!(result, "global");
    }

    // --- skill_entry_source ---

    #[test]
    fn skill_entry_source_workspace_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("workspace");
        let skills_dir = ws_dir.join("skills").join("pdf");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("SKILL.md"),
            "---\ndescription: PDF\n---\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: PathBuf::from("/nonexistent"),
            workspace_dir: Some(ws_dir),
        };
        let result = skill_entry_source(&layers, "pdf");
        assert_eq!(result, "workspace");
    }

    #[test]
    fn skill_entry_source_global_fallback_no_workspace() {
        let layers = Layers {
            global_dir: PathBuf::from("/nonexistent"),
            workspace_dir: None,
        };
        let result = skill_entry_source(&layers, "pdf");
        assert_eq!(result, "global");
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
        std::fs::write(
            &config_path,
            "defaults:\n  agent: old\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        set_default_agent(&layers, "new").unwrap();
        let raw = load_config_layer(&layers.global_dir).unwrap();
        let agent = raw.defaults.get("agent").unwrap();
        assert_eq!(agent.as_str().unwrap(), "new");
        // Ensure there's only one entry for "agent" (the BTreeMap should have exactly one)
        let agent_count = raw.defaults.values().filter(|v| {
            if let Some(s) = v.as_str() {
                s == "new"
            } else {
                false
            }
        }).count();
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
