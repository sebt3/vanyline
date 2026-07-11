use async_trait::async_trait;
use serde::Deserialize;

use vanyline_lib::domain::{Agent, McpServer, ModelProfile, Provider, ProviderType, McpTransport, SkillMeta, Toolset};
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

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct RawMcpEntry {
    #[serde(rename = "type")]
    transport: McpTransport,
    url: String,
    #[serde(default)]
    headers: std::collections::BTreeMap<String, String>,
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

    /// Stub — implémenté en tâche 02b (agents/*.md, toolsets/*.yaml).
    async fn list_toolsets(&self) -> Result<Vec<Toolset>, VnyError> {
        Ok(Vec::new())
    }

    /// Stub — implémenté en tâche 02b.
    async fn list_agents(&self) -> Result<Vec<Agent>, VnyError> {
        Ok(Vec::new())
    }

    /// Stub — implémenté en tâche 02c (skills/<name>/SKILL.md).
    async fn list_skills(&self) -> Result<Vec<SkillMeta>, VnyError> {
        Ok(Vec::new())
    }

    /// Stub — implémenté en tâche 02c.
    async fn load_skill(&self, name: &str) -> Result<String, VnyError> {
        Err(VnyError::UnknownReference("skill", name.to_string()))
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

    // 9. stubs_return_empty_or_unknown
    #[tokio::test]
    async fn stubs_return_empty_or_unknown() {
        let tmp = tempdir().unwrap();
        let layers = Layers {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);

        // List stubs return empty vectors
        let toolsets = store.list_toolsets().await.unwrap();
        assert!(toolsets.is_empty());

        let agents = store.list_agents().await.unwrap();
        assert!(agents.is_empty());

        let skills = store.list_skills().await.unwrap();
        assert!(skills.is_empty());

        // load_skill returns UnknownReference
        let result = store.load_skill("x").await;
        match result {
            Err(VnyError::UnknownReference(kind, name)) => {
                assert_eq!(kind, "skill");
                assert_eq!(name, "x");
            }
            _ => panic!("Expected UnknownReference(\"skill\")"),
        }
    }
}
