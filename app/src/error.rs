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
}

impl From<vanyline_lib::VnyError> for AppError {
    fn from(e: vanyline_lib::VnyError) -> Self {
        AppError::InternalError(e.to_string())
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
            AppError::UnprocessableReference(_) => (StatusCode::UNPROCESSABLE_ENTITY, self.to_string()),
            AppError::ConversationNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::ConversationAccessDenied => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::RequestError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::InternalError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
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
}
