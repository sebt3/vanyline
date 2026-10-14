use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("VNL-AUTH-004: Forbidden")]
    Forbidden,
    #[error("VNL-LLM-001: LLM provider error: {0}")]
    LlmError(String),
    #[error("VNL-LLM-002: LLM provider not found")]
    LlmProviderNotFound,
    #[error("VNL-MCP-001: MCP server error: {0}")]
    McpError(String),
    #[error("VNL-AGT-001: Agent not found")]
    AgentNotFound,
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
    /// Panne de RELAIS app→sandbox (transport, timeout, statut de la frappe du ticket) —
    /// `./api/sandboxes.sdd` §Must « Erreurs du RELAIS opaques ». SANS champ embarqué : le
    /// `Display` de `reqwest::Error` porte l'URL interne de la sandbox, qui ne sort JAMAIS
    /// de l'app ; la panne complète est consignée en `tracing::warn!` par l'appelant.
    /// Marqueur pris en `017+` : la plage `VNL-SBX-004..016` appartient aux erreurs git de
    /// la sandbox (`sandbox/src/git.sdd`) — `./error.sdd`.
    #[error("VNL-SBX-017: sandbox unavailable")]
    SandboxRelayUnavailable,
    /// Workflow inexistant OU appartenant à un autre utilisateur (404, aucune distinction —
    /// `./api/workflows.sdd`).
    #[error("VNL-WFA-002: workflow not found")]
    WorkflowNotFound,
    /// Nom de workflow déjà pris dans le projet (409) — payload = nom rendu par l'API.
    #[error("VNL-WFA-003: workflow name already taken: {0}")]
    WorkflowNameTaken(String),
    /// DAG rejeté par `validate_dag` de miryad-core (400) — le marqueur `MRD-WORKFLOW-004`
    /// est volontairement imbriqué dans le corps.
    #[error("VNL-WFA-004: MRD-WORKFLOW-004: {0}")]
    WorkflowDagInvalid(String),
    /// Kind de step non autorisé pour la portée (422) — le payload nomme le step et le kind.
    #[error("VNL-WFA-005: {0}")]
    WorkflowKindForbidden(String),
    /// Moteur de workflow non configuré (503) — garde commune des routes workflows.
    #[error("VNL-WFA-006: workflow engine not configured")]
    WorkflowEngineNotConfigured,
    /// Portée textuelle inconnue (400).
    #[error("VNL-WFA-007: unknown scope: {0}")]
    WorkflowUnknownScope(String),
    /// La portée d'un workflow existant n'est pas modifiable (400).
    #[error("VNL-WFA-008: scope is not editable; create a new workflow instead")]
    WorkflowScopeNotEditable,
    /// Sandbox référencée par un lancement, absente OU appartenant à un autre utilisateur
    /// (404, aucune distinction — `./api/workflows.sdd`).
    #[error("VNL-WFA-011: sandbox not found")]
    WorkflowSandboxNotFound,
    /// Run absent OU appartenant à un autre utilisateur (404, aucune distinction —
    /// `./api/workflows.sdd`).
    #[error("VNL-WFA-012: run not found")]
    WorkflowRunNotFound,
    /// Sandbox hors du projet visé au lancement (409) — payload = nom de la sandbox.
    #[error("VNL-WFA-013: sandbox does not belong to the project: {0}")]
    WorkflowSandboxForeignProject(String),
    /// Sandbox non démarrée, phase ≠ `Running` (409) — payload = « sandbox : phase ».
    #[error("VNL-WFA-014: sandbox not running: {0}")]
    WorkflowSandboxNotRunning(String),
    /// Lancement d'un workflow de portée sandbox sans `sandbox` fournie (409).
    #[error("VNL-WFA-015: sandbox-scoped workflow requires a sandbox")]
    WorkflowSandboxRequired,
    /// `sandbox` fournie pour un workflow de portée projet (400).
    #[error("VNL-WFA-016: sandbox is not allowed for a project-scoped workflow")]
    WorkflowSandboxNotAllowed,
    /// Moteur configuré mais Restate injoignable ou en refus (503, `Raises` de
    /// `./api/workflows.sdd`) — payload = l'erreur miryad-core (`MRD-WORKFLOW-*`).
    #[error("VNL-WFA-017: workflow engine unavailable: {0}")]
    WorkflowEngineUnavailable(String),
}

/// Remplace le `fn db_err` dupliqué verbatim dans 9 modules (`api/{conversations,llm_providers,
/// local_tools,mcp_servers,me,owners,projects,sandboxes}.rs`, `ws/chat.rs`) — trouvé en revue
/// Phase 3 (miryad-core-integration). Même code qu'avant (`VNL-DB-006`, tout `DbErr` mappé en
/// `InternalError`) : la consolidation ne change aucun comportement observable, juste l'endroit
/// où il vit. `#[from]` permet `?` directement sur un `Result<_, sea_orm::DbErr>`.
impl From<sea_orm::DbErr> for AppError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::InternalError(format!("VNL-DB-006: {e}"))
    }
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

impl From<vanyline_cfgstore::CfgStoreError> for AppError {
    fn from(e: vanyline_cfgstore::CfgStoreError) -> Self {
        Self::InternalError(e.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::Forbidden | Self::ConversationAccessDenied => {
                (StatusCode::FORBIDDEN, self.to_string())
            }
            Self::LlmError(_)
            | Self::McpError(_)
            | Self::RequestError(_)
            | Self::K8sConfigError(_)
            | Self::SandboxRelayUnavailable => (StatusCode::BAD_GATEWAY, self.to_string()),
            Self::LlmProviderNotFound
            | Self::AgentNotFound
            | Self::ConversationNotFound
            | Self::K8sNotFound(_)
            | Self::WorkflowSandboxNotFound
            | Self::WorkflowRunNotFound
            | Self::WorkflowNotFound => (StatusCode::NOT_FOUND, self.to_string()),
            Self::UnprocessableReference(_)
            | Self::SandboxBranchEmpty
            | Self::WorkflowKindForbidden(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
            }
            Self::InternalError(_) | Self::K8sApiError(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
            Self::SandboxNotExposed
            | Self::WorkflowNameTaken(_)
            | Self::WorkflowSandboxForeignProject(_)
            | Self::WorkflowSandboxNotRunning(_)
            | Self::WorkflowSandboxRequired => (StatusCode::CONFLICT, self.to_string()),
            Self::GitPathInvalid
            | Self::WorkflowDagInvalid(_)
            | Self::WorkflowUnknownScope(_)
            | Self::WorkflowSandboxNotAllowed
            | Self::WorkflowScopeNotEditable => (StatusCode::BAD_REQUEST, self.to_string()),
            Self::WorkflowEngineNotConfigured | Self::WorkflowEngineUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
            }
        };

        let body = Json(json!({ "error": message }));
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
