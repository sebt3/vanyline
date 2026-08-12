use crate::{error::AppError, AppState};
use vanyline_lib::k8s::VnlK8sClient;

/// Retourne le client K8s, découvert lazily à la première requête qui en a besoin
/// (jamais au démarrage). Caché dans `AppState::k8s`. `discover` ré-utilise le
/// namespace résolu (kubeconfig courant si `VNL_K8S_NAMESPACE` absent).
pub async fn client(state: &AppState) -> Result<VnlK8sClient, AppError> {
    let mut guard = state.k8s.lock().await;
    if let Some(c) = guard.as_ref() {
        return Ok(c.clone());
    }
    let client = VnlK8sClient::discover(state.config.k8s_namespace.clone())
        .await
        .map_err(|e| AppError::K8sConfigError(e.to_string()))?;
    let client = client;
    *guard = Some(client.clone());
    Ok(client)
}
