use std::collections::BTreeMap;

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use vanyline_lib::domain::{
    Agent, AgentMode, McpSelection, McpServer, McpTransport, ModelProfile, Provider,
    ProviderType, SkillMeta, SkillSelection, Toolset,
};
use vanyline_lib::store::ConfigStore;
use vanyline_lib::VnyError;

use crate::db::models::{AgentRow, LlmProvider as DbLlmProvider, McpServer as DbMcpServer};

// ---------------------------------------------------------------------------
// Fonctions pures — testées sans base de données (fixtures construites à la
// main). Aucune ne prend de référence à `PgPool` ni ne fait d'I/O.
// ---------------------------------------------------------------------------

fn provider_type_from_str(s: &str) -> Result<ProviderType, VnyError> {
    match s {
        "ollama" => Ok(ProviderType::Ollama),
        "openai-compatible" => Ok(ProviderType::OpenaiCompatible),
        other => Err(VnyError::UnknownProviderType(other.to_string())),
    }
}

fn headers_from_json(v: &serde_json::Value) -> BTreeMap<String, String> {
    if let serde_json::Value::Object(obj) = v {
        obj.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect()
    } else {
        BTreeMap::new()
    }
}

fn domain_mcp_server_selection(row: &DbMcpServer) -> Option<McpSelection> {
    if row.server_type == "http-streamable" {
        Some(McpSelection {
            server: row.name.clone(),
            tools: vec![],
        })
    } else {
        None
    }
}

/// `None` si `row.server_type` n'est pas `http-streamable` (àm logger par l'appelant,
/// PAS ici — cette fonction est pure, pas de `tracing::warn!` dedans).
fn domain_mcp_server(row: &DbMcpServer) -> Option<McpServer> {
    match row.server_type.as_str() {
        "http-streamable" => Some(McpServer {
            name: row.name.clone(),
            transport: McpTransport::HttpStreamable,
            url: row.url.clone(),
            headers: headers_from_json(&row.headers),
        }),
        _ => None,
    }
}

fn domain_provider(row: &DbLlmProvider) -> Result<Provider, VnyError> {
    Ok(Provider {
        name: row.name.clone(),
        provider_type: provider_type_from_str(&row.provider_type)?,
        endpoint: row.endpoint.clone(),
        api_key: row.api_key.clone(),
    })
}

/// `None` si `agent.llm_provider_id` référence un id absent de `providers`,
/// ou si aucun `is_default` n'existe quand `llm_provider_id` est `None` — le
/// `Result`/`Err` exact (`LlmProviderNotFound` vs `NoProviderConfigured`) est
/// choisi ICI (fonction pure, testable sans DB).
fn resolve_provider_for_agent<'a>(
    agent: &AgentRow,
    providers: &'a [DbLlmProvider],
) -> Result<&'a DbLlmProvider, VnyError> {
    if let Some(id) = agent.llm_provider_id {
        providers
            .iter()
            .find(|p| p.id == id)
            .ok_or(VnyError::LlmProviderNotFound)
    } else {
        providers
            .iter()
            .find(|p| p.is_default)
            .ok_or(VnyError::NoProviderConfigured)
    }
}

fn model_profile_for_agent(agent: &AgentRow, provider: &DbLlmProvider) -> Result<ModelProfile, VnyError> {
    let model = agent
        .model
        .clone()
        .or_else(|| provider.default_model.clone())
        .ok_or(VnyError::NoModelConfigured)?;
    Ok(ModelProfile {
        name: agent.name.clone(),
        provider: provider.name.clone(),
        model,
        temperature: None,
        max_tokens: None,
        options: serde_json::Map::new(),
    })
}

fn domain_agent(agent: &AgentRow) -> Agent {
    Agent {
        name: agent.name.clone(),
        description: agent.description.clone(),
        mode: AgentMode::Primary,
        model: agent.name.clone(),
        toolsets: vec![agent.name.clone()],
        skills: SkillSelection::None,
        system_prompt: agent.system_prompt.clone(),
    }
}

