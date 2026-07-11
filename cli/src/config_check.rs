use std::collections::HashSet;

use vanyline_lib::domain::SkillSelection;
use vanyline_lib::store::ConfigStore;
use vanyline_lib::VnyError;

/// Charge toutes les listes du store et croise les références. Ne s'arrête
/// PAS à la première erreur : chaque `list_x()` est tenté indépendamment
/// (si l'un échoue — ex. YAML invalide — son erreur est ajoutée au rapport
/// et la liste correspondante est traitée comme vide pour la suite, les
/// AUTRES vérifications continuent quand même). Retourne la liste de tous
/// les problèmes trouvés — vide = config saine.
pub async fn check_config(store: &dyn ConfigStore) -> Vec<VnyError> {
    let mut problems = Vec::new();

    let providers = store.list_providers().await.unwrap_or_else(|e| { problems.push(e); Vec::new() });
    let models = store.list_models().await.unwrap_or_else(|e| { problems.push(e); Vec::new() });
    let mcp_servers = store.list_mcp_servers().await.unwrap_or_else(|e| { problems.push(e); Vec::new() });
    let toolsets = store.list_toolsets().await.unwrap_or_else(|e| { problems.push(e); Vec::new() });
    let agents = store.list_agents().await.unwrap_or_else(|e| { problems.push(e); Vec::new() });
    let skills = store.list_skills().await.unwrap_or_else(|e| { problems.push(e); Vec::new() });
    let default_agent = store.default_agent().await.unwrap_or_else(|e| { problems.push(e); None });

    // Doublons — défensif : la construction de FsConfigStore ne devrait
    // jamais en produire (clé de map / nom de fichier uniques par couche),
    // mais un ConfigStore custom (tests, futur backend) pourrait en avoir.
    check_duplicate_names(&providers, "provider", |p| &p.name, &mut problems);
    check_duplicate_names(&models, "model", |m| &m.name, &mut problems);
    check_duplicate_names(&mcp_servers, "mcp_server", |s| &s.name, &mut problems);
    check_duplicate_names(&toolsets, "toolset", |t| &t.name, &mut problems);
    check_duplicate_names(&agents, "agent", |a| &a.name, &mut problems);
    check_duplicate_names(&skills, "skill", |s| &s.name, &mut problems);

    // Références croisées. Convention de message : le second champ de
    // `UnknownReference` porte "<nom manquant> (referenced by <kind> '<porteur>')"
    // — pas de fichier ici (les entités du domaine ne portent pas leur
    // chemin source), juste de quoi identifier le référent.
    for m in &models {
        if !providers.iter().any(|p| p.name == m.provider) {
            problems.push(VnyError::UnknownReference(
                "provider",
                format!("{} (referenced by model '{}')", m.provider, m.name),
            ));
        }
    }
    for t in &toolsets {
        for sel in &t.mcp {
            if !mcp_servers.iter().any(|s| s.name == sel.server) {
                problems.push(VnyError::UnknownReference(
                    "mcp_server",
                    format!("{} (referenced by toolset '{}')", sel.server, t.name),
                ));
            }
        }
    }
    for a in &agents {
        if !models.iter().any(|m| m.name == a.model) {
            problems.push(VnyError::UnknownReference(
                "model",
                format!("{} (referenced by agent '{}')", a.model, a.name),
            ));
        }
        for ts_name in &a.toolsets {
            if !toolsets.iter().any(|t| &t.name == ts_name) {
                problems.push(VnyError::UnknownReference(
                    "toolset",
                    format!("{} (referenced by agent '{}')", ts_name, a.name),
                ));
            }
        }
        if let SkillSelection::Named(names) = &a.skills {
            for sk in names {
                if !skills.iter().any(|s| &s.name == sk) {
                    problems.push(VnyError::UnknownReference(
                        "skill",
                        format!("{} (referenced by agent '{}')", sk, a.name),
                    ));
                }
            }
        }
        // SkillSelection::Auto / ::None -> rien à valider, jamais de problème.
    }
    if let Some(name) = &default_agent {
        if !agents.iter().any(|a| &a.name == name) {
            problems.push(VnyError::UnknownReference(
                "agent",
                format!("{} (defaults.agent)", name),
            ));
        }
    }

    problems
}

