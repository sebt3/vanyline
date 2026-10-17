use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::CfgStoreError;

/// Paires de répertoires de couches : global obligatoire, workspace
/// optionnel. Pointe vers les racines contenant `config.yaml` et les
/// sous-répertoires `agents/`, `toolsets/`, `skills/`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Layers {
    /// Répertoire de la couche globale.
    pub global_dir: PathBuf,
    /// Répertoire de la couche workspace, absente si `None`.
    pub workspace_dir: Option<PathBuf>,
}

#[allow(dead_code)]
impl Layers {
    /// Charge et fusionne le `config.yaml` des couches (global + workspace si
    /// présente).
    ///
    /// # Errors
    ///
    /// `CfgStoreError::Config` si un `config.yaml` est illisible (YAML
    /// invalide), `CfgStoreError::Io` pour toute autre erreur d'E/S.
    pub fn load_merged_config(&self) -> Result<RawConfigFile, CfgStoreError> {
        let global = load_config_layer(&self.global_dir)?;
        let workspace = match &self.workspace_dir {
            Some(dir) => Some(load_config_layer(dir)?),
            None => None,
        };
        Ok(merge_config_layers(global, workspace))
    }

    /// Découvre les fichiers d'extension `ext` sous `<couche>/<subdir>/` pour
    /// chaque couche, fusionnés (workspace gagne à stem égal).
    ///
    /// # Errors
    ///
    /// `CfgStoreError::Io` en cas d'erreur de lecture de répertoire (hors
    /// répertoire absent, traité comme vide).
    pub fn resolve_named_files(
        &self,
        subdir: &str,
        ext: &str,
    ) -> Result<BTreeMap<String, PathBuf>, CfgStoreError> {
        let global = list_layer_files(&self.global_dir.join(subdir), ext)?;
        let workspace = match &self.workspace_dir {
            Some(dir) => Some(list_layer_files(&dir.join(subdir), ext)?),
            None => None,
        };
        Ok(merge_layer_files(global, workspace))
    }

    /// Découvre les `skills/<nom>/SKILL.md` des deux couches, fusionnés
    /// (workspace gagne à nom égal).
    ///
    /// # Errors
    ///
    /// `CfgStoreError::Io` en cas d'erreur de lecture de répertoire (hors
    /// répertoire absent, traité comme vide).
    pub fn resolve_skill_files(&self) -> Result<BTreeMap<String, PathBuf>, CfgStoreError> {
        let global = list_layer_skill_dirs(&self.global_dir.join("skills"))?;
        let workspace = match &self.workspace_dir {
            Some(dir) => Some(list_layer_skill_dirs(&dir.join("skills"))?),
            None => None,
        };
        Ok(merge_layer_files(global, workspace))
    }
}

/// Contenu brut d'un `config.yaml` : maps nommées non typées, fusionnables
/// couche par couche (le typage du domaine se fait à la lecture, pas ici).
#[allow(dead_code)]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RawConfigFile {
    /// Map `providers:` brute, indexée par nom.
    #[serde(default)]
    pub providers: BTreeMap<String, yaml_serde::Value>,
    /// Map `models:` brute, indexée par nom.
    #[serde(default)]
    pub models: BTreeMap<String, yaml_serde::Value>,
    /// Map `mcp:` brute, indexée par nom.
    #[serde(default)]
    pub mcp: BTreeMap<String, yaml_serde::Value>,
    /// Map `defaults:` brute (ex. clé `agent` = agent par défaut).
    #[serde(default)]
    pub defaults: BTreeMap<String, yaml_serde::Value>,
}

