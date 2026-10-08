use crate::store::Layer;
use thiserror::Error;

/// Erreurs de la couche config (lecture + écriture). Identifiants stables
/// `VNL-CFG-*` : `001..004` lecture/IO, `005..010` écriture.
#[derive(Debug, Error)]
pub enum CfgStoreError {
    /// VNL-CFG-001 : erreur de configuration (parsing, forme inattendue).
    #[error("VNL-CFG-001: Configuration error: {0}")]
    Config(String),
    /// VNL-CFG-002 : deux entrées portent le même nom dans une collection.
    #[error("VNL-CFG-002: Duplicate name '{1}' for {0}")]
    DuplicateName(&'static str, String),
    /// VNL-CFG-003 : référence vers un nom inexistant.
    #[error("VNL-CFG-003: Unknown {0} reference: '{1}'")]
    UnknownReference(&'static str, String),
    /// VNL-CFG-004 : erreur d'E/S sous-jacente.
    #[error("VNL-CFG-004: I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// VNL-CFG-005 : nom non conforme à la contrainte anti-traversal.
    #[error("VNL-CFG-005: Invalid name: {0}")]
    InvalidName(String),
    /// VNL-CFG-006 : écriture demandée sur un item absent de la couche visée.
    #[error("VNL-CFG-006: {kind} '{name}' not found in {layer:?} layer")]
    NotFound {
        /// Type d'entité visé (ex. `provider`, `agent`).
        kind: &'static str,
        /// Nom cherché.
        name: String,
        /// Couche dans laquelle la recherche a échoué.
        layer: Layer,
    },
    /// VNL-CFG-007 : création d'un nom déjà présent dans la couche visée.
    #[error("VNL-CFG-007: {kind} '{name}' already exists in {layer:?} layer")]
    NameConflict {
        /// Type d'entité visé (ex. `provider`, `agent`).
        kind: &'static str,
        /// Nom en conflit.
        name: String,
        /// Couche où le conflit a été détecté.
        layer: Layer,
    },
    /// VNL-CFG-008 : échec de sérialisation ou d'écriture d'un artefact.
    #[error("VNL-CFG-008: Write error: {0}")]
    WriteError(String),
    /// VNL-CFG-009 : écriture tentée sur un store en lecture seule.
    #[error("VNL-CFG-009: Config store is read-only")]
    ReadOnly,
    /// VNL-CFG-010 : valeur de patch refusée par la validation.
    #[error("VNL-CFG-010: Invalid value: {0}")]
    Validation(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes() {
        let e1 = CfgStoreError::Config("boom".to_string());
        assert!(format!("{e1}").contains("VNL-CFG-001"));
        let e2 = CfgStoreError::DuplicateName("agent", "build".to_string());
        assert!(format!("{e2}").contains("VNL-CFG-002"));
        let e3 = CfgStoreError::UnknownReference("model", "qwen-code".to_string());
        assert!(format!("{e3}").contains("VNL-CFG-003"));
        let e4 = CfgStoreError::from(std::io::Error::other("io"));
        assert!(format!("{e4}").contains("VNL-CFG-004"));
        let e5 = CfgStoreError::InvalidName(String::new());
        assert!(format!("{e5}").contains("VNL-CFG-005"));
        let e6 = CfgStoreError::NotFound {
            kind: "provider",
            name: "x".to_string(),
            layer: Layer::Global,
        };
        assert!(format!("{e6}").contains("VNL-CFG-006"));
        let e7 = CfgStoreError::NameConflict {
            kind: "model",
            name: "x".to_string(),
            layer: Layer::Workspace,
        };
        assert!(format!("{e7}").contains("VNL-CFG-007"));
        let e8 = CfgStoreError::WriteError("boom".to_string());
        assert!(format!("{e8}").contains("VNL-CFG-008"));
        let e9 = CfgStoreError::ReadOnly;
        assert!(format!("{e9}").contains("VNL-CFG-009"));
        let e10 = CfgStoreError::Validation("type".to_string());
        assert!(format!("{e10}").contains("VNL-CFG-010"));
        let e11 = CfgStoreError::InvalidName("a".to_string());
        assert!(matches!(e11, CfgStoreError::InvalidName(_)));
    }
}
