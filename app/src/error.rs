use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
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
}

impl From<vanyline_lib::VnyError> for AppError {
    fn from(e: vanyline_lib::VnyError) -> Self {
        match &e {
            vanyline_lib::VnyError::K8sConfigError(_) => AppError::K8sConfigError(e.to_string()),
            vanyline_lib::VnyError::K8sApiError(s)
                if s.contains("404")
                    || s.contains("NotFound")
                    || s.contains("not found") =>
            {
                AppError::K8sNotFound(e.to_string())
            }
            vanyline_lib::VnyError::K8sApiError(_) => AppError::K8sApiError(e.to_string()),
            _ => AppError::InternalError(e.to_string()),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotAuthenticated => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::InvalidToken => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::OidcError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::ConfigError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            AppError::DatabaseError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            AppError::LlmError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::LlmProviderNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::McpError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::AgentNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::ModelProfileNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::ToolsetNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::SkillNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::UnprocessableReference(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
            }
            AppError::ConversationNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::ConversationAccessDenied => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::RequestError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::InternalError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            AppError::K8sConfigError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::K8sApiError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            AppError::K8sNotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
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
}
