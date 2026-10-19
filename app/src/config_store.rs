use std::collections::BTreeMap;

use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use vanyline_cfgstore::CfgStoreError;
use vanyline_lib::VnyError;
use vanyline_lib::domain::{
    Agent, AgentMode, McpSelection, McpServer, McpTransport, ModelProfile, Provider, ProviderType,
    SkillMeta, SkillSelection, Toolset,
};
use vanyline_lib::store::ConfigStore;

use crate::db::entities::{
    agents::Model as AgentRecord, llm_providers::Model as DbLlmProvider,
    mcp_servers::Model as DbMcpServer, model_profiles::Model as DbModelProfile,
    skills::Model as DbSkill, toolsets::Model as DbToolset,
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

/// `None` si `row.server_type` est inconnu (à logger par l'appelant, PAS ici —
/// fonction pure). `sse` mappe sur `McpTransport::Sse` : accepté, mais la
/// connexion remonte `VNL-MCP-004` en `ToolUnavailable` (cf. `prefixed_mcp`).
fn domain_mcp_server(row: &DbMcpServer) -> Option<McpServer> {
    let transport = match row.server_type.as_str() {
        "http-streamable" => McpTransport::HttpStreamable,
        "sse" => McpTransport::Sse,
        _ => return None,
    };
    Some(McpServer {
        name: row.name.clone(),
        transport,
        url: row.url.clone(),
        headers: headers_from_json(&row.headers),
    })
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
        max_tokens: row.max_tokens.map(i64::cast_unsigned),
        options,
    })
}

/// `agent.mode` est une garantie d'intégrité (CHECK en base), donc en
/// pratique `Err` n'arrive jamais — mais on garde le Result parce que
/// `agent_mode_from_str` expose un contrat avec le consommateur final.
fn agent_mode_from_str(s: &str) -> Result<AgentMode, VnyError> {
    match s {
        "primary" => Ok(AgentMode::Primary),
        "subagent" => Ok(AgentMode::Subagent),
        "all" => Ok(AgentMode::All),
        other => Err(VnyError::ConfigError(format!(
            "unknown agent mode: {other}"
        ))),
    }
}

/// `profiles` = résultat de `load_model_profiles()` de CE `PgConfigStore`
/// (même utilisateur — la garantie de cohérence vient de l'appelant, pas de
/// cette fonction). L'Agent.model porte le NOM du profil, jamais son id.
fn resolve_model_profile_name(
    agent: &AgentRecord,
    profiles: &[DbModelProfile],
) -> Result<String, VnyError> {
    profiles
        .iter()
        .find(|p| p.id == agent.model_profile_id)
        .map(|p| p.name.clone())
        .ok_or_else(|| VnyError::UnknownReference("model", agent.model_profile_id.to_string()))
}

/// `toolsets`/`skills` : repli défensif JSON invalide -> vide/`SkillSelection`
/// par défaut — un JSON malformé sur ces deux colonnes ne doit pas faire
/// échouer tout `list_agents`. En revanche `mode` invalide et
/// `model_profile_id` orphelin restent des erreurs dures.
fn domain_agent_record(row: &AgentRecord, profiles: &[DbModelProfile]) -> Result<Agent, VnyError> {
    let model = resolve_model_profile_name(row, profiles)?;
    let mode = agent_mode_from_str(&row.mode)?;
    let toolsets: Vec<String> = serde_json::from_value(row.toolsets.clone()).unwrap_or_default();
    let skills: SkillSelection = serde_json::from_value(row.skills.clone()).unwrap_or_default();
    Ok(Agent {
        name: row.name.clone(),
        description: row.description.clone(),
        mode,
        model,
        toolsets,
        skills,
        system_prompt: row.system_prompt.clone(),
    })
}

/// Infaillible par design : `local_tools`/`mcp` sont `NOT NULL DEFAULT '[]'`
/// en base et n'y sont écrits que par l'app elle-même — un JSON qui ne
/// correspond pas à la forme attendue retombe silencieusement sur une liste
/// vide plutôt que d'échouer tout `list_toolsets` (même posture défensive
/// que `domain_model_profile` pour `options`).
fn domain_toolset(row: &DbToolset) -> Toolset {
    let local_tools: Vec<String> =
        serde_json::from_value(row.local_tools.clone()).unwrap_or_default();
    let mcp: Vec<McpSelection> = serde_json::from_value(row.mcp.clone()).unwrap_or_default();
    Toolset {
        name: row.name.clone(),
        description: row.description.clone(),
        prompt: row.prompt.clone(),
        local_tools,
        mcp,
    }
}

/// Index léger : ne porte jamais `body` (cohérent avec `SkillMeta` côté
/// domaine — le corps est chargé séparément par `load_skill`).
fn domain_skill_meta(row: &DbSkill) -> SkillMeta {
    SkillMeta {
        name: row.name.clone(),
        description: row.description.clone(),
    }
}

fn cfg_err(e: &VnyError) -> CfgStoreError {
    CfgStoreError::Config(e.to_string())
}

