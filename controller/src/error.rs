use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("VNL-CTL-001: Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),
}