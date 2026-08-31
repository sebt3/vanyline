use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::{
    Agent, AgentMode, McpSelection, McpServer, McpTransport, ModelProfile, Provider, ProviderType,
    SkillMeta, SkillSelection, Toolset,
};
use crate::error::CfgStoreError;
use crate::layers::{
    Layers, RawConfigFile, list_layer_files, list_layer_skill_dirs, load_config_layer,
};
use crate::store::{ConfigStore, Layer};

/// Contrainte anti-traversal : `name` devient une clé de map dans `config.yaml`
/// (et, tâche 2, un nom de fichier/répertoire). `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`,
/// longueur ≤ 64, et rejette explicitement `..` (sous-chaîne), `/`, `\`, un `.`
/// ou `..` seul, tout chemin absolu (couvert par le premier caractère
/// alphanumérique + le rejet de `/` et `\`).
pub fn validate_name(name: &str) -> Result<(), CfgStoreError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.contains("..");
    if valid {
        Ok(())
    } else {
        Err(CfgStoreError::InvalidName(name.to_string()))
    }
}

/// Implémente `ConfigStore` sur les deux couches YAML (`Layers`, tâche 1).
/// Store actif de toutes les commandes CLI depuis la tâche 04b (cutover) —
/// l'ancien `CliConfigStore` (format JSON) a été supprimé.
#[allow(dead_code)]
#[derive(Clone)]
pub struct FsConfigStore {
    layers: Layers,
}

impl FsConfigStore {
    #[allow(dead_code)]
    pub const fn new(layers: Layers) -> Self {
        Self { layers }
    }

    #[allow(dead_code)]
    pub const fn layers(&self) -> &Layers {
        &self.layers
    }

    // --- Helpers privés pour les méthodes d'écriture ---

    fn layer_dir(&self, layer: Layer) -> Result<std::path::PathBuf, CfgStoreError> {
        match layer {
            Layer::Global => Ok(self.layers.global_dir.clone()),
            Layer::Workspace => {
                self.layers.workspace_dir.clone().ok_or_else(|| {
                    CfgStoreError::Config("no workspace layer configured".to_string())
                })
            }
        }
    }

    async fn write_config_layer(
        &self,
        dir: &std::path::Path,
        raw: RawConfigFile,
    ) -> Result<(), CfgStoreError> {
        std::fs::create_dir_all(dir).map_err(CfgStoreError::from)?;
        let path = dir.join("config.yaml");
        let content = yaml_serde::to_string(&raw).map_err(|e| {
            CfgStoreError::WriteError(format!("Failed to serialize {}: {}", path.display(), e))
        })?;
        std::fs::write(&path, content).map_err(CfgStoreError::from)?;
        Ok(())
    }
}