// ---------------------------------------------------------------------------
// PgConfigStore — méthodes ConfigStore, fines : une requête + un appel aux
// fonctions pures ci-dessus.
// ---------------------------------------------------------------------------

pub struct PgConfigStore {
    db: DatabaseConnection,
    user_id: i32,
}

impl PgConfigStore {
    pub fn new(db: DatabaseConnection, user_id: i32) -> Self {
        Self { db, user_id }
    }

    async fn load_providers(&self) -> Result<Vec<DbLlmProvider>, CfgStoreError> {
        crate::db::entities::llm_providers::Entity::find()
            .all(&self.db)
            .await
            .map_err(|e| CfgStoreError::Config(e.to_string()))
    }

    async fn load_model_profiles(&self) -> Result<Vec<DbModelProfile>, CfgStoreError> {
        use crate::db::entities::model_profiles::Column;
        crate::db::entities::model_profiles::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .all(&self.db)
            .await
            .map_err(|e| CfgStoreError::Config(e.to_string()))
    }

    async fn load_toolsets(&self) -> Result<Vec<DbToolset>, CfgStoreError> {
        use crate::db::entities::toolsets::Column;
        crate::db::entities::toolsets::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .all(&self.db)
            .await
            .map_err(|e| CfgStoreError::Config(e.to_string()))
    }

    async fn load_agent_records(&self) -> Result<Vec<AgentRecord>, CfgStoreError> {
        use crate::db::entities::agents::Column;
        crate::db::entities::agents::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .all(&self.db)
            .await
            .map_err(|e| CfgStoreError::Config(e.to_string()))
    }

    async fn load_skills(&self) -> Result<Vec<DbSkill>, CfgStoreError> {
        use crate::db::entities::skills::Column;
        crate::db::entities::skills::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .all(&self.db)
            .await
            .map_err(|e| CfgStoreError::Config(e.to_string()))
    }
}

#[async_trait]
impl ConfigStore for PgConfigStore {
    async fn list_providers(&self) -> Result<Vec<Provider>, CfgStoreError> {
        let rows = self.load_providers().await?;
        rows.iter()
            .map(domain_provider)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| cfg_err(&e))
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, CfgStoreError> {
        let rows = self.load_model_profiles().await?;
        let providers = self.load_providers().await?;
        rows.iter()
            .map(|r| domain_model_profile(r, &providers))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| cfg_err(&e))
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, CfgStoreError> {
        let rows = crate::db::entities::mcp_servers::Entity::find()
            .all(&self.db)
            .await
            .map_err(|e| CfgStoreError::Config(e.to_string()))?;
        let mut result = Vec::new();
        for row in &rows {
            if let Some(server) = domain_mcp_server(row) {
                result.push(server);
            } else {
                tracing::warn!(
                    "Skipping MCP server '{}': unknown server_type '{}'",
                    row.name,
                    row.server_type
                );
            }
        }
        Ok(result)
    }

    async fn list_toolsets(&self) -> Result<Vec<Toolset>, CfgStoreError> {
        let rows = self.load_toolsets().await?;
        Ok(rows.iter().map(domain_toolset).collect())
    }

    async fn list_agents(&self) -> Result<Vec<Agent>, CfgStoreError> {
        let rows = self.load_agent_records().await?;
        let profiles = self.load_model_profiles().await?;
        let mut agents = Vec::with_capacity(rows.len());
        for row in &rows {
            match domain_agent_record(row, &profiles) {
                Ok(agent) => agents.push(agent),
                // Profil absent du jeu de l'owner (autre owner, ou orphelin) : la ligne
                // est sautée, elle n'empoisonne pas la liste (config_store.sdd, BUG CONFIRMÉ).
                Err(VnyError::UnknownReference("model", profile_id)) => {
                    tracing::warn!(
                        "Skipping agent '{}': model profile '{}' not found for this owner",
                        row.name,
                        profile_id
                    );
                }
                Err(e) => return Err(cfg_err(&e)),
            }
        }
        Ok(agents)
    }

    async fn list_skills(&self) -> Result<Vec<SkillMeta>, CfgStoreError> {
        let rows = self.load_skills().await?;
        Ok(rows.iter().map(domain_skill_meta).collect())
    }

    async fn load_skill(&self, name: &str) -> Result<String, CfgStoreError> {
        use crate::db::entities::skills::Column;
        crate::db::entities::skills::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .filter(Column::Name.eq(name))
            .one(&self.db)
            .await
            .map_err(|e| CfgStoreError::Config(e.to_string()))?
            .map(|row| row.body)
            .ok_or_else(|| CfgStoreError::UnknownReference("skill", name.to_string()))
    }

    async fn default_agent(&self) -> Result<Option<String>, CfgStoreError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_provider(
        id: i32,
        name: &str,
        provider_type: &str,
        is_default: bool,
    ) -> DbLlmProvider {
        DbLlmProvider {
            id,
            name: name.to_string(),
            provider_type: provider_type.to_string(),
            endpoint: "http://localhost:11434".to_string(),
            api_key: None,
            available_models: serde_json::json!([]),
            is_default,
        }
    }

