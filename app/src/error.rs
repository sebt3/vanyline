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
    #[error("VNL-CNV-001: Conversation not found")]
    ConversationNotFound,
    #[error("VNL-CNV-002: Access denied to conversation")]
    ConversationAccessDenied,
    #[error("VNL-REQ-001: Request error: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("VNL-INT-001: Internal error: {0}")]
    InternalError(String),
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
            AppError::ConversationNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::ConversationAccessDenied => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::RequestError(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            AppError::InternalError(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        let body = Json(json!({ "error": message }));
        (status, body).into_response()
    }
}
