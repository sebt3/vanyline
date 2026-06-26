use crate::db::models::LlmProvider;
use crate::error::AppError;

pub fn build_ollama_model(
    provider: &LlmProvider,
    model_name: &str,
) -> Result<impl rig_core::completion::CompletionModel + 'static, AppError> {
    let lib_provider = to_lib_provider(provider);
    vanyline_lib::build_ollama_model(&lib_provider, model_name).map_err(Into::into)
}

pub fn build_openai_compat_model(
    provider: &LlmProvider,
    model_name: &str,
) -> Result<impl rig_core::completion::CompletionModel + 'static, AppError> {
    let lib_provider = to_lib_provider(provider);
    vanyline_lib::build_openai_compat_model(&lib_provider, model_name).map_err(Into::into)
}

fn to_lib_provider(p: &LlmProvider) -> vanyline_lib::LlmProvider {
    vanyline_lib::LlmProvider {
        id: p.id,
        name: p.name.clone(),
        provider_type: p.provider_type.clone(),
        endpoint: p.endpoint.clone(),
        api_key: p.api_key.clone(),
        default_model: p.default_model.clone(),
        available_models: p.available_models.clone(),
    }
}