/// Charge `<dir>/config.yaml` ; fichier absent -> `RawConfigFile::default()`
/// (une couche peut n'avoir aucun `config.yaml`).
///
/// # Errors
///
/// `CfgStoreError::Config` si le YAML est invalide, `CfgStoreError::Io` pour
/// toute autre erreur de lecture.
#[allow(dead_code)]
pub fn load_config_layer(dir: &std::path::Path) -> Result<RawConfigFile, CfgStoreError> {
    let path = dir.join("config.yaml");
    match std::fs::read_to_string(&path) {
        Ok(content) => yaml_serde::from_str(&content).map_err(|e| {
            CfgStoreError::Config(format!("Failed to parse {}: {}", path.display(), e))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RawConfigFile::default()),
        Err(e) => Err(CfgStoreError::from(e)),
    }
}

/// Fusionne deux couches **clé par clé, au niveau de chaque map nommée**
/// (`providers`, `models`, `mcp`, `defaults`) : si `workspace` est `Some`, pour
/// chaque map, ses entrées sont insérées dans celles de `global` — une clé
/// présente dans les deux est intégralement remplacée par la valeur workspace
/// (pas de deep-merge intra-entrée, cf. design). `workspace: None` -> retourne
/// `global` inchangé.
#[allow(dead_code)]
#[must_use]
pub fn merge_config_layers(
    global: RawConfigFile,
    workspace: Option<RawConfigFile>,
) -> RawConfigFile {
    let Some(ws) = workspace else { return global };
    let mut providers = global.providers;
    providers.extend(ws.providers);
    let mut models = global.models;
    models.extend(ws.models);
    let mut mcp = global.mcp;
    mcp.extend(ws.mcp);
    let mut defaults = global.defaults;
    defaults.extend(ws.defaults);
    RawConfigFile {
        providers,
        models,
        mcp,
        defaults,
    }
}

/// Liste les fichiers d'extension `ext` (sans le point, ex. `"md"`)
/// directement sous `dir` (non récursif — les sous-répertoires, ex.
/// `skills/<name>/`, sont ignorés), indexés par `stem` (nom de fichier sans
/// extension). `dir` absent -> map vide, PAS une erreur (une couche peut ne
/// pas avoir ce sous-répertoire).
///
/// # Errors
///
/// `CfgStoreError::Io` en cas d'erreur de lecture de répertoire (hors
/// répertoire absent, traité comme vide).
#[allow(dead_code)]
pub fn list_layer_files(
    dir: &std::path::Path,
    ext: &str,
) -> Result<BTreeMap<String, PathBuf>, CfgStoreError> {
    let mut result = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(CfgStoreError::from(e)),
    };
    for entry in entries {
        let entry = entry.map_err(CfgStoreError::from)?;
        let path = entry.path();
        if path.is_file()
            && path.extension().and_then(|s| s.to_str()) == Some(ext)
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            result.insert(stem.to_string(), path);
        }
    }
    Ok(result)
}