    fn sample_model_profile(
        id: i32,
        owner_id: i32,
        name: &str,
        provider_id: i32,
        model: &str,
    ) -> DbModelProfile {
        DbModelProfile {
            id,
            owner_id,
            name: name.to_string(),
            provider_id,
            model: model.to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::json!({}),
        }
    }

    fn sample_agent_record(
        id: i32,
        owner_id: i32,
        name: &str,
        mode: &str,
        model_profile_id: i32,
        toolsets: serde_json::Value,
        skills: serde_json::Value,
    ) -> AgentRecord {
        AgentRecord {
            id,
            owner_id,
            name: name.to_string(),
            description: None,
            mode: mode.to_string(),
            model_profile_id,
            toolsets,
            skills,
            system_prompt: "prompt".to_string(),
            system_key: None,
            system_version: None,
        }
    }

    fn sample_toolset(
        id: i32,
        owner_id: i32,
        name: &str,
        local_tools: serde_json::Value,
        mcp: serde_json::Value,
    ) -> DbToolset {
        DbToolset {
            id,
            owner_id,
            name: name.to_string(),
            description: None,
            prompt: None,
            local_tools,
            mcp,
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
            id: 1,
            name: "my-server".to_string(),
            server_type: "http-streamable".to_string(),
            url: "http://localhost:8080".to_string(),
            headers: serde_json::json!({"X-Custom": "value"}),
            available_tools: serde_json::json!([]),
        };
        let server = domain_mcp_server(&row).unwrap();
        assert_eq!(server.name, "my-server");
        assert!(matches!(server.transport, McpTransport::HttpStreamable));
        assert_eq!(server.url, "http://localhost:8080");
        assert_eq!(server.headers.get("X-Custom"), Some(&"value".to_string()));
    }

    // 4. domain_mcp_server_sse_skipped
    #[test]
    fn domain_mcp_server_sse_mapped() {
        let row = DbMcpServer {
            id: 1,
            name: "sse-srv".to_string(),
            server_type: "sse".to_string(),
            url: "http://localhost:9090".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
        };
        let server = domain_mcp_server(&row).expect("sse mappé, plus ignoré");
        assert_eq!(server.transport, McpTransport::Sse);
    }

    #[test]
    fn domain_mcp_server_unknown_type_skipped() {
        let row = DbMcpServer {
            id: 1,
            name: "bogus".to_string(),
            server_type: "carrier-pigeon".to_string(),
            url: "http://localhost:9090".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
        };
        assert!(domain_mcp_server(&row).is_none());
    }

    // 5. domain_model_profile_resolves_provider_name
    #[test]
    fn domain_model_profile_resolves_provider_name() {
        let pid = 1;
        let row = sample_model_profile(2, 10, "qwen", pid, "qwen2.5");
        let provider = sample_provider(pid, "ollama", "ollama", false);
        let result = domain_model_profile(&row, &[provider]).unwrap();
        assert_eq!(result.provider, "ollama");
        assert_eq!(result.name, "qwen");
        assert_eq!(result.model, "qwen2.5");
    }

    // 11. domain_model_profile_unknown_provider_errors
    #[test]
    fn domain_model_profile_unknown_provider_errors() {
        let row = sample_model_profile(2, 10, "qwen", 999, "qwen2.5"); // provider 999 absent
        let provider = sample_provider(1, "ollama", "ollama", false);
        let err = domain_model_profile(&row, &[provider]).unwrap_err();
        assert!(matches!(err, VnyError::LlmProviderNotFound));
    }

    // 12. domain_model_profile_options_object_passthrough
    #[test]
    fn domain_model_profile_options_object_passthrough() {
        let pid = 1;
        let row = DbModelProfile {
            id: 2,
            owner_id: 10,
            name: "qwen".to_string(),
            provider_id: pid,
            model: "qwen2.5".to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::json!({"num_ctx": 65536}),
        };
        let provider = sample_provider(pid, "ollama", "ollama", false);
        let result = domain_model_profile(&row, &[provider]).unwrap();
        assert_eq!(
            result.options.get("num_ctx"),
            Some(&serde_json::json!(65536))
        );
    }

    // 13. domain_model_profile_options_non_object_defaults_empty
    #[test]
    fn domain_model_profile_options_non_object_defaults_empty() {
        let pid = 1;
        let row = DbModelProfile {
            id: 2,
            owner_id: 10,
            name: "qwen".to_string(),
            provider_id: pid,
            model: "qwen2.5".to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::Value::Null,
        };
        let provider = sample_provider(pid, "ollama", "ollama", false);
        let result = domain_model_profile(&row, &[provider]).unwrap();
        assert!(result.options.is_empty());
    }

    // 14. domain_model_profile_max_tokens_conversion
    #[test]
    fn domain_model_profile_max_tokens_conversion() {
        let pid = 1;
        let row = DbModelProfile {
            id: 2,
            owner_id: 10,
            name: "qwen".to_string(),
            provider_id: pid,
            model: "qwen2.5".to_string(),
            temperature: None,
            max_tokens: Some(4096),
            options: serde_json::json!({}),
        };
        let provider = sample_provider(pid, "ollama", "ollama", false);
        let result = domain_model_profile(&row, &[provider]).unwrap();
        assert_eq!(result.max_tokens, Some(4096u64));
    }

