use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use uuid::Uuid;

use vanyline_lib::domain::{
    Agent, AgentMode, McpSelection, McpServer, ModelProfile, Provider, ProviderType,
    SkillMeta, SkillSelection, Toolset,
};
use vanyline_lib::store::ConfigStore;
use vanyline_lib::VnyError;

#[derive(Debug, Deserialize)]
struct StoredMcpServer {
    #[allow(dead_code)]
    id: Uuid,
    name: String,
    server_type: String,
    url: String,
    #[serde(default)]
    headers: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct StoredProvider {
    id: Uuid,
    name: String,
    provider_type: String,
    endpoint: String,
    api_key: Option<String>,
    default_model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StoredAgent {
    #[allow(dead_code)]
    id: Uuid,
    name: String,
    description: Option<String>,
    system_prompt: String,
    llm_provider_id: Option<Uuid>,
    model: Option<String>,
    #[serde(default)]
    mcp_servers: Vec<StoredMcpServer>,
}

/// Adapte le format JSON existant (`providers.json`, `agents.json` dans
/// `config_dir`) à `ConfigStore`, en synthétisant `ModelProfile`/`Toolset`
/// (absents de l'ancien format) — voir la section "Le pont ancien format ↔
/// nouveau modèle" du fichier de tâche pour la logique complète.
pub struct CliConfigStore {
    config_dir: PathBuf,
}

impl CliConfigStore {
    pub fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    fn load_stored_providers(&self) -> Result<Vec<StoredProvider>, VnyError> {
        let path = self.config_dir.join("providers.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let providers: Vec<StoredProvider> =
                    serde_json::from_str(&content).map_err(|e| VnyError::ConfigError(format!("Failed to parse {}: {}", path.display(), e)))?;
                Ok(providers)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(VnyError::from(e)),
        }
    }

    fn load_stored_agents(&self) -> Result<Vec<StoredAgent>, VnyError> {
        let path = self.config_dir.join("agents.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let agents: Vec<StoredAgent> =
                    serde_json::from_str(&content).map_err(|e| VnyError::ConfigError(format!("Failed to parse {}: {}", path.display(), e)))?;
                Ok(agents)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(VnyError::from(e)),
        }
    }

    /// Résout le `StoredProvider` d'un `StoredAgent` (via `llm_provider_id`,
    /// ou premier provider au nom non vide si absent) — voir "Résolution
    /// Provider d'un Agent" plus haut.
    fn resolve_stored_provider<'a>(
        &self,
        agent: &StoredAgent,
        providers: &'a [StoredProvider],
    ) -> Result<&'a StoredProvider, VnyError> {
        if let Some(ref_id) = agent.llm_provider_id {
            providers
                .iter()
                .find(|p| p.id == ref_id)
                .ok_or(VnyError::LlmProviderNotFound)
        } else {
            providers
                .iter()
                .find(|p| !p.name.is_empty())
                .ok_or(VnyError::NoProviderConfigured)
        }
    }
}

