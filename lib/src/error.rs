use thiserror::Error;

/// Erreur unique de `vanyline-lib`, à identifiants stables `VNL-<DOMAINE>-<NNN>`
/// (cf. `./error.sdd` — convention « message d'erreur avec identifiant unique »
/// de l'AGENTS.md). Chaque variante affiche son marqueur en tête de `Display`,
/// suivi de `": "`. Exposée à la racine du crate (`vanyline_lib::VnyError`) ;
/// type purement interne Rust, jamais un type de transport — les codes diffusés
/// au wire passent par [`crate::event::ChatEvent::Error`].
#[derive(Debug, Error)]
pub enum VnyError {
    /// VNL-CFG-001 : erreur de configuration (parsing, forme inattendue).
    #[error("VNL-CFG-001: Configuration error: {0}")]
    ConfigError(String),
    /// VNL-LLM-001 : erreur du provider LLM (message de la cause).
    #[error("VNL-LLM-001: LLM provider error: {0}")]
    LlmError(String),
    /// VNL-LLM-002 : provider LLM introuvable.
    #[error("VNL-LLM-002: LLM provider not found")]
    LlmProviderNotFound,
    /// VNL-LLM-005 : type de provider inconnu (champ 0 = type rencontré).
    #[error("VNL-LLM-005: Unknown provider type: {0}")]
    UnknownProviderType(String),
    /// VNL-LLM-006 : échec de construction du modèle (champ 0 = message de la
    /// cause, ex. affichage de l'erreur rig).
    #[error("VNL-LLM-006: Cannot build model: {0}")]
    ModelBuildError(String),
    /// VNL-LLM-007 : aucun provider LLM configuré.
    #[error("VNL-LLM-007: No LLM provider configured")]
    NoProviderConfigured,
    /// VNL-LLM-008 : aucun modèle configuré.
    #[error("VNL-LLM-008: No model configured")]
    NoModelConfigured,
    /// VNL-MCP-001 : erreur du serveur MCP (message de la cause).
    #[error("VNL-MCP-001: MCP server error: {0}")]
    McpError(String),
    /// VNL-MCP-003 : type de serveur MCP inconnu (champ 0 = type rencontré).
    #[error("VNL-MCP-003: Unknown server type: {0}")]
    UnknownServerType(String),
    /// VNL-MCP-004 : transport SSE accepté en configuration mais jamais
    /// connectable ici (cf. `./prefixed_mcp.sdd`).
    #[error("VNL-MCP-004: SSE transport not yet implemented")]
    SseNotImplemented,
    /// VNL-MCP-005 : connexion à un serveur MCP impossible (champ 0 = nom du
    /// serveur, champ 1 = message de la cause).
    #[error("VNL-MCP-005: Cannot connect to {0}: {1}")]
    McpConnectError(String, String),
    /// VNL-MCP-006 : listing des tools d'un serveur MCP impossible (mêmes rôles
    /// que [`VnyError::McpConnectError`]).
    #[error("VNL-MCP-006: Cannot list tools from {0}: {1}")]
    McpToolsError(String, String),
    /// VNL-AGT-001 : agent introuvable.
    #[error("VNL-AGT-001: Agent not found")]
    AgentNotFound,
    /// VNL-IO-001 : erreur d'E/S sous-jacente (`#[from]` → `source()` rend
    /// l'erreur d'origine).
    #[error("VNL-IO-001: I/O error: {0}")]
    IoError(#[from] std::io::Error),
    /// VNL-JSON-001 : erreur serde JSON sous-jacente (`#[from]`, `source()`
    /// idem).
    #[error("VNL-JSON-001: JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    /// VNL-INT-001 : erreur interne (message libre).
    #[error("VNL-INT-001: Internal error: {0}")]
    InternalError(String),
    /// VNL-CFG-002 : deux entrées portent le même nom (champ 0 = sorte,
    /// champ 1 = nom — le `Display` rend le nom AVANT la sorte, asymétrie
    /// verrouillée par `./error.sdd`).
    #[error("VNL-CFG-002: Duplicate name '{1}' for {0}")]
    DuplicateName(&'static str, String),
    /// VNL-CFG-003 : référence vers un nom inexistant (champ 0 = sorte,
    /// champ 1 = nom, rendus dans l'ordre naturel).
    #[error("VNL-CFG-003: Unknown {0} reference: '{1}'")]
    UnknownReference(&'static str, String),
    /// VNL-K8S-001 : cluster Kubernetes injoignable (message de la cause) ;
    /// gated par la feature `k8s`, en miroir du module [`crate::k8s`].
    #[cfg(feature = "k8s")]
    #[error("VNL-K8S-001: Cannot reach Kubernetes cluster: {0}")]
    K8sConfigError(String),
    /// VNL-K8S-002 : erreur de l'API Kubernetes (message de la cause) ; gated
    /// par la feature `k8s`.
    #[cfg(feature = "k8s")]
    #[error("VNL-K8S-002: Kubernetes API error: {0}")]
    K8sApiError(String),
}

/// Conversion depuis l'erreur de la couche config : les trois variantes aux
/// gabarits partagés (`Config`, `DuplicateName`, `UnknownReference`) sont
/// mappées à l'identique (rendu `Display` byte-identique) ; toute autre
/// variante (connue ou future) est dégradée en [`VnyError::ConfigError`] avec
/// l'affichage de la source comme message — double marqueur assumé, cf.
/// `./error.sdd` (`Handles`).
impl From<vanyline_cfgstore::CfgStoreError> for VnyError {
    fn from(e: vanyline_cfgstore::CfgStoreError) -> Self {
        match e {
            vanyline_cfgstore::CfgStoreError::Config(msg) => VnyError::ConfigError(msg),
            vanyline_cfgstore::CfgStoreError::DuplicateName(kind, name) => {
                VnyError::DuplicateName(kind, name)
            }
            vanyline_cfgstore::CfgStoreError::UnknownReference(kind, name) => {
                VnyError::UnknownReference(kind, name)
            }
            other => VnyError::ConfigError(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    /// Scenario `display_exhaustif_des_marqueurs_vnl`.
    #[test]
    fn display_exhaustif_des_marqueurs_vnl() {
        let json_err = serde_json::from_str::<u8>("x").unwrap_err();
        let json_expected = format!("VNL-JSON-001: JSON error: {json_err}");
        let cases: Vec<(VnyError, String)> = vec![
            (
                VnyError::ConfigError("boom".into()),
                "VNL-CFG-001: Configuration error: boom".into(),
            ),
            (
                VnyError::LlmError("boom".into()),
                "VNL-LLM-001: LLM provider error: boom".into(),
            ),
            (
                VnyError::LlmProviderNotFound,
                "VNL-LLM-002: LLM provider not found".into(),
            ),
            (
                VnyError::UnknownProviderType("boom".into()),
                "VNL-LLM-005: Unknown provider type: boom".into(),
            ),
            (
                VnyError::ModelBuildError("boom".into()),
                "VNL-LLM-006: Cannot build model: boom".into(),
            ),
            (
                VnyError::NoProviderConfigured,
                "VNL-LLM-007: No LLM provider configured".into(),
            ),
            (
                VnyError::NoModelConfigured,
                "VNL-LLM-008: No model configured".into(),
            ),
            (
                VnyError::McpError("boom".into()),
                "VNL-MCP-001: MCP server error: boom".into(),
            ),
            (
                VnyError::UnknownServerType("boom".into()),
                "VNL-MCP-003: Unknown server type: boom".into(),
            ),
            (
                VnyError::SseNotImplemented,
                "VNL-MCP-004: SSE transport not yet implemented".into(),
            ),
            (
                VnyError::McpConnectError("s1".into(), "refusé".into()),
                "VNL-MCP-005: Cannot connect to s1: refusé".into(),
            ),
            (
                VnyError::McpToolsError("s1".into(), "refusé".into()),
                "VNL-MCP-006: Cannot list tools from s1: refusé".into(),
            ),
            (
                VnyError::AgentNotFound,
                "VNL-AGT-001: Agent not found".into(),
            ),
            (
                VnyError::IoError(std::io::Error::other("boom")),
                "VNL-IO-001: I/O error: boom".into(),
            ),
            (VnyError::JsonError(json_err), json_expected),
            (
                VnyError::InternalError("boom".into()),
                "VNL-INT-001: Internal error: boom".into(),
            ),
            (
                VnyError::DuplicateName("agent", "dup".into()),
                "VNL-CFG-002: Duplicate name 'dup' for agent".into(),
            ),
            (
                VnyError::UnknownReference("model", "qwen".into()),
                "VNL-CFG-003: Unknown model reference: 'qwen'".into(),
            ),
        ];
        assert_eq!(cases.len(), 18, "18 variantes sans la feature k8s");
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
        #[cfg(feature = "k8s")]
        {
            assert_eq!(
                VnyError::K8sConfigError("boom".into()).to_string(),
                "VNL-K8S-001: Cannot reach Kubernetes cluster: boom"
            );
            assert_eq!(
                VnyError::K8sApiError("boom".into()).to_string(),
                "VNL-K8S-002: Kubernetes API error: boom"
            );
        }
    }

    /// Scenario `conversion_cfgstore_preserve_rendu_sur_trois_variantes`.
    #[test]
    fn conversion_cfgstore_preserve_rendu_sur_trois_variantes() {
        use vanyline_cfgstore::CfgStoreError as C;
        let pairs = [
            (
                C::Config("boom".into()),
                VnyError::ConfigError("boom".into()),
            ),
            (
                C::DuplicateName("agent", "dup".into()),
                VnyError::DuplicateName("agent", "dup".into()),
            ),
            (
                C::UnknownReference("model", "qwen".into()),
                VnyError::UnknownReference("model", "qwen".into()),
            ),
        ];
        for (source, expected) in pairs {
            let source_display = source.to_string();
            let converted = VnyError::from(source);
            assert_eq!(converted.to_string(), expected.to_string());
            assert_eq!(converted.to_string(), source_display);
        }
    }

    /// Scenario `conversion_cfgstore_inconnue_degenere_en_config_error_double_marqueur`.
    #[test]
    fn conversion_cfgstore_inconnue_degenere_en_config_error_double_marqueur() {
        let converted = VnyError::from(vanyline_cfgstore::CfgStoreError::ReadOnly);
        assert!(matches!(converted, VnyError::ConfigError(_)));
        assert_eq!(
            converted.to_string(),
            "VNL-CFG-001: Configuration error: VNL-CFG-009: Config store is read-only"
        );
    }

    /// Scenario `conversions_from_io_et_json_generables_par_operateur_question`.
    #[test]
    fn conversions_from_io_et_json_generables_par_operateur_question() {
        fn json_fail() -> Result<(), VnyError> {
            serde_json::from_str::<u8>("x")?;
            Ok(())
        }
        fn io_fail() -> Result<(), VnyError> {
            Err::<(), std::io::Error>(std::io::Error::other("disque"))?;
            Ok(())
        }

        let json = json_fail().unwrap_err();
        assert!(matches!(json, VnyError::JsonError(_)));
        assert!(json.source().is_some());
        assert!(json.to_string().starts_with("VNL-JSON-001"));

        let io = io_fail().unwrap_err();
        assert!(matches!(io, VnyError::IoError(_)));
        assert!(io.source().is_some());
        assert!(io.to_string().starts_with("VNL-IO-001"));
    }

    /// Scenario `source_std_error_absente_sauf_io_et_json`.
    #[test]
    fn source_std_error_absente_sauf_io_et_json() {
        let llm = VnyError::LlmError("x".into());
        assert!(llm.source().is_none());

        let io = VnyError::IoError(std::io::Error::other("disque"));
        assert_eq!(
            io.source().map(ToString::to_string).as_deref(),
            Some("disque")
        );
    }

    /// Scenario `rendu_inverse_des_champs_de_duplicate_name`.
    #[test]
    fn rendu_inverse_des_champs_de_duplicate_name() {
        assert_eq!(
            VnyError::DuplicateName("agent", "dup".into()).to_string(),
            "VNL-CFG-002: Duplicate name 'dup' for agent"
        );
        assert_eq!(
            VnyError::UnknownReference("model", "qwen".into()).to_string(),
            "VNL-CFG-003: Unknown model reference: 'qwen'"
        );
    }
}