    // 15. domain_toolset_parses_local_tools_and_mcp
    #[test]
    fn domain_toolset_parses_local_tools_and_mcp() {
        let local_tools = serde_json::json!(["read_file", "search"]);
        let mcp = serde_json::json!([{"server": "fs", "tools": ["read"]}]);
        let row = sample_toolset(1, 10, "test-toolset", local_tools, mcp);
        let toolset = domain_toolset(&row);
        assert_eq!(
            toolset.local_tools,
            vec!["read_file".to_string(), "search".to_string()]
        );
        assert_eq!(toolset.mcp.len(), 1);
        assert_eq!(toolset.mcp[0].server, "fs");
        assert_eq!(toolset.mcp[0].tools, vec!["read".to_string()]);
    }

    // 16. domain_toolset_defaults_on_invalid_json
    #[test]
    fn domain_toolset_defaults_on_invalid_json() {
        let row = DbToolset {
            id: 1,
            owner_id: 10,
            name: "test-toolset".to_string(),
            description: None,
            prompt: None,
            local_tools: serde_json::Value::Null,
            mcp: serde_json::Value::Null,
        };
        let toolset = domain_toolset(&row);
        assert!(toolset.local_tools.is_empty());
        assert!(toolset.mcp.is_empty());
    }

    // 17. domain_toolset_preserves_description_and_prompt
    #[test]
    fn domain_toolset_preserves_description_and_prompt() {
        let row = DbToolset {
            id: 1,
            owner_id: 10,
            name: "desc-toolset".to_string(),
            description: Some("desc".to_string()),
            prompt: Some("frag".to_string()),
            local_tools: serde_json::json!([]),
            mcp: serde_json::json!([]),
        };
        let toolset = domain_toolset(&row);
        assert_eq!(toolset.description, Some("desc".to_string()));
        assert_eq!(toolset.prompt, Some("frag".to_string()));

        // second case: both None stay None
        let row2 = DbToolset {
            id: 2,
            owner_id: 10,
            name: "blank-toolset".to_string(),
            description: None,
            prompt: None,
            local_tools: serde_json::json!([]),
            mcp: serde_json::json!([]),
        };
        let toolset2 = domain_toolset(&row2);
        assert_eq!(toolset2.description, None);
        assert_eq!(toolset2.prompt, None);
    }

    // 18. domain_skill_meta_index_only
    #[test]
    fn domain_skill_meta_index_only() {
        let row = DbSkill {
            id: 1,
            owner_id: 10,
            name: "pdf".to_string(),
            description: "PDF processing".to_string(),
            body: "# corps long...".to_string(),
        };
        let meta = domain_skill_meta(&row);
        assert_eq!(meta.name, "pdf");
        assert_eq!(meta.description, "PDF processing");
    }

    // 19. domain_skill_meta_empty_description
    #[test]
    fn domain_skill_meta_empty_description() {
        let row = DbSkill {
            id: 1,
            owner_id: 10,
            name: "blank".to_string(),
            description: String::new(),
            body: "body".to_string(),
        };
        let meta = domain_skill_meta(&row);
        assert_eq!(meta.name, "blank");
        assert_eq!(meta.description, "");
    }

    // --- agents (pg-store) ---

    // 20. agent_mode_from_str_known_values
    #[test]
    fn agent_mode_from_str_known_values() {
        assert_eq!(agent_mode_from_str("primary").unwrap(), AgentMode::Primary);
        assert_eq!(
            agent_mode_from_str("subagent").unwrap(),
            AgentMode::Subagent
        );
        assert_eq!(agent_mode_from_str("all").unwrap(), AgentMode::All);
    }

    // 21. agent_mode_from_str_unknown_errors
    #[test]
    fn agent_mode_from_str_unknown_errors() {
        let err = agent_mode_from_str("bogus").unwrap_err();
        assert!(matches!(err, VnyError::ConfigError(_)));
    }

    // 22. resolve_model_profile_name_found
    #[test]
    fn resolve_model_profile_name_found() {
        let row = sample_model_profile(2, 10, "qwen-pro", 1, "qwen2.5");
        let agent = sample_agent_record(
            3,
            10,
            "coder",
            "primary",
            2,
            serde_json::json!([]),
            serde_json::json!([]),
        );
        let profiles = vec![row];
        let name = resolve_model_profile_name(&agent, &profiles).unwrap();
        assert_eq!(name, "qwen-pro");
    }

    // 23. resolve_model_profile_name_unknown_errors
    #[test]
    fn resolve_model_profile_name_unknown_errors() {
        let row = sample_model_profile(2, 10, "qwen", 1, "qwen2.5");
        let orphan_id = 999; // id qui n'appartient à aucun profil
        let agent = sample_agent_record(
            3,
            10,
            "coder",
            "primary",
            orphan_id, // id absent de profiles
            serde_json::json!([]),
            serde_json::json!([]),
        );
        let err = resolve_model_profile_name(&agent, &[row]).unwrap_err();
        assert!(
            matches!(err, VnyError::UnknownReference("model", ref id) if *id == orphan_id.to_string())
        );
    }