// --- Formes brutes d'une entrée de map nommée dans config.yaml : mêmes
// champs que le type du domaine correspondant, MOINS `name` (porté par la
// clé de la map, pas par la valeur). Construits avec
// `yaml_serde::from_value::<RawXEntry>(value.clone())`.

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct RawProviderEntry {
    #[serde(rename = "type")]
    provider_type: ProviderType,
    endpoint: String,
    #[serde(default)]
    api_key: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct RawMcpEntry {
    #[serde(rename = "type")]
    transport: McpTransport,
    url: String,
    #[serde(default)]
    headers: std::collections::BTreeMap<String, String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct RawModelEntry {
    provider: String,
    model: String,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    options: serde_json::Map<String, serde_json::Value>,
}

use std::path::Path;

/// Sépare le frontmatter YAML (entre deux lignes `---` exactes) du corps
/// markdown. La première ligne du fichier DOIT être exactement `---` ; la
/// fermeture est la prochaine ligne exactement `---`. Le corps est tout ce
/// qui suit la ligne de fermeture. Erreur `CfgStoreError::Config` (avec `path`
/// dans le message) si la première ligne n'est pas `---` ou si aucune
/// fermeture n'est trouvée. Pas de crate — extraction manuelle (cf. design).
fn split_frontmatter(path: &Path, content: &str) -> Result<(String, String), CfgStoreError> {
    let mut lines = content.lines();
    match lines.next() {
        Some("---") => {}
        _ => {
            return Err(CfgStoreError::Config(format!(
                "{}: must start with '---' frontmatter delimiter",
                path.display()
            )));
        }
    }
    let mut frontmatter = Vec::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line == "---" {
            closed = true;
            break;
        }
        frontmatter.push(line);
    }
    if !closed {
        return Err(CfgStoreError::Config(format!(
            "{}: missing closing '---' for frontmatter",
            path.display()
        )));
    }
    let body: Vec<&str> = lines.collect();
    Ok((frontmatter.join("\n"), body.join("\n")))
}

#[derive(Debug, Deserialize)]
struct RawAgentFrontmatter {
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_agent_mode")]
    mode: AgentMode,
    model: String,
    #[serde(default)]
    toolsets: Vec<String>,
    #[serde(default)]
    skills: SkillSelection,
}

const fn default_agent_mode() -> AgentMode {
    AgentMode::Primary
}

fn parse_agent_file(name: &str, path: &Path) -> Result<Agent, CfgStoreError> {
    let content = std::fs::read_to_string(path).map_err(CfgStoreError::from)?;
    let (frontmatter, body) = split_frontmatter(path, &content)?;
    let raw: RawAgentFrontmatter = yaml_serde::from_str(&frontmatter)
        .map_err(|e| CfgStoreError::Config(format!("{}: {}", path.display(), e)))?;
    Ok(Agent {
        name: name.to_string(),
        description: raw.description,
        mode: raw.mode,
        model: raw.model,
        toolsets: raw.toolsets,
        skills: raw.skills,
        system_prompt: body.trim().to_string(),
    })
}

#[derive(Debug, Default, Deserialize)]
struct RawToolsSection {
    #[serde(default)]
    local: Vec<String>,
    #[serde(default)]
    mcp: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct RawToolsetFile {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    tools: RawToolsSection,
}

#[derive(Debug, Deserialize)]
struct RawSkillFrontmatter {
    description: String,
    // `name` peut être présent (format compat écosystème) mais n'est pas
    // déclaré ici -> ignoré par serde, jamais utilisé (le nom canonique est
    // le nom du répertoire, porté séparément par `resolve_skill_files`).
}

fn parse_toolset_file(name: &str, path: &Path) -> Result<Toolset, CfgStoreError> {
    let content = std::fs::read_to_string(path).map_err(CfgStoreError::from)?;
    let raw: RawToolsetFile = yaml_serde::from_str(&content)
        .map_err(|e| CfgStoreError::Config(format!("{}: {}", path.display(), e)))?;
    Ok(Toolset {
        name: name.to_string(),
        description: raw.description,
        prompt: raw.prompt,
        local_tools: raw.tools.local,
        mcp: raw
            .tools
            .mcp
            .into_iter()
            .map(|(server, tools)| McpSelection { server, tools })
            .collect(),
    })
}

/// Forme fichier d'un toolset : `description?`, `prompt?`, `tools: { local?,
/// mcp? }` — `local` et `mcp` inclus seulement si non vides, `tools: {}` si
/// les deux sont vides. `mcp` est une map serveur -> liste d'outils.
fn toolset_to_yaml_json(t: &Toolset) -> Result<serde_json::Value, CfgStoreError> {
    let mut tools = serde_json::Map::new();
    if !t.local_tools.is_empty() {
        tools.insert(
            "local".into(),
            serde_json::Value::Array(
                t.local_tools
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    if !t.mcp.is_empty() {
        let mut mcp = serde_json::Map::new();
        for sel in &t.mcp {
            mcp.insert(
                sel.server.clone(),
                serde_json::Value::Array(
                    sel.tools
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
        tools.insert("mcp".into(), serde_json::Value::Object(mcp));
    }
    let mut obj = serde_json::Map::new();
    if let Some(d) = &t.description {
        obj.insert("description".into(), serde_json::Value::String(d.clone()));
    }
    if let Some(p) = &t.prompt {
        obj.insert("prompt".into(), serde_json::Value::String(p.clone()));
    }
    obj.insert("tools".into(), serde_json::Value::Object(tools));
    Ok(serde_json::Value::Object(obj))
}

/// `---\n<frontmatter yaml>\n---\n<corps>` — frontmatter : `description?`,
/// `mode` (toujours), `model` (toujours), `toolsets?` (si non vide), `skills`
/// (toujours). Corps = `system_prompt.trim()` (le read-side trime à la
/// lecture : round-trip exact).
fn agent_to_md(a: &Agent) -> Result<String, CfgStoreError> {
    let mut front = serde_json::Map::new();
    if let Some(d) = &a.description {
        front.insert("description".into(), serde_json::Value::String(d.clone()));
    }
    front.insert(
        "mode".into(),
        serde_json::Value::String(
            match a.mode {
                AgentMode::Primary => "primary",
                AgentMode::Subagent => "subagent",
                AgentMode::All => "all",
            }
            .to_string(),
        ),
    );
    front.insert("model".into(), serde_json::Value::String(a.model.clone()));
    if !a.toolsets.is_empty() {
        front.insert(
            "toolsets".into(),
            serde_json::Value::Array(
                a.toolsets
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    let skills = match &a.skills {
        SkillSelection::Auto => serde_json::Value::String("auto".into()),
        SkillSelection::None => serde_json::Value::String("none".into()),
        SkillSelection::Named(names) => serde_json::Value::Array(
            names
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect(),
        ),
    };
    front.insert("skills".into(), skills);
    let yaml = yaml_serde::to_string(&serde_json::Value::Object(front))
        .map_err(|e| CfgStoreError::WriteError(format!("agent '{}': {e}", a.name)))?;
    Ok(format!("---\n{yaml}---\n{}", a.system_prompt.trim()))
}

/// `---\n<frontmatter yaml: name + description>\n---\n<corps>` — le corps est
/// `body.trim()` (le read-side trime à la lecture).
fn skill_to_md(meta: &SkillMeta, body: &str) -> Result<String, CfgStoreError> {
    let mut front = serde_json::Map::new();
    front.insert("name".into(), serde_json::Value::String(meta.name.clone()));
    front.insert(
        "description".into(),
        serde_json::Value::String(meta.description.clone()),
    );
    let yaml = yaml_serde::to_string(&serde_json::Value::Object(front))
        .map_err(|e| CfgStoreError::WriteError(format!("skill '{}': {e}", meta.name)))?;
    Ok(format!("---\n{yaml}---\n{}", body.trim()))
}

#[async_trait]
impl ConfigStore for FsConfigStore {
    async fn list_providers(&self) -> Result<Vec<Provider>, CfgStoreError> {
        let merged = self.layers.load_merged_config()?;
        let mut result = Vec::new();
        for (name, value) in merged.providers {
            let raw: RawProviderEntry = yaml_serde::from_value(value)
                .map_err(|e| CfgStoreError::Config(format!("provider '{name}': {e}")))?;
            result.push(Provider {
                name,
                provider_type: raw.provider_type,
                endpoint: raw.endpoint,
                api_key: raw.api_key,
            });
        }
        Ok(result)
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, CfgStoreError> {
        let merged = self.layers.load_merged_config()?;
        let mut result = Vec::new();
        for (name, value) in merged.models {
            let raw: RawModelEntry = yaml_serde::from_value(value)
                .map_err(|e| CfgStoreError::Config(format!("model '{name}': {e}")))?;
            result.push(ModelProfile {
                name,
                provider: raw.provider,
                model: raw.model,
                temperature: raw.temperature,
                max_tokens: raw.max_tokens,
                options: raw.options,
            });
        }
        Ok(result)
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, CfgStoreError> {
        let merged = self.layers.load_merged_config()?;
        let mut result = Vec::new();
        for (name, value) in merged.mcp {
            let raw: RawMcpEntry = yaml_serde::from_value(value)
                .map_err(|e| CfgStoreError::Config(format!("mcp server '{name}': {e}")))?;
            result.push(McpServer {
                name,
                transport: raw.transport,
                url: raw.url,
                headers: raw.headers,
            });
        }
        Ok(result)
    }

    /// Implémenté en tâche 02b (agents/*.md, toolsets/*.yaml).
    async fn list_toolsets(&self) -> Result<Vec<Toolset>, CfgStoreError> {
        let files = self.layers.resolve_named_files("toolsets", "yaml")?;
        files
            .iter()
            .map(|(name, path)| parse_toolset_file(name, path))
            .collect()
    }

    /// Implémenté en tâche 02b (agents/*.md, toolsets/*.yaml).
    async fn list_agents(&self) -> Result<Vec<Agent>, CfgStoreError> {
        let files = self.layers.resolve_named_files("agents", "md")?;
        files
            .iter()
            .map(|(name, path)| parse_agent_file(name, path))
            .collect()
    }

    /// Résolu en tâche 02c (skills/<name>/SKILL.md).
    async fn list_skills(&self) -> Result<Vec<SkillMeta>, CfgStoreError> {
        let files = self.layers.resolve_skill_files()?;
        files
            .iter()
            .map(|(name, path)| {
                let content = std::fs::read_to_string(path).map_err(CfgStoreError::from)?;
                let (frontmatter, _body) = split_frontmatter(path, &content)?;
                let raw: RawSkillFrontmatter = yaml_serde::from_str(&frontmatter)
                    .map_err(|e| CfgStoreError::Config(format!("{}: {}", path.display(), e)))?;
                Ok(SkillMeta {
                    name: name.clone(),
                    description: raw.description,
                })
            })
            .collect()
    }

    /// Résolu en tâche 02c.
    async fn load_skill(&self, name: &str) -> Result<String, CfgStoreError> {
        let files = self.layers.resolve_skill_files()?;
        let path = files
            .get(name)
            .ok_or_else(|| CfgStoreError::UnknownReference("skill", name.to_string()))?;
        let content = std::fs::read_to_string(path).map_err(CfgStoreError::from)?;
        let (_frontmatter, body) = split_frontmatter(path, &content)?;
        Ok(body.trim().to_string())
    }

    async fn default_agent(&self) -> Result<Option<String>, CfgStoreError> {
        let merged = self.layers.load_merged_config()?;
        match merged.defaults.get("agent") {
            None => Ok(None),
            Some(value) => value.as_str().map(|s| Some(s.to_string())).ok_or_else(|| {
                CfgStoreError::Config("defaults.agent must be a string".to_string())
            }),
        }
    }

    // --- Write methods --- create/update/delete providers/models/mcp_servers

    async fn create_provider(&self, layer: Layer, item: Provider) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        if raw.providers.contains_key(&item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "provider",
                name: item.name.clone(),
                layer,
            });
        }
        let mut json_entry = serde_json::to_value(&item)
            .map_err(|e| CfgStoreError::WriteError(format!("provider '{}': {e}", item.name)))?;
        json_entry
            .as_object_mut()
            .ok_or_else(|| {
                CfgStoreError::WriteError(format!("provider '{}': not an object", item.name))
            })?
            .remove("name");
        let entry = yaml_serde::to_value(&json_entry)
            .map_err(|e| CfgStoreError::WriteError(format!("provider '{}': {e}", item.name)))?;
        raw.providers.insert(item.name.clone(), entry);
        self.write_config_layer(&dir, raw).await
    }

    async fn update_provider(
        &self,
        layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        let entry = raw
            .providers
            .get(name)
            .ok_or_else(|| CfgStoreError::NotFound {
                kind: "provider",
                name: name.to_string(),
                layer,
            })?
            .clone();
        let mut json_entry = serde_json::to_value(&entry)
            .map_err(|e| CfgStoreError::WriteError(format!("provider '{name}': {e}")))?;
        let obj = json_entry.as_object_mut().ok_or_else(|| {
            CfgStoreError::Config(format!(
                "provider '{name}': config.yaml entry is not a mapping"
            ))
        })?;
        let Some(patch_obj) = patch.as_object() else {
            return Err(CfgStoreError::Config(
                "update patch must be a JSON object".to_string(),
            ));
        };
        for (k, v) in patch_obj {
            if !matches!(k.as_str(), "type" | "endpoint" | "api_key") {
                continue;
            }
            if v.is_null() {
                obj.remove(k);
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
        // Validation du type énuméré après patch
        if let Some(v) = obj.get("type") {
            let t = v.as_str().ok_or_else(|| {
                CfgStoreError::Validation(format!("provider '{name}': 'type' must be a string"))
            })?;
            if !matches!(t, "ollama" | "openai-compatible") {
                return Err(CfgStoreError::Validation(format!(
                    "provider '{name}': unknown provider_type '{t}'"
                )));
            }
        }
        let entry = yaml_serde::to_value(&json_entry)
            .map_err(|e| CfgStoreError::WriteError(format!("provider '{name}': {e}")))?;
        raw.providers.insert(name.to_string(), entry);
        self.write_config_layer(&dir, raw).await
    }

    async fn delete_provider(&self, layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        if raw.providers.remove(name).is_none() {
            return Err(CfgStoreError::NotFound {
                kind: "provider",
                name: name.to_string(),
                layer,
            });
        }
        self.write_config_layer(&dir, raw).await
    }

    async fn create_model(&self, layer: Layer, item: ModelProfile) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        if raw.models.contains_key(&item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "model",
                name: item.name.clone(),
                layer,
            });
        }
        let mut json_entry = serde_json::to_value(&item)
            .map_err(|e| CfgStoreError::WriteError(format!("model '{}': {e}", item.name)))?;
        json_entry
            .as_object_mut()
            .ok_or_else(|| {
                CfgStoreError::WriteError(format!("model '{}': not an object", item.name))
            })?
            .remove("name");
        let entry = yaml_serde::to_value(&json_entry)
            .map_err(|e| CfgStoreError::WriteError(format!("model '{}': {e}", item.name)))?;
        raw.models.insert(item.name.clone(), entry);
        self.write_config_layer(&dir, raw).await
    }

    async fn update_model(
        &self,
        layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        let entry = raw
            .models
            .get(name)
            .ok_or_else(|| CfgStoreError::NotFound {
                kind: "model",
                name: name.to_string(),
                layer,
            })?
            .clone();
        let mut json_entry = serde_json::to_value(&entry)
            .map_err(|e| CfgStoreError::WriteError(format!("model '{name}': {e}")))?;
        let obj = json_entry.as_object_mut().ok_or_else(|| {
            CfgStoreError::Config(format!(
                "model '{name}': config.yaml entry is not a mapping"
            ))
        })?;
        let Some(patch_obj) = patch.as_object() else {
            return Err(CfgStoreError::Config(
                "update patch must be a JSON object".to_string(),
            ));
        };
        for (k, v) in patch_obj {
            if !matches!(
                k.as_str(),
                "provider" | "model" | "temperature" | "max_tokens" | "options"
            ) {
                continue;
            }
            if v.is_null() {
                obj.remove(k);
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
        let entry = yaml_serde::to_value(&json_entry)
            .map_err(|e| CfgStoreError::WriteError(format!("model '{name}': {e}")))?;
        raw.models.insert(name.to_string(), entry);
        self.write_config_layer(&dir, raw).await
    }

    async fn delete_model(&self, layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        if raw.models.remove(name).is_none() {
            return Err(CfgStoreError::NotFound {
                kind: "model",
                name: name.to_string(),
                layer,
            });
        }
        self.write_config_layer(&dir, raw).await
    }

    async fn create_mcp_server(&self, layer: Layer, item: McpServer) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        if raw.mcp.contains_key(&item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "mcp_server",
                name: item.name.clone(),
                layer,
            });
        }
        let mut json_entry = serde_json::to_value(&item)
            .map_err(|e| CfgStoreError::WriteError(format!("mcp server '{}': {e}", item.name)))?;
        json_entry
            .as_object_mut()
            .ok_or_else(|| {
                CfgStoreError::WriteError(format!("mcp server '{}': not an object", item.name))
            })?
            .remove("name");
        let entry = yaml_serde::to_value(&json_entry)
            .map_err(|e| CfgStoreError::WriteError(format!("mcp server '{}': {e}", item.name)))?;
        raw.mcp.insert(item.name.clone(), entry);
        self.write_config_layer(&dir, raw).await
    }

    async fn update_mcp_server(
        &self,
        layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        let entry = raw
            .mcp
            .get(name)
            .ok_or_else(|| CfgStoreError::NotFound {
                kind: "mcp_server",
                name: name.to_string(),
                layer,
            })?
            .clone();
        let mut json_entry = serde_json::to_value(&entry)
            .map_err(|e| CfgStoreError::WriteError(format!("mcp server '{name}': {e}")))?;
        let obj = json_entry.as_object_mut().ok_or_else(|| {
            CfgStoreError::Config(format!(
                "mcp server '{name}': config.yaml entry is not a mapping"
            ))
        })?;
        let Some(patch_obj) = patch.as_object() else {
            return Err(CfgStoreError::Config(
                "update patch must be a JSON object".to_string(),
            ));
        };
        for (k, v) in patch_obj {
            if !matches!(k.as_str(), "type" | "url" | "headers") {
                continue;
            }
            if v.is_null() {
                obj.remove(k);
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
        // Validation de l'enum MCP transport après patch
        if let Some(v) = obj.get("type") {
            let t = v.as_str().ok_or_else(|| {
                CfgStoreError::Validation(format!("mcp server '{name}': 'type' must be a string"))
            })?;
            if !matches!(t, "http-streamable" | "sse") {
                return Err(CfgStoreError::Validation(format!(
                    "mcp server '{name}': unknown transport '{t}'"
                )));
            }
        }
        let entry = yaml_serde::to_value(&json_entry)
            .map_err(|e| CfgStoreError::WriteError(format!("mcp server '{name}': {e}")))?;
        raw.mcp.insert(name.to_string(), entry);
        self.write_config_layer(&dir, raw).await
    }

    async fn delete_mcp_server(&self, layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        if raw.mcp.remove(name).is_none() {
            return Err(CfgStoreError::NotFound {
                kind: "mcp_server",
                name: name.to_string(),
                layer,
            });
        }
        self.write_config_layer(&dir, raw).await
    }

    async fn set_default_agent(&self, layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        let dir = self.layer_dir(layer)?;
        let mut raw = load_config_layer(&dir)?;
        raw.defaults.insert(
            "agent".to_string(),
            yaml_serde::Value::String(name.to_string()),
        );
        self.write_config_layer(&dir, raw).await
    }

    async fn create_toolset(&self, layer: Layer, item: Toolset) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let dir = self.layer_dir(layer)?;
        let files = list_layer_files(&dir.join("toolsets"), "yaml")?;
        if files.contains_key(&item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "toolset",
                name: item.name.clone(),
                layer,
            });
        }
        let content = yaml_serde::to_string(&toolset_to_yaml_json(&item)?)
            .map_err(|e| CfgStoreError::WriteError(format!("toolset '{}': {e}", item.name)))?;
        std::fs::create_dir_all(dir.join("toolsets")).map_err(CfgStoreError::from)?;
        std::fs::write(
            dir.join("toolsets").join(format!("{}.yaml", item.name)),
            content,
        )
        .map_err(CfgStoreError::from)?;
        Ok(())
    }

    async fn delete_toolset(&self, layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let files = list_layer_files(&dir.join("toolsets"), "yaml")?;
        let path = files.get(name).ok_or_else(|| CfgStoreError::NotFound {
            kind: "toolset",
            name: name.to_string(),
            layer,
        })?;
        std::fs::remove_file(path).map_err(CfgStoreError::from)?;
        Ok(())
    }

    async fn create_agent(&self, layer: Layer, item: Agent) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let dir = self.layer_dir(layer)?;
        let files = list_layer_files(&dir.join("agents"), "md")?;
        if files.contains_key(&item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "agent",
                name: item.name.clone(),
                layer,
            });
        }
        let content = agent_to_md(&item)?;
        std::fs::create_dir_all(dir.join("agents")).map_err(CfgStoreError::from)?;
        std::fs::write(
            dir.join("agents").join(format!("{}.md", item.name)),
            content,
        )
        .map_err(CfgStoreError::from)?;
        Ok(())
    }

    async fn delete_agent(&self, layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let files = list_layer_files(&dir.join("agents"), "md")?;
        let path = files.get(name).ok_or_else(|| CfgStoreError::NotFound {
            kind: "agent",
            name: name.to_string(),
            layer,
        })?;
        std::fs::remove_file(path).map_err(CfgStoreError::from)?;
        Ok(())
    }

    async fn create_skill(
        &self,
        layer: Layer,
        meta: SkillMeta,
        body: String,
    ) -> Result<(), CfgStoreError> {
        validate_name(&meta.name)?;
        let dir = self.layer_dir(layer)?;
        let skills = list_layer_skill_dirs(&dir.join("skills"))?;
        if skills.contains_key(&meta.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "skill",
                name: meta.name.clone(),
                layer,
            });
        }
        let content = skill_to_md(&meta, &body)?;
        std::fs::create_dir_all(dir.join("skills").join(&meta.name))
            .map_err(CfgStoreError::from)?;
        std::fs::write(
            dir.join("skills").join(&meta.name).join("SKILL.md"),
            content,
        )
        .map_err(CfgStoreError::from)?;
        Ok(())
    }

    async fn delete_skill(&self, layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let dir = self.layer_dir(layer)?;
        let skills = list_layer_skill_dirs(&dir.join("skills"))?;
        let path = skills.get(name).ok_or_else(|| CfgStoreError::NotFound {
            kind: "skill",
            name: name.to_string(),
            layer,
        })?;
        std::fs::remove_file(path).map_err(CfgStoreError::from)?;
        if let Some(parent) = path.parent() {
            std::fs::remove_dir(parent).map_err(CfgStoreError::from)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;
    use crate::layers::Layers;

    fn write_config_yaml(dir: &std::path::Path, content: &str) {
        let path = dir.join("config.yaml");
        std::fs::File::create(&path)
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to create config.yaml in tempdir at {}: {e}",
                    path.display()
                )
            })
            .write_all(content.as_bytes())
            .unwrap_or_else(|e| panic!("Failed to write config.yaml at {}: {e}", path.display()));
    }

    // 1. list_providers_from_config_yaml
    #[tokio::test]
    async fn list_providers_from_config_yaml() {
        let tmp = tempdir().unwrap();
        write_config_yaml(
            tmp.path(),
            "providers:\n  strix:\n    type: openai-compatible\n    endpoint: http://localhost:11434\n",
        );
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let providers = store.list_providers().await.unwrap();
        assert_eq!(providers.len(), 1);
        let p = &providers[0];
        assert_eq!(p.name, "strix");
        assert_eq!(p.provider_type, ProviderType::OpenaiCompatible);
        assert_eq!(p.endpoint, "http://localhost:11434");
        assert_eq!(p.api_key, None);
    }

    // 2. list_models_from_config_yaml
    #[tokio::test]
    async fn list_models_from_config_yaml() {
        let tmp = tempdir().unwrap();
        write_config_yaml(
            tmp.path(),
            "models:\n  qwen-code:\n    provider: ollama\n    model: qwen2.5\n    options:\n      num_ctx: 65536\n",
        );
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let models = store.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        let m = &models[0];
        assert_eq!(m.name, "qwen-code");
        assert_eq!(m.provider, "ollama");
        assert_eq!(m.model, "qwen2.5");
        let num_ctx = m.options.get("num_ctx");
        assert!(num_ctx.is_some());
        assert_eq!(num_ctx.unwrap().as_u64().unwrap(), 65536);
    }

    // 3. list_mcp_servers_from_config_yaml
    #[tokio::test]
    async fn list_mcp_servers_from_config_yaml() {
        let tmp = tempdir().unwrap();
        write_config_yaml(
            tmp.path(),
            "mcp:\n  grafana-kydah:\n    type: http-streamable\n    url: http://mcp:3000\n    headers:\n      X-Token: secret\n",
        );
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let servers = store.list_mcp_servers().await.unwrap();
        assert_eq!(servers.len(), 1);
        let s = &servers[0];
        assert_eq!(s.name, "grafana-kydah");
        assert_eq!(s.transport, McpTransport::HttpStreamable);
        assert_eq!(s.url, "http://mcp:3000");
        assert_eq!(
            s.headers.get("X-Token").map(std::string::String::as_str),
            Some("secret")
        );
    }

    // 4. default_agent_reads_defaults_agent
    #[tokio::test]
    async fn default_agent_reads_defaults_agent() {
        let tmp = tempdir().unwrap();
        write_config_yaml(tmp.path(), "defaults:\n  agent: build\n");
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.default_agent().await.unwrap();
        assert_eq!(result, Some("build".to_string()));
    }

    // 5. default_agent_absent_is_none
    #[tokio::test]
    async fn default_agent_absent_is_none() {
        // Empty tempdir — no config.yaml at all
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.default_agent().await.unwrap();
        assert_eq!(result, None);
    }

    // 6. default_agent_wrong_type_errors
    #[tokio::test]
    async fn default_agent_wrong_type_errors() {
        let tmp = tempdir().unwrap();
        // Number instead of string
        write_config_yaml(tmp.path(), "defaults:\n  agent: 123\n");
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.default_agent().await;
        match result {
            Err(CfgStoreError::Config(msg)) => {
                assert!(msg.contains("must be a string"));
            }
            _ => panic!("Expected ConfigError"),
        }
    }

    // 7. two_layer_override_visible_in_provider_list
    #[tokio::test]
    async fn two_layer_override_visible_in_provider_list() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();

        write_config_yaml(
            global_dir.path(),
            "providers:\n  strix:\n    type: openai-compatible\n    endpoint: http://global:11434\n",
        );
        write_config_yaml(
            workspace_dir.path(),
            "providers:\n  strix:\n    type: openai-compatible\n    endpoint: http://workspace:11434\n",
        );

        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);
        let providers = store.list_providers().await.unwrap();

        // Only one entry — workspace overrides global
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "strix");
        assert_eq!(providers[0].endpoint, "http://workspace:11434");
    }

    // 8. malformed_provider_entry_errors_with_name
    #[tokio::test]
    async fn malformed_provider_entry_errors_with_name() {
        let tmp = tempdir().unwrap();
        write_config_yaml(
            tmp.path(),
            "providers:\n  strix:\n    type: openai-compatible\n",
        );
        // Missing required "endpoint" field
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.list_providers().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            CfgStoreError::Config(msg) => {
                assert!(msg.contains("strix"));
            }
            _ => panic!("Expected ConfigError, got {err:?}"),
        }
    }

    // 10. list_agents_parses_frontmatter
    #[tokio::test]
    async fn list_agents_parses_frontmatter() {
        let tmp = tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("build.md"),
            "---
description: Agent d'implémentation
mode: primary
model: qwen-code
toolsets:
  - fs
  - grafana-kydah
skills: auto
---

Tu es un agent d'implémentation.",
        )
        .unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let agents = store.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        let a = &agents[0];
        assert_eq!(a.name, "build");
        assert_eq!(a.description.as_deref(), Some("Agent d'implémentation"));
        assert_eq!(a.mode, AgentMode::Primary);
        assert_eq!(a.model, "qwen-code");
        assert_eq!(a.toolsets, vec!["fs", "grafana-kydah"]);
        assert_eq!(a.system_prompt, "Tu es un agent d'implémentation.");
    }

    // 11. list_agents_defaults
    #[tokio::test]
    async fn list_agents_defaults() {
        let tmp = tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("simple.md"), "---\nmodel: x\n---\n").unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let agents = store.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        let a = &agents[0];
        assert_eq!(a.name, "simple");
        assert_eq!(a.mode, AgentMode::Primary);
        assert!(a.description.is_none());
        assert!(a.toolsets.is_empty());
        assert_eq!(a.skills, SkillSelection::Auto);
    }

    // 12. list_agents_missing_opening_delimiter_errors
    #[tokio::test]
    async fn list_agents_missing_opening_delimiter_errors() {
        let tmp = tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("bad.md"), "model: x\n---\nbody\n").unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.list_agents().await;
        match result {
            Err(CfgStoreError::Config(msg)) => {
                assert!(msg.contains("bad.md"));
                assert!(msg.contains("must start with '---'"));
            }
            other => panic!("Expected ConfigError, got {other:?}"),
        }
    }

    // 13. list_agents_missing_closing_delimiter_errors
    #[tokio::test]
    async fn list_agents_missing_closing_delimiter_errors() {
        let tmp = tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("bad.md"), "---\nmodel: x\nbody\n").unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.list_agents().await;
        match result {
            Err(CfgStoreError::Config(msg)) => {
                assert!(msg.contains("bad.md"));
                assert!(msg.contains("missing closing '---'"));
            }
            other => panic!("Expected ConfigError, got {other:?}"),
        }
    }

    // 14. list_toolsets_parses_tools_section
    #[tokio::test]
    async fn list_toolsets_parses_tools_section() {
        let tmp = tempdir().unwrap();
        let toolsets_dir = tmp.path().join("toolsets");
        std::fs::create_dir_all(&toolsets_dir).unwrap();
        std::fs::write(
            toolsets_dir.join("grafana.yaml"),
            "\
description: Outils Grafana
prompt: Interroger Grafana
tools:
  local:
    - grafana_query
  mcp:
    grafana-kydah:
      - query_dashboard
      - query_metrics
",
        )
        .unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let toolsets = store.list_toolsets().await.unwrap();
        assert_eq!(toolsets.len(), 1);
        let t = &toolsets[0];
        assert_eq!(t.name, "grafana");
        assert_eq!(t.description.as_deref(), Some("Outils Grafana"));
        assert_eq!(t.prompt.as_deref(), Some("Interroger Grafana"));
        assert_eq!(t.local_tools, vec!["grafana_query"]);
        assert_eq!(t.mcp.len(), 1);
        let mcp_sel = &t.mcp[0];
        assert_eq!(mcp_sel.server, "grafana-kydah");
        assert_eq!(mcp_sel.tools, vec!["query_dashboard", "query_metrics"]);
    }

    // 15. list_toolsets_defaults
    #[tokio::test]
    async fn list_toolsets_defaults() {
        let tmp = tempdir().unwrap();
        let toolsets_dir = tmp.path().join("toolsets");
        std::fs::create_dir_all(&toolsets_dir).unwrap();
        std::fs::write(toolsets_dir.join("empty.yaml"), "tools: {}").unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let toolsets = store.list_toolsets().await.unwrap();
        assert_eq!(toolsets.len(), 1);
        let t = &toolsets[0];
        assert_eq!(t.name, "empty");
        assert!(t.description.is_none());
        assert!(t.prompt.is_none());
        assert!(t.local_tools.is_empty());
        assert!(t.mcp.is_empty());
    }

    // 16. list_agents_and_toolsets_two_layer_override
    #[tokio::test]
    async fn list_agents_and_toolsets_two_layer_override() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        std::fs::create_dir_all(global_dir.path().join("agents")).unwrap();
        std::fs::create_dir_all(workspace_dir.path().join("agents")).unwrap();
        std::fs::write(
            global_dir.path().join("agents").join("build.md"),
            "---\nmodel: global-model\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(
            workspace_dir.path().join("agents").join("build.md"),
            "---\nmodel: workspace-model\n---\nbody\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);
        let agents = store.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "build");
        assert_eq!(agents[0].model, "workspace-model");
    }

    // 17. agent_frontmatter_temperature_ignored
    #[tokio::test]
    async fn agent_frontmatter_temperature_ignored() {
        let tmp = tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("build.md"),
            "---
description: Agent d'implémentation
mode: primary
model: qwen-code
temperature: 0.9
---
Tu es un agent d'implémentation.",
        )
        .unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let agents = store.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        let a = &agents[0];
        assert_eq!(a.name, "build");
        assert_eq!(a.description.as_deref(), Some("Agent d'implémentation"));
        assert_eq!(a.mode, AgentMode::Primary);
        assert_eq!(a.model, "qwen-code");
        assert_eq!(a.system_prompt, "Tu es un agent d'implémentation.");
        // temperature est ignoré : le type Agent n'a aucun champ temperature,
        // et le frontmatter avec temperature reste parfaitement valide.
    }

    // --- list_skills / load_skill ---

    // 5. list_skills_parses_metadata_dirname_is_canonical
    #[tokio::test]
    async fn list_skills_parses_metadata_dirname_is_canonical() {
        let tmp = tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::create_dir_all(skills_dir.join("pdf")).unwrap();
        let mut buf = std::fs::File::create(skills_dir.join("pdf").join("SKILL.md")).unwrap();
        use std::io::Write;
        buf.write_all(b"---\nname: something-else\ndescription: PDF processing\n---\n# body\n")
            .unwrap();
        drop(buf);
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let skills = store.list_skills().await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
        assert_eq!(skills[0].description, "PDF processing");
    }

    // 6. load_skill_returns_body
    #[tokio::test]
    async fn load_skill_returns_body() {
        let tmp = tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::create_dir_all(skills_dir.join("pdf")).unwrap();
        std::fs::write(
            skills_dir.join("pdf").join("SKILL.md"),
            "---\ndescription: PDF\n---\n# PDF skill\n\nDétails...",
        )
        .unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let body = store.load_skill("pdf").await.unwrap();
        assert_eq!(body, "# PDF skill\n\nDétails...");
    }

    // 7. load_skill_unknown_errors
    #[tokio::test]
    async fn load_skill_unknown_errors() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.load_skill("nope").await;
        match result {
            Err(CfgStoreError::UnknownReference(kind, name)) => {
                assert_eq!(kind, "skill");
                assert_eq!(name, "nope");
            }
            other => panic!("Expected UnknownReference, got {other:?}"),
        }
    }

    // 8. list_skills_ignores_dirs_without_skill_md
    #[tokio::test]
    async fn list_skills_ignores_dirs_without_skill_md() {
        let tmp = tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::create_dir_all(skills_dir.join("random")).unwrap();
        std::fs::create_dir_all(skills_dir.join("pdf")).unwrap();
        std::fs::write(
            skills_dir.join("pdf").join("SKILL.md"),
            "---\ndescription: PDF\n---\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let skills = store.list_skills().await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
    }

    // 9. list_skills_and_load_skill_two_layer_override
    #[tokio::test]
    async fn list_skills_and_load_skill_two_layer_override() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        std::fs::create_dir_all(global_dir.path().join("skills").join("pdf")).unwrap();
        std::fs::create_dir_all(workspace_dir.path().join("skills").join("pdf")).unwrap();
        std::fs::write(
            global_dir
                .path()
                .join("skills")
                .join("pdf")
                .join("SKILL.md"),
            "---\ndescription: old\n---\nold body",
        )
        .unwrap();
        std::fs::write(
            workspace_dir
                .path()
                .join("skills")
                .join("pdf")
                .join("SKILL.md"),
            "---\ndescription: new\n---\nnew body",
        )
        .unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);
        let skills = store.list_skills().await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
        assert_eq!(skills[0].description, "new");
        let body = store.load_skill("pdf").await.unwrap();
        assert_eq!(body, "new body");
    }

    // 10. list_skills_missing_description_errors
    #[tokio::test]
    async fn list_skills_missing_description_errors() {
        let tmp = tempdir().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::create_dir_all(skills_dir.join("bad")).unwrap();
        std::fs::write(skills_dir.join("bad").join("SKILL.md"), "---\n---\nbody\n").unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.list_skills().await;
        match result {
            Err(CfgStoreError::Config(msg)) => {
                assert!(msg.contains("SKILL.md") || msg.contains("bad"));
            }
            other => panic!("Expected ConfigError, got {other:?}"),
        }
    }

    // --- validate_name (anti-traversal) ---

    #[test]
    fn validate_name_accepts_valid() {
        let valid = ["a", "foo", "a.b", "a-b", "a_b", "a1", "a1b2c3", "a.b-c_d"];
        for n in valid {
            let r = validate_name(n);
            assert!(r.is_ok(), "validate_name({n}) should be Ok");
        }
    }

    #[test]
    fn validate_name_rejects_path_traversal() {
        // "../evil", "a/b", "..", "/abs", "a\b"
        let bad = ["../evil", "a/b", "..", "/abs", "a\\b"];
        for n in bad {
            let r = validate_name(n);
            assert!(r.is_err(), "validate_name({n}) should be Err");
        }
    }

    #[test]
    fn validate_name_rejects_empty_and_too_long() {
        assert!(validate_name("").is_err());
        assert!(validate_name("a").is_ok());
        let long = "a".repeat(65);
        assert!(validate_name(&long).is_err());
        let err = validate_name(&"a".repeat(65)).unwrap_err();
        assert!(matches!(err, CfgStoreError::InvalidName(_)));
    }

    #[test]
    fn validate_name_rejects_leading_dot() {
        assert!(validate_name(".hidden").is_err());
    }

    #[test]
    fn validate_name_no_fs_side_effect_on_invalid() {
        let tmp = tempdir().unwrap();
        // These should all fail and create nothing
        for n in &["../evil", "a/b", ".."] {
            validate_name(n).unwrap_err();
        }
        // Nothing should have been created
        let entries: std::vec::Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "validate_name must not create any files/dirs"
        );
    }

    // --- create_provider — full cycle ---

    #[tokio::test]
    async fn create_provider_full_cycle() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        // create
        store
            .create_provider(
                Layer::Global,
                Provider {
                    name: "strix".to_string(),
                    provider_type: ProviderType::OpenaiCompatible,
                    endpoint: "http://localhost:11434".to_string(),
                    api_key: None,
                },
            )
            .await
            .unwrap();

        // list contains it
        let list = store.list_providers().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "strix");

        // update (partial patch: modify endpoint only, type preserved)
        store
            .update_provider(
                Layer::Global,
                "strix",
                serde_json::json!({"endpoint": "http://localhost:12345"}),
            )
            .await
            .unwrap();

        let list = store.list_providers().await.unwrap();
        assert_eq!(list[0].endpoint, "http://localhost:12345");
        assert_eq!(list[0].provider_type, ProviderType::OpenaiCompatible);

        // delete
        store.delete_provider(Layer::Global, "strix").await.unwrap();
        let list = store.list_providers().await.unwrap();
        assert_eq!(list.len(), 0);
    }

    // --- create_provider duplicate → NameConflict ---

    #[tokio::test]
    async fn create_provider_name_conflict() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_provider(
                Layer::Global,
                Provider {
                    name: "strix".to_string(),
                    provider_type: ProviderType::Ollama,
                    endpoint: "http://x".to_string(),
                    api_key: None,
                },
            )
            .await
            .unwrap();

        let err = store
            .create_provider(
                Layer::Global,
                Provider {
                    name: "strix".to_string(),
                    provider_type: ProviderType::OpenaiCompatible,
                    endpoint: "http://y".to_string(),
                    api_key: None,
                },
            )
            .await
            .unwrap_err();
        match err {
            CfgStoreError::NameConflict {
                kind,
                name,
                layer: _l,
            } => {
                assert_eq!(kind, "provider");
                assert_eq!(name, "strix");
            }
            other => panic!("Expected NameConflict, got {other:?}"),
        }
    }

    // --- update_provider / delete_provider on non-existent → NotFound ---

    #[tokio::test]
    async fn update_nonexistent_provider_not_found() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        let err = store
            .update_provider(
                Layer::Global,
                "nonexistent",
                serde_json::json!({"type": "ollama"}),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            &err,
            CfgStoreError::NotFound {
                kind: "provider",
                ..
            }
        ));

        let err = store
            .delete_provider(Layer::Global, "nonexistent")
            .await
            .unwrap_err();
        assert!(matches!(
            &err,
            CfgStoreError::NotFound {
                kind: "provider",
                ..
            }
        ));
    }

    // --- update_provider with null deletes field ---

    #[tokio::test]
    async fn update_provider_null_deletes_field() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        // Créer avec api_key
        store
            .create_provider(
                Layer::Global,
                Provider {
                    name: "p1".to_string(),
                    provider_type: ProviderType::OpenaiCompatible,
                    endpoint: "http://x".to_string(),
                    api_key: Some("key123".to_string()),
                },
            )
            .await
            .unwrap();

        // Supprimer api_key
        store
            .update_provider(Layer::Global, "p1", serde_json::json!({"api_key": null}))
            .await
            .unwrap();

        let list = store.list_providers().await.unwrap();
        assert_eq!(list[0].name, "p1");
        assert_eq!(list[0].api_key, None);
    }

    // --- provider.type unknown enum → Validation ---

    #[tokio::test]
    async fn update_provider_unknown_type_validation() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_provider(
                Layer::Global,
                Provider {
                    name: "p1".to_string(),
                    provider_type: ProviderType::Ollama,
                    endpoint: "http://x".to_string(),
                    api_key: None,
                },
            )
            .await
            .unwrap();

        let err = store
            .update_provider(
                Layer::Global,
                "p1",
                serde_json::json!({"type": "carrier-pigeon"}),
            )
            .await
            .unwrap_err();
        assert!(matches!(&err, CfgStoreError::Validation(_)));
    }

    // --- mcp.type unknown enum → Validation ---

    #[tokio::test]
    async fn update_mcp_unknown_type_validation() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_mcp_server(
                Layer::Global,
                McpServer {
                    name: "m1".to_string(),
                    transport: McpTransport::HttpStreamable,
                    url: "http://x".to_string(),
                    headers: Default::default(),
                },
            )
            .await
            .unwrap();

        let err = store
            .update_mcp_server(Layer::Global, "m1", serde_json::json!({"type": "unknown"}))
            .await
            .unwrap_err();
        assert!(matches!(&err, CfgStoreError::Validation(_)));
    }

    // --- layer isolation: Workspace vs Global ---

    #[tokio::test]
    async fn write_isolates_workspace_vs_global() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();

        // Write provider to workspace
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);

        store
            .create_provider(
                Layer::Workspace,
                Provider {
                    name: "ws-only".to_string(),
                    provider_type: ProviderType::Ollama,
                    endpoint: "http://ws".to_string(),
                    api_key: None,
                },
            )
            .await
            .unwrap();

        // Verify: workspace has it, global doesn't
        let ws_raw = load_config_layer(workspace_dir.path()).unwrap();
        assert!(ws_raw.providers.contains_key("ws-only"));

        let global_raw = load_config_layer(global_dir.path()).unwrap();
        assert!(!global_raw.providers.contains_key("ws-only"));
    }

    // --- layer: Workspace without workspace_dir → Config ---

    #[tokio::test]
    async fn workspace_layer_without_workspace_dir_errors() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        let err = store
            .create_provider(
                Layer::Workspace,
                Provider {
                    name: "test".to_string(),
                    provider_type: ProviderType::Ollama,
                    endpoint: "http://x".to_string(),
                    api_key: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(&err, CfgStoreError::Config(msg) if msg.contains("workspace")));
    }

    // --- Preserve other maps on write ---

    #[tokio::test]
    async fn create_preserves_other_maps() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        // Ecrire un initial avec des données dans plusieurs maps
        write_config_yaml(
            tmp.path(),
            "providers:\n  strix:\n    type: openai-compatible\n    endpoint: http://global:11434\nmodels:\n  qwen-code:\n    provider: ollama\n    model: qwen2.5\ndefaults:\n  agent: build\n",
        );

        // Créer un nouveau provider
        store
            .create_provider(
                Layer::Global,
                Provider {
                    name: "new-p".to_string(),
                    provider_type: ProviderType::Ollama,
                    endpoint: "http://new".to_string(),
                    api_key: None,
                },
            )
            .await
            .unwrap();

        let raw = load_config_layer(tmp.path()).unwrap();
        // L'ancien provider existe toujours
        assert!(raw.providers.contains_key("strix"));
        // Le nouveau aussi
        assert!(raw.providers.contains_key("new-p"));
        // Les models sont préservés
        assert!(raw.models.contains_key("qwen-code"));
        // Les defaults sont préservés
        assert_eq!(
            raw.defaults.get("agent"),
            Some(&yaml_serde::Value::String("build".into()))
        );
    }

    // --- set_default_agent: moved tests from cli/src/config.rs ---

    #[tokio::test]
    async fn set_default_agent_writes_new_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.keep();
        let layers = Layers {
            global_dir: path.clone(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        store
            .set_default_agent(Layer::Global, "build")
            .await
            .unwrap();
        let raw = load_config_layer(&path).unwrap();
        assert_eq!(
            raw.defaults.get("agent"),
            Some(&yaml_serde::Value::String("build".into()))
        );
    }

    #[tokio::test]
    async fn set_default_agent_preserves_existing_content() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        let config_path = path.join("config.yaml");
        std::fs::write(
            &config_path,
            "providers:\n  strix:\n    type: openai-compatible\n    endpoint: http://localhost\nmodels:\n  qwen-code:\n    provider: ollama\n    model: qwen2.5\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: path.clone(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        store
            .set_default_agent(Layer::Global, "build")
            .await
            .unwrap();
        let raw = load_config_layer(&path).unwrap();
        assert!(raw.providers.contains_key("strix"));
        assert!(raw.models.contains_key("qwen-code"));
        assert_eq!(
            raw.defaults.get("agent"),
            Some(&yaml_serde::Value::String("build".into()))
        );
    }

    #[tokio::test]
    async fn set_default_agent_overwrites_existing_default() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        let config_path = path.join("config.yaml");
        std::fs::write(&config_path, "defaults:\n  agent: old\n").unwrap();
        let layers = Layers {
            global_dir: path.clone(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        store.set_default_agent(Layer::Global, "new").await.unwrap();
        let raw = load_config_layer(&path).unwrap();
        let agent = raw.defaults.get("agent").unwrap();
        assert_eq!(agent.as_str().unwrap(), "new");
    }

    #[tokio::test]
    async fn set_default_agent_creates_missing_global_dir() {
        let tmp = tempdir().unwrap();
        let non_existent = tmp.path().join("does-not-exist-yet");
        assert!(!non_existent.exists());
        let layers = Layers {
            global_dir: non_existent.to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        store
            .set_default_agent(Layer::Global, "build")
            .await
            .unwrap();
        assert!(non_existent.is_dir());
        let raw = load_config_layer(&non_existent).unwrap();
        assert_eq!(
            raw.defaults.get("agent"),
            Some(&yaml_serde::Value::String("build".into()))
        );
    }

    #[tokio::test]
    async fn set_default_agent_workspace_layer() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        let path = tmp.path().to_path_buf();
        let layers = Layers {
            global_dir: path.clone(),
            workspace_dir: Some(ws.clone()),
        };
        let store = FsConfigStore::new(layers);
        store
            .set_default_agent(Layer::Workspace, "debug")
            .await
            .unwrap();
        // Vérifier que le workspace a le default
        let ws_raw = load_config_layer(&ws).unwrap();
        assert_eq!(
            ws_raw.defaults.get("agent"),
            Some(&yaml_serde::Value::String("debug".into()))
        );
        // Le global ne doit pas être modifié
        let global_raw = load_config_layer(&path).unwrap();
        assert_eq!(global_raw.defaults.get("agent"), None);
    }

    // --- create/delete toolset ---

    // 1. create_toolset_full_cycle
    #[tokio::test]
    async fn create_toolset_full_cycle() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "grafana".to_string(),
                    description: Some("Outils Grafana".to_string()),
                    prompt: Some("Interroger Grafana".to_string()),
                    local_tools: vec!["grafana_query".to_string()],
                    mcp: vec![McpSelection {
                        server: "grafana-kydah".to_string(),
                        tools: vec!["query_dashboard".to_string(), "query_metrics".to_string()],
                    }],
                },
            )
            .await
            .unwrap();

        // list contains it
        let list = store.list_toolsets().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "grafana");
        assert_eq!(list[0].description.as_deref(), Some("Outils Grafana"));
        assert_eq!(list[0].prompt.as_deref(), Some("Interroger Grafana"));
        assert_eq!(list[0].local_tools, vec!["grafana_query".to_string()]);
        assert_eq!(list[0].mcp.len(), 1);
        assert_eq!(list[0].mcp[0].server, "grafana-kydah");
        assert_eq!(
            list[0].mcp[0].tools,
            vec!["query_dashboard".to_string(), "query_metrics".to_string()]
        );

        // delete
        store
            .delete_toolset(Layer::Global, "grafana")
            .await
            .unwrap();
        let list = store.list_toolsets().await.unwrap();
        assert_eq!(list.len(), 0);
    }

    // 2. create_toolset_name_conflict
    #[tokio::test]
    async fn create_toolset_name_conflict() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "fs".to_string(),
                    description: None,
                    prompt: None,
                    local_tools: vec![],
                    mcp: vec![],
                },
            )
            .await
            .unwrap();

        let err = store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "fs".to_string(),
                    description: Some("other".to_string()),
                    prompt: None,
                    local_tools: vec!["other".to_string()],
                    mcp: vec![],
                },
            )
            .await
            .unwrap_err();
        match err {
            CfgStoreError::NameConflict {
                kind,
                name,
                layer: _l,
            } => {
                assert_eq!(kind, "toolset");
                assert_eq!(name, "fs");
            }
            other => panic!("Expected NameConflict, got {other:?}"),
        }
    }

    // 3. delete_toolset_not_found
    #[tokio::test]
    async fn delete_toolset_not_found() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        let err = store
            .delete_toolset(Layer::Global, "absent")
            .await
            .unwrap_err();
        match err {
            CfgStoreError::NotFound {
                kind,
                name,
                layer: _l,
            } => {
                assert_eq!(kind, "toolset");
                assert_eq!(name, "absent");
            }
            other => panic!("Expected NotFound, got {other:?}"),
        }
    }

    // 4. create_toolset_invalid_names
    #[tokio::test]
    async fn create_toolset_invalid_names() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        for name in &["../evil", "a/b", "..", "/abs", "a\\b"] {
            let err = store
                .create_toolset(
                    Layer::Global,
                    Toolset {
                        name: name.to_string(),
                        description: None,
                        prompt: None,
                        local_tools: vec![],
                        mcp: vec![],
                    },
                )
                .await
                .unwrap_err();
            assert!(
                matches!(err, CfgStoreError::InvalidName(_)),
                "should fail for {name}"
            );
        }

        // No files created outside config directory
        let entries: std::vec::Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "create_toolset must not create files/dirs for invalid names"
        );
    }

    // 5. create_toolset_layer_isolation
    #[tokio::test]
    async fn create_toolset_layer_isolation() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);

        store
            .create_toolset(
                Layer::Workspace,
                Toolset {
                    name: "ws-tool".to_string(),
                    description: None,
                    prompt: None,
                    local_tools: vec![],
                    mcp: vec![],
                },
            )
            .await
            .unwrap();

        // Workspace has it, global doesn't
        let ws_files = list_layer_files(&workspace_dir.path().join("toolsets"), "yaml").unwrap();
        assert!(ws_files.contains_key("ws-tool"));

        let global_files = list_layer_files(&global_dir.path().join("toolsets"), "yaml").unwrap();
        assert!(!global_files.contains_key("ws-tool"));
    }

    // 6. create_toolset_roundtrip
    #[tokio::test]
    async fn create_toolset_roundtrip() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "rt".to_string(),
                    description: Some("description text".to_string()),
                    prompt: Some("prompt text".to_string()),
                    local_tools: vec!["tool1".to_string()],
                    mcp: vec![McpSelection {
                        server: "server1".to_string(),
                        tools: vec!["func1".to_string(), "func2".to_string()],
                    }],
                },
            )
            .await
            .unwrap();

        let result = store.get_toolset("rt").await.unwrap();
        assert_eq!(result.name, "rt");
        assert_eq!(result.description, Some("description text".to_string()));
        assert_eq!(result.prompt, Some("prompt text".to_string()));
        assert_eq!(result.local_tools, vec!["tool1".to_string()]);
        assert_eq!(result.mcp.len(), 1);
        assert_eq!(result.mcp[0].server, "server1");
        assert_eq!(
            result.mcp[0].tools,
            vec!["func1".to_string(), "func2".to_string()]
        );
    }

    // 7. create_toolset_empty_tools
    #[tokio::test]
    async fn create_toolset_empty_tools() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "empty".to_string(),
                    description: None,
                    prompt: None,
                    local_tools: vec![],
                    mcp: vec![],
                },
            )
            .await
            .unwrap();

        let list = store.list_toolsets().await.unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].local_tools.is_empty());
        assert!(list[0].mcp.is_empty());
        assert!(list[0].description.is_none());
        assert!(list[0].prompt.is_none());
    }

    // --- create/delete agent ---

    // 1. create_agent_full_cycle
    #[tokio::test]
    async fn create_agent_full_cycle() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_agent(
                Layer::Global,
                Agent {
                    name: "build".to_string(),
                    description: Some("Agent d'implémentation".to_string()),
                    mode: AgentMode::Primary,
                    model: "qwen-code".to_string(),
                    toolsets: vec!["fs".to_string(), "grafana-kydah".to_string()],
                    skills: SkillSelection::Auto,
                    system_prompt: "Tu es un agent d'implémentation.".to_string(),
                },
            )
            .await
            .unwrap();

        // list contains it
        let list = store.list_agents().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "build");
        assert_eq!(
            list[0].description.as_deref(),
            Some("Agent d'implémentation")
        );
        assert_eq!(list[0].mode, AgentMode::Primary);
        assert_eq!(list[0].model, "qwen-code");
        assert_eq!(
            list[0].toolsets,
            vec!["fs".to_string(), "grafana-kydah".to_string()]
        );
        assert_eq!(list[0].skills, SkillSelection::Auto);
        assert_eq!(list[0].system_prompt, "Tu es un agent d'implémentation.");

        // delete
        store.delete_agent(Layer::Global, "build").await.unwrap();
        let list = store.list_agents().await.unwrap();
        assert_eq!(list.len(), 0);
    }

    // 2. create_agent_name_conflict
    #[tokio::test]
    async fn create_agent_name_conflict() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_agent(
                Layer::Global,
                Agent {
                    name: "simple".to_string(),
                    description: None,
                    mode: AgentMode::Primary,
                    model: "x".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::Auto,
                    system_prompt: "prompt".to_string(),
                },
            )
            .await
            .unwrap();

        let err = store
            .create_agent(
                Layer::Global,
                Agent {
                    name: "simple".to_string(),
                    description: Some("other".to_string()),
                    mode: AgentMode::Subagent,
                    model: "y".to_string(),
                    toolsets: vec!["other".to_string()],
                    skills: SkillSelection::Named(vec!["s".to_string()]),
                    system_prompt: "other prompt".to_string(),
                },
            )
            .await
            .unwrap_err();
        match err {
            CfgStoreError::NameConflict {
                kind,
                name,
                layer: _l,
            } => {
                assert_eq!(kind, "agent");
                assert_eq!(name, "simple");
            }
            other => panic!("Expected NameConflict, got {other:?}"),
        }
    }

    // 3. delete_agent_not_found
    #[tokio::test]
    async fn delete_agent_not_found() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        let err = store
            .delete_agent(Layer::Global, "absent")
            .await
            .unwrap_err();
        match err {
            CfgStoreError::NotFound {
                kind,
                name,
                layer: _l,
            } => {
                assert_eq!(kind, "agent");
                assert_eq!(name, "absent");
            }
            other => panic!("Expected NotFound, got {other:?}"),
        }
    }

    // 4. create_agent_invalid_names
    #[tokio::test]
    async fn create_agent_invalid_names() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        for name in &["../evil", "a/b", "..", "/abs", "a\\b"] {
            let err = store
                .create_agent(
                    Layer::Global,
                    Agent {
                        name: name.to_string(),
                        description: None,
                        mode: AgentMode::Primary,
                        model: "m".to_string(),
                        toolsets: vec![],
                        skills: SkillSelection::Auto,
                        system_prompt: "prompt".to_string(),
                    },
                )
                .await
                .unwrap_err();
            assert!(
                matches!(err, CfgStoreError::InvalidName(_)),
                "should fail for {name}"
            );
        }

        // No files created outside config directory
        let entries: std::vec::Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "create_agent must not create files/dirs for invalid names"
        );
    }

    // 5. create_agent_layer_isolation
    #[tokio::test]
    async fn create_agent_layer_isolation() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);

        store
            .create_agent(
                Layer::Workspace,
                Agent {
                    name: "ws-agent".to_string(),
                    description: None,
                    mode: AgentMode::Subagent,
                    model: "ws-model".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::None,
                    system_prompt: "ws prompt".to_string(),
                },
            )
            .await
            .unwrap();

        // Workspace has it, global doesn't
        let ws_files = list_layer_files(&workspace_dir.path().join("agents"), "md").unwrap();
        assert!(ws_files.contains_key("ws-agent"));

        let global_files = list_layer_files(&global_dir.path().join("agents"), "md").unwrap();
        assert!(!global_files.contains_key("ws-agent"));
    }

    // 6. create_agent_roundtrip
    #[tokio::test]
    async fn create_agent_roundtrip() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_agent(
                Layer::Global,
                Agent {
                    name: "rt".to_string(),
                    description: Some("roundtrip agent".to_string()),
                    mode: AgentMode::Subagent,
                    model: "qwen2.5".to_string(),
                    toolsets: vec!["fs".to_string()],
                    skills: SkillSelection::Named(vec!["pdf".to_string()]),
                    system_prompt: "You are a roundtrip agent.".to_string(),
                },
            )
            .await
            .unwrap();

        let result = store.get_agent("rt").await.unwrap();
        assert_eq!(result.name, "rt");
        assert_eq!(result.description, Some("roundtrip agent".to_string()));
        assert_eq!(result.mode, AgentMode::Subagent);
        assert_eq!(result.model, "qwen2.5");
        assert_eq!(result.toolsets, vec!["fs".to_string()]);
        assert_eq!(
            result.skills,
            SkillSelection::Named(vec!["pdf".to_string()])
        );
        assert_eq!(result.system_prompt, "You are a roundtrip agent.");
    }

    // 7. create_agent_defaults
    #[tokio::test]
    async fn create_agent_defaults() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_agent(
                Layer::Global,
                Agent {
                    name: "minimal".to_string(),
                    description: None,
                    mode: AgentMode::Primary,
                    model: "model-x".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::Auto,
                    system_prompt: "Minimal agent prompt.".to_string(),
                },
            )
            .await
            .unwrap();

        let result = store.get_agent("minimal").await.unwrap();
        assert_eq!(result.name, "minimal");
        assert!(result.description.is_none());
        assert_eq!(result.mode, AgentMode::Primary);
        assert_eq!(result.model, "model-x");
        assert!(result.toolsets.is_empty());
        assert_eq!(result.skills, SkillSelection::Auto);
        assert_eq!(result.system_prompt, "Minimal agent prompt.");
    }

    // --- create/delete skill ---

    // 1. create_skill_full_cycle
    #[tokio::test]
    async fn create_skill_full_cycle() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_skill(
                Layer::Global,
                SkillMeta {
                    name: "pdf".to_string(),
                    description: "PDF processing".to_string(),
                },
                "# PDF skill\n\nDétails sur le traitement PDF.".to_string(),
            )
            .await
            .unwrap();

        // list_skills contains it
        let list = store.list_skills().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "pdf");
        assert_eq!(list[0].description, "PDF processing");

        // load_skill returns body
        let body = store.load_skill("pdf").await.unwrap();
        assert_eq!(body, "# PDF skill\n\nDétails sur le traitement PDF.");

        // delete
        store.delete_skill(Layer::Global, "pdf").await.unwrap();

        // list_skills no longer contains it
        let list = store.list_skills().await.unwrap();
        assert_eq!(list.len(), 0);

        // load_skill returns error
        assert!(store.load_skill("pdf").await.is_err());
    }

    // 2. create_skill_name_conflict
    #[tokio::test]
    async fn create_skill_name_conflict() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_skill(
                Layer::Global,
                SkillMeta {
                    name: "csv".to_string(),
                    description: "CSV skill".to_string(),
                },
                "csv body".to_string(),
            )
            .await
            .unwrap();

        let err = store
            .create_skill(
                Layer::Global,
                SkillMeta {
                    name: "csv".to_string(),
                    description: "other CSV skill".to_string(),
                },
                "other body".to_string(),
            )
            .await
            .unwrap_err();
        match err {
            CfgStoreError::NameConflict {
                kind,
                name,
                layer: _l,
            } => {
                assert_eq!(kind, "skill");
                assert_eq!(name, "csv");
            }
            other => panic!("Expected NameConflict, got {other:?}"),
        }
    }

    // 3. delete_skill_not_found
    #[tokio::test]
    async fn delete_skill_not_found() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        let err = store
            .delete_skill(Layer::Global, "absent")
            .await
            .unwrap_err();
        match err {
            CfgStoreError::NotFound {
                kind,
                name,
                layer: _l,
            } => {
                assert_eq!(kind, "skill");
                assert_eq!(name, "absent");
            }
            other => panic!("Expected NotFound, got {other:?}"),
        }
    }

    // 4. create_skill_invalid_names
    #[tokio::test]
    async fn create_skill_invalid_names() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        for name in &["../evil", "a/b", "..", "/abs", "a\\b"] {
            let err = store
                .create_skill(
                    Layer::Global,
                    SkillMeta {
                        name: name.to_string(),
                        description: "desc".to_string(),
                    },
                    "body".to_string(),
                )
                .await
                .unwrap_err();
            assert!(
                matches!(err, CfgStoreError::InvalidName(_)),
                "should fail for {name}"
            );
        }

        // No files created outside config directory
        let entries: std::vec::Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "create_skill must not create files/dirs for invalid names"
        );
    }

    // 5. create_skill_layer_isolation
    #[tokio::test]
    async fn create_skill_layer_isolation() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);

        store
            .create_skill(
                Layer::Workspace,
                SkillMeta {
                    name: "ws-skill".to_string(),
                    description: "workspace skill".to_string(),
                },
                "ws body".to_string(),
            )
            .await
            .unwrap();

        // Workspace has it, global doesn't
        let ws_skills = list_layer_skill_dirs(&workspace_dir.path().join("skills")).unwrap();
        assert!(ws_skills.contains_key("ws-skill"));

        let global_skills = list_layer_skill_dirs(&global_dir.path().join("skills")).unwrap();
        assert!(!global_skills.contains_key("ws-skill"));
    }

    // 6. create_skill_roundtrip
    #[tokio::test]
    async fn create_skill_roundtrip() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_skill(
                Layer::Global,
                SkillMeta {
                    name: "rt".to_string(),
                    description: "roundtrip skill".to_string(),
                },
                "# My Skill\n\nSome details here.".to_string(),
            )
            .await
            .unwrap();

        let result = store.get_skill("rt").await.unwrap();
        assert_eq!(result.name, "rt");
        assert_eq!(result.description, "roundtrip skill");

        let body = store.load_skill("rt").await.unwrap();
        assert_eq!(body, "# My Skill\n\nSome details here.");
    }

    // 7. skill_body_preserved
    #[tokio::test]
    async fn skill_body_preserved() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        store
            .create_skill(
                Layer::Global,
                SkillMeta {
                    name: "multi".to_string(),
                    description: "multi-line skill".to_string(),
                },
                "line1\n\nline3\n".to_string(),
            )
            .await
            .unwrap();

        let body = store.load_skill("multi").await.unwrap();
        assert_eq!(body, "line1\n\nline3");
    }
}
