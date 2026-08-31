#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::domain::{
    Agent, McpServer, McpTransport, ModelProfile, Provider, ProviderType, SkillMeta, Toolset,
};
use crate::error::CfgStoreError;
use crate::fs_store::validate_name;

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
) -> Result<T, CfgStoreError> {
    let mut iter = items.into_iter().filter(|it| it.name() == name);
    let first = iter.next();
    match first {
        None => Err(CfgStoreError::UnknownReference(kind, name.to_string())),
        Some(item) => {
            if iter.next().is_some() {
                Err(CfgStoreError::DuplicateName(kind, name.to_string()))
            } else {
                Ok(item)
            }
        }
    }
}

/// Couche ciblée par une écriture. La résolution "workspace si dispo sinon
/// global" est faite par l'appelant (handler RPC) — le trait prend un Layer
/// explicite. InMemoryConfigStore ignore ce paramètre (jeu unique en mémoire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Global,
    Workspace,
}

#[async_trait]
pub trait ConfigStore: Send + Sync {
    // --- Read methods ---
    async fn list_providers(&self) -> Result<Vec<Provider>, CfgStoreError>;
    async fn list_models(&self) -> Result<Vec<ModelProfile>, CfgStoreError>;
    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, CfgStoreError>;
    async fn list_toolsets(&self) -> Result<Vec<Toolset>, CfgStoreError>;
    async fn list_agents(&self) -> Result<Vec<Agent>, CfgStoreError>;
    /// Index léger : name + description uniquement (lazy-loading de la liste).
    async fn list_skills(&self) -> Result<Vec<SkillMeta>, CfgStoreError>;
    /// Corps du skill, chargé à la demande uniquement. Erreur
    /// `UnknownReference("skill", name)` si absent.
    async fn load_skill(&self, name: &str) -> Result<String, CfgStoreError>;
    async fn default_agent(&self) -> Result<Option<String>, CfgStoreError>;

    /// Méthodes défaut — résolution par nom, filtrent `list_x()`.
    async fn get_provider(&self, name: &str) -> Result<Provider, CfgStoreError> {
        resolve_by_name(self.list_providers().await?, "provider", name)
    }
    async fn get_model(&self, name: &str) -> Result<ModelProfile, CfgStoreError> {
        resolve_by_name(self.list_models().await?, "model", name)
    }
    async fn get_mcp_server(&self, name: &str) -> Result<McpServer, CfgStoreError> {
        resolve_by_name(self.list_mcp_servers().await?, "mcp_server", name)
    }
    async fn get_toolset(&self, name: &str) -> Result<Toolset, CfgStoreError> {
        resolve_by_name(self.list_toolsets().await?, "toolset", name)
    }
    async fn get_agent(&self, name: &str) -> Result<Agent, CfgStoreError> {
        resolve_by_name(self.list_agents().await?, "agent", name)
    }
    async fn get_skill(&self, name: &str) -> Result<SkillMeta, CfgStoreError> {
        resolve_by_name(self.list_skills().await?, "skill", name)
    }

