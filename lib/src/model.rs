use rig_core::client::{CompletionClient, Nothing};
use rig_core::providers::{ollama, openai};

use crate::domain::{ModelProfile, Provider};
use crate::error::VnyError;

/// Construit le CompletionModel ollama depuis les types v2. Même logique que
/// `crate::llm::build_ollama_model` (ancien, inchangé) mais sur `domain::Provider`
/// / `domain::ModelProfile` — `profile.model` remplace le `model_name: &str` séparé
/// (le nom du modèle brut chez le provider est désormais porté par le profil).
pub fn build_ollama_model(
    provider: &Provider,
    profile: &ModelProfile,
) -> Result<impl rig_core::completion::CompletionModel + 'static, VnyError> {
    let client = match provider.api_key.as_deref() {
        Some(key) => ollama::Client::builder()
            .api_key(key)
            .base_url(&provider.endpoint)
            .build(),
        None => ollama::Client::builder()
            .api_key(Nothing)
            .base_url(&provider.endpoint)
            .build(),
    }
    .map_err(|e| VnyError::ModelBuildError(format!("{e}")))?;
    Ok(client.completion_model(&profile.model))
}

/// Idem pour openai-compatible. Même construction que
/// `crate::llm::build_openai_compat_model` (ancien, inchangé) : base_url suffixé
/// `/v1`, api_key vide si absent.
pub fn build_openai_compat_model(
    provider: &Provider,
    profile: &ModelProfile,
) -> Result<impl rig_core::completion::CompletionModel + 'static, VnyError> {
    let api_key = provider.api_key.as_deref().unwrap_or("");
    let base_url = format!("{}/v1", provider.endpoint.trim_end_matches('/'));
    let client = openai::Client::builder()
        .api_key(api_key)
        .base_url(&base_url)
        .build()
        .map_err(|e| VnyError::ModelBuildError(format!("{e}")))?
        .completions_api();
    Ok(client.completion_model(&profile.model))
}

/// Paramètres à appliquer sur `rig_core::agent::AgentBuilder`
/// (`.temperature(..)`, `.max_tokens(..)`, `.additional_params(..)`), dérivés d'un
/// `ModelProfile`. Pure — aucune I/O, aucun réseau. C'est la tâche 6
/// (session-engine) qui les appliquera lors de la construction de l'`Agent<M>`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentParams {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    /// `None` si `profile.options` est vide ; sinon
    /// `Some(serde_json::Value::Object(profile.options.clone()))` — c'est la forme
    /// attendue par `AgentBuilder::additional_params(serde_json::Value)`.
    pub additional_params: Option<serde_json::Value>,
}

pub fn agent_params(profile: &ModelProfile) -> AgentParams {
    let additional_params = if profile.options.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(profile.options.clone()))
    };

    AgentParams {
        temperature: profile.temperature,
        max_tokens: profile.max_tokens,
        additional_params,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Provider, ProviderType};

    fn sample_provider(provider_type: ProviderType, endpoint: &str) -> Provider {
        Provider {
            name: "p".to_string(),
            provider_type,
            endpoint: endpoint.to_string(),
            api_key: None,
        }
    }

    fn sample_profile(options: serde_json::Map<String, serde_json::Value>) -> ModelProfile {
        ModelProfile {
            name: "m".to_string(),
            provider: "p".to_string(),
            model: "qwen2.5".to_string(),
            temperature: Some(0.7),
            max_tokens: Some(4096),
            options,
        }
    }

    #[test]
    fn build_ollama_model_succeeds() {
        let provider = sample_provider(ProviderType::Ollama, "http://localhost:11434");
        let profile = sample_profile(serde_json::Map::new());
        let result = build_ollama_model(&provider, &profile);
        assert!(result.is_ok());
    }

    #[test]
    fn build_ollama_model_with_api_key_succeeds() {
        let provider = Provider {
            name: "p".to_string(),
            provider_type: ProviderType::Ollama,
            endpoint: "http://localhost:11434".to_string(),
            api_key: Some("secret".to_string()),
        };
        let profile = sample_profile(serde_json::Map::new());
        let result = build_ollama_model(&provider, &profile);
        assert!(result.is_ok());
    }

    #[test]
    fn build_openai_compat_model_succeeds() {
        let provider = sample_provider(ProviderType::OpenaiCompatible, "http://localhost:8080");
        let profile = sample_profile(serde_json::Map::new());
        let result = build_openai_compat_model(&provider, &profile);
        assert!(result.is_ok());
    }

    #[test]
    fn agent_params_passthrough() {
        let profile = sample_profile(serde_json::Map::new());
        let params = agent_params(&profile);
        assert_eq!(params.temperature, Some(0.7));
        assert_eq!(params.max_tokens, Some(4096));
        assert_eq!(params.additional_params, None);
    }

    #[test]
    fn agent_params_with_options() {
        let options = serde_json::json!({"num_ctx": 65536, "top_k": 20})
            .as_object()
            .unwrap()
            .clone();
        let profile = sample_profile(options);
        let params = agent_params(&profile);
        assert_eq!(
            params.additional_params,
            Some(serde_json::json!({"num_ctx": 65536, "top_k": 20}))
        );
    }

    #[test]
    fn agent_params_minimal_profile() {
        let profile = ModelProfile {
            name: "m".to_string(),
            provider: "p".to_string(),
            model: "qwen2.5".to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::Map::new(),
        };
        let params = agent_params(&profile);
        assert_eq!(params.temperature, None);
        assert_eq!(params.max_tokens, None);
        assert_eq!(params.additional_params, None);
    }
}