    // 24. domain_agent_record_full_conversion
    #[test]
    fn domain_agent_record_full_conversion() {
        let row = sample_model_profile(2, 10, "qwen-pro", 1, "qwen2.5");
        let agent = sample_agent_record(
            3,
            10,
            "coder",
            "subagent",
            2, // model_profile_id pointe sur le row créé ci-dessus
            serde_json::json!(["fs", "build"]),
            serde_json::json!(["pdf"]),
        );
        let result = domain_agent_record(&agent, &[row]).unwrap();
        assert_eq!(result.mode, AgentMode::Subagent);
        assert_eq!(result.model, "qwen-pro");
        assert_eq!(result.toolsets, vec!["fs".to_string(), "build".to_string()]);
        assert!(matches!(
            result.skills,
            SkillSelection::Named(names) if names == vec!["pdf".to_string()]
        ));
    }

    // 25. domain_agent_record_skills_auto_and_none
    #[test]
    fn domain_agent_record_skills_auto_and_none() {
        let row = sample_model_profile(2, 10, "qwen", 1, "qwen2.5");
        let agent_auto = sample_agent_record(
            3,
            10,
            "a",
            "primary",
            2,
            serde_json::json!([]),
            serde_json::json!("auto"),
        );
        let agent_none = sample_agent_record(
            4,
            10,
            "b",
            "primary",
            2,
            serde_json::json!([]),
            serde_json::json!("none"),
        );
        let result_auto = domain_agent_record(&agent_auto, std::slice::from_ref(&row)).unwrap();
        let result_none = domain_agent_record(&agent_none, std::slice::from_ref(&row)).unwrap();
        assert!(matches!(result_auto.skills, SkillSelection::Auto));
        assert!(matches!(result_none.skills, SkillSelection::None));
    }

    // 27. domain_agent_record_invalid_toolsets_json_defaults_empty
    #[test]
    fn domain_agent_record_invalid_toolsets_json_defaults_empty() {
        let row = sample_model_profile(2, 10, "qwen", 1, "qwen2.5");
        let agent = sample_agent_record(
            3,
            10,
            "a",
            "primary",
            2,
            serde_json::Value::Null,
            serde_json::json!([]),
        );
        let result = domain_agent_record(&agent, &[row]).unwrap();
        assert!(result.toolsets.is_empty());
    }

    // 28. domain_agent_record_invalid_skills_json_defaults_auto
    #[test]
    fn domain_agent_record_invalid_skills_json_defaults_auto() {
        let row = sample_model_profile(2, 10, "qwen", 1, "qwen2.5");
        let agent = sample_agent_record(
            3,
            10,
            "a",
            "primary",
            2,
            serde_json::json!([]),
            serde_json::Value::Null,
        );
        let result = domain_agent_record(&agent, &[row]).unwrap();
        assert!(matches!(result.skills, SkillSelection::Auto));
    }

    // 29. domain_agent_record_unknown_model_profile_errors
    #[test]
    fn domain_agent_record_unknown_model_profile_errors() {
        let agent_record = sample_agent_record(
            3,
            10,
            "a",
            "primary",
            999, // absent de profiles
            serde_json::json!([]),
            serde_json::json!([]),
        );
        let profiles: Vec<DbModelProfile> = Vec::new();
        let err = domain_agent_record(&agent_record, &profiles).unwrap_err();
        assert!(matches!(err, VnyError::UnknownReference("model", _)));
    }

    // 30. domain_agent_record_invalid_mode_errors
    #[test]
    fn domain_agent_record_invalid_mode_errors() {
        let row = sample_model_profile(2, 10, "qwen", 1, "qwen2.5");
        let agent = sample_agent_record(
            3,
            10,
            "a",
            "bogus",
            2,
            serde_json::json!([]),
            serde_json::json!([]),
        );
        let err = domain_agent_record(&agent, &[row]).unwrap_err();
        assert!(matches!(err, VnyError::ConfigError(_)));
    }

    // ---------------------------------------------------------------------
    // Scenario de méthode (config_store.sdd) sur base réelle sqlite en mémoire.
    // Les inserts passent par `Entity::insert` pour contourner les hooks miryad-core.
    // Consignés et NON écrits (bug de sécurité ou posture `[!]`/arbitrage) : voir
    // .claude/memory/haiku-questions/app.md, passe 3.
    // ---------------------------------------------------------------------

    use crate::db::entities::{
        agents as agent_entity, llm_providers, mcp_servers, model_profiles, skills, toolsets,
    };
    use crate::db::test_support::real_db;
    use sea_orm::{ActiveValue::NotSet, Database, DatabaseConnection, EntityTrait, Set};

    async fn seed_user(db: &DatabaseConnection, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("user resolves")
            .id
    }

