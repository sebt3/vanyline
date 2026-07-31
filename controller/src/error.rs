use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("VNL-CTL-001: Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error("VNL-CTL-002: Owner '{owner}' not found or not Ready (no pvc_name in status) for Project '{project}'")]
    OwnerNotReady { owner: String, project: String },

    #[error("VNL-CTL-003: purge job for Project '{project}' not finished yet")]
    PurgePending { project: String },

    #[error("VNL-CTL-004: finalizer error: {0}")]
    Finalizer(String),

    #[error("VNL-CTL-005: Project '{project}' not cloned yet for Sandbox '{sandbox}'")]
    ProjectNotReady { project: String, sandbox: String },

    #[error("VNL-CTL-006: worktree removal job for Sandbox '{sandbox}' not finished yet")]
    WorktreeRemovalPending { sandbox: String },
}