/// `mcp_rows` = déjà filtrées à cet agent (résultat de la jointure
/// `agent_mcp_servers`, faite par l'appelant). `local_tools` toujours vide
/// (app n'a pas d'outils locaux).
fn toolset_for_agent(agent: &AgentRow, mcp_rows: &[DbMcpServer]) -> Toolset {
    let mcp: Vec<_> = mcp_rows.iter().filter_map(domain_mcp_server_selection).collect();
    Toolset {
        name: agent.name.clone(),
        description: None,
        prompt: None,
        local_tools: vec![],
        mcp,
    }
}

// ---------------------------------------------------------------------------
// PgConfigStore — méthodes ConfigStore, fines : une requête + un appel aux
// fonctions pures ci-dessus.
// ---------------------------------------------------------------------------

pub struct PgConfigStore {
    pool: PgPool,
}

impl PgConfigStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn load_providers(&self) -> Result<Vec<DbLlmProvider>, VnyError> {
        sqlx::query_as::<_, DbLlmProvider>("SELECT * FROM llm_providers")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))
    }

    async fn load_agents(&self) -> Result<Vec<AgentRow>, VnyError> {
        sqlx::query_as::<_, AgentRow>("SELECT * FROM agents")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))
    }

    async fn load_mcp_servers_for_agent(&self, agent_id: Uuid) -> Result<Vec<DbMcpServer>, VnyError> {
        sqlx::query_as::<_, DbMcpServer>(
            r#"SELECT m.* FROM mcp_servers m
               JOIN agent_mcp_servers ams ON ams.mcp_server_id = m.id
               WHERE ams.agent_id = $1 ORDER BY m.name"#,
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| VnyError::ConfigError(e.to_string()))
    }
}

#[async_trait]
impl ConfigStore for PgConfigStore {
    async fn list_providers(&self) -> Result<Vec<Provider>, VnyError> {
        let rows = self.load_providers().await?;
        rows.iter().map(domain_provider).collect()
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, VnyError> {
        let agents = self.load_agents().await?;
        let providers = self.load_providers().await?;
        agents
            .iter()
            .map(|a| {
                let provider = resolve_provider_for_agent(a, &providers)?;
                model_profile_for_agent(a, provider)
            })
            .collect()
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, VnyError> {
        let rows = sqlx::query_as::<_, DbMcpServer>("SELECT * FROM mcp_servers")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))?;
        let mut result = Vec::new();
        for row in &rows {
            match domain_mcp_server(row) {
                Some(server) => result.push(server),
                None => tracing::warn!(
                    "Skipping MCP server '{}': unknown server_type '{}'",
                    row.name, row.server_type
                ),
            }
        }
        Ok(result)
    }

    async fn list_toolsets(&self) -> Result<Vec<Toolset>, VnyError> {
        let agents = self.load_agents().await?;
        let mut result = Vec::with_capacity(agents.len());
        for agent in &agents {
            let mcp_rows = self.load_mcp_servers_for_agent(agent.id).await?;
            result.push(toolset_for_agent(agent, &mcp_rows));
        }
        Ok(result)
    }

    async fn list_agents(&self) -> Result<Vec<Agent>, VnyError> {
        let agents = self.load_agents().await?;
        let providers = self.load_providers().await?;
        for agent in &agents {
            resolve_provider_for_agent(agent, &providers)?; // valide la référence
        }
        Ok(agents.iter().map(domain_agent).collect())
    }

    async fn list_skills(&self) -> Result<Vec<SkillMeta>, VnyError> {
        Ok(Vec::new())
    }

    async fn load_skill(&self, name: &str) -> Result<String, VnyError> {
        Err(VnyError::UnknownReference("skill", name.to_string()))
    }

