use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::domain::{
    Agent, AgentMode, McpSelection, McpServer, McpTransport, ModelProfile, Provider, ProviderType,
    SkillMeta, SkillSelection, Toolset,
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
        Ok(self
            .providers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    async fn list_models(&self) -> Result<Vec<ModelProfile>, CfgStoreError> {
        Ok(self
            .models
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, CfgStoreError> {
        Ok(self
            .mcp_servers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    async fn list_toolsets(&self) -> Result<Vec<Toolset>, CfgStoreError> {
        Ok(self
            .toolsets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    async fn list_agents(&self) -> Result<Vec<Agent>, CfgStoreError> {
        Ok(self
            .agents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    async fn list_skills(&self) -> Result<Vec<SkillMeta>, CfgStoreError> {
        Ok(self
            .skills
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    async fn load_skill(&self, name: &str) -> Result<String, CfgStoreError> {
        self.skill_bodies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
            .ok_or_else(|| CfgStoreError::UnknownReference("skill", name.to_string()))
    }

    async fn default_agent(&self) -> Result<Option<String>, CfgStoreError> {
        Ok(self
            .default_agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    // --- Write methods ---

    async fn create_provider(&self, _layer: Layer, item: Provider) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let mut providers = self.providers.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut providers = self.providers.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut providers = self.providers.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut models = self.models.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut models = self.models.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut models = self.models.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut servers = self.mcp_servers.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut servers = self.mcp_servers.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut servers = self.mcp_servers.lock().unwrap_or_else(|e| e.into_inner());
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
        *self.default_agent.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.to_string());
        Ok(())
    }

    async fn create_toolset(&self, _layer: Layer, item: Toolset) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let mut toolsets = self.toolsets.lock().unwrap_or_else(|e| e.into_inner());
        if toolsets.iter().any(|t| t.name == item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "toolset",
                name: item.name.clone(),
                layer: _layer,
            });
        }
        toolsets.push(item);
        Ok(())
    }

    async fn update_toolset(
        &self,
        _layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut toolsets = self.toolsets.lock().unwrap_or_else(|e| e.into_inner());
        let toolset = toolsets
            .iter_mut()
            .find(|t| t.name == name)
            .ok_or_else(|| CfgStoreError::NotFound {
                kind: "toolset",
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
                "description" => {
                    toolset.description = if v.is_null() {
                        None
                    } else {
                        Some(
                            v.as_str()
                                .ok_or_else(|| {
                                    CfgStoreError::Validation(
                                        "toolset: 'description' must be a string".to_string(),
                                    )
                                })?
                                .to_string(),
                        )
                    };
                }
                "prompt" => {
                    toolset.prompt = if v.is_null() {
                        None
                    } else {
                        Some(
                            v.as_str()
                                .ok_or_else(|| {
                                    CfgStoreError::Validation(
                                        "toolset: 'prompt' must be a string".to_string(),
                                    )
                                })?
                                .to_string(),
                        )
                    };
                }
                "local_tools" => {
                    toolset.local_tools = if v.is_null() {
                        Vec::new()
                    } else {
                        let arr = v.as_array().ok_or_else(|| {
                            CfgStoreError::Validation(
                                "toolset: 'local_tools' must be an array".to_string(),
                            )
                        })?;
                        arr.iter()
                            .map(|x| {
                                x.as_str().map(String::from).ok_or_else(|| {
                                    CfgStoreError::Validation(
                                        "toolset: 'local_tools' entries must be strings"
                                            .to_string(),
                                    )
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?
                    };
                }
                "mcp" => {
                    toolset.mcp = if v.is_null() {
                        Vec::new()
                    } else {
                        let arr = v.as_array().ok_or_else(|| {
                            CfgStoreError::Validation("toolset: 'mcp' must be an array".to_string())
                        })?;
                        let mut out = Vec::new();
                        for item in arr {
                            let server =
                                item.get("server").and_then(|s| s.as_str()).ok_or_else(|| {
                                    CfgStoreError::Validation(
                                        "toolset: 'mcp' entry must have a string 'server'"
                                            .to_string(),
                                    )
                                })?;
                            let tools = match item.get("tools") {
                                None | Some(serde_json::Value::Null) => Vec::new(),
                                Some(t) => {
                                    let arr = t.as_array().ok_or_else(|| {
                                        CfgStoreError::Validation(
                                            "toolset: 'mcp' entry 'tools' must be an array"
                                                .to_string(),
                                        )
                                    })?;
                                    arr.iter()
                                        .map(|x| {
                                            x.as_str().map(String::from).ok_or_else(|| {
                                                CfgStoreError::Validation(
                                                    "toolset: 'mcp' entry tools must be strings"
                                                        .to_string(),
                                                )
                                            })
                                        })
                                        .collect::<Result<Vec<_>, _>>()?
                                }
                            };
                            out.push(McpSelection {
                                server: server.to_string(),
                                tools,
                            });
                        }
                        out
                    };
                }
                _ => {} // clés inconnues ignorées
            }
        }
        Ok(())
    }

    async fn delete_toolset(&self, _layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut toolsets = self.toolsets.lock().unwrap_or_else(|e| e.into_inner());
        let idx = toolsets
            .iter()
            .position(|t| t.name == name)
            .ok_or_else(|| CfgStoreError::NotFound {
                kind: "toolset",
                name: name.to_string(),
                layer: _layer,
            })?;
        toolsets.remove(idx);
        Ok(())
    }

    async fn create_agent(&self, _layer: Layer, item: Agent) -> Result<(), CfgStoreError> {
        validate_name(&item.name)?;
        let mut agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        if agents.iter().any(|a| a.name == item.name) {
            return Err(CfgStoreError::NameConflict {
                kind: "agent",
                name: item.name.clone(),
                layer: _layer,
            });
        }
        agents.push(item);
        Ok(())
    }

    async fn update_agent(
        &self,
        _layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        let agent =
            agents
                .iter_mut()
                .find(|a| a.name == name)
                .ok_or_else(|| CfgStoreError::NotFound {
                    kind: "agent",
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
                "description" => {
                    agent.description = if v.is_null() {
                        None
                    } else {
                        Some(
                            v.as_str()
                                .ok_or_else(|| {
                                    CfgStoreError::Validation(
                                        "agent: 'description' must be a string".to_string(),
                                    )
                                })?
                                .to_string(),
                        )
                    };
                }
                "mode" => {
                    let m = v.as_str().ok_or_else(|| {
                        CfgStoreError::Validation("agent: 'mode' must be a string".to_string())
                    })?;
                    agent.mode = match m {
                        "primary" => AgentMode::Primary,
                        "subagent" => AgentMode::Subagent,
                        "all" => AgentMode::All,
                        other => {
                            return Err(CfgStoreError::Validation(format!(
                                "agent: unknown mode '{other}'"
                            )));
                        }
                    };
                }
                "model" => {
                    agent.model = v
                        .as_str()
                        .ok_or_else(|| {
                            CfgStoreError::Validation("agent: 'model' must be a string".to_string())
                        })?
                        .to_string();
                }
                "toolsets" => {
                    agent.toolsets = if v.is_null() {
                        Vec::new()
                    } else {
                        let arr = v.as_array().ok_or_else(|| {
                            CfgStoreError::Validation(
                                "agent: 'toolsets' must be an array".to_string(),
                            )
                        })?;
                        arr.iter()
                            .map(|x| {
                                x.as_str().map(String::from).ok_or_else(|| {
                                    CfgStoreError::Validation(
                                        "agent: 'toolsets' entries must be strings".to_string(),
                                    )
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?
                    };
                }
                "skills" => {
                    agent.skills = if v.is_null() {
                        SkillSelection::Auto
                    } else {
                        match v {
                            serde_json::Value::String(s) if s == "auto" => SkillSelection::Auto,
                            serde_json::Value::String(s) if s == "none" => SkillSelection::None,
                            serde_json::Value::Array(arr) => SkillSelection::Named(
                                arr.iter()
                                    .map(|x| {
                                        x.as_str().map(String::from).ok_or_else(|| {
                                            CfgStoreError::Validation(
                                                "agent: 'skills' entries must be strings"
                                                    .to_string(),
                                            )
                                        })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?,
                            ),
                            _ => {
                                return Err(CfgStoreError::Validation(
                                    "agent: 'skills' must be 'auto', 'none' or an array"
                                        .to_string(),
                                ));
                            }
                        }
                    };
                }
                "system_prompt" => {
                    agent.system_prompt = v
                        .as_str()
                        .ok_or_else(|| {
                            CfgStoreError::Validation(
                                "agent: 'system_prompt' must be a string".to_string(),
                            )
                        })?
                        .to_string();
                }
                _ => {} // clés inconnues ignorées
            }
        }
        Ok(())
    }

    async fn delete_agent(&self, _layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        let idx =
            agents
                .iter()
                .position(|a| a.name == name)
                .ok_or_else(|| CfgStoreError::NotFound {
                    kind: "agent",
                    name: name.to_string(),
                    layer: _layer,
                })?;
        agents.remove(idx);
        Ok(())
    }

    async fn create_skill(
        &self,
        _layer: Layer,
        meta: SkillMeta,
        body: String,
    ) -> Result<(), CfgStoreError> {
        validate_name(&meta.name)?;
        {
            let skills = self.skills.lock().unwrap_or_else(|e| e.into_inner());
            if skills.iter().any(|s| s.name == meta.name) {
                return Err(CfgStoreError::NameConflict {
                    kind: "skill",
                    name: meta.name.clone(),
                    layer: _layer,
                });
            }
        }
        let name = meta.name.clone();
        self.skills
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(meta);
        self.skill_bodies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name, body);
        Ok(())
    }

    async fn update_skill(
        &self,
        _layer: Layer,
        name: &str,
        patch: serde_json::Value,
    ) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let Some(obj) = patch.as_object() else {
            return Err(CfgStoreError::Config(
                "update patch must be a JSON object".to_string(),
            ));
        };
        let mut skills = self.skills.lock().unwrap_or_else(|e| e.into_inner());
        let meta =
            skills
                .iter_mut()
                .find(|s| s.name == name)
                .ok_or_else(|| CfgStoreError::NotFound {
                    kind: "skill",
                    name: name.to_string(),
                    layer: _layer,
                })?;
        for (k, v) in obj {
            match k.as_str() {
                "description" => {
                    meta.description = v
                        .as_str()
                        .ok_or_else(|| {
                            CfgStoreError::Validation(
                                "skill: 'description' must be a string".to_string(),
                            )
                        })?
                        .to_string();
                }
                "body" => {
                    let body = v
                        .as_str()
                        .ok_or_else(|| {
                            CfgStoreError::Validation("skill: 'body' must be a string".to_string())
                        })?
                        .to_string();
                    self.skill_bodies
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(name.to_string(), body);
                }
                _ => {} // clés inconnues ignorées
            }
        }
        Ok(())
    }

    async fn delete_skill(&self, _layer: Layer, name: &str) -> Result<(), CfgStoreError> {
        validate_name(name)?;
        let mut skills = self.skills.lock().unwrap_or_else(|e| e.into_inner());
        let idx =
            skills
                .iter()
                .position(|s| s.name == name)
                .ok_or_else(|| CfgStoreError::NotFound {
                    kind: "skill",
                    name: name.to_string(),
                    layer: _layer,
                })?;
        skills.remove(idx);
        self.skill_bodies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
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

    // ==========================================================================
    // Toolsets — create / update / delete
    // ==========================================================================

    #[tokio::test]
    async fn create_toolset_global() {
        let store = InMemoryConfigStore::new();
        store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "ts-cli".to_string(),
                    description: Some("CLI toolset".to_string()),
                    prompt: Some("Use CLI tools.".to_string()),
                    local_tools: vec!["bash".to_string()],
                    mcp: vec![],
                },
            )
            .await
            .unwrap();
        let list = store.list_toolsets().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "ts-cli");
        assert_eq!(list[0].description, Some("CLI toolset".to_string()));
        assert_eq!(list[0].prompt, Some("Use CLI tools.".to_string()));
        assert_eq!(list[0].local_tools, vec!["bash".to_string()]);

        // get_toolset returns the right fields
        let got = store.get_toolset("ts-cli").await.unwrap();
        assert_eq!(got.name, "ts-cli");
        assert_eq!(got.local_tools, vec!["bash".to_string()]);
    }

    #[tokio::test]
    async fn update_toolset_partial_patch() {
        let store = sample_store();
        // Patch only description — prompt and others preserved
        store
            .update_toolset(
                Layer::Global,
                "default",
                serde_json::json!({"description": "Updated description"}),
            )
            .await
            .unwrap();
        let ts = store.get_toolset("default").await.unwrap();
        assert_eq!(ts.description, Some("Updated description".to_string()));
        assert_eq!(ts.prompt, None); // was None, now still None (not touched)
        assert_eq!(ts.local_tools, Vec::<String>::new());
    }

    #[tokio::test]
    async fn update_toolset_null_clears_optional() {
        let store = InMemoryConfigStore::new();
        store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "ts".to_string(),
                    description: Some("desc".to_string()),
                    prompt: Some("prompt".to_string()),
                    local_tools: vec![],
                    mcp: vec![],
                },
            )
            .await
            .unwrap();
        // null on description → None
        store
            .update_toolset(
                Layer::Global,
                "ts",
                serde_json::json!({"description": null}),
            )
            .await
            .unwrap();
        let ts = store.get_toolset("ts").await.unwrap();
        assert!(ts.description.is_none());
        assert_eq!(ts.prompt, Some("prompt".to_string())); // untouched
    }

    #[tokio::test]
    async fn update_toolset_null_clears_list() {
        let store = sample_store();
        store
            .update_toolset(
                Layer::Global,
                "default",
                serde_json::json!({"local_tools": null}),
            )
            .await
            .unwrap();
        let ts = store.get_toolset("default").await.unwrap();
        assert!(ts.local_tools.is_empty());
    }

    #[tokio::test]
    async fn update_toolset_mcp() {
        let store = InMemoryConfigStore::new();
        store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "ts".to_string(),
                    description: None,
                    prompt: None,
                    local_tools: vec![],
                    mcp: vec![],
                },
            )
            .await
            .unwrap();
        store
            .update_toolset(
                Layer::Global,
                "ts",
                serde_json::json!({"mcp": [
                    {"server": "fs", "tools": ["read", "write"]},
                    {"server": "git"}
                ]}),
            )
            .await
            .unwrap();
        let ts = store.list_toolsets().await.unwrap()[0].clone();
        assert_eq!(ts.mcp.len(), 2);
        assert_eq!(ts.mcp[0].server, "fs");
        assert_eq!(
            ts.mcp[0].tools,
            vec!["read".to_string(), "write".to_string()]
        );
        assert_eq!(ts.mcp[1].server, "git");
        assert!(ts.mcp[1].tools.is_empty());
    }

    #[tokio::test]
    async fn update_toolset_mcp_invalid_form() {
        let store = sample_store();
        let result = store
            .update_toolset(
                Layer::Global,
                "default",
                serde_json::json!({"mcp": "not-an-array"}),
            )
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CfgStoreError::Validation(_)));
    }

    #[tokio::test]
    async fn delete_toolset_by_name() {
        let store = sample_store();
        store
            .delete_toolset(Layer::Global, "default")
            .await
            .unwrap();
        let list = store.list_toolsets().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn create_toolset_name_conflict() {
        let store = sample_store();
        let result = store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "default".to_string(),
                    description: None,
                    prompt: None,
                    local_tools: vec![],
                    mcp: vec![],
                },
            )
            .await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NameConflict {
                kind: "toolset",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn update_toolset_not_found() {
        let store = InMemoryConfigStore::new();
        let result = store
            .update_toolset(Layer::Global, "nope", serde_json::json!({}))
            .await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn delete_toolset_not_found() {
        let store = InMemoryConfigStore::new();
        let result = store.delete_toolset(Layer::Global, "nope").await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn create_toolset_invalid_name() {
        let store = InMemoryConfigStore::new();
        let result = store
            .create_toolset(
                Layer::Global,
                Toolset {
                    name: "".to_string(),
                    description: None,
                    prompt: None,
                    local_tools: vec![],
                    mcp: vec![],
                },
            )
            .await;
        assert!(matches!(result.unwrap_err(), CfgStoreError::InvalidName(_)));
    }

    // ==========================================================================
    // Agents — create / update / delete
    // ==========================================================================

    #[tokio::test]
    async fn create_agent_global() {
        let store = InMemoryConfigStore::new();
        store
            .create_agent(
                Layer::Global,
                Agent {
                    name: "dev".to_string(),
                    description: Some("Dev agent".to_string()),
                    mode: AgentMode::Subagent,
                    model: "qwen2.5".to_string(),
                    toolsets: vec!["default".to_string()],
                    skills: SkillSelection::Named(vec!["pdf".to_string()]),
                    system_prompt: "You are a dev assistant.".to_string(),
                },
            )
            .await
            .unwrap();
        let list = store.list_agents().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "dev");
        assert_eq!(list[0].mode, AgentMode::Subagent);
        assert_eq!(list[0].model, "qwen2.5");
        assert_eq!(
            list[0].skills,
            SkillSelection::Named(vec!["pdf".to_string()])
        );

        let got = store.get_agent("dev").await.unwrap();
        assert_eq!(got.name, "dev");
        assert_eq!(got.system_prompt, "You are a dev assistant.");
    }

    #[tokio::test]
    async fn update_agent_partial_patch() {
        let store = sample_store();
        // Patch only model — mode, skills preserved
        store
            .update_agent(
                Layer::Global,
                "build",
                serde_json::json!({"model": "qwen3.5"}),
            )
            .await
            .unwrap();
        let a = store.get_agent("build").await.unwrap();
        assert_eq!(a.mode, AgentMode::Primary);
        assert_eq!(a.skills, SkillSelection::Auto);
        assert_eq!(a.model, "qwen3.5");
    }

    #[tokio::test]
    async fn update_agent_null_clears_optionals_and_list() {
        let store = sample_store();
        // Null on description → None, null on toolsets → empty
        store
            .update_agent(
                Layer::Global,
                "build",
                serde_json::json!({"description": null, "toolsets": null}),
            )
            .await
            .unwrap();
        let a = store.get_agent("build").await.unwrap();
        assert!(a.description.is_none());
        // description was Some → stays None after null, toolsets was non-empty→ empty
        assert!(a.toolsets.is_empty());
    }

    #[tokio::test]
    async fn update_agent_mode() {
        let store = sample_store();
        store
            .update_agent(Layer::Global, "build", serde_json::json!({"mode": "all"}))
            .await
            .unwrap();
        assert_eq!(store.get_agent("build").await.unwrap().mode, AgentMode::All);
    }

    #[tokio::test]
    async fn update_agent_skills_variants() {
        let store = sample_store();
        // null → Auto
        store
            .update_agent(Layer::Global, "build", serde_json::json!({"skills": null}))
            .await
            .unwrap();
        assert_eq!(
            store.get_agent("build").await.unwrap().skills,
            SkillSelection::Auto
        );
        // "auto" → Auto
        store
            .update_agent(
                Layer::Global,
                "build",
                serde_json::json!({"skills": "auto"}),
            )
            .await
            .unwrap();
        assert_eq!(
            store.get_agent("build").await.unwrap().skills,
            SkillSelection::Auto
        );
        // "none" → None
        store
            .update_agent(
                Layer::Global,
                "build",
                serde_json::json!({"skills": "none"}),
            )
            .await
            .unwrap();
        assert_eq!(
            store.get_agent("build").await.unwrap().skills,
            SkillSelection::None
        );
        // array → Named
        store
            .update_agent(
                Layer::Global,
                "build",
                serde_json::json!({"skills": ["pdf", "render"]}),
            )
            .await
            .unwrap();
        assert_eq!(
            store.get_agent("build").await.unwrap().skills,
            SkillSelection::Named(vec!["pdf".to_string(), "render".to_string()])
        );
    }

    #[tokio::test]
    async fn update_agent_invalid_mode() {
        let store = sample_store();
        let result = store
            .update_agent(
                Layer::Global,
                "build",
                serde_json::json!({"mode": "unknown"}),
            )
            .await;
        assert!(matches!(result.unwrap_err(), CfgStoreError::Validation(_)));
    }

    #[tokio::test]
    async fn create_agent_name_conflict() {
        let store = sample_store();
        let result = store
            .create_agent(
                Layer::Global,
                Agent {
                    name: "build".to_string(),
                    description: None,
                    mode: AgentMode::Primary,
                    model: "qwen2.5".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::Auto,
                    system_prompt: "prompt".to_string(),
                },
            )
            .await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NameConflict { kind: "agent", .. }
        ));
    }

    #[tokio::test]
    async fn update_agent_not_found() {
        let store = InMemoryConfigStore::new();
        let result = store
            .update_agent(Layer::Global, "nope", serde_json::json!({}))
            .await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn delete_agent_by_name() {
        let store = sample_store();
        store.delete_agent(Layer::Global, "build").await.unwrap();
        let list = store.list_agents().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn delete_agent_not_found() {
        let store = InMemoryConfigStore::new();
        let result = store.delete_agent(Layer::Global, "nope").await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn create_agent_invalid_name() {
        let store = InMemoryConfigStore::new();
        let result = store
            .create_agent(
                Layer::Global,
                Agent {
                    name: "..invalid".to_string(),
                    description: None,
                    mode: AgentMode::Primary,
                    model: "m".to_string(),
                    toolsets: vec![],
                    skills: SkillSelection::Auto,
                    system_prompt: "p".to_string(),
                },
            )
            .await;
        assert!(matches!(result.unwrap_err(), CfgStoreError::InvalidName(_)));
    }

    // ==========================================================================
    // Skills — create / update / delete (meta + body)
    // ==========================================================================

    #[tokio::test]
    async fn create_skill() {
        let store = InMemoryConfigStore::new();
        store
            .create_skill(
                Layer::Global,
                SkillMeta {
                    name: "render".to_string(),
                    description: "Render skill".to_string(),
                },
                "---\n# Render body\n".to_string(),
            )
            .await
            .unwrap();
        // list_skills contains it
        let list = store.list_skills().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "render");
        assert_eq!(list[0].description, "Render skill");
        // load_skill returns the body
        let body = store.load_skill("render").await.unwrap();
        assert!(body.contains("Render body"));
    }

    #[tokio::test]
    async fn update_skill_description_and_body() {
        let store = sample_store();
        // Update description
        store
            .update_skill(
                Layer::Global,
                "pdf",
                serde_json::json!({"description": "Updated PDF skill"}),
            )
            .await
            .unwrap();
        let meta = store.get_skill("pdf").await.unwrap();
        assert_eq!(meta.description, "Updated PDF skill");

        // Update body
        store
            .update_skill(
                Layer::Global,
                "pdf",
                serde_json::json!({"body": "NEW BODY CONTENT"}),
            )
            .await
            .unwrap();
        let body = store.load_skill("pdf").await.unwrap();
        assert_eq!(body, "NEW BODY CONTENT");
    }

    #[tokio::test]
    async fn delete_skill_removes_from_both() {
        let store = sample_store();
        assert_eq!(store.list_skills().await.unwrap().len(), 1);
        assert!(store.load_skill("pdf").await.is_ok());

        store.delete_skill(Layer::Global, "pdf").await.unwrap();

        assert!(store.list_skills().await.unwrap().is_empty());
        assert!(store.load_skill("pdf").await.is_err());
    }

    #[tokio::test]
    async fn create_skill_name_conflict() {
        let store = sample_store();
        let result = store
            .create_skill(
                Layer::Global,
                SkillMeta {
                    name: "pdf".to_string(),
                    description: "dup".to_string(),
                },
                "---\n---\n".to_string(),
            )
            .await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NameConflict { kind: "skill", .. }
        ));
    }

    #[tokio::test]
    async fn update_skill_not_found() {
        let store = InMemoryConfigStore::new();
        let result = store
            .update_skill(Layer::Global, "nope", serde_json::json!({}))
            .await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn delete_skill_not_found() {
        let store = InMemoryConfigStore::new();
        let result = store.delete_skill(Layer::Global, "nope").await;
        assert!(matches!(
            result.unwrap_err(),
            CfgStoreError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn create_skill_invalid_name() {
        let store = InMemoryConfigStore::new();
        let result = store
            .create_skill(
                Layer::Global,
                SkillMeta {
                    name: "".to_string(),
                    description: "d".to_string(),
                },
                "---\n---\n".to_string(),
            )
            .await;
        assert!(matches!(result.unwrap_err(), CfgStoreError::InvalidName(_)));
    }
}