/// Fusionne deux maps stem -> chemin : `workspace` (si `Some`) remplace
/// `global` à stem égal (remplacement de la valeur, comme
/// `merge_config_layers`). `None` -> `global` inchangé.
#[allow(dead_code)]
#[must_use]
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
///
/// # Errors
///
/// `CfgStoreError::Io` en cas d'erreur de lecture de répertoire (hors
/// répertoire absent, traité comme vide).
#[allow(dead_code)]
pub fn list_layer_skill_dirs(
    dir: &std::path::Path,
) -> Result<BTreeMap<String, PathBuf>, CfgStoreError> {
    let mut result = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(CfgStoreError::from(e)),
    };
    for entry in entries {
        let entry = entry.map_err(CfgStoreError::from)?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_file = path.join("SKILL.md");
        if skill_file.is_file()
            && let Some(name) = path.file_name().and_then(|s| s.to_str())
        {
            result.insert(name.to_string(), skill_file);
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
#[must_use]
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
#[must_use]
pub fn skill_entry_source(layers: &Layers, name: &str) -> &'static str {
    match &layers.workspace_dir {
        Some(dir) => match list_layer_skill_dirs(&dir.join("skills")) {
            Ok(files) if files.contains_key(name) => "workspace",
            _ => "global",
        },
        None => "global",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use tempfile::tempdir;

    use super::*;

    // --- load_config_layer ---

    #[test]
    fn missing_file_is_default() {
        let tmp = tempdir().unwrap();
        let result = load_config_layer(tmp.path()).unwrap();
        assert!(result.providers.is_empty());
        assert!(result.models.is_empty());
        assert!(result.mcp.is_empty());
        assert!(result.defaults.is_empty());
    }

    #[test]
    fn parses_valid_yaml() {
        let tmp = tempdir().unwrap();
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
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "{invalid: [yaml\n").unwrap();
        let result = load_config_layer(tmp.path());
        match result {
            Err(CfgStoreError::Config(msg)) => assert!(
                msg.contains(&path.display().to_string()),
                "Error message should contain the path"
            ),
            other => panic!("Expected ConfigError, got {other:?}"),
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
        assert_eq!(format!("{result:?}"), format!("{:?}", global));
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
            models: BTreeMap::from([(
                "qwen-code".into(),
                yaml_serde::from_str("temperature: 0.1\nmax_tokens: 512").unwrap(),
            )]),
            ..Default::default()
        };
        let workspace = RawConfigFile {
            models: BTreeMap::from([(
                "qwen-code".into(),
                yaml_serde::from_str("temperature: 0.5").unwrap(),
            )]),
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
        let tmp = tempdir().unwrap();
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
        let tmp = tempdir().unwrap();
        let path_a = tmp.path().join("a.md");
        std::fs::write(&path_a, "# A").unwrap();
        std::fs::create_dir(tmp.path().join("sub.md")).unwrap();
        let result = list_layer_files(tmp.path(), "md").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("a"));
    }

    #[test]
    fn list_layer_files_missing_dir_is_empty() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does_not_exist");
        let result = list_layer_files(&missing, "md").unwrap();
        assert!(result.is_empty());
    }

    // --- merge_layer_files ---

    #[test]
    fn merge_layer_files_workspace_overrides() {
        let tmp_a = tempdir().unwrap();
        let tmp_b = tempdir().unwrap();
        let tmp_c = tempdir().unwrap();
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
        assert_eq!(result["debug"], tmp_c.path());
    }

    // --- resolve_named_files ---

    #[test]
    fn resolve_named_files_merges_across_layers() {
        let tmp = tempdir().unwrap();
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
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("pdf")).unwrap();
        std::fs::create_dir_all(tmp.path().join("empty-dir")).unwrap();
        std::fs::write(
            tmp.path().join("pdf").join("SKILL.md"),
            "---\ndescription: PDF\n---\n",
        )
        .unwrap();
        let result = list_layer_skill_dirs(tmp.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("pdf"));
        assert_eq!(result["pdf"], tmp.path().join("pdf").join("SKILL.md"));
    }

    #[test]
    fn list_layer_skill_dirs_ignores_plain_files() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("not-a-skill.md"), "---\n---\n").unwrap();
        let result = list_layer_skill_dirs(tmp.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_layer_skill_dirs_missing_dir_is_empty() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does_not_exist");
        let result = list_layer_skill_dirs(&missing).unwrap();
        assert!(result.is_empty());
    }

    // --- resolve_skill_files ---

    #[test]
    fn resolve_skill_files_merges_across_layers() {
        let tmp = tempdir().unwrap();
        let global = tmp.path().join("global");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(global.join("skills").join("pdf")).unwrap();
        std::fs::create_dir_all(workspace.join("skills").join("csv")).unwrap();
        std::fs::write(
            global.join("skills").join("pdf").join("SKILL.md"),
            "---\n---\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join("skills").join("csv").join("SKILL.md"),
            "---\n---\n",
        )
        .unwrap();
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
        let tmp = tempdir().unwrap();
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
        let tmp = tempdir().unwrap();
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
        let tmp = tempdir().unwrap();
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
        let tmp = tempdir().unwrap();
        let ws_dir = tmp.path().join("workspace");
        let skills_dir = ws_dir.join("skills").join("pdf");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(skills_dir.join("SKILL.md"), "---\ndescription: PDF\n---\n").unwrap();
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

    #[test]
    fn load_config_layer_io_non_not_found_rend_cfg_004() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("config.yaml")).unwrap();
        let err = load_config_layer(tmp.path()).unwrap_err();
        assert!(matches!(err, CfgStoreError::Io(_)));
        assert!(err.to_string().starts_with("VNL-CFG-004"));
    }

    #[test]
    fn load_merged_config_fusionne_global_et_workspace() {
        let global = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        std::fs::write(
            global.path().join("config.yaml"),
            "providers:\n  strix:\n    type: ollama\ndefaults:\n  agent: build\n",
        )
        .unwrap();
        std::fs::write(
            workspace.path().join("config.yaml"),
            "models:\n  qwen-code:\n    provider: strix\ndefaults:\n  agent: debug\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: global.path().to_path_buf(),
            workspace_dir: Some(workspace.path().to_path_buf()),
        };
        let merged = layers.load_merged_config().unwrap();
        assert!(merged.providers.contains_key("strix"));
        assert!(merged.models.contains_key("qwen-code"));
        assert_eq!(
            merged.defaults.get("agent").and_then(|v| v.as_str()),
            Some("debug")
        );
    }

    #[test]
    fn load_merged_config_propage_erreur_du_workspace_invalide() {
        let global = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        std::fs::write(
            global.path().join("config.yaml"),
            "defaults:\n  agent: build\n",
        )
        .unwrap();
        let ws_path = workspace.path().join("config.yaml");
        std::fs::write(&ws_path, "providers: [\n  : invalide\n").unwrap();
        let layers = Layers {
            global_dir: global.path().to_path_buf(),
            workspace_dir: Some(workspace.path().to_path_buf()),
        };
        match layers.load_merged_config().unwrap_err() {
            CfgStoreError::Config(msg) => {
                assert!(msg.contains(&ws_path.display().to_string()), "{msg}");
            }
            autre => panic!("Config attendu, reçu {autre}"),
        }
    }

    #[test]
    fn list_layer_files_extension_exacte_sensible_a_la_casse() {
        let tmp = tempdir().unwrap();
        for nom in ["a.md", "b.md", "c.txt", "A.MD"] {
            std::fs::write(tmp.path().join(nom), "x").unwrap();
        }
        let files = list_layer_files(tmp.path(), "md").unwrap();
        let cles: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(cles, ["a", "b"]);
    }

    #[test]
    fn resolve_named_files_meme_stem_le_workspace_gagne_sans_doublon() {
        let global = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        std::fs::create_dir_all(global.path().join("agents")).unwrap();
        std::fs::create_dir_all(workspace.path().join("agents")).unwrap();
        std::fs::write(global.path().join("agents/build.md"), "g").unwrap();
        std::fs::write(workspace.path().join("agents/build.md"), "w").unwrap();
        std::fs::write(workspace.path().join("agents/debug.md"), "w").unwrap();
        let layers = Layers {
            global_dir: global.path().to_path_buf(),
            workspace_dir: Some(workspace.path().to_path_buf()),
        };
        let files = layers.resolve_named_files("agents", "md").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files.get("build"),
            Some(&workspace.path().join("agents/build.md"))
        );
        assert!(files.contains_key("debug"));
    }

    #[test]
    fn resolve_skill_files_nom_commun_le_workspace_gagne() {
        let global = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        for (base, nom) in [
            (global.path(), "pdf"),
            (workspace.path(), "pdf"),
            (workspace.path(), "csv"),
        ] {
            let dir = base.join("skills").join(nom);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), "x").unwrap();
        }
        let layers = Layers {
            global_dir: global.path().to_path_buf(),
            workspace_dir: Some(workspace.path().to_path_buf()),
        };
        let files = layers.resolve_skill_files().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            files.get("pdf"),
            Some(&workspace.path().join("skills/pdf/SKILL.md"))
        );
        assert!(files.contains_key("csv"));
    }

    fn pick_models(raw: &RawConfigFile) -> &BTreeMap<String, yaml_serde::Value> {
        &raw.models
    }

    #[test]
    fn config_entry_source_repond_global_sur_yaml_workspace_invalide() {
        let global = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        std::fs::write(
            workspace.path().join("config.yaml"),
            "models: [\n  : invalide\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: global.path().to_path_buf(),
            workspace_dir: Some(workspace.path().to_path_buf()),
        };
        assert_eq!(config_entry_source(&layers, "qwen", pick_models), "global");
        assert_eq!(
            config_entry_source(&layers, "n-importe-quoi", pick_models),
            "global"
        );
    }

    #[test]
    fn file_entry_source_extension_differentee_repond_global() {
        let global = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("toolsets")).unwrap();
        std::fs::write(workspace.path().join("toolsets/grafana.yml"), "x").unwrap();
        let layers = Layers {
            global_dir: global.path().to_path_buf(),
            workspace_dir: Some(workspace.path().to_path_buf()),
        };
        assert_eq!(
            file_entry_source(&layers, "toolsets", "yaml", "grafana"),
            "global"
        );
        assert_eq!(
            file_entry_source(&layers, "toolsets", "yml", "grafana"),
            "workspace"
        );
    }

    #[test]
    fn skill_entry_source_sans_workspace_repond_global() {
        let global = tempdir().unwrap();
        let layers = Layers {
            global_dir: global.path().to_path_buf(),
            workspace_dir: None,
        };
        assert_eq!(skill_entry_source(&layers, "pdf"), "global");
        assert_eq!(skill_entry_source(&layers, "inconnu"), "global");
    }

    #[test]
    fn provenance_absente_des_deux_couches_repond_global() {
        let global = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        std::fs::write(
            workspace.path().join("config.yaml"),
            "models:\n  autre:\n    provider: p\n",
        )
        .unwrap();
        std::fs::write(
            global.path().join("config.yaml"),
            "models:\n  autre:\n    provider: p\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: global.path().to_path_buf(),
            workspace_dir: Some(workspace.path().to_path_buf()),
        };
        assert_eq!(config_entry_source(&layers, "ghost", pick_models), "global");
    }
}