#[async_trait]
impl ConfigStore for CliConfigStore {
    async fn list_providers(&self) -> Result<Vec<Provider>, VnyError> {
        let stored = self.load_stored_providers()?;
        let mut result = Vec::new();
        for s in &stored {
            let provider_type = match s.provider_type.as_str() {
                "ollama" => ProviderType::Ollama,
                "openai-compatible" => ProviderType::OpenaiCompatible,
                other => return Err(VnyError::UnknownProviderType(other.to_string())),
            };
            result.push(Provider {
                name: s.name.clone(),
                provider_type,
                endpoint: s.endpoint.clone(),
                api_key: s.api_key.clone(),
            });
        }
        Ok(result)
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, VnyError> {
        let agents = self.load_stored_agents()?;
        let providers = self.load_stored_providers()?;
        let mut result = Vec::new();

        for agent in &agents {
            let stored_provider = self.resolve_stored_provider(agent, &providers)?;

            let model_value = agent.model.clone().or_else(|| {
                stored_provider.default_model.clone()
            });

            let model_str = match model_value {
                Some(m) => m,
                None => return Err(VnyError::NoModelConfigured),
            };

            result.push(ModelProfile {
                name: agent.name.clone(),
                provider: stored_provider.name.clone(),
                model: model_str,
                temperature: None,
                max_tokens: None,
                options: serde_json::Map::new(),
            });
        }

        Ok(result)
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, VnyError> {
        let agents = self.load_stored_agents()?;
        // Aggregate MCP servers from all agents, deduplicated by name (last wins)
        let mut seen = HashMap::new();
        for agent in &agents {
            for server in &agent.mcp_servers {
                match server.server_type.as_str() {
                    "http-streamable" => {
                        // Convert headers: only keep string values
                        let headers = if let serde_json::Value::Object(obj) = &server.headers {
                            obj.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect::<BTreeMap<_, _>>()
                        } else {
                            BTreeMap::new()
                        };
                        seen.insert(
                            server.name.clone(),
                            McpServer {
                                name: server.name.clone(),
                                transport: vanyline_lib::domain::McpTransport::HttpStreamable,
                                url: server.url.clone(),
                                headers,
                            },
                        );
                    }
                    other => {
                        tracing::warn!(
                            "Skipping MCP server '{}' from agent '{}': unknown server_type '{}'",
                            server.name,
                            agent.name,
                            other
                        );
                    }
                }
            }
        }
        Ok(seen.into_values().collect())
    }

    async fn list_toolsets(&self) -> Result<Vec<Toolset>, VnyError> {
        let agents = self.load_stored_agents()?;
        let providers = self.load_stored_providers()?;

        let mut result = Vec::new();
        for agent in &agents {
            // Resolve provider for model reference
            let _provider = self.resolve_stored_provider(agent, &providers)?;

            // Build local tools list — always the 8 CLI tools
            let local_tools: Vec<String> = CLI_LOCAL_TOOLS.iter().map(|s| s.to_string()).collect();

            // Build MCP selections from embedded servers, filtering to known types only
            // and deduplicating by name (last wins).
            let mut seen_mcp = HashMap::new();
            for server in &agent.mcp_servers {
                match server.server_type.as_str() {
                    "http-streamable" => {
                        if !seen_mcp.contains_key(&server.name) {
                            seen_mcp.insert(server.name.clone(), ());
                        }
                    }
                    other => {
                        tracing::warn!(
                            "Skipping MCP server '{}' from agent '{}': unknown server_type '{}'",
                            server.name,
                            agent.name,
                            other
                        );
                    }
                }
            }

            let mcp: Vec<McpSelection> = seen_mcp.keys()
                .map(|name| McpSelection {
                    server: name.clone(),
                    tools: Vec::new(),
                })
                .collect();

            result.push(Toolset {
                name: agent.name.clone(),
                description: None,
                prompt: None,
                local_tools,
                mcp,
            });
        }

        Ok(result)
    }

    async fn list_agents(&self) -> Result<Vec<Agent>, VnyError> {
        let agents = self.load_stored_agents()?;
        let providers = self.load_stored_providers()?;

        let mut result = Vec::new();
        for agent in &agents {
            // Ensure the agent has a valid provider reference (needed for model resolution)
            let _stored_provider =
                self.resolve_stored_provider(agent, &providers)?;
            // Build model reference (agent.name referencing its own ModelProfile)
            let model_ref = agent.name.clone();

            // Build toolset reference (agent.name referencing its own Toolset)
            let toolsets = vec![agent.name.clone()];

            result.push(Agent {
                name: agent.name.clone(),
                description: agent.description.clone(),
                mode: AgentMode::Primary,
                model: model_ref,
                toolsets,
                skills: SkillSelection::None,
                system_prompt: agent.system_prompt.clone(),
            });
        }

        Ok(result)
    }

    async fn list_skills(&self) -> Result<Vec<SkillMeta>, VnyError> {
        Ok(Vec::new())
    }

    async fn load_skill(&self, name: &str) -> Result<String, VnyError> {
        Err(VnyError::UnknownReference("skill", name.to_string()))
    }

    async fn default_agent(&self) -> Result<Option<String>, VnyError> {
        let path = self.config_dir.join("default-agent.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let name: String = serde_json::from_str(&content).map_err(|e| {
                    VnyError::ConfigError(format!(
                        "Failed to parse {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                Ok(Some(name))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(VnyError::from(e)),
        }
    }
}

impl CliConfigStore {
    /// Écrit le nom par défaut dans `default-agent.json`.
    pub fn set_default_agent_name(&self, name: &str) -> Result<(), VnyError> {
        let path = self.config_dir.join("default-agent.json");
        let content = serde_json::to_string_pretty(name)
            .map_err(|e| VnyError::ConfigError(format!("serialize default-agent: {e}")))?;
        std::fs::write(&path, content).map_err(VnyError::from)?;
        Ok(())
    }
}

/// Les 8 outils locaux CLI (utilisés par `SessionContext.local_tools` — tâche
/// 9b, pas cette tâche). Nom constant, réutilisé par `list_toolsets` pour
/// synthétiser `Toolset.local_tools` de chaque agent.
pub(crate) const CLI_LOCAL_TOOLS: &[&str] = &[
    "read_file", "write_file", "edit_file", "delete_file",
    "list_directory", "find_files", "search", "execute_command",
];

#[cfg(test)]
mod tests {
    use super::*;

    use vanyline_lib::domain::McpTransport;
    use std::path::Path;

    fn make_store(dir: &Path) -> CliConfigStore {
        CliConfigStore::new(dir.to_path_buf())
    }

    fn write_json(path: &Path, content: &str) {
        std::fs::write(path, content).expect("failed to write test file");
    }

    #[tokio::test]
    async fn list_providers_translates_name_keyed() {
        let dir = tempfile::tempdir().unwrap();
        let providers_json = r#"[{"id":"550e8400-e29b-41d4-a716-446655440000","name":"local-ollama","provider_type":"ollama","endpoint":"http://localhost:11434","api_key":null,"default_model":"qwen2.5"}]"#;
        write_json(dir.path().join("providers.json").as_path(), providers_json);

        let store = make_store(dir.path());
        let providers = store.list_providers().await.unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "local-ollama");
        assert_eq!(providers[0].provider_type, ProviderType::Ollama);
        assert_eq!(providers[0].endpoint, "http://localhost:11434");
        assert_eq!(providers[0].api_key, None);
    }

    #[tokio::test]
    async fn list_providers_unknown_type_errors() {
        let dir = tempfile::tempdir().unwrap();
        let providers_json = r#"[{"id":"550e8400-e29b-41d4-a716-446655440000","name":"bogus","provider_type":"bogus","endpoint":"http://localhost:11434","api_key":null,"default_model":null}]"#;
        write_json(dir.path().join("providers.json").as_path(), providers_json);

        let store = make_store(dir.path());
        let result = store.list_providers().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            VnyError::UnknownProviderType(t) => assert_eq!(t, "bogus"),
            _ => panic!("expected UnknownProviderType, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn list_providers_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());
        let providers = store.list_providers().await.unwrap();
        assert!(providers.is_empty());
    }

    #[tokio::test]
    async fn list_agents_and_models_and_toolsets_synthesized() {
        let dir = tempfile::tempdir().unwrap();
        let provider_uid = "550e8400-e29b-41d4-a716-446655440001";
        let agent_uid = "550e8400-e29b-41d4-a716-446655440002";

        let providers_json = format!(
            r#"[{{"id":"{}","name":"local-ollama","provider_type":"ollama","endpoint":"http://localhost:11434","api_key":null,"default_model":"qwen2.5"}}]"#,
            provider_uid
        );
        write_json(dir.path().join("providers.json").as_path(), &providers_json);

        let agents_json = format!(
            r#"[{{"id":"{}","name":"build","description":"Build helper","system_prompt":"You build things.","llm_provider_id":"{}","model":"qwen2.5-coder","mcp_servers":[]}}]"#,
            agent_uid, provider_uid
        );
        write_json(dir.path().join("agents.json").as_path(), &agents_json);

        let store = make_store(dir.path());

        // list_agents
        let agents = store.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "build");
        assert_eq!(agents[0].description, Some("Build helper".into()));
        assert_eq!(agents[0].mode, AgentMode::Primary);
        assert_eq!(agents[0].model, "build");
        assert_eq!(agents[0].toolsets, vec!["build".to_string()]);
        assert_eq!(agents[0].skills, SkillSelection::None);
        assert_eq!(agents[0].system_prompt, "You build things.".to_string());

        // list_models
        let models = store.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "build");
        assert_eq!(models[0].provider, "local-ollama");
        assert_eq!(models[0].model, "qwen2.5-coder");
        assert_eq!(models[0].temperature, None);
        assert_eq!(models[0].max_tokens, None);
        assert!(models[0].options.is_empty());

        // list_toolsets
        let toolsets = store.list_toolsets().await.unwrap();
        assert_eq!(toolsets.len(), 1);
        assert_eq!(toolsets[0].name, "build");
        assert_eq!(toolsets[0].local_tools.len(), 8);
        let expected_tools: Vec<&str> = CLI_LOCAL_TOOLS.to_vec();
        let actual_tools: Vec<&str> = toolsets[0].local_tools.iter().map(|s| s.as_str()).collect();
        assert_eq!(actual_tools, expected_tools);
        assert!(toolsets[0].mcp.is_empty());
        assert!(toolsets[0].description.is_none());
        assert!(toolsets[0].prompt.is_none());
    }

    #[tokio::test]
    async fn agent_without_llm_provider_id_uses_first_provider() {
        let dir = tempfile::tempdir().unwrap();
        let provider_uid = "550e8400-e29b-41d4-a716-446655440003";

        let providers_json =
            "[{\"id\":\"".to_string() + provider_uid
            + "\",\"name\":\"local-ollama\",\"provider_type\":\"ollama\",\"endpoint\":\"http://localhost:11434\",\"api_key\":null,\"default_model\":\"qwen2.5\"}]";
        write_json(dir.path().join("providers.json").as_path(), &providers_json);

        let agents_json = r#"[{"id":"550e8400-e29b-41d4-a716-446655440004","name":"build","description":null,"system_prompt":"You build.","llm_provider_id":null,"model":"qwen2.5","mcp_servers":[]}]"#;
        write_json(dir.path().join("agents.json").as_path(), agents_json);

        let store = make_store(dir.path());
        let models = store.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider, "local-ollama");
    }

    #[tokio::test]
    async fn agent_without_model_falls_back_to_provider_default() {
        let dir = tempfile::tempdir().unwrap();
        let provider_uid = "550e8400-e29b-41d4-a716-446655440005";

        let providers_json =
            "[{\"id\":\"".to_string() + provider_uid
            + "\",\"name\":\"local-ollama\",\"provider_type\":\"ollama\",\"endpoint\":\"http://localhost:11434\",\"api_key\":null,\"default_model\":\"qwen2.5\"}]";
        write_json(dir.path().join("providers.json").as_path(), &providers_json);

        let agents_json =
            "[{\"id\":\"550e8400-e29b-41d4-a716-446655440006\",\"name\":\"build\",\"description\":null,\"system_prompt\":\"You build.\",\"llm_provider_id\":\""
                .to_string()
                + provider_uid
                + "\",\"model\":null,\"mcp_servers\":[]}]";
        write_json(dir.path().join("agents.json").as_path(), &agents_json);

        let store = make_store(dir.path());
        let models = store.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "qwen2.5");
    }

