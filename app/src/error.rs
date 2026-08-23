use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("VNL-AUTH-001: Not authenticated")]
    NotAuthenticated,
    #[error("VNL-AUTH-002: Invalid or expired token")]
    InvalidToken,
    #[error("VNL-AUTH-003: OIDC error: {0}")]
    OidcError(String),
    #[error("VNL-AUTH-004: Forbidden")]
    Forbidden,
    #[error("VNL-CFG-001: Configuration error: {0}")]
    ConfigError(String),
    #[error("VNL-DB-001: Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("VNL-LLM-001: LLM provider error: {0}")]
    LlmError(String),
    #[error("VNL-LLM-002: LLM provider not found")]
    LlmProviderNotFound,
    #[error("VNL-MCP-001: MCP server error: {0}")]
    McpError(String),
    #[error("VNL-AGT-001: Agent not found")]
    AgentNotFound,
    #[error("VNL-CFG-004: Model profile not found")]
    ModelProfileNotFound,
    #[error("VNL-CFG-005: Toolset not found")]
    ToolsetNotFound,
    #[error("VNL-CFG-006: Skill not found")]
    SkillNotFound,
    #[error("{0}")]
    UnprocessableReference(vanyline_lib::VnyError),
    #[error("VNL-CNV-001: Conversation not found")]
    ConversationNotFound,
    #[error("VNL-CNV-002: Access denied to conversation")]
    ConversationAccessDenied,
    #[error("VNL-REQ-001: Request error: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("VNL-INT-001: Internal error: {0}")]
    InternalError(String),
    #[error("VNL-K8S-001: Kubernetes config error: {0}")]
    K8sConfigError(String),
    #[error("VNL-K8S-002: Kubernetes API error: {0}")]
    K8sApiError(String),
    #[error("VNL-K8S-003: Kubernetes resource not found: {0}")]
    K8sNotFound(String),
    #[error("VNL-SBX-001: sandbox has no public endpoint (owner has no application_ref)")]
    SandboxNotExposed,
    #[error("VNL-SBX-002: branch must not be empty")]
    SandboxBranchEmpty,
    #[error("VNL-SBX-003: invalid git path segment")]
    GitPathInvalid,
}

impl From<vanyline_lib::VnyError> for AppError {
    fn from(e: vanyline_lib::VnyError) -> Self {
        match &e {
            vanyline_lib::VnyError::K8sConfigError(_) => Self::K8sConfigError(e.to_string()),
            vanyline_lib::VnyError::K8sApiError(s)
                if s.contains("404") || s.contains("NotFound") || s.contains("not found") =>
            {
                Self::K8sNotFound(e.to_string())
            }
            vanyline_lib::VnyError::K8sApiError(_) => Self::K8sApiError(e.to_string()),
            _ => Self::InternalError(e.to_string()),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::NotAuthenticated => (StatusCode::UNAUTHORIZED, self.to_string()),
            Self::InvalidToken => (StatusCode::UNAUTHORIZED, self.to_string()),
            Self::OidcError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Self::Forbidden => (StatusCode::FORBIDDEN, self.to_string()),
            Self::ConfigError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            Self::DatabaseError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            Self::LlmError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Self::LlmProviderNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            Self::McpError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Self::AgentNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            Self::ModelProfileNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            Self::ToolsetNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            Self::SkillNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            Self::UnprocessableReference(_) => (StatusCode::UNPROCESSABLE_ENTITY, self.to_string()),
            Self::ConversationNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            Self::ConversationAccessDenied => (StatusCode::FORBIDDEN, self.to_string()),
            Self::RequestError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Self::InternalError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            Self::K8sConfigError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Self::K8sApiError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            Self::K8sNotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            Self::SandboxNotExposed => (StatusCode::CONFLICT, self.to_string()),
            Self::SandboxBranchEmpty => (StatusCode::UNPROCESSABLE_ENTITY, self.to_string()),
            Self::GitPathInvalid => (StatusCode::BAD_REQUEST, self.to_string()),
        };

        let body = Json(json!({ "error": message }));
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_profile_not_found_maps_to_404() {
        let resp = AppError::ModelProfileNotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn toolset_not_found_maps_to_404() {
        let resp = AppError::ToolsetNotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn unprocessable_reference_maps_to_422() {
        let err = vanyline_lib::VnyError::UnknownReference("provider", "ghost".to_string());
        let resp = AppError::UnprocessableReference(err).into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn k8s_config_error_maps_to_502() {
        let resp = AppError::K8sConfigError("x".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn k8s_api_error_maps_to_500() {
        let resp = AppError::K8sApiError("x".into()).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn k8s_not_found_maps_to_404() {
        let resp = AppError::K8sNotFound("x".into()).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn sandbox_branch_empty_maps_to_422() {
        let resp = AppError::SandboxBranchEmpty.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