    async fn insert_provider(
        db: &DatabaseConnection,
        name: &str,
        provider_type: &str,
        api_key: Option<&str>,
    ) -> i32 {
        llm_providers::Entity::insert(llm_providers::ActiveModel {
            id: NotSet,
            name: Set(name.to_string()),
            provider_type: Set(provider_type.to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(api_key.map(str::to_string)),
            available_models: Set(serde_json::json!([])),
            is_default: Set(false),
        })
        .exec(db)
        .await
        .expect("provider inserts")
        .last_insert_id
    }

    async fn insert_profile(
        db: &DatabaseConnection,
        owner: i32,
        name: &str,
        provider_id: i32,
        max_tokens: Option<i64>,
    ) -> i32 {
        model_profiles::Entity::insert(model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            provider_id: Set(provider_id),
            model: Set("qwen2.5".to_string()),
            temperature: Set(Some(0.2)),
            max_tokens: Set(max_tokens),
            options: Set(serde_json::json!({"think": true})),
        })
        .exec(db)
        .await
        .expect("profile inserts")
        .last_insert_id
    }

    async fn insert_mcp(
        db: &DatabaseConnection,
        name: &str,
        server_type: &str,
        headers: serde_json::Value,
    ) {
        mcp_servers::Entity::insert(mcp_servers::ActiveModel {
            id: NotSet,
            name: Set(name.to_string()),
            server_type: Set(server_type.to_string()),
            url: Set("http://localhost:3000/mcp".to_string()),
            headers: Set(headers),
            available_tools: Set(serde_json::json!([])),
        })
        .exec(db)
        .await
        .expect("mcp server inserts");
    }

