use std::collections::HashMap;

use async_trait::async_trait;

use crate::domain::{Agent, McpServer, ModelProfile, Provider, SkillMeta, Toolset};
use crate::error::VnyError;

/// Permet à `resolve_by_name` de lire le nom d'un item du domaine sans que
/// chaque appelant ne réécrive `.name.clone()`. Implémenté ici (pas dans
/// domain.rs) pour respecter la stratégie additive.
trait HasName {
    fn name(&self) -> &str;
}

impl HasName for Provider {
    fn name(&self) -> &str {
        &self.name
    }
}
impl HasName for ModelProfile {
    fn name(&self) -> &str {
        &self.name
    }
}
impl HasName for McpServer {
    fn name(&self) -> &str {
        &self.name
    }
}
impl HasName for Toolset {
    fn name(&self) -> &str {
        &self.name
    }
}
impl HasName for Agent {
    fn name(&self) -> &str {
        &self.name
    }
}
impl HasName for SkillMeta {
    fn name(&self) -> &str {
        &self.name
    }
}

/// Résout un item par nom dans une liste : 0 match -> `UnknownReference(kind, name)`,
/// >1 match -> `DuplicateName(kind, name)`, exactement 1 -> `Ok(item)`.
fn resolve_by_name<T: HasName>(
    items: Vec<T>,
    kind: &'static str,
    name: &str,
) -> Result<T, VnyError> {
    let mut iter = items.into_iter().filter(|it| it.name() == name);
    let first = iter.next();
    match first {
        None => Err(VnyError::UnknownReference(kind, name.to_string())),
        Some(item) => {
            if iter.next().is_some() {
                Err(VnyError::DuplicateName(kind, name.to_string()))
            } else {
                Ok(item)
            }
        }
    }
}

#[async_trait]
pub trait ConfigStore: Send + Sync {
    async fn list_providers(&self) -> Result<Vec<Provider>, VnyError>;
    async fn list_models(&self) -> Result<Vec<ModelProfile>, VnyError>;
    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, VnyError>;
    async fn list_toolsets(&self) -> Result<Vec<Toolset>, VnyError>;
    async fn list_agents(&self) -> Result<Vec<Agent>, VnyError>;
    /// Index léger : name + description uniquement (lazy-loading de la liste).
    async fn list_skills(&self) -> Result<Vec<SkillMeta>, VnyError>;
    /// Corps du skill, chargé à la demande uniquement. Erreur
    /// `UnknownReference("skill", name)` si absent.
    async fn load_skill(&self, name: &str) -> Result<String, VnyError>;
    async fn default_agent(&self) -> Result<Option<String>, VnyError>;

    /// Méthodes défaut — résolution par nom, filtrent `list_x()`.
    async fn get_provider(&self, name: &str) -> Result<Provider, VnyError> {
        resolve_by_name(self.list_providers().await?, "provider", name)
    }
    async fn get_model(&self, name: &str) -> Result<ModelProfile, VnyError> {
        resolve_by_name(self.list_models().await?, "model", name)
    }
    async fn get_mcp_server(&self, name: &str) -> Result<McpServer, VnyError> {
        resolve_by_name(self.list_mcp_servers().await?, "mcp_server", name)
    }
    async fn get_toolset(&self, name: &str) -> Result<Toolset, VnyError> {
        resolve_by_name(self.list_toolsets().await?, "toolset", name)
    }
    async fn get_agent(&self, name: &str) -> Result<Agent, VnyError> {
        resolve_by_name(self.list_agents().await?, "agent", name)
    }
    async fn get_skill(&self, name: &str) -> Result<SkillMeta, VnyError> {
        resolve_by_name(self.list_skills().await?, "skill", name)
    }
}

/// Store in-memory — fixture réutilisable pour les tests de cette tâche ET des
/// tâches suivantes (session-engine, builtin-skill, builtin-task). PAS gardé
/// derrière `#[cfg(test)]` : les tâches futures (fichiers de test différents,
/// commits différents) doivent pouvoir faire
/// `use vanyline_lib::store::InMemoryConfigStore;` (accès via le chemin qualifié
/// du module `pub mod store;`).
#[derive(Default)]
pub struct InMemoryConfigStore {
    pub providers: Vec<Provider>,
    pub models: Vec<ModelProfile>,
    pub mcp_servers: Vec<McpServer>,
    pub toolsets: Vec<Toolset>,
    pub agents: Vec<Agent>,
    pub skills: Vec<SkillMeta>,
    /// Corps des skills, séparé de `skills` (qui ne porte que name+description) —
    /// même distinction lazy-loading que dans `ConfigStore`.
    pub skill_bodies: HashMap<String, String>,
    pub default_agent: Option<String>,
}

