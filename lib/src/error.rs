use thiserror::Error;

/// Erreur de `vanyline-lib`, à identifiants stables `VNL-<DOMAINE>-<NNN>`
/// (cf. `./error.sdd` — convention « message d'erreur avec identifiant unique »
/// de l'AGENTS.md). Chaque variante affiche son marqueur en tête de `Display`,
/// suivi de `": "`. Exposée à la racine du crate (`vanyline_lib::VnyError`) ;
/// type purement interne Rust, jamais un type de transport. Les variantes sont
/// gated par la feature `k8s`, en miroir du module [`crate::k8s`] : sans la
/// feature, l'enum n'a aucune variante.
#[derive(Debug, Error)]
pub enum VnyError {
    /// VNL-K8S-001 : cluster Kubernetes injoignable (message de la cause).
    #[cfg(feature = "k8s")]
    #[error("VNL-K8S-001: Cannot reach Kubernetes cluster: {0}")]
    K8sConfigError(String),
    /// VNL-K8S-002 : erreur de l'API Kubernetes (message de la cause).
    #[cfg(feature = "k8s")]
    #[error("VNL-K8S-002: Kubernetes API error: {0}")]
    K8sApiError(String),
    /// VNL-K8S-003 : ressource Kubernetes introuvable (l'apiserver a répondu 404).
    #[cfg(feature = "k8s")]
    #[error("VNL-K8S-003: Kubernetes resource not found: {0}")]
    K8sNotFound(String),
}

#[cfg(all(test, feature = "k8s"))]
mod tests {
    use super::*;

    /// Scenario `rendu des trois marqueurs` (`./error.sdd`).
    #[test]
    fn rendu_des_trois_marqueurs() {
        assert_eq!(
            VnyError::K8sConfigError("boom".into()).to_string(),
            "VNL-K8S-001: Cannot reach Kubernetes cluster: boom"
        );
        assert_eq!(
            VnyError::K8sApiError("boom".into()).to_string(),
            "VNL-K8S-002: Kubernetes API error: boom"
        );
        assert_eq!(
            VnyError::K8sNotFound("boom".into()).to_string(),
            "VNL-K8S-003: Kubernetes resource not found: boom"
        );
    }
}