    async fn insert_toolset(
        db: &DatabaseConnection,
        owner: i32,
        name: &str,
        local_tools: serde_json::Value,
    ) {
        toolsets::Entity::insert(toolsets::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            description: Set(None),
            prompt: Set(None),
            local_tools: Set(local_tools),
            mcp: Set(serde_json::json!([])),
        })
        .exec(db)
        .await
        .expect("toolset inserts");
    }

    async fn insert_skill(
        db: &DatabaseConnection,
        owner: i32,
        name: &str,
        description: &str,
        body: &str,
    ) {
        skills::Entity::insert(skills::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            description: Set(description.to_string()),
            body: Set(body.to_string()),
        })
        .exec(db)
        .await
        .expect("skill inserts");
    }

    async fn insert_agent(
        db: &DatabaseConnection,
        owner: i32,
        name: &str,
        profile_id: i32,
        skills_json: serde_json::Value,
    ) {
        agent_entity::Entity::insert(agent_entity::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            description: Set(None),
            mode: Set("primary".to_string()),
            model_profile_id: Set(profile_id),
            toolsets: Set(serde_json::json!([])),
            skills: Set(skills_json),
            system_prompt: Set(String::new()),
            system_key: Set(None),
            system_version: Set(None),
        })
        .exec(db)
        .await
        .expect("agent inserts");
    }

    /// Scenario : `domain_provider_type_inconnu_est_erreur`.
    #[test]
    fn domain_provider_type_inconnu_est_erreur() {
        let row = sample_provider(1, "p", "bogus", false);
        let err = domain_provider(&row).expect_err("unknown provider type is an error");
        assert!(matches!(err, VnyError::UnknownProviderType(ref t) if t == "bogus"));
    }

    /// Scenario : `list_providers_retourne_le_domaine_sans_filtrer_owner`.
    /// NB : l'assertion sur `api_key` est volontairement omise (exposition en clair = bug de
    /// sécurité consigné, `llm_providers.sdd`).
    #[tokio::test]
    async fn list_providers_retourne_le_domaine_sans_filtrer_owner() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let bob = seed_user(&db, "bob").await;
        insert_provider(&db, "ollama-local", "ollama", Some("sk-test")).await;

        for owner in [alice, bob] {
            let providers = PgConfigStore::new(db.clone(), owner)
                .list_providers()
                .await
                .expect("list succeeds");
            assert_eq!(providers.len(), 1);
            assert_eq!(providers[0].name, "ollama-local");
            assert_eq!(providers[0].endpoint, "http://localhost:11434");
            assert!(matches!(providers[0].provider_type, ProviderType::Ollama));
        }
    }

    /// Scenario : `erreur_requete_remappe_en_vnl_cfg_001`.
    #[tokio::test]
    async fn erreur_requete_remappe_en_vnl_cfg_001() {
        // Base sans aucune table : toute requête échoue côté SeaORM.
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("sqlite connects");
        let err = PgConfigStore::new(db, 1)
            .list_toolsets()
            .await
            .expect_err("missing table is an error");
        assert!(matches!(err, CfgStoreError::Config(_)));
        assert!(
            err.to_string()
                .starts_with("VNL-CFG-001: Configuration error: "),
            "got: {err}"
        );
    }

    /// Scenario : `list_models_resout_le_provider_par_nom_table_globale`.
    #[tokio::test]
    async fn list_models_resout_le_provider_par_nom_table_globale() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let bob = seed_user(&db, "bob").await;
        let provider = insert_provider(&db, "ollama-local", "ollama", None).await;
        insert_profile(&db, alice, "qwen", provider, Some(4096)).await;
        insert_profile(&db, bob, "bob-only", provider, None).await;

        let models = PgConfigStore::new(db.clone(), alice)
            .list_models()
            .await
            .expect("list succeeds");
        assert_eq!(models.len(), 1);
        let m = &models[0];
        assert_eq!(m.name, "qwen");
        assert_eq!(m.provider, "ollama-local");
        assert_eq!(m.model, "qwen2.5");
        assert!(m.temperature.is_some_and(|t| (t - 0.2).abs() < 1e-6));
        assert_eq!(m.max_tokens, Some(4096));
        assert_eq!(m.options.get("think"), Some(&serde_json::json!(true)));
    }

    /// Scenario : `list_models_traverse_un_type_de_provider_inconnu`.
    #[tokio::test]
    async fn list_models_traverse_un_type_de_provider_inconnu() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let provider = insert_provider(&db, "legacy", "bogus", None).await;
        insert_profile(&db, alice, "qwen", provider, None).await;

        let models = PgConfigStore::new(db.clone(), alice)
            .list_models()
            .await
            .expect("list succeeds despite unknown provider type");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider, "legacy");
    }

    /// Scenario : `list_mcp_servers_saute_le_type_inconnu_sans_erreur`.
    #[tokio::test]
    async fn list_mcp_servers_saute_le_type_inconnu_sans_erreur() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        insert_mcp(&db, "srv-ok", "http-streamable", serde_json::json!({})).await;
        insert_mcp(&db, "pigeon", "carrier-pigeon", serde_json::json!({})).await;

        let servers = PgConfigStore::new(db.clone(), alice)
            .list_mcp_servers()
            .await
            .expect("list succeeds");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "srv-ok");
        assert!(matches!(servers[0].transport, McpTransport::HttpStreamable));
    }

    /// Scenario : `list_mcp_servers_filtre_les_headers_non_string`.
    #[tokio::test]
    async fn list_mcp_servers_filtre_les_headers_non_string() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        insert_mcp(
            &db,
            "h-mixte",
            "http-streamable",
            serde_json::json!({"X-Foo": "bar", "X-Num": 42}),
        )
        .await;
        insert_mcp(&db, "h-null", "http-streamable", serde_json::Value::Null).await;

        let servers = PgConfigStore::new(db.clone(), alice)
            .list_mcp_servers()
            .await
            .expect("list succeeds");
        let mixte = servers
            .iter()
            .find(|s| s.name == "h-mixte")
            .expect("mixte listed");
        assert_eq!(mixte.headers.len(), 1);
        assert_eq!(mixte.headers.get("X-Foo"), Some(&"bar".to_string()));
        let null = servers
            .iter()
            .find(|s| s.name == "h-null")
            .expect("null listed");
        assert!(null.headers.is_empty());
    }

    /// Scenario : `list_toolsets_et_list_skills_sont_infaillibles`.
    #[tokio::test]
    async fn list_toolsets_et_list_skills_sont_infaillibles() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        insert_toolset(&db, alice, "poison", serde_json::json!({"a": 1})).await;
        insert_skill(&db, alice, "pdf", "lecture pdf", "corps non vide").await;

        let store = PgConfigStore::new(db.clone(), alice);
        let toolsets = store.list_toolsets().await.expect("toolsets list succeeds");
        assert_eq!(toolsets.len(), 1);
        assert!(toolsets[0].local_tools.is_empty());
        let skills = store.list_skills().await.expect("skills list succeeds");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
    }

    /// Scenario : `list_agents_skill_selection_auto_none_named`.
    #[tokio::test]
    async fn list_agents_skill_selection_auto_none_named() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let provider = insert_provider(&db, "ollama-local", "ollama", None).await;
        let profile = insert_profile(&db, alice, "qwen", provider, None).await;
        insert_agent(&db, alice, "a-auto", profile, serde_json::json!("auto")).await;
        insert_agent(&db, alice, "a-none", profile, serde_json::json!("none")).await;
        insert_agent(
            &db,
            alice,
            "a-named",
            profile,
            serde_json::json!(["pdf", "render"]),
        )
        .await;

        let agents = PgConfigStore::new(db.clone(), alice)
            .list_agents()
            .await
            .expect("list succeeds");
        assert_eq!(agents.len(), 3);
        let by_name = |n: &str| {
            agents
                .iter()
                .find(|a| a.name == n)
                .unwrap_or_else(|| panic!("agent {n} listed"))
        };
        assert!(matches!(by_name("a-auto").skills, SkillSelection::Auto));
        assert!(matches!(by_name("a-none").skills, SkillSelection::None));
        assert!(matches!(
            &by_name("a-named").skills,
            SkillSelection::Named(names) if names == &vec!["pdf".to_string(), "render".to_string()]
        ));
    }

    /// Scenario : `list_agents_profil_dun_autre_owner_est_saute_sans_empoisonner_la_liste`.
    /// Décision Sébastien 2026-09-27 : l'agent dont le profil appartient à un autre owner
    /// est sauté (+ warn), il ne fait pas échouer la liste de l'owner.
    #[tokio::test]
    async fn list_agents_profil_dun_autre_owner_est_saute_sans_empoisonner_la_liste() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let bob = seed_user(&db, "bob").await;
        let provider = insert_provider(&db, "ollama-local", "ollama", None).await;
        let p_alice = insert_profile(&db, alice, "qwen-alice", provider, None).await;
        let p_bob = insert_profile(&db, bob, "qwen-bob", provider, None).await;
        insert_agent(&db, alice, "a-valide", p_alice, serde_json::json!("auto")).await;
        insert_agent(&db, alice, "a-croise", p_bob, serde_json::json!("auto")).await;

        let agents = PgConfigStore::new(db.clone(), alice)
            .list_agents()
            .await
            .expect("la liste d'alice reste lisible malgré le profil de bob");
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["a-valide"]);
        assert_eq!(agents[0].model, "qwen-alice");
    }

    /// Scenario : `les_quatre_tables_owner_filtrees_isolent_les_owners`.
    #[tokio::test]
    async fn les_quatre_tables_owner_filtrees_isolent_les_owners() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let bob = seed_user(&db, "bob").await;
        let provider = insert_provider(&db, "ollama-local", "ollama", None).await;
        let p_alice = insert_profile(&db, alice, "qwen-alice", provider, None).await;
        insert_profile(&db, bob, "qwen-bob", provider, None).await;
        insert_toolset(&db, alice, "fs-alice", serde_json::json!([])).await;
        insert_toolset(&db, bob, "fs-bob", serde_json::json!([])).await;
        insert_skill(&db, alice, "pdf-alice", "", "").await;
        insert_skill(&db, bob, "pdf-bob", "", "").await;
        insert_agent(&db, alice, "ag-alice", p_alice, serde_json::json!("auto")).await;
        insert_agent(&db, bob, "ag-bob", p_alice, serde_json::json!("auto")).await;

        let store = PgConfigStore::new(db.clone(), alice);
        let models: Vec<String> = store
            .list_models()
            .await
            .expect("models")
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(models, ["qwen-alice"]);
        let tools: Vec<String> = store
            .list_toolsets()
            .await
            .expect("toolsets")
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(tools, ["fs-alice"]);
        let skill_names: Vec<String> = store
            .list_skills()
            .await
            .expect("skills")
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(skill_names, ["pdf-alice"]);
        let agent_names: Vec<String> = store
            .list_agents()
            .await
            .expect("agents")
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(agent_names, ["ag-alice"]);
    }

    /// Scenario : `load_skill_dun_autre_owner_rend_vnl_cfg_003`.
    #[tokio::test]
    async fn load_skill_dun_autre_owner_rend_vnl_cfg_003() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let bob = seed_user(&db, "bob").await;
        insert_skill(&db, bob, "pdf", "", "corps-bob").await;

        let err = PgConfigStore::new(db.clone(), alice)
            .load_skill("pdf")
            .await
            .expect_err("alice does not see bob's skill");
        assert!(matches!(err, CfgStoreError::UnknownReference("skill", ref n) if n == "pdf"));
        let body = PgConfigStore::new(db.clone(), bob)
            .load_skill("pdf")
            .await
            .expect("bob reads his skill");
        assert_eq!(body, "corps-bob");
    }

    /// Scenario : `default_agent_ne_designe_jamais_rien`.
    #[tokio::test]
    async fn default_agent_ne_designe_jamais_rien() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let provider = insert_provider(&db, "ollama-local", "ollama", None).await;
        let profile = insert_profile(&db, alice, "qwen", provider, None).await;
        insert_agent(&db, alice, "ag", profile, serde_json::json!("auto")).await;

        let default = PgConfigStore::new(db.clone(), alice)
            .default_agent()
            .await
            .expect("default_agent succeeds");
        assert_eq!(default, None);
    }

    /// Scenario : `get_skill_rend_les_meta_pas_le_corps`.
    #[tokio::test]
    async fn get_skill_rend_les_meta_pas_le_corps() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        insert_skill(&db, alice, "pdf", "lecture pdf", "corps non vide").await;

        let store = PgConfigStore::new(db.clone(), alice);
        let meta = store.get_skill("pdf").await.expect("skill resolves");
        assert_eq!(meta.name, "pdf");
        assert_eq!(meta.description, "lecture pdf");
        let err = store.get_skill("absent").await.expect_err("absent skill");
        assert!(matches!(err, CfgStoreError::UnknownReference("skill", ref n) if n == "absent"));
    }

    /// Scenario : `get_mcp_server_sur_ligne_sautee_rend_vnl_cfg_003`.
    #[tokio::test]
    async fn get_mcp_server_sur_ligne_sautee_rend_vnl_cfg_003() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        insert_mcp(&db, "pigeon", "carrier-pigeon", serde_json::json!({})).await;

        let err = PgConfigStore::new(db.clone(), alice)
            .get_mcp_server("pigeon")
            .await
            .expect_err("skipped row is indistinguishable from absence");
        assert!(
            matches!(err, CfgStoreError::UnknownReference("mcp_server", ref n) if n == "pigeon")
        );
    }
}
