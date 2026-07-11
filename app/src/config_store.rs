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

use crate::db::models::{
    AgentRow, LlmProvider as DbLlmProvider, McpServer as DbMcpServer,
    ModelProfile as DbModelProfile, Toolset as DbToolset,
};

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

/// `providers` = résultat de `load_providers()` de CE `PgConfigStore` (même
/// utilisateur — la garantie de cohérence vient de l'appelant, pas de cette
/// fonction). `VnyError::LlmProviderNotFound` si `row.provider_id` n'y
/// figure pas.
fn domain_model_profile(
    row: &DbModelProfile,
    providers: &[DbLlmProvider],
) -> Result<ModelProfile, VnyError> {
    let provider = providers
        .iter()
        .find(|p| p.id == row.provider_id)
        .ok_or(VnyError::LlmProviderNotFound)?;
    let options = match &row.options {
        serde_json::Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    Ok(ModelProfile {
        name: row.name.clone(),
        provider: provider.name.clone(),
        model: row.model.clone(),
        temperature: row.temperature,
        max_tokens: row.max_tokens.map(|v| v as u64),
        options,
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

/// Infaillible par design : `local_tools`/`mcp` sont `NOT NULL DEFAULT '[]'`
/// en base et n'y sont écrits que par l'app elle-même — un JSON qui ne
/// correspond pas à la forme attendue retombe silencieusement sur une liste
/// vide plutôt que d'échouer tout `list_toolsets` (même posture défensive
/// que `domain_model_profile` pour `options`).
fn domain_toolset(row: &DbToolset) -> Toolset {
    let local_tools: Vec<String> =
        serde_json::from_value(row.local_tools.clone()).unwrap_or_default();
    let mcp: Vec<McpSelection> =
        serde_json::from_value(row.mcp.clone()).unwrap_or_default();
    Toolset {
        name: row.name.clone(),
        description: row.description.clone(),
        prompt: row.prompt.clone(),
        local_tools,
        mcp,
    }
}

// ---------------------------------------------------------------------------
// PgConfigStore — méthodes ConfigStore, fines : une requête + un appel aux
// fonctions pures ci-dessus.
// ---------------------------------------------------------------------------

pub struct PgConfigStore {
    pool: PgPool,
    user_id: Uuid,
}

impl PgConfigStore {
    pub fn new(pool: PgPool, user_id: Uuid) -> Self {
        Self { pool, user_id }
    }

    async fn load_providers(&self) -> Result<Vec<DbLlmProvider>, VnyError> {
        sqlx::query_as::<_, DbLlmProvider>("SELECT * FROM llm_providers WHERE user_id = $1")
            .bind(self.user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))
    }

    async fn load_model_profiles(&self) -> Result<Vec<DbModelProfile>, VnyError> {
        sqlx::query_as::<_, DbModelProfile>("SELECT * FROM model_profiles WHERE user_id = $1")
            .bind(self.user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))
    }

    async fn load_toolsets(&self) -> Result<Vec<DbToolset>, VnyError> {
        sqlx::query_as::<_, DbToolset>("SELECT * FROM toolsets WHERE user_id = $1")
            .bind(self.user_id)
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
}

#[async_trait]
impl ConfigStore for PgConfigStore {
    async fn list_providers(&self) -> Result<Vec<Provider>, VnyError> {
        let rows = self.load_providers().await?;
        rows.iter().map(domain_provider).collect()
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, VnyError> {
        let rows = self.load_model_profiles().await?;
        let providers = self.load_providers().await?;
        rows.iter().map(|r| domain_model_profile(r, &providers)).collect()
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, VnyError> {
        let rows =
            sqlx::query_as::<_, DbMcpServer>("SELECT * FROM mcp_servers WHERE user_id = $1")
                .bind(self.user_id)
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
        let rows = self.load_toolsets().await?;
        Ok(rows.iter().map(domain_toolset).collect())
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

    fn sample_provider(id: Uuid, user_id: Uuid, name: &str, provider_type: &str, is_default: bool) -> DbLlmProvider {
        DbLlmProvider {
            id,
            user_id,
            name: name.to_string(),
            provider_type: provider_type.to_string(),
            endpoint: "http://localhost:11434".to_string(),
            api_key: None,
            available_models: serde_json::json!([]),
            is_default,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn sample_model_profile(id: Uuid, user_id: Uuid, name: &str, provider_id: Uuid, model: &str) -> DbModelProfile {
        DbModelProfile {
            id,
            user_id,
            name: name.to_string(),
            provider_id,
            model: model.to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::json!({}),
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

    fn sample_toolset(
        id: Uuid,
        user_id: Uuid,
        name: &str,
        local_tools: serde_json::Value,
        mcp: serde_json::Value,
    ) -> DbToolset {
        DbToolset {
            id,
            user_id,
            name: name.to_string(),
            description: None,
            prompt: None,
            local_tools,
            mcp,
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
            user_id: Uuid::new_v4(),
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
            user_id: Uuid::new_v4(),
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
        let uid = Uuid::new_v4();
        let p1 = sample_provider(id1, uid, "ollama", "ollama", false);
        let p2 = sample_provider(id2, uid, "openai", "openai-compatible", false);
        let agents = vec![p1.clone(), p2.clone()];
        let agent = sample_agent(Uuid::new_v4(), "test-agent", Some(id2), None);
        let result = resolve_provider_for_agent(&agent, &agents).unwrap();
        assert_eq!(result.name, "openai");
    }

    // 6. resolve_provider_for_agent_unknown_id
    #[test]
    fn resolve_provider_for_agent_unknown_id() {
        let unknown_id = Uuid::new_v4();
        let p1 = sample_provider(Uuid::new_v4(), Uuid::new_v4(), "ollama", "ollama", false);
        let agent = sample_agent(Uuid::new_v4(), "test-agent", Some(unknown_id), None);
        let err = resolve_provider_for_agent(&agent, &[p1]).unwrap_err();
        assert!(matches!(err, VnyError::LlmProviderNotFound));
    }

    // 7. resolve_provider_for_agent_default_fallback
    #[test]
    fn resolve_provider_for_agent_default_fallback() {
        let p1 = sample_provider(Uuid::new_v4(), Uuid::new_v4(), "ollama", "ollama", true);
        let p2 = sample_provider(Uuid::new_v4(), Uuid::new_v4(), "openai", "openai-compatible", false);
        let agent = sample_agent(Uuid::new_v4(), "test-agent", None, None);
        let providers = vec![p1.clone(), p2];
        let result = resolve_provider_for_agent(&agent, &providers).unwrap();
        assert_eq!(result.name, "ollama");
        assert!(result.is_default);
    }

    // 8. resolve_provider_for_agent_no_default
    #[test]
    fn resolve_provider_for_agent_no_default() {
        let p1 = sample_provider(Uuid::new_v4(), Uuid::new_v4(), "ollama", "ollama", false);
        let p2 = sample_provider(Uuid::new_v4(), Uuid::new_v4(), "openai", "openai-compatible", false);
        let agent = sample_agent(Uuid::new_v4(), "test-agent", None, None);
        let err = resolve_provider_for_agent(&agent, &[p1, p2]).unwrap_err();
        assert!(matches!(err, VnyError::NoProviderConfigured));
    }

    // 9. domain_agent_synthesizes_self_references
    #[test]
    fn domain_agent_synthesizes_self_references() {
        let agent_row = sample_agent(Uuid::new_v4(), "my-agent", None, None);
        let agent = domain_agent(&agent_row);
        assert_eq!(agent.model, "my-agent");
        assert_eq!(agent.toolsets, vec!["my-agent".to_string()]);
        assert_eq!(agent.mode, AgentMode::Primary);
    }

    // 10. domain_model_profile_resolves_provider_name
    #[test]
    fn domain_model_profile_resolves_provider_name() {
        let pid = Uuid::new_v4();
        let uid = Uuid::new_v4();
        let row = sample_model_profile(Uuid::new_v4(), uid, "qwen", pid, "qwen2.5");
        let provider = sample_provider(pid, uid, "ollama", "ollama", false);
        let result = domain_model_profile(&row, &[provider]).unwrap();
        assert_eq!(result.provider, "ollama");
        assert_eq!(result.name, "qwen");
        assert_eq!(result.model, "qwen2.5");
    }

    // 11. domain_model_profile_unknown_provider_errors
    #[test]
    fn domain_model_profile_unknown_provider_errors() {
        let uid = Uuid::new_v4();
        let row = sample_model_profile(Uuid::new_v4(), uid, "qwen", Uuid::new_v4(), "qwen2.5");
        let provider = sample_provider(Uuid::new_v4(), uid, "ollama", "ollama", false);
        let err = domain_model_profile(&row, &[provider]).unwrap_err();
        assert!(matches!(err, VnyError::LlmProviderNotFound));
    }

    // 12. domain_model_profile_options_object_passthrough
    #[test]
    fn domain_model_profile_options_object_passthrough() {
        let uid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let row = DbModelProfile {
            id: Uuid::new_v4(),
            user_id: uid,
            name: "qwen".to_string(),
            provider_id: pid,
            model: "qwen2.5".to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::json!({"num_ctx": 65536}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let provider = sample_provider(pid, uid, "ollama", "ollama", false);
        let result = domain_model_profile(&row, &[provider]).unwrap();
        assert_eq!(result.options.get("num_ctx"), Some(&serde_json::json!(65536)));
    }

    // 13. domain_model_profile_options_non_object_defaults_empty
    #[test]
    fn domain_model_profile_options_non_object_defaults_empty() {
        let uid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let row = DbModelProfile {
            id: Uuid::new_v4(),
            user_id: uid,
            name: "qwen".to_string(),
            provider_id: pid,
            model: "qwen2.5".to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::Value::Null,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let provider = sample_provider(pid, uid, "ollama", "ollama", false);
        let result = domain_model_profile(&row, &[provider]).unwrap();
        assert!(result.options.is_empty());
    }

    // 14. domain_model_profile_max_tokens_conversion
    #[test]
    fn domain_model_profile_max_tokens_conversion() {
        let uid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let row = DbModelProfile {
            id: Uuid::new_v4(),
            user_id: uid,
            name: "qwen".to_string(),
            provider_id: pid,
            model: "qwen2.5".to_string(),
            temperature: None,
            max_tokens: Some(4096),
            options: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let provider = sample_provider(pid, uid, "ollama", "ollama", false);
        let result = domain_model_profile(&row, &[provider]).unwrap();
        assert_eq!(result.max_tokens, Some(4096u64));
    }

    // 15. domain_toolset_parses_local_tools_and_mcp
    #[test]
    fn domain_toolset_parses_local_tools_and_mcp() {
        let uid = Uuid::new_v4();
        let id = Uuid::new_v4();
        let local_tools = serde_json::json!(["read_file", "search"]);
        let mcp = serde_json::json!([{"server": "fs", "tools": ["read"]}]);
        let row = sample_toolset(id, uid, "test-toolset", local_tools.clone(), mcp.clone());
        let toolset = domain_toolset(&row);
        assert_eq!(toolset.local_tools, vec!["read_file".to_string(), "search".to_string()]);
        assert_eq!(toolset.mcp.len(), 1);
        assert_eq!(toolset.mcp[0].server, "fs");
        assert_eq!(toolset.mcp[0].tools, vec!["read".to_string()]);
    }

    // 16. domain_toolset_defaults_on_invalid_json
    #[test]
    fn domain_toolset_defaults_on_invalid_json() {
        let uid = Uuid::new_v4();
        let id = Uuid::new_v4();
        let row = DbToolset {
            id,
            user_id: uid,
            name: "test-toolset".to_string(),
            description: None,
            prompt: None,
            local_tools: serde_json::Value::Null,
            mcp: serde_json::Value::Null,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let toolset = domain_toolset(&row);
        assert!(toolset.local_tools.is_empty());
        assert!(toolset.mcp.is_empty());
    }

    // 17. domain_toolset_preserves_description_and_prompt
    #[test]
    fn domain_toolset_preserves_description_and_prompt() {
        let uid = Uuid::new_v4();
        let id = Uuid::new_v4();
        let row = DbToolset {
            id,
            user_id: uid,
            name: "desc-toolset".to_string(),
            description: Some("desc".to_string()),
            prompt: Some("frag".to_string()),
            local_tools: serde_json::json!([]),
            mcp: serde_json::json!([]),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let toolset = domain_toolset(&row);
        assert_eq!(toolset.description, Some("desc".to_string()));
        assert_eq!(toolset.prompt, Some("frag".to_string()));

        // second case: both None stay None
        let row2 = DbToolset {
            id: Uuid::new_v4(),
            user_id: uid,
            name: "blank-toolset".to_string(),
            description: None,
            prompt: None,
            local_tools: serde_json::json!([]),
            mcp: serde_json::json!([]),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let toolset2 = domain_toolset(&row2);
        assert_eq!(toolset2.description, None);
        assert_eq!(toolset2.prompt, None);
    }
}
