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

    fn reqwest_error() -> reqwest::Error {
        // Builder invalide (URL non parsable) : aucune I/O réseau, `send` échoue avant.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        rt.block_on(async { reqwest::Client::new().get("::pas une url").send().await })
            .expect_err("invalid URL yields a reqwest error")
    }

    /// Chaque variante d'`AppError` avec son rendu `Display` attendu et son statut HTTP
    /// attendu (barème d'`Exposes`, `SandboxRelayUnavailable` inclus : 502).
    fn fixtures() -> Vec<(AppError, String, StatusCode)> {
        let mut all = fixtures_app();
        all.extend(fixtures_workflow());
        all
    }

    fn fixtures_app() -> Vec<(AppError, String, StatusCode)> {
        let req = reqwest_error();
        let req_display = format!("VNL-REQ-001: Request error: {req}");
        vec![
            (
                AppError::Forbidden,
                "VNL-AUTH-004: Forbidden".into(),
                StatusCode::FORBIDDEN,
            ),
            (
                AppError::LlmError("boom".into()),
                "VNL-LLM-001: LLM provider error: boom".into(),
                StatusCode::BAD_GATEWAY,
            ),
            (
                AppError::LlmProviderNotFound,
                "VNL-LLM-002: LLM provider not found".into(),
                StatusCode::NOT_FOUND,
            ),
            (
                AppError::McpError("boom".into()),
                "VNL-MCP-001: MCP server error: boom".into(),
                StatusCode::BAD_GATEWAY,
            ),
            (
                AppError::AgentNotFound,
                "VNL-AGT-001: Agent not found".into(),
                StatusCode::NOT_FOUND,
            ),
            (
                AppError::UnprocessableReference(vanyline_lib::VnyError::UnknownReference(
                    "agent",
                    "ghost".into(),
                )),
                "VNL-CFG-003: Unknown agent reference: 'ghost'".into(),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                AppError::ConversationNotFound,
                "VNL-CNV-001: Conversation not found".into(),
                StatusCode::NOT_FOUND,
            ),
            (
                AppError::ConversationAccessDenied,
                "VNL-CNV-002: Access denied to conversation".into(),
                StatusCode::FORBIDDEN,
            ),
            (
                AppError::RequestError(req),
                req_display,
                StatusCode::BAD_GATEWAY,
            ),
            (
                AppError::InternalError("boom".into()),
                "VNL-INT-001: Internal error: boom".into(),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AppError::K8sConfigError("x".into()),
                "VNL-K8S-001: Kubernetes config error: x".into(),
                StatusCode::BAD_GATEWAY,
            ),
            (
                AppError::K8sApiError("x".into()),
                "VNL-K8S-002: Kubernetes API error: x".into(),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AppError::K8sNotFound("x".into()),
                "VNL-K8S-003: Kubernetes resource not found: x".into(),
                StatusCode::NOT_FOUND,
            ),
            (
                AppError::SandboxNotExposed,
                "VNL-SBX-001: sandbox has no public endpoint (owner has no application_ref)".into(),
                StatusCode::CONFLICT,
            ),
            (
                AppError::SandboxBranchEmpty,
                "VNL-SBX-002: branch must not be empty".into(),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                AppError::GitPathInvalid,
                "VNL-SBX-003: invalid git path segment".into(),
                StatusCode::BAD_REQUEST,
            ),
            (
                AppError::SandboxRelayUnavailable,
                "VNL-SBX-017: sandbox unavailable".into(),
                StatusCode::BAD_GATEWAY,
            ),
        ]
    }

    fn fixtures_workflow() -> Vec<(AppError, String, StatusCode)> {
        vec![
            (
                AppError::WorkflowNotFound,
                "VNL-WFA-002: workflow not found".into(),
                StatusCode::NOT_FOUND,
            ),
            (
                AppError::WorkflowNameTaken("x".into()),
                "VNL-WFA-003: workflow name already taken: x".into(),
                StatusCode::CONFLICT,
            ),
            (
                AppError::WorkflowDagInvalid("x".into()),
                "VNL-WFA-004: MRD-WORKFLOW-004: x".into(),
                StatusCode::BAD_REQUEST,
            ),
            (
                AppError::WorkflowKindForbidden("x".into()),
                "VNL-WFA-005: x".into(),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                AppError::WorkflowEngineNotConfigured,
                "VNL-WFA-006: workflow engine not configured".into(),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                AppError::WorkflowUnknownScope("x".into()),
                "VNL-WFA-007: unknown scope: x".into(),
                StatusCode::BAD_REQUEST,
            ),
            (
                AppError::WorkflowScopeNotEditable,
                "VNL-WFA-008: scope is not editable; create a new workflow instead".into(),
                StatusCode::BAD_REQUEST,
            ),
            (
                AppError::WorkflowSandboxNotFound,
                "VNL-WFA-011: sandbox not found".into(),
                StatusCode::NOT_FOUND,
            ),
            (
                AppError::WorkflowRunNotFound,
                "VNL-WFA-012: run not found".into(),
                StatusCode::NOT_FOUND,
            ),
            (
                AppError::WorkflowSandboxForeignProject("x".into()),
                "VNL-WFA-013: sandbox does not belong to the project: x".into(),
                StatusCode::CONFLICT,
            ),
            (
                AppError::WorkflowSandboxNotRunning("x".into()),
                "VNL-WFA-014: sandbox not running: x".into(),
                StatusCode::CONFLICT,
            ),
            (
                AppError::WorkflowSandboxRequired,
                "VNL-WFA-015: sandbox-scoped workflow requires a sandbox".into(),
                StatusCode::CONFLICT,
            ),
            (
                AppError::WorkflowSandboxNotAllowed,
                "VNL-WFA-016: sandbox is not allowed for a project-scoped workflow".into(),
                StatusCode::BAD_REQUEST,
            ),
            (
                AppError::WorkflowEngineUnavailable("x".into()),
                "VNL-WFA-017: workflow engine unavailable: x".into(),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        ]
    }

    #[test]
    fn display_exhaustif_des_variantes_apperror() {
        for (err, expected, _) in fixtures() {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn bareme_exhaustif_statuts_http_des_variantes() {
        for (err, display, expected) in fixtures() {
            let resp = err.into_response();
            assert_eq!(resp.status(), expected, "statut pour « {display} »");
        }
    }

    #[tokio::test]
    async fn corps_json_error_unique_et_content_type() {
        let resp = AppError::Forbidden.into_response();
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("body is JSON");
        assert_eq!(
            json,
            serde_json::json!({"error": "VNL-AUTH-004: Forbidden"})
        );
    }

    #[tokio::test]
    async fn unprocessable_reference_rend_le_marqueur_du_vny_interne() {
        let resp = AppError::UnprocessableReference(vanyline_lib::VnyError::UnknownReference(
            "agent",
            "ghost".into(),
        ))
        .into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("body is JSON");
        assert_eq!(
            json,
            serde_json::json!({"error": "VNL-CFG-003: Unknown agent reference: 'ghost'"})
        );
    }

    #[test]
    fn conversion_dberr_vers_internal_error_prefixe_vnl_db_006() {
        // Le Display réel de `DbErr::Custom` ajoute « Custom Error: » (écart avec le
        // Scenario, consigné) : l'attendu est construit depuis ce Display, pas en dur.
        let db = sea_orm::DbErr::Custom("x".into());
        let expected = format!("VNL-INT-001: Internal error: VNL-DB-006: {db}");
        let err = AppError::from(db);
        assert!(matches!(err, AppError::InternalError(_)));
        assert_eq!(err.to_string(), expected);
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn conversion_vnyerror_k8s_config_double_marqueur_et_502() {
        let err = AppError::from(vanyline_lib::VnyError::K8sConfigError("boom".into()));
        assert_eq!(
            err.to_string(),
            "VNL-K8S-001: Kubernetes config error: VNL-K8S-001: Cannot reach Kubernetes cluster: boom"
        );
        assert_eq!(err.into_response().status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn conversion_vnyerror_k8s_api_avec_signature_not_found_vers_404() {
        for payload in ["404", "NotFound", "not found"] {
            let err = AppError::from(vanyline_lib::VnyError::K8sApiError(payload.into()));
            assert!(matches!(err, AppError::K8sNotFound(_)), "payload {payload}");
            assert_eq!(
                err.to_string(),
                format!(
                    "VNL-K8S-003: Kubernetes resource not found: VNL-K8S-002: Kubernetes API error: {payload}"
                )
            );
            assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
        }
    }

    #[test]
    fn conversion_vnyerror_k8s_api_sans_signature_vers_500() {
        let err = AppError::from(vanyline_lib::VnyError::K8sApiError("boom".into()));
        assert!(matches!(err, AppError::K8sApiError(_)));
        assert_eq!(
            err.to_string(),
            "VNL-K8S-002: Kubernetes API error: VNL-K8S-002: Kubernetes API error: boom"
        );
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn heuristic_not_found_insensible_a_la_casse_et_a_faux_positifs() {
        let casse = AppError::from(vanyline_lib::VnyError::K8sApiError("Not Found".into()));
        assert_eq!(
            casse.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        let faux_positif = AppError::from(vanyline_lib::VnyError::K8sApiError(
            "port 4041 ouvert".into(),
        ));
        assert_eq!(faux_positif.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn conversion_vnyerror_autres_variantes_degradees_en_500() {
        let llm = AppError::from(vanyline_lib::VnyError::LlmError("x".into()));
        assert!(matches!(llm, AppError::InternalError(_)));
        assert_eq!(
            llm.to_string(),
            "VNL-INT-001: Internal error: VNL-LLM-001: LLM provider error: x"
        );
        let unknown = AppError::from(vanyline_lib::VnyError::UnknownReference(
            "agent",
            "ghost".into(),
        ));
        assert!(matches!(unknown, AppError::InternalError(_)));
        assert_eq!(
            unknown.to_string(),
            "VNL-INT-001: Internal error: VNL-CFG-003: Unknown agent reference: 'ghost'"
        );
        assert_eq!(
            llm.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            unknown.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn conversion_cfgstore_toutes_variantes_vers_500_display_complet() {
        let ro = AppError::from(vanyline_cfgstore::CfgStoreError::ReadOnly);
        assert_eq!(
            ro.to_string(),
            "VNL-INT-001: Internal error: VNL-CFG-009: Config store is read-only"
        );
        assert_eq!(
            ro.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let nf = AppError::from(vanyline_cfgstore::CfgStoreError::NotFound {
            kind: "provider",
            name: "p".into(),
            layer: vanyline_cfgstore::store::Layer::Workspace,
        });
        assert_eq!(
            nf.to_string(),
            "VNL-INT-001: Internal error: VNL-CFG-006: provider 'p' not found in Workspace layer"
        );
        assert_eq!(
            nf.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn conversion_reqwest_generee_source_std_error_et_502() {
        let err = AppError::RequestError(reqwest_error());
        assert!(err.to_string().starts_with("VNL-REQ-001: Request error: "));
        let source = std::error::Error::source(&err).expect("source is the reqwest error");
        assert!(source.downcast_ref::<reqwest::Error>().is_some());
        assert_eq!(err.into_response().status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn source_std_error_absente_partout_sauf_request_error() {
        assert!(std::error::Error::source(&AppError::Forbidden).is_none());
        assert!(std::error::Error::source(&AppError::InternalError("x".into())).is_none());
        let req = AppError::RequestError(reqwest_error());
        assert!(std::error::Error::source(&req).is_some());
    }

    #[test]
    fn marqueurs_propres_de_apperror_uniques_dans_le_fichier() {
        let source = include_str!("error.rs");
        let prod = source
            .split("#[cfg(test)]")
            .next()
            .expect("production part precedes tests");
        let expected: [(&str, usize); 31] = [
            ("VNL-AUTH-004", 1),
            ("VNL-LLM-001", 1),
            ("VNL-LLM-002", 1),
            ("VNL-MCP-001", 1),
            ("VNL-AGT-001", 1),
            ("VNL-CNV-001", 1),
            ("VNL-CNV-002", 1),
            ("VNL-REQ-001", 1),
            ("VNL-INT-001", 1),
            ("VNL-K8S-001", 1),
            ("VNL-K8S-002", 1),
            ("VNL-K8S-003", 1),
            ("VNL-SBX-001", 1),
            ("VNL-SBX-002", 1),
            ("VNL-SBX-003", 1),
            ("VNL-SBX-017", 1),
            ("VNL-WFA-002", 1),
            ("VNL-WFA-003", 1),
            ("VNL-WFA-004", 1),
            ("VNL-WFA-005", 1),
            ("VNL-WFA-006", 1),
            ("VNL-WFA-007", 1),
            ("VNL-WFA-008", 1),
            ("VNL-WFA-011", 1),
            ("VNL-WFA-012", 1),
            ("VNL-WFA-013", 1),
            ("VNL-WFA-014", 1),
            ("VNL-WFA-015", 1),
            ("VNL-WFA-016", 1),
            ("VNL-WFA-017", 1),
            ("VNL-DB-006", 2),
        ];
        for (marker, count) in expected {
            assert_eq!(
                prod.matches(marker).count(),
                count,
                "occurrences de {marker} hors tests"
            );
        }
        let templates: Vec<&str> = prod
            .lines()
            .filter(|l| l.trim_start().starts_with("#[error(\"VNL-"))
            .collect();
        let mut markers: Vec<&str> = templates
            .iter()
            .filter_map(|l| l.split("VNL-").nth(1)?.get(..)?.split(':').next())
            .collect();
        let total = markers.len();
        markers.sort_unstable();
        markers.dedup();
        assert_eq!(markers.len(), total, "deux variantes partagent un marqueur");
    }
}
