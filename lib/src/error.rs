use thiserror::Error;

#[derive(Debug, Error)]
pub enum VnyError {
    #[error("VNL-CFG-001: Configuration error: {0}")]
    ConfigError(String),
    #[error("VNL-LLM-001: LLM provider error: {0}")]
    LlmError(String),
    #[error("VNL-LLM-002: LLM provider not found")]
    LlmProviderNotFound,
    #[error("VNL-LLM-005: Unknown provider type: {0}")]
    UnknownProviderType(String),
    #[error("VNL-LLM-006: Cannot build model: {0}")]
    ModelBuildError(String),
    #[error("VNL-LLM-007: No LLM provider configured")]
    NoProviderConfigured,
    #[error("VNL-LLM-008: No model configured")]
    NoModelConfigured,
    #[error("VNL-MCP-001: MCP server error: {0}")]
    McpError(String),
    #[error("VNL-MCP-003: Unknown server type: {0}")]
    UnknownServerType(String),
    #[error("VNL-MCP-004: SSE transport not yet implemented")]
    SseNotImplemented,
    #[error("VNL-MCP-005: Cannot connect to {0}: {1}")]
    McpConnectError(String, String),
    #[error("VNL-MCP-006: Cannot list tools from {0}: {1}")]
    McpToolsError(String, String),
    #[error("VNL-AGT-001: Agent not found")]
    AgentNotFound,
    #[error("VNL-IO-001: I/O error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("VNL-JSON-001: JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("VNL-INT-001: Internal error: {0}")]
    InternalError(String),
    #[error("VNL-CFG-002: Duplicate name '{1}' for {0}")]
    DuplicateName(&'static str, String),
    #[error("VNL-CFG-003: Unknown {0} reference: '{1}'")]
    UnknownReference(&'static str, String),
    #[cfg(feature = "k8s")]
    #[error("VNL-K8S-001: Cannot reach Kubernetes cluster: {0}")]
    K8sConfigError(String),
    #[cfg(feature = "k8s")]
    #[error("VNL-K8S-002: Kubernetes API error: {0}")]
    K8sApiError(String),
}
