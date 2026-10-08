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
