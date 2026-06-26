use rig_core::client::{CompletionClient, Nothing};
use rig_core::providers::{ollama, openai};

use crate::error::VnyError;
use crate::types::LlmProvider;

pub fn build_ollama_model(
    provider: &LlmProvider,
    model_name: &str,
) -> Result<impl rig_core::completion::CompletionModel + 'static, VnyError> {
    let client = ollama::Client::builder()
        .api_key(Nothing)
        .base_url(&provider.endpoint)
        .build()
        .map_err(|e| VnyError::ModelBuildError(format!("{e}")))?;
    Ok(client.completion_model(model_name))
}

pub fn build_openai_compat_model(
    provider: &LlmProvider,
    model_name: &str,
) -> Result<impl rig_core::completion::CompletionModel + 'static, VnyError> {
    let api_key = provider.api_key.as_deref().unwrap_or("");
    let base_url = format!("{}/v1", provider.endpoint.trim_end_matches('/'));
    let client = openai::Client::builder()
        .api_key(api_key)
        .base_url(&base_url)
        .build()
        .map_err(|e| VnyError::ModelBuildError(format!("{e}")))?
        .completions_api();
    Ok(client.completion_model(model_name))
}