impl InMemoryConfigStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ConfigStore for InMemoryConfigStore {
    async fn list_providers(&self) -> Result<Vec<Provider>, VnyError> {
        Ok(self.providers.clone())
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, VnyError> {
        Ok(self.models.clone())
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, VnyError> {
        Ok(self.mcp_servers.clone())
    }

    async fn list_toolsets(&self) -> Result<Vec<Toolset>, VnyError> {
        Ok(self.toolsets.clone())
    }

    async fn list_agents(&self) -> Result<Vec<Agent>, VnyError> {
        Ok(self.agents.clone())
    }

    async fn list_skills(&self) -> Result<Vec<SkillMeta>, VnyError> {
        Ok(self.skills.clone())
    }

    async fn load_skill(&self, name: &str) -> Result<String, VnyError> {
        self.skill_bodies
            .get(name)
            .cloned()
            .ok_or_else(|| VnyError::UnknownReference("skill", name.to_string()))
    }

    async fn default_agent(&self) -> Result<Option<String>, VnyError> {
        Ok(self.default_agent.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_store() -> InMemoryConfigStore {
        InMemoryConfigStore {
            providers: vec![Provider {
                name: "ollama-local".to_string(),
                provider_type: crate::domain::ProviderType::Ollama,
                endpoint: "http://localhost:11434".to_string(),
                api_key: None,
            }],
            models: vec![ModelProfile {
                name: "qwen2.5".to_string(),
                provider: "ollama".to_string(),
                model: "qwen2.5".to_string(),
                temperature: None,
                max_tokens: None,
                options: serde_json::Map::new(),
            }],
            mcp_servers: vec![McpServer {
                name: "fs".to_string(),
                transport: crate::domain::McpTransport::HttpStreamable,
                url: "http://mcp-fs:3000".to_string(),
                headers: Default::default(),
            }],
            toolsets: vec![Toolset {
                name: "default".to_string(),
                description: None,
                prompt: None,
                local_tools: vec![],
                mcp: vec![],
            }],
            agents: vec![Agent {
                name: "build".to_string(),
                description: Some("Build agent".to_string()),
                mode: crate::domain::AgentMode::Primary,
                model: "qwen2.5".to_string(),
                toolsets: vec!["default".to_string()],
                skills: crate::domain::SkillSelection::Auto,
                system_prompt: "You are a build assistant.".to_string(),
            }],
            skills: vec![SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing skill".to_string(),
            }],
            skill_bodies: {
                let mut m = HashMap::new();
                m.insert("pdf".to_string(), "# PDF skill\n...".to_string());
                m
            },
            default_agent: None,
        }
    }

    #[tokio::test]
    async fn get_provider_found() {
        let store = sample_store();
        let result = store.get_provider("ollama-local").await;
        assert!(result.is_ok());
        let provider = result.unwrap();
        assert_eq!(provider.name, "ollama-local");
    }

    #[tokio::test]
    async fn get_provider_unknown() {
        let store = InMemoryConfigStore::new();
        let result = store.get_provider("nope").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("VNL-CFG-003"));
        assert!(msg.contains("nope"));
    }

    #[tokio::test]
    async fn get_provider_duplicate() {
        let store = InMemoryConfigStore {
            providers: vec![
                Provider {
                    name: "dup".to_string(),
                    provider_type: crate::domain::ProviderType::Ollama,
                    endpoint: "http://localhost:11434".to_string(),
                    api_key: None,
                },
                Provider {
                    name: "dup".to_string(),
                    provider_type: crate::domain::ProviderType::OpenaiCompatible,
                    endpoint: "http://localhost:8080".to_string(),
                    api_key: None,
                },
            ],
            ..Default::default()
        };
        let result = store.get_provider("dup").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("VNL-CFG-002"));
    }

    #[tokio::test]
    async fn get_agent_found() {
        let store = sample_store();
        let result = store.get_agent("build").await;
        assert!(result.is_ok());
        let agent = result.unwrap();
        assert_eq!(agent.name, "build");
    }

    #[tokio::test]
    async fn get_agent_unknown() {
        let store = InMemoryConfigStore::new();
        let result = store.get_agent("nope").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("VNL-CFG-003"));
        assert!(msg.contains("nope"));
    }

    #[tokio::test]
    async fn get_agent_duplicate() {
        let store = InMemoryConfigStore {
            agents: vec![
                Agent {
                    name: "dup".to_string(),
                    description: None,
                    mode: crate::domain::AgentMode::Primary,
                    model: "qwen2.5".to_string(),
                    toolsets: vec![],
                    skills: crate::domain::SkillSelection::Auto,
                    system_prompt: "prompt".to_string(),
                },
                Agent {
                    name: "dup".to_string(),
                    description: None,
                    mode: crate::domain::AgentMode::Subagent,
                    model: "qwen2.5".to_string(),
                    toolsets: vec![],
                    skills: crate::domain::SkillSelection::Auto,
                    system_prompt: "prompt2".to_string(),
                },
            ],
            ..Default::default()
        };
        let result = store.get_agent("dup").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("VNL-CFG-002"));
    }

    #[tokio::test]
    async fn list_skills_is_lightweight() {
        let store = sample_store();
        let skills = store.list_skills().await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
        // La liste ne contient PAS le corps
        for skill in &skills {
            assert!(
                !skill.description.contains("PDF skill")
                    || skill.description == "PDF processing skill"
            );
        }
        // load_skill retourne le corps exact
        let body = store.load_skill("pdf").await.unwrap();
        assert!(body.contains("PDF skill"));
    }

    #[tokio::test]
    async fn load_skill_unknown() {
        let store = sample_store();
        let result = store.load_skill("absent").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("VNL-CFG-003"));
        assert!(msg.contains("absent"));
    }

    #[tokio::test]
    async fn default_agent_none() {
        let store = InMemoryConfigStore::new();
        let result = store.default_agent().await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn default_agent_some() {
        let store = InMemoryConfigStore {
            default_agent: Some("build".to_string()),
            ..Default::default()
        };
        let result = store.default_agent().await.unwrap();
        assert_eq!(result, Some("build".to_string()));
    }

    #[tokio::test]
    async fn get_x_helpers_cover_all_kinds() {
        let store = sample_store();
        // Vérifier que toutes les méthodes get_* retournent Ok avec la fixture peuplée
        assert!(store.get_provider("ollama-local").await.is_ok());
        assert!(store.get_model("qwen2.5").await.is_ok());
        assert!(store.get_mcp_server("fs").await.is_ok());
        assert!(store.get_toolset("default").await.is_ok());
        assert!(store.get_agent("build").await.is_ok());
        assert!(store.get_skill("pdf").await.is_ok());
    }
}
