use std::collections::BTreeMap;

use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

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

    async fn load_providers(&self) -> Result<Vec<DbLlmProvider>, VnyError> {
        crate::db::entities::llm_providers::Entity::find()
            .all(&self.db)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))
    }

    async fn load_model_profiles(&self) -> Result<Vec<DbModelProfile>, VnyError> {
        use crate::db::entities::model_profiles::Column;
        crate::db::entities::model_profiles::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .all(&self.db)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))
    }

    async fn load_toolsets(&self) -> Result<Vec<DbToolset>, VnyError> {
        use crate::db::entities::toolsets::Column;
        crate::db::entities::toolsets::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .all(&self.db)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))
    }

    async fn load_agent_records(&self) -> Result<Vec<AgentRecord>, VnyError> {
        use crate::db::entities::agents::Column;
        crate::db::entities::agents::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .all(&self.db)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))
    }

    async fn load_skills(&self) -> Result<Vec<DbSkill>, VnyError> {
        use crate::db::entities::skills::Column;
        crate::db::entities::skills::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .all(&self.db)
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
        rows.iter()
            .map(|r| domain_model_profile(r, &providers))
            .collect()
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, VnyError> {
        let rows = crate::db::entities::mcp_servers::Entity::find()
            .all(&self.db)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))?;
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

    async fn list_toolsets(&self) -> Result<Vec<Toolset>, VnyError> {
        let rows = self.load_toolsets().await?;
        Ok(rows.iter().map(domain_toolset).collect())
    }

    async fn list_agents(&self) -> Result<Vec<Agent>, VnyError> {
        let rows = self.load_agent_records().await?;
        let profiles = self.load_model_profiles().await?;
        rows.iter()
            .map(|r| domain_agent_record(r, &profiles))
            .collect()
    }

    async fn list_skills(&self) -> Result<Vec<SkillMeta>, VnyError> {
        let rows = self.load_skills().await?;
        Ok(rows.iter().map(domain_skill_meta).collect())
    }

    async fn load_skill(&self, name: &str) -> Result<String, VnyError> {
        use crate::db::entities::skills::Column;
        crate::db::entities::skills::Entity::find()
            .filter(Column::OwnerId.eq(self.user_id))
            .filter(Column::Name.eq(name))
            .one(&self.db)
            .await
            .map_err(|e| VnyError::ConfigError(e.to_string()))?
            .map(|row| row.body)
            .ok_or_else(|| VnyError::UnknownReference("skill", name.to_string()))
    }

    async fn default_agent(&self) -> Result<Option<String>, VnyError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
    fn domain_mcp_server_sse_skipped() {
        let row = DbMcpServer {
            id: 1,
            name: "sse-srv".to_string(),
            server_type: "sse".to_string(),
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
}