fn check_duplicate_names<T>(
    items: &[T],
    kind: &'static str,
    name_of: impl Fn(&T) -> &str,
    problems: &mut Vec<VnyError>,
) {
    let mut seen = HashSet::new();
    for item in items {
        let name = name_of(item);
        if !seen.insert(name) {
            problems.push(VnyError::DuplicateName(kind, name.to_string()));
        }
    }
}

    #[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use vanyline_lib::domain::{Agent, AgentMode, McpSelection, McpServer, McpTransport, ModelProfile, Provider, ProviderType, SkillSelection, SkillMeta, Toolset};
    use vanyline_lib::store::InMemoryConfigStore;

    fn make_consistent_store() -> InMemoryConfigStore {
        InMemoryConfigStore {
            providers: vec![
                Provider {
                    name: "ollama".to_string(),
                    provider_type: ProviderType::Ollama,
                    endpoint: "http://localhost:11434".to_string(),
                    api_key: None,
                },
            ],
            models: vec![
                ModelProfile {
                    name: "qwen2.5".to_string(),
                    provider: "ollama".to_string(),
                    model: "qwen2.5".to_string(),
                    temperature: None,
                    max_tokens: None,
                    options: serde_json::Map::new(),
                },
            ],
            mcp_servers: vec![
                McpServer {
                    name: "fs".to_string(),
                    transport: McpTransport::HttpStreamable,
                    url: "http://mcp-fs:3000".to_string(),
                    headers: Default::default(),
                },
            ],
            toolsets: vec![
                Toolset {
                    name: "default".to_string(),
                    description: None,
                    prompt: None,
                    local_tools: vec![],
                    mcp: vec![
                        McpSelection { server: "fs".to_string(), tools: vec!["*".to_string()] },
                    ],
                },
            ],
            agents: vec![
                Agent {
                    name: "build".to_string(),
                    description: Some("Build agent".to_string()),
                    mode: AgentMode::Primary,
                    model: "qwen2.5".to_string(),
                    toolsets: vec!["default".to_string()],
                    skills: SkillSelection::Auto,
                    system_prompt: "Build helper.".to_string(),
                },
            ],
            skills: vec![
                SkillMeta {
                    name: "pdf".to_string(),
                    description: "PDF processing".to_string(),
                },
            ],
            skill_bodies: Default::default(),
            default_agent: Some("build".to_string()),
        }
    }

    #[tokio::test]
    async fn no_problems_on_consistent_config() {
        let store = make_consistent_store();
        let problems = check_config(&store).await;
        assert!(problems.is_empty(), "expected no problems, got: {:?}", problems);
    }

    #[tokio::test]
    async fn unknown_model_provider_detected() {
        let mut store = make_consistent_store();
        // Break the model -> provider reference
        store.models[0].provider = "ghost".to_string();
        let problems = check_config(&store).await;
        assert!(!problems.is_empty());
        match &problems[0] {
            VnyError::UnknownReference(kind, msg) => {
                assert_eq!(*kind, "provider");
                assert!(msg.contains("ghost"));
                assert!(msg.contains("qwen2.5"));
            }
            _ => panic!("Expected UnknownReference"),
        }
    }

    #[tokio::test]
    async fn unknown_toolset_mcp_server_detected() {
        let mut store = make_consistent_store();
        store.toolsets[0].mcp[0].server = "ghost".to_string();
        let problems = check_config(&store).await;
        assert!(!problems.is_empty());
        match &problems[0] {
            VnyError::UnknownReference(kind, msg) => {
                assert_eq!(*kind, "mcp_server");
                assert!(msg.contains("ghost"));
                assert!(msg.contains("default"));
            }
            _ => panic!("Expected UnknownReference"),
        }
    }

    #[tokio::test]
    async fn unknown_agent_model_detected() {
        let mut store = make_consistent_store();
        store.agents[0].model = "ghost".to_string();
        let problems = check_config(&store).await;
        let model_err = problems.into_iter().find(|p| matches!(p, VnyError::UnknownReference(k, _) if *k == "model"));
        assert!(model_err.is_some(), "expected UnknownReference(model)");
    }

    #[tokio::test]
    async fn unknown_agent_toolset_detected() {
        let mut store = make_consistent_store();
        store.agents[0].toolsets = vec!["ghost".to_string()];
        let problems = check_config(&store).await;
        let ts_err = problems.into_iter().find(|p| matches!(p, VnyError::UnknownReference(k, _) if *k == "toolset"));
        assert!(ts_err.is_some(), "expected UnknownReference(toolset)");
    }

    #[tokio::test]
    async fn unknown_agent_named_skill_detected() {
        let mut store = make_consistent_store();
        store.agents[0].skills = SkillSelection::Named(vec!["ghost".to_string()]);
        let problems = check_config(&store).await;
        let sk_err = problems.into_iter().find(|p| matches!(p, VnyError::UnknownReference(k, _) if *k == "skill"));
        assert!(sk_err.is_some(), "expected UnknownReference(skill)");
    }

    #[tokio::test]
    async fn agent_skills_auto_and_none_never_flagged() {
        let store = InMemoryConfigStore {
            agents: vec![
                Agent {
                    name: "auto-agent".to_string(),
                    description: None,
                    mode: AgentMode::Primary,
                    model: "qwen2.5".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::Auto,
                    system_prompt: "auto".to_string(),
                },
                Agent {
                    name: "none-agent".to_string(),
                    description: None,
                    mode: AgentMode::Primary,
                    model: "qwen2.5".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::None,
                    system_prompt: "none".to_string(),
                },
            ],
            models: vec![
                ModelProfile {
                    name: "qwen2.5".to_string(),
                    provider: "ollama".to_string(),
                    model: "qwen2.5".to_string(),
                    temperature: None,
                    max_tokens: None,
                    options: serde_json::Map::new(),
                },
            ],
            providers: vec![
                Provider {
                    name: "ollama".to_string(),
                    provider_type: ProviderType::Ollama,
                    endpoint: "http://localhost:11434".to_string(),
                    api_key: None,
                },
            ],
            mcp_servers: vec![],
            toolsets: vec![],
            skills: vec![],
            skill_bodies: Default::default(),
            default_agent: None,
        };
        let problems = check_config(&store).await;
        for p in &problems {
            match p {
                VnyError::UnknownReference(kind, _) => {
                    assert_ne!(*kind, "skill", "skill should never be flagged for Auto/None");
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn unknown_default_agent_detected() {
        let store = InMemoryConfigStore {
            agents: Vec::new(),
            default_agent: Some("ghost".to_string()),
            ..Default::default()
        };
        let problems = check_config(&store).await;
        let agent_err = problems.into_iter().find(|p| matches!(p, VnyError::UnknownReference(k, _) if *k == "agent"));
        assert!(agent_err.is_some(), "expected UnknownReference(agent) for ghost");
    }

    #[tokio::test]
    async fn default_agent_none_is_fine() {
        let store = make_consistent_store();
        let problems_check = check_config(&store).await;

        let mut store2 = store;
        store2.default_agent = None;
        let problems_none = check_config(&store2).await;

        // Without default_agent, there should be fewer or same problems
        // (specifically, no default_agent UnknownReference)
        let missing_default = problems_check.iter()
            .filter(|p| matches!(p, VnyError::UnknownReference(k, _) if *k == "agent"))
            .count();
        let missing_none = problems_none.iter()
            .filter(|p| matches!(p, VnyError::UnknownReference(k, _) if *k == "agent"))
            .count();
        assert!(missing_none <= missing_default, "None default_agent should produce fewer agent errors");
    }

    #[tokio::test]
    async fn duplicate_name_detected() {
        let store = InMemoryConfigStore {
            agents: vec![
                Agent {
                    name: "dup".to_string(),
                    description: None,
                    mode: AgentMode::Primary,
                    model: "m".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::Auto,
                    system_prompt: "p".to_string(),
                },
                Agent {
                    name: "dup".to_string(),
                    description: None,
                    mode: AgentMode::Subagent,
                    model: "m".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::Auto,
                    system_prompt: "p2".to_string(),
                },
            ],
            ..Default::default()
        };
        let problems = check_config(&store).await;
        let dup_err = problems.into_iter().find(|p| matches!(p, VnyError::DuplicateName(k, n) if *k == "agent" && *n == "dup"));
        assert!(dup_err.is_some(), "expected DuplicateName(agent, dup)");
    }

    /// Custom ConfigStore that always errors on list_skills() but works for everything else.
    struct ErrSkillsStore {
        inner: InMemoryConfigStore,
    }

    #[async_trait]
    impl ConfigStore for ErrSkillsStore {
        async fn list_providers(&self) -> Result<Vec<Provider>, VnyError> {
            self.inner.list_providers().await
        }
        async fn list_models(&self) -> Result<Vec<ModelProfile>, VnyError> {
            self.inner.list_models().await
        }
        async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, VnyError> {
            self.inner.list_mcp_servers().await
        }
        async fn list_toolsets(&self) -> Result<Vec<Toolset>, VnyError> {
            self.inner.list_toolsets().await
        }
        async fn list_agents(&self) -> Result<Vec<Agent>, VnyError> {
            self.inner.list_agents().await
        }
        async fn list_skills(&self) -> Result<Vec<SkillMeta>, VnyError> {
            Err(VnyError::ConfigError("boom".to_string()))
        }
        async fn load_skill(&self, _name: &str) -> Result<String, VnyError> {
            self.inner.load_skill(_name).await
        }
        async fn default_agent(&self) -> Result<Option<String>, VnyError> {
            self.inner.default_agent().await
        }
    }

    #[tokio::test]
    async fn partial_store_error_does_not_block_other_checks() {
        let inner = InMemoryConfigStore {
            models: vec![
                ModelProfile {
                    name: "ghost-model".to_string(),
                    provider: "ghost".to_string(), // unknown provider
                    model: "qwen2.5".to_string(),
                    temperature: None,
                    max_tokens: None,
                    options: serde_json::Map::new(),
                },
            ],
            providers: vec![],
            mcp_servers: vec![],
            toolsets: vec![],
            agents: vec![
                Agent {
                    name: "build".to_string(),
                    description: None,
                    mode: AgentMode::Primary,
                    model: "nonexistent-model".to_string(), // references model not in list
                    toolsets: vec![],
                    skills: SkillSelection::Named(vec!["ghost".to_string()]), // refs skills
                    system_prompt: "prompt".to_string(),
                },
            ],
            skills: vec![SkillMeta { name: "pdf".to_string(), description: "PDF".to_string() }],
            skill_bodies: Default::default(),
            default_agent: None,
        };
        let store = ErrSkillsStore { inner };
        let problems = check_config(&store).await;

        // Should contain both the ConfigError and the UnknownReference
        let config_err = problems.iter().any(|p| matches!(p, VnyError::ConfigError(msg) if msg == "boom"));
        let unknown_ref = problems.iter().any(|p| matches!(p, VnyError::UnknownReference(k, _) if *k == "model"));
        assert!(config_err, "should report ConfigError from list_skills");
        assert!(unknown_ref, "should still report UnknownReference(model) despite list_skills failure");
    }

    #[tokio::test]
    async fn fs_config_store_end_to_end_detects_unknown_model() {
        use std::io::Write;
        use tempfile::tempdir;
        use crate::config::Layers;
        use crate::fs_store::FsConfigStore;

        let tmp = tempdir().unwrap();
        // Write an agent file with an unknown model
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        let mut f = std::fs::File::create(&agents_dir.join("build.md")).unwrap();
        write!(f, "---\nmodel: does-not-exist\n---\n\nBuild agent.").unwrap();
        drop(f);

        let layers = Layers {
            global_dir: tempdir().unwrap().keep(), // empty temp dir as global
            workspace_dir: Some(tmp.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);
        let problems = check_config(&store).await;

        let model_err = problems.iter().find(|p| matches!(p, VnyError::UnknownReference(k, _) if *k == "model"));
        assert!(model_err.is_some(), "expected UnknownReference(model) for does-not-exist");
        match model_err.unwrap() {
            VnyError::UnknownReference(_, msg) => {
                assert!(msg.contains("does-not-exist"), "message should contain 'does-not-exist'");
                assert!(msg.contains("build"), "message should contain 'build'");
            }
            _ => panic!("Expected UnknownReference"),
        }
    }
}
