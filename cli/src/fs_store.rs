use async_trait::async_trait;
use serde::Deserialize;

use vanyline_lib::domain::{Agent, AgentMode, McpSelection, McpServer, ModelProfile, Provider, ProviderType, McpTransport, SkillSelection, SkillMeta, Toolset};
use vanyline_lib::store::ConfigStore;
use vanyline_lib::VnyError;

use crate::config::Layers;

/// Implémente `ConfigStore` sur les deux couches YAML (`Layers`, tâche 1).
/// Remplace à terme `CliConfigStore` (ancien format JSON) — câblage dans la
/// tâche `commands`, pas ici.
#[allow(dead_code)]
pub struct FsConfigStore {
    layers: Layers,
}

impl FsConfigStore {
    #[allow(dead_code)]
    pub fn new(layers: Layers) -> Self {
        Self { layers }
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
/// qui suit la ligne de fermeture. Erreur `ConfigError` (avec `path` dans le
/// message) si la première ligne n'est pas `---` ou si aucune fermeture
/// n'est trouvée. Pas de crate — extraction manuelle (cf. design).
fn split_frontmatter(path: &Path, content: &str) -> Result<(String, String), VnyError> {
    let mut lines = content.lines();
    match lines.next() {
        Some("---") => {}
        _ => return Err(VnyError::ConfigError(format!(
            "{}: must start with '---' frontmatter delimiter", path.display()
        ))),
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
        return Err(VnyError::ConfigError(format!(
            "{}: missing closing '---' for frontmatter", path.display()
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

fn default_agent_mode() -> AgentMode {
    AgentMode::Primary
}

fn parse_agent_file(name: &str, path: &Path) -> Result<Agent, VnyError> {
    let content = std::fs::read_to_string(path).map_err(VnyError::from)?;
    let (frontmatter, body) = split_frontmatter(path, &content)?;
    let raw: RawAgentFrontmatter = yaml_serde::from_str(&frontmatter)
        .map_err(|e| VnyError::ConfigError(format!("{}: {}", path.display(), e)))?;
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

fn parse_toolset_file(name: &str, path: &Path) -> Result<Toolset, VnyError> {
    let content = std::fs::read_to_string(path).map_err(VnyError::from)?;
    let raw: RawToolsetFile = yaml_serde::from_str(&content)
        .map_err(|e| VnyError::ConfigError(format!("{}: {}", path.display(), e)))?;
    Ok(Toolset {
        name: name.to_string(),
        description: raw.description,
        prompt: raw.prompt,
        local_tools: raw.tools.local,
        mcp: raw.tools.mcp.into_iter().map(|(server, tools)| McpSelection { server, tools }).collect(),
    })
}

#[async_trait]
impl ConfigStore for FsConfigStore {
    async fn list_providers(&self) -> Result<Vec<Provider>, VnyError> {
        let merged = self.layers.load_merged_config()?;
        let mut result = Vec::new();
        for (name, value) in merged.providers {
            let raw: RawProviderEntry = yaml_serde::from_value(value)
                .map_err(|e| VnyError::ConfigError(format!("provider '{name}': {e}")))?;
            result.push(Provider {
                name,
                provider_type: raw.provider_type,
                endpoint: raw.endpoint,
                api_key: raw.api_key,
            });
        }
        Ok(result)
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, VnyError> {
        let merged = self.layers.load_merged_config()?;
        let mut result = Vec::new();
        for (name, value) in merged.models {
            let raw: RawModelEntry = yaml_serde::from_value(value)
                .map_err(|e| VnyError::ConfigError(format!("model '{name}': {e}")))?;
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

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, VnyError> {
        let merged = self.layers.load_merged_config()?;
        let mut result = Vec::new();
        for (name, value) in merged.mcp {
            let raw: RawMcpEntry = yaml_serde::from_value(value)
                .map_err(|e| VnyError::ConfigError(format!("mcp server '{name}': {e}")))?;
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
    async fn list_toolsets(&self) -> Result<Vec<Toolset>, VnyError> {
        let files = self.layers.resolve_named_files("toolsets", "yaml")?;
        files.iter().map(|(name, path)| parse_toolset_file(name, path)).collect()
    }

    /// Implémenté en tâche 02b (agents/*.md, toolsets/*.yaml).
    async fn list_agents(&self) -> Result<Vec<Agent>, VnyError> {
        let files = self.layers.resolve_named_files("agents", "md")?;
        files.iter().map(|(name, path)| parse_agent_file(name, path)).collect()
    }

    /// Résolu en tâche 02c (skills/<name>/SKILL.md).
    async fn list_skills(&self) -> Result<Vec<SkillMeta>, VnyError> {
        let files = self.layers.resolve_skill_files()?;
        files.iter().map(|(name, path)| {
            let content = std::fs::read_to_string(path).map_err(VnyError::from)?;
            let (frontmatter, _body) = split_frontmatter(path, &content)?;
            let raw: RawSkillFrontmatter = yaml_serde::from_str(&frontmatter)
                .map_err(|e| VnyError::ConfigError(format!("{}: {}", path.display(), e)))?;
            Ok(SkillMeta { name: name.clone(), description: raw.description })
        }).collect()
    }

    /// Résolu en tâche 02c.
    async fn load_skill(&self, name: &str) -> Result<String, VnyError> {
        let files = self.layers.resolve_skill_files()?;
        let path = files.get(name)
            .ok_or_else(|| VnyError::UnknownReference("skill", name.to_string()))?;
        let content = std::fs::read_to_string(path).map_err(VnyError::from)?;
        let (_frontmatter, body) = split_frontmatter(path, &content)?;
        Ok(body.trim().to_string())
    }

    async fn default_agent(&self) -> Result<Option<String>, VnyError> {
        let merged = self.layers.load_merged_config()?;
        match merged.defaults.get("agent") {
            None => Ok(None),
            Some(value) => value
                .as_str()
                .map(|s| Some(s.to_string()))
                .ok_or_else(|| VnyError::ConfigError("defaults.agent must be a string".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;
    use crate::config::Layers;

    fn write_config_yaml(dir: &std::path::Path, content: &str) {
        let path = dir.join("config.yaml");
        std::fs::File::create(&path)
            .unwrap_or_else(|e| panic!("Failed to create config.yaml in tempdir at {}: {e}", path.display()))
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
        assert_eq!(s.headers.get("X-Token").map(|s| s.as_str()), Some("secret"));
    }

    // 4. default_agent_reads_defaults_agent
    #[tokio::test]
    async fn default_agent_reads_defaults_agent() {
        let tmp = tempdir().unwrap();
        write_config_yaml(
            tmp.path(),
            "defaults:\n  agent: build\n",
        );
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
        write_config_yaml(
            tmp.path(),
            "defaults:\n  agent: 123\n",
        );
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = store.default_agent().await;
        match result {
            Err(VnyError::ConfigError(msg)) => {
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
            VnyError::ConfigError(msg) => {
                assert!(msg.contains("strix"));
            }
            _ => panic!("Expected ConfigError, got {:?}", err),
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
            Err(VnyError::ConfigError(msg)) => {
                assert!(msg.contains("bad.md"));
                assert!(msg.contains("must start with '---'"));
            }
            other => panic!("Expected ConfigError, got {:?}", other),
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
            Err(VnyError::ConfigError(msg)) => {
                assert!(msg.contains("bad.md"));
                assert!(msg.contains("missing closing '---'"));
            }
            other => panic!("Expected ConfigError, got {:?}", other),
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
            Err(VnyError::UnknownReference(kind, name)) => {
                assert_eq!(kind, "skill");
                assert_eq!(name, "nope");
            }
            other => panic!("Expected UnknownReference, got {:?}", other),
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
        std::fs::write(skills_dir.join("pdf").join("SKILL.md"), "---\ndescription: PDF\n---\n").unwrap();
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
            global_dir.path().join("skills").join("pdf").join("SKILL.md"),
            "---\ndescription: old\n---\nold body",
        )
        .unwrap();
        std::fs::write(
            workspace_dir.path().join("skills").join("pdf").join("SKILL.md"),
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
            Err(VnyError::ConfigError(msg)) => {
                assert!(msg.contains("SKILL.md") || msg.contains("bad"));
            }
            other => panic!("Expected ConfigError, got {:?}", other),
        }
    }
}