    #[tokio::test]
    async fn agent_without_model_and_provider_default_errors() {
        let dir = tempfile::tempdir().unwrap();
        let provider_uid = "550e8400-e29b-41d4-a716-446655440007";

        let providers_json = "[{\"id\":\""
            .to_string() + provider_uid
            + "\",\"name\":\"local-ollama\",\"provider_type\":\"ollama\",\"endpoint\":\"http://localhost:11434\",\"api_key\":null,\"default_model\":null}]";
        write_json(dir.path().join("providers.json").as_path(), &providers_json);

        let agents_json =
            "[{\"id\":\"550e8400-e29b-41d4-a716-446655440008\",\"name\":\"build\",\"description\":null,\"system_prompt\":\"You build.\",\"llm_provider_id\":\""
                .to_string()
                + provider_uid
                + "\",\"model\":null,\"mcp_servers\":[]}]";
        write_json(dir.path().join("agents.json").as_path(), &agents_json);

        let store = make_store(dir.path());
        let result = store.list_models().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            VnyError::NoModelConfigured => {} // expected
            e => panic!("expected NoModelConfigured, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn no_provider_configured_errors() {
        let dir = tempfile::tempdir().unwrap();

        // Empty providers file — valid JSON but empty array
        write_json(dir.path().join("providers.json").as_path(), "[]");

        let agents_json = r#"[{"id":"550e8400-e29b-41d4-a716-446655440009","name":"build","description":null,"system_prompt":"You build.","llm_provider_id":null,"model":"qwen2.5","mcp_servers":[]}]"#;
        write_json(dir.path().join("agents.json").as_path(), agents_json);

        let store = make_store(dir.path());
        let result = store.list_models().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            VnyError::NoProviderConfigured => {} // expected
            e => panic!("expected NoProviderConfigured, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn mcp_servers_aggregated_and_selected() {
        let dir = tempfile::tempdir().unwrap();

        // Provider with non-empty name satisfies resolve_stored_provider for
        // agents with llm_provider_id: null, without impacting the MCP test
        let providers_json = "[{\"id\":\"00000000-0000-0000-0000-000000000001\",\"name\":\"_\",\"provider_type\":\"ollama\",\"endpoint\":\"\",\"api_key\":null,\"default_model\":null}]";
        write_json(dir.path().join("providers.json").as_path(), providers_json);

        let agent_uid = "550e8400-e29b-41d4-a716-446655440010";
        let mcp_uid = "550e8400-e29b-41d4-a716-446655440011";
        let agents_json =
            "[{\"id\":\"".to_string() + agent_uid
            + "\",\"name\":\"build\",\"description\":null,\"system_prompt\":\"You build.\",\"llm_provider_id\":null,\"model\":\"x\",\"mcp_servers\":[{\"id\":\""
            + mcp_uid
            + "\",\"name\":\"fs\",\"server_type\":\"http-streamable\",\"url\":\"http://mcp:3000\",\"headers\":{\"X-Foo\":\"bar\"}}]}]";
        write_json(dir.path().join("agents.json").as_path(), &agents_json);

        let store = make_store(dir.path());

        // list_mcp_servers
        let mcp_servers = store.list_mcp_servers().await.unwrap();
        assert_eq!(mcp_servers.len(), 1);
        assert_eq!(mcp_servers[0].name, "fs");
        assert_eq!(mcp_servers[0].transport, McpTransport::HttpStreamable);
        assert_eq!(mcp_servers[0].url, "http://mcp:3000");
        assert_eq!(mcp_servers[0].headers.get("X-Foo").map(|s| s.as_str()), Some("bar"));

        // list_toolsets
        let toolsets = store.list_toolsets().await.unwrap();
        assert_eq!(toolsets.len(), 1);
        assert_eq!(toolsets[0].mcp.len(), 1);
        assert_eq!(toolsets[0].mcp[0].server, "fs");
        assert!(toolsets[0].mcp[0].tools.is_empty());
    }

    #[tokio::test]
    async fn mcp_server_unknown_type_skipped() {
        let dir = tempfile::tempdir().unwrap();

        // Provider with non-empty name for resolve_stored_provider compliance
        let providers_json = "[{\"id\":\"00000000-0000-0000-0000-000000000002\",\"name\":\"_\",\"provider_type\":\"ollama\",\"endpoint\":\"\",\"api_key\":null,\"default_model\":null}]";
        write_json(dir.path().join("providers.json").as_path(), providers_json);

        let agent_uid = "550e8400-e29b-41d4-a716-446655440012";
        let mcp_uid = "550e8400-e29b-41d4-a716-446655440013";
        // server_type: "sse" should be skipped
        let agents_json =
            "[{\"id\":\"".to_string() + agent_uid
            + "\",\"name\":\"build\",\"description\":null,\"system_prompt\":\"You build.\",\"llm_provider_id\":null,\"model\":\"x\",\"mcp_servers\":[{\"id\":\""
            + mcp_uid
            + "\",\"name\":\"fs\",\"server_type\":\"sse\",\"url\":\"http://mcp:3000\",\"headers\":{}}]}]";
        write_json(dir.path().join("agents.json").as_path(), &agents_json);

        let store = make_store(dir.path());

        let mcp_servers = store.list_mcp_servers().await.unwrap();
        assert!(mcp_servers.is_empty());

        let toolsets = store.list_toolsets().await.unwrap();
        assert_eq!(toolsets.len(), 1);
        assert!(toolsets[0].mcp.is_empty());
    }

    #[tokio::test]
    async fn list_skills_always_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        let skills = store.list_skills().await.unwrap();
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn load_skill_always_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        let result = store.load_skill("anything").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            VnyError::UnknownReference("skill", name) => assert_eq!(name, "anything"),
            e => panic!("expected UnknownReference(\"skill\"), got {:?}", e),
        }
    }

    #[tokio::test]
    async fn default_agent_reads_name() {
        let dir = tempfile::tempdir().unwrap();
        write_json(dir.path().join("default-agent.json").as_path(), "\"build\"");

        let store = make_store(dir.path());
        let result = store.default_agent().await.unwrap();
        assert_eq!(result, Some("build".to_string()));
    }

    #[tokio::test]
    async fn default_agent_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        let result = store.default_agent().await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn set_default_agent_name_then_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        store.set_default_agent_name("build").unwrap();
        let result = store.default_agent().await.unwrap();
        assert_eq!(result, Some("build".to_string()));
    }

    #[tokio::test]
    async fn set_default_agent_name_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let store = make_store(dir.path());

        store.set_default_agent_name("build").unwrap();
        store.set_default_agent_name("chat").unwrap();

        let result = store.default_agent().await.unwrap();
        assert_eq!(result, Some("chat".to_string()));
    }
}