    async fn default_agent(&self) -> Result<Option<String>, VnyError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_provider(id: Uuid, name: &str, provider_type: &str, is_default: bool, default_model: Option<&str>) -> DbLlmProvider {
        DbLlmProvider {
            id,
            name: name.to_string(),
            provider_type: provider_type.to_string(),
            endpoint: "http://localhost:11434".to_string(),
            api_key: None,
            default_model: default_model.map(String::from),
            available_models: serde_json::json!([]),
            is_default,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn sample_agent(id: Uuid, name: &str, llm_provider_id: Option<Uuid>, model: Option<&str>) -> AgentRow {
        AgentRow {
            id,
            name: name.to_string(),
            description: None,
            system_prompt: "prompt".to_string(),
            llm_provider_id,
            model: model.map(String::from),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    // 1. provider_type_from_str_known_values
    #[test]
    fn provider_type_from_str_known_values() {
        let ollama = provider_type_from_str("ollama").unwrap();
        assert_eq!(ollama, ProviderType::Ollama);

        let openai = provider_type_from_str("openai-compatible").unwrap();
        assert_eq!(openai, ProviderType::OpenaiCompatible);

        let err = provider_type_from_str("bogus").unwrap_err();
        assert!(matches!(err, VnyError::UnknownProviderType(_)));
    }

    // 2. headers_from_json_keeps_string_values_only
    #[test]
    fn headers_from_json_keeps_string_values_only() {
        let obj = serde_json::json!({"X-Foo": "bar", "X-Num": 42});
        let h = headers_from_json(&obj);
        assert_eq!(h.get("X-Foo"), Some(&"bar".to_string()));
        assert_eq!(h.get("X-Num"), None);

        let null = serde_json::json!(null);
        let h2 = headers_from_json(&null);
        assert!(h2.is_empty());
    }

    // 3. domain_mcp_server_http_streamable
    #[test]
    fn domain_mcp_server_http_streamable() {
        let row = DbMcpServer {
            id: Uuid::new_v4(),
            name: "my-server".to_string(),
            server_type: "http-streamable".to_string(),
            url: "http://localhost:8080".to_string(),
            headers: serde_json::json!({"X-Custom": "value"}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let server = domain_mcp_server(&row).unwrap();
        assert_eq!(server.name, "my-server");
        assert!(matches!(server.transport, McpTransport::HttpStreamable));
        assert_eq!(server.url, "http://localhost:8080");
        assert_eq!(server.headers.get("X-Custom"), Some(&"value".to_string()));
    }

    // 4. domain_mcp_server_sse_skipped
    #[test]
    fn domain_mcp_server_sse_skipped() {
        let row = DbMcpServer {
            id: Uuid::new_v4(),
            name: "sse-srv".to_string(),
            server_type: "sse".to_string(),
            url: "http://localhost:9090".to_string(),
            headers: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert!(domain_mcp_server(&row).is_none());
    }

    // 5. resolve_provider_for_agent_by_id
    #[test]
    fn resolve_provider_for_agent_by_id() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let p1 = sample_provider(id1, "ollama", "ollama", false, None);
        let p2 = sample_provider(id2, "openai", "openai-compatible", false, None);
        let agents = vec![p1.clone(), p2.clone()];
        let agent = sample_agent(Uuid::new_v4(), "test-agent", Some(id2), None);
        let result = resolve_provider_for_agent(&agent, &agents).unwrap();
        assert_eq!(result.name, "openai");
    }

    // 6. resolve_provider_for_agent_unknown_id
    #[test]
    fn resolve_provider_for_agent_unknown_id() {
        let unknown_id = Uuid::new_v4();
        let p1 = sample_provider(Uuid::new_v4(), "ollama", "ollama", false, None);
        let agent = sample_agent(Uuid::new_v4(), "test-agent", Some(unknown_id), None);
        let err = resolve_provider_for_agent(&agent, &[p1]).unwrap_err();
        assert!(matches!(err, VnyError::LlmProviderNotFound));
    }

    // 7. resolve_provider_for_agent_default_fallback
    #[test]
    fn resolve_provider_for_agent_default_fallback() {
        let p1 = sample_provider(Uuid::new_v4(), "ollama", "ollama", true, None);
        let p2 = sample_provider(Uuid::new_v4(), "openai", "openai-compatible", false, None);
        let agent = sample_agent(Uuid::new_v4(), "test-agent", None, None);
        let providers = vec![p1.clone(), p2];
        let result = resolve_provider_for_agent(&agent, &providers).unwrap();
        assert_eq!(result.name, "ollama");
        assert!(result.is_default);
    }

    // 8. resolve_provider_for_agent_no_default
    #[test]
    fn resolve_provider_for_agent_no_default() {
        let p1 = sample_provider(Uuid::new_v4(), "ollama", "ollama", false, None);
        let p2 = sample_provider(Uuid::new_v4(), "openai", "openai-compatible", false, None);
        let agent = sample_agent(Uuid::new_v4(), "test-agent", None, None);
        let err = resolve_provider_for_agent(&agent, &[p1, p2]).unwrap_err();
        assert!(matches!(err, VnyError::NoProviderConfigured));
    }

    // 9. model_profile_for_agent_uses_agent_model
    #[test]
    fn model_profile_for_agent_uses_agent_model() {
        let p = sample_provider(Uuid::new_v4(), "ollama", "ollama", false, Some("qwen2.5"));
        let agent = sample_agent(Uuid::new_v4(), "test-agent", Some(p.id), Some("qwen2.5-coder"));
        let result = model_profile_for_agent(&agent, &p).unwrap();
        assert_eq!(result.model, "qwen2.5-coder");
        assert_eq!(result.provider, "ollama");
    }

    // 10. model_profile_for_agent_falls_back_to_provider_default
    #[test]
    fn model_profile_for_agent_falls_back_to_provider_default() {
        let p = sample_provider(Uuid::new_v4(), "ollama", "ollama", false, Some("qwen2.5"));
        let agent = sample_agent(Uuid::new_v4(), "test-agent", Some(p.id), None);
        let result = model_profile_for_agent(&agent, &p).unwrap();
        assert_eq!(result.model, "qwen2.5");
    }

    // 11. model_profile_for_agent_errors_without_any_model
    #[test]
    fn model_profile_for_agent_errors_without_any_model() {
        let p = sample_provider(Uuid::new_v4(), "ollama", "ollama", false, None);
        let agent = sample_agent(Uuid::new_v4(), "test-agent", Some(p.id), None);
        let err = model_profile_for_agent(&agent, &p).unwrap_err();
        assert!(matches!(err, VnyError::NoModelConfigured));
    }

    // 12. domain_agent_synthesizes_self_references
    #[test]
    fn domain_agent_synthesizes_self_references() {
        let agent_row = sample_agent(Uuid::new_v4(), "my-agent", None, None);
        let agent = domain_agent(&agent_row);
        assert_eq!(agent.model, "my-agent");
        assert_eq!(agent.toolsets, vec!["my-agent".to_string()]);
        assert_eq!(agent.mode, AgentMode::Primary);
    }

    // 13. toolset_for_agent_local_tools_always_empty
    #[test]
    fn toolset_for_agent_local_tools_always_empty() {
        let agent = sample_agent(Uuid::new_v4(), "test-agent", None, None);
        let toolset = toolset_for_agent(&agent, &[]);
        assert!(toolset.local_tools.is_empty());
    }

    // 14. toolset_for_agent_mcp_selections_from_rows
    #[test]
    fn toolset_for_agent_mcp_selections_from_rows() {
        let agent = sample_agent(Uuid::new_v4(), "test-agent", None, None);
        let row_http = DbMcpServer {
            id: Uuid::new_v4(),
            name: "http-srv".to_string(),
            server_type: "http-streamable".to_string(),
            url: "http://localhost:1234".to_string(),
            headers: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let row_sse = DbMcpServer {
            id: Uuid::new_v4(),
            name: "sse-srv".to_string(),
            server_type: "sse".to_string(),
            url: "http://localhost:5678".to_string(),
            headers: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let mcp_rows = vec![row_http, row_sse];
        let toolset = toolset_for_agent(&agent, &mcp_rows);
        assert_eq!(toolset.mcp.len(), 1);
        assert_eq!(toolset.mcp[0].server, "http-srv");
    }
}
