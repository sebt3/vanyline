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
    #[error("VNL-CFG-009: Config store is read-only")]
    ReadOnly,
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
        let e4 = CfgStoreError::from(std::io::Error::new(std::io::ErrorKind::Other, "io"));
        assert!(format!("{}", e4).contains("VNL-CFG-004"));
        let e9 = CfgStoreError::ReadOnly;
        assert!(format!("{}", e9).contains("VNL-CFG-009"));
    }
}
