use crate::store::Layer;
use thiserror::Error;

/// Erreurs de la couche config (lecture + écriture). Identifiants stables
/// `VNL-CFG-*`. Les variantes write-side (`VNL-CFG-005..010`) arrivent en tâche 1.
#[derive(Debug, Error)]
pub enum CfgStoreError {
    #[error("VNL-CFG-001: Configuration error: {0}")]
    Config(String),
    #[error("VNL-CFG-002: Duplicate name '{1}' for {0}")]
    DuplicateName(&'static str, String),
    #[error("VNL-CFG-003: Unknown {0} reference: '{1}'")]
    UnknownReference(&'static str, String),
    #[error("VNL-CFG-004: I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("VNL-CFG-005: Invalid name: {0}")]
    InvalidName(String),
    #[error("VNL-CFG-006: {kind} '{name}' not found in {layer:?} layer")]
    NotFound {
        kind: &'static str,
        name: String,
        layer: Layer,
    },
    #[error("VNL-CFG-007: {kind} '{name}' already exists in {layer:?} layer")]
    NameConflict {
        kind: &'static str,
        name: String,
        layer: Layer,
    },
    #[error("VNL-CFG-008: Write error: {0}")]
    WriteError(String),
    #[error("VNL-CFG-009: Config store is read-only")]
    ReadOnly,
    #[error("VNL-CFG-010: Invalid value: {0}")]
    Validation(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes() {
        let e1 = CfgStoreError::Config("boom".to_string());
        assert!(format!("{}", e1).contains("VNL-CFG-001"));
        let e2 = CfgStoreError::DuplicateName("agent", "build".to_string());
        assert!(format!("{}", e2).contains("VNL-CFG-002"));
        let e3 = CfgStoreError::UnknownReference("model", "qwen-code".to_string());
        assert!(format!("{}", e3).contains("VNL-CFG-003"));
        let e4 = CfgStoreError::from(std::io::Error::other("io"));
        assert!(format!("{}", e4).contains("VNL-CFG-004"));
        let e5 = CfgStoreError::InvalidName("".to_string());
        assert!(format!("{}", e5).contains("VNL-CFG-005"));
        let e6 = CfgStoreError::NotFound {
            kind: "provider",
            name: "x".to_string(),
            layer: Layer::Global,
        };
        assert!(format!("{}", e6).contains("VNL-CFG-006"));
        let e7 = CfgStoreError::NameConflict {
            kind: "model",
            name: "x".to_string(),
            layer: Layer::Workspace,
        };
        assert!(format!("{}", e7).contains("VNL-CFG-007"));
        let e8 = CfgStoreError::WriteError("boom".to_string());
        assert!(format!("{}", e8).contains("VNL-CFG-008"));
        let e9 = CfgStoreError::ReadOnly;
        assert!(format!("{}", e9).contains("VNL-CFG-009"));
        let e10 = CfgStoreError::Validation("type".to_string());
        assert!(format!("{}", e10).contains("VNL-CFG-010"));
        let e11 = CfgStoreError::InvalidName("a".to_string());
        assert!(matches!(e11, CfgStoreError::InvalidName(_)));
    }
}