    // --- Write method signatures (défaut CfgStoreError::ReadOnly dans cette tâche) ---
    async fn create_provider(&self, _layer: Layer, _item: Provider) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn update_provider(
        &self,
        _layer: Layer,
        _name: &str,
        _patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn delete_provider(&self, _layer: Layer, _name: &str) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn create_model(&self, _layer: Layer, _item: ModelProfile) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn update_model(
        &self,
        _layer: Layer,
        _name: &str,
        _patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn delete_model(&self, _layer: Layer, _name: &str) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn create_mcp_server(
        &self,
        _layer: Layer,
        _item: McpServer,
    ) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn update_mcp_server(
        &self,
        _layer: Layer,
        _name: &str,
        _patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn delete_mcp_server(&self, _layer: Layer, _name: &str) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn create_toolset(&self, _layer: Layer, _item: Toolset) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn update_toolset(
        &self,
        _layer: Layer,
        _name: &str,
        _patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn delete_toolset(&self, _layer: Layer, _name: &str) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn create_agent(&self, _layer: Layer, _item: Agent) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn update_agent(
        &self,
        _layer: Layer,
        _name: &str,
        _patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn delete_agent(&self, _layer: Layer, _name: &str) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn create_skill(
        &self,
        _layer: Layer,
        _meta: SkillMeta,
        _body: String,
    ) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn update_skill(
        &self,
        _layer: Layer,
        _name: &str,
        _patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn delete_skill(&self, _layer: Layer, _name: &str) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
    }
    async fn set_default_agent(&self, _layer: Layer, _name: &str) -> Result<(), CfgStoreError> {
        Err(CfgStoreError::ReadOnly)
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
    pub providers: Mutex<Vec<Provider>>,
    pub models: Mutex<Vec<ModelProfile>>,
    pub mcp_servers: Mutex<Vec<McpServer>>,
    pub toolsets: Mutex<Vec<Toolset>>,
    pub agents: Mutex<Vec<Agent>>,
    pub skills: Mutex<Vec<SkillMeta>>,
    /// Corps des skills, séparé de `skills` (qui ne porte que name+description) —
    /// même distinction lazy-loading que dans `ConfigStore`.
    pub skill_bodies: Mutex<HashMap<String, String>>,
    pub default_agent: Mutex<Option<String>>,
}

impl InMemoryConfigStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ConfigStore for InMemoryConfigStore {
    async fn list_providers(&self) -> Result<Vec<Provider>, CfgStoreError> {
        Ok(self.providers.lock().unwrap().clone())
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, CfgStoreError> {
        Ok(self.models.lock().unwrap().clone())
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, CfgStoreError> {
        Ok(self.mcp_servers.lock().unwrap().clone())
    }

    async fn list_toolsets(&self) -> Result<Vec<Toolset>, CfgStoreError> {
        Ok(self.toolsets.lock().unwrap().clone())
    }

    async fn list_agents(&self) -> Result<Vec<Agent>, CfgStoreError> {
        Ok(self.agents.lock().unwrap().clone())
    }

    async fn list_skills(&self) -> Result<Vec<SkillMeta>, CfgStoreError> {
        Ok(self.skills.lock().unwrap().clone())
    }

    async fn load_skill(&self, name: &str) -> Result<String, CfgStoreError> {
        self.skill_bodies
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| CfgStoreError::UnknownReference("skill", name.to_string()))
    }

    async fn default_agent(&self) -> Result<Option<String>, CfgStoreError> {
        Ok(self.default_agent.lock().unwrap().clone())
    }

    // --- Write methods ---

    async fn create_provider(&self, _layer: Layer, item: Provider) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let mut providers = self.providers.lock().unwrap();
        if providers.iter().any(|p| p.name == item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "provider",
                name: item.name.clone(),
                layer: _layer,
            });
        }
        providers.push(item);
        Ok(())
    }

    async fn update_provider(
        &self,
        _layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut providers = self.providers.lock().unwrap();
        let provider = providers
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| CfgStoreError::NotFound {
                kind: "provider",
                name: name.to_string(),
                layer: _layer,
            })?;
        let Some(obj) = patch.as_object() else {
            return Err(CfgStoreError::Config(
                "update patch must be a JSON object".to_string(),
            ));
        };
        for (k, v) in obj {
            match k.as_str() {
                "type" => {
                    let t = v.as_str().ok_or_else(|| {
                        CfgStoreError::Validation("provider: 'type' must be a string".to_string())
                    })?;
                    provider.provider_type = match t {
                        "ollama" => ProviderType::Ollama,
                        "openai-compatible" => ProviderType::OpenaiCompatible,
                        other => {
                            return Err(CfgStoreError::Validation(format!(
                                "provider: unknown provider_type '{other}'"
                            )));
                        }
                    };
                }
                "endpoint" => {
                    provider.endpoint = v
                        .as_str()
                        .ok_or_else(|| {
                            CfgStoreError::Validation(
                                "provider: 'endpoint' must be a string".to_string(),
                            )
                        })?
                        .to_string();
                }
                "api_key" => {
                    provider.api_key = if v.is_null() {
                        None
                    } else {
                        Some(
                            v.as_str()
                                .ok_or_else(|| {
                                    CfgStoreError::Validation(
                                        "provider: 'api_key' must be a string".to_string(),
                                    )
                                })?
                                .to_string(),
                        )
                    };
                }
                _ => {} // clés inconnues ignorées
            }
        }
        Ok(())
    }

    async fn delete_provider(&self, _layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut providers = self.providers.lock().unwrap();
        let idx = providers
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| CfgStoreError::NotFound {
                kind: "provider",
                name: name.to_string(),
                layer: _layer,
            })?;
        providers.remove(idx);
        Ok(())
    }

    async fn create_model(&self, _layer: Layer, item: ModelProfile) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let mut models = self.models.lock().unwrap();
        if models.iter().any(|m| m.name == item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "model",
                name: item.name.clone(),
                layer: _layer,
            });
        }
        models.push(item);
        Ok(())
    }

    async fn update_model(
        &self,
        _layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut models = self.models.lock().unwrap();
        let model =
            models
                .iter_mut()
                .find(|m| m.name == name)
                .ok_or_else(|| CfgStoreError::NotFound {
                    kind: "model",
                    name: name.to_string(),
                    layer: _layer,
                })?;
        let Some(obj) = patch.as_object() else {
            return Err(CfgStoreError::Config(
                "update patch must be a JSON object".to_string(),
            ));
        };
        for (k, v) in obj {
            match k.as_str() {
                "provider" => {
                    model.provider = v
                        .as_str()
                        .ok_or_else(|| {
                            CfgStoreError::Validation(
                                "model: 'provider' must be a string".to_string(),
                            )
                        })?
                        .to_string();
                }
                "model" => {
                    model.model = v
                        .as_str()
                        .ok_or_else(|| {
                            CfgStoreError::Validation("model: 'model' must be a string".to_string())
                        })?
                        .to_string();
                }
                "temperature" => {
                    model.temperature = if v.is_null() {
                        None
                    } else {
                        match v.as_f64() {
                            Some(n) => Some(n),
                            None => {
                                return Err(CfgStoreError::Validation(
                                    "model: 'temperature' must be a number".to_string(),
                                ));
                            }
                        }
                    };
                }
                "max_tokens" => {
                    model.max_tokens = if v.is_null() {
                        None
                    } else {
                        match v.as_u64() {
                            Some(n) => Some(n),
                            None => {
                                return Err(CfgStoreError::Validation(
                                    "model: 'max_tokens' must be an integer".to_string(),
                                ));
                            }
                        }
                    };
                }
                "options" => {
                    if v.is_null() {
                        model.options.clear();
                    } else {
                        match v.as_object() {
                            Some(o) => {
                                model.options = o.clone();
                            }
                            None => {
                                return Err(CfgStoreError::Validation(
                                    "model: 'options' must be an object".to_string(),
                                ));
                            }
                        }
                    }
                }
                _ => {} // clés inconnues ignorées
            }
        }
        Ok(())
    }

    async fn delete_model(&self, _layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut models = self.models.lock().unwrap();
        let idx =
            models
                .iter()
                .position(|m| m.name == name)
                .ok_or_else(|| CfgStoreError::NotFound {
                    kind: "model",
                    name: name.to_string(),
                    layer: _layer,
                })?;
        models.remove(idx);
        Ok(())
    }

    async fn create_mcp_server(&self, _layer: Layer, item: McpServer) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let mut servers = self.mcp_servers.lock().unwrap();
        if servers.iter().any(|s| s.name == item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "mcp_server",
                name: item.name.clone(),
                layer: _layer,
            });
        }
        servers.push(item);
        Ok(())
    }

    async fn update_mcp_server(
        &self,
        _layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut servers = self.mcp_servers.lock().unwrap();
        let server =
            servers
                .iter_mut()
                .find(|s| s.name == name)
                .ok_or_else(|| CfgStoreError::NotFound {
                    kind: "mcp_server",
                    name: name.to_string(),
                    layer: _layer,
                })?;
        let Some(obj) = patch.as_object() else {
            return Err(CfgStoreError::Config(
                "update patch must be a JSON object".to_string(),
            ));
        };
        for (k, v) in obj {
            match k.as_str() {
                "type" => {
                    let t = v.as_str().ok_or_else(|| {
                        CfgStoreError::Validation("mcp_server: 'type' must be a string".to_string())
                    })?;
                    server.transport = match t {
                        "http-streamable" => McpTransport::HttpStreamable,
                        "sse" => McpTransport::Sse,
                        other => {
                            return Err(CfgStoreError::Validation(format!(
                                "mcp_server: unknown transport '{other}'"
                            )));
                        }
                    };
                }
                "url" => {
                    server.url = v
                        .as_str()
                        .ok_or_else(|| {
                            CfgStoreError::Validation(
                                "mcp_server: 'url' must be a string".to_string(),
                            )
                        })?
                        .to_string();
                }
                "headers" => {
                    if v.is_null() {
                        server.headers.clear();
                    } else {
                        match v.as_object() {
                            Some(o) => {
                                server.headers.clear();
                                for (hk, hv) in o {
                                    if let Some(hv_str) = hv.as_str() {
                                        server.headers.insert(hk.clone(), hv_str.to_string());
                                    }
                                    // valeurs non-string ignorées
                                }
                            }
                            None => {
                                return Err(CfgStoreError::Validation(
                                    "mcp_server: 'headers' must be an object".to_string(),
                                ));
                            }
                        }
                    }
                }
                _ => {} // clés inconnues ignorées
            }
        }
        Ok(())
    }

    async fn delete_mcp_server(&self, _layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut servers = self.mcp_servers.lock().unwrap();
        let idx =
            servers
                .iter()
                .position(|s| s.name == name)
                .ok_or_else(|| CfgStoreError::NotFound {
                    kind: "mcp_server",
                    name: name.to_string(),
                    layer: _layer,
                })?;
        servers.remove(idx);
        Ok(())
    }

    async fn set_default_agent(&self, _layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        *self.default_agent.lock().unwrap() = Some(name.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn sample_store() -> InMemoryConfigStore {
        InMemoryConfigStore {
            providers: Mutex::new(vec![Provider {
                name: "ollama-local".to_string(),
                provider_type: crate::domain::ProviderType::Ollama,
                endpoint: "http://localhost:11434".to_string(),
                api_key: None,
            }]),
            models: Mutex::new(vec![ModelProfile {
                name: "qwen2.5".to_string(),
                provider: "ollama".to_string(),
                model: "qwen2.5".to_string(),
                temperature: None,
                max_tokens: None,
                options: serde_json::Map::new(),
            }]),
            mcp_servers: Mutex::new(vec![McpServer {
                name: "fs".to_string(),
                transport: crate::domain::McpTransport::HttpStreamable,
                url: "http://mcp-fs:3000".to_string(),
                headers: Default::default(),
            }]),
            toolsets: Mutex::new(vec![Toolset {
                name: "default".to_string(),
                description: None,
                prompt: None,
                local_tools: vec![],
                mcp: vec![],
            }]),
            agents: Mutex::new(vec![Agent {
                name: "build".to_string(),
                description: Some("Build agent".to_string()),
                mode: crate::domain::AgentMode::Primary,
                model: "qwen2.5".to_string(),
                toolsets: vec!["default".to_string()],
                skills: crate::domain::SkillSelection::Auto,
                system_prompt: "You are a build assistant.".to_string(),
            }]),
            skills: Mutex::new(vec![SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing skill".to_string(),
            }]),
            skill_bodies: Mutex::new({
                let mut m = HashMap::new();
                m.insert("pdf".to_string(), "# PDF skill\n...".to_string());
                m
            }),
            default_agent: Mutex::new(None),
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
            providers: Mutex::new(vec![
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
            ]),
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
            agents: Mutex::new(vec![
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
            ]),
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
            default_agent: Mutex::new(Some("build".to_string())),
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
