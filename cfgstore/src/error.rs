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

    #[test]
    fn display_exhaustif_des_dix_marqueurs_vnl_cfg() {
        let cases: Vec<(CfgStoreError, &str)> = vec![
            (
                CfgStoreError::Config("boom".to_string()),
                "VNL-CFG-001: Configuration error: boom",
            ),
            (
                CfgStoreError::DuplicateName("agent", "build".to_string()),
                "VNL-CFG-002: Duplicate name 'build' for agent",
            ),
            (
                CfgStoreError::UnknownReference("model", "qwen-code".to_string()),
                "VNL-CFG-003: Unknown model reference: 'qwen-code'",
            ),
            (
                CfgStoreError::from(std::io::Error::other("io")),
                "VNL-CFG-004: I/O error: io",
            ),
            (
                CfgStoreError::InvalidName(String::new()),
                "VNL-CFG-005: Invalid name: ",
            ),
            (
                CfgStoreError::NotFound {
                    kind: "provider",
                    name: "x".to_string(),
                    layer: Layer::Global,
                },
                "VNL-CFG-006: provider 'x' not found in Global layer",
            ),
            (
                CfgStoreError::NameConflict {
                    kind: "model",
                    name: "x".to_string(),
                    layer: Layer::Workspace,
                },
                "VNL-CFG-007: model 'x' already exists in Workspace layer",
            ),
            (
                CfgStoreError::WriteError("boom".to_string()),
                "VNL-CFG-008: Write error: boom",
            ),
            (
                CfgStoreError::ReadOnly,
                "VNL-CFG-009: Config store is read-only",
            ),
            (
                CfgStoreError::Validation("type".to_string()),
                "VNL-CFG-010: Invalid value: type",
            ),
        ];
        for (err, attendu) in cases {
            assert_eq!(format!("{err}"), attendu);
        }
    }

    #[test]
    fn rendu_inverse_des_champs_de_duplicate_name() {
        let dup = CfgStoreError::DuplicateName("agent", "dup".to_string());
        assert_eq!(
            format!("{dup}"),
            "VNL-CFG-002: Duplicate name 'dup' for agent"
        );
        let unk = CfgStoreError::UnknownReference("model", "qwen".to_string());
        assert_eq!(
            format!("{unk}"),
            "VNL-CFG-003: Unknown model reference: 'qwen'"
        );
    }

    #[test]
    fn rendu_debug_de_layer_dans_not_found_et_name_conflict() {
        let nf = CfgStoreError::NotFound {
            kind: "provider",
            name: "p".to_string(),
            layer: Layer::Workspace,
        };
        assert_eq!(
            format!("{nf}"),
            "VNL-CFG-006: provider 'p' not found in Workspace layer"
        );
        let nc = CfgStoreError::NameConflict {
            kind: "provider",
            name: "p".to_string(),
            layer: Layer::Workspace,
        };
        assert_eq!(
            format!("{nc}"),
            "VNL-CFG-007: provider 'p' already exists in Workspace layer"
        );
    }

    #[test]
    fn conversion_depuis_std_io_error_generee_par_operateur_question() {
        fn ecrit_dans_chemin_invalide(dir: &std::path::Path) -> Result<(), CfgStoreError> {
            std::fs::write(dir.join("absent").join("fichier"), b"x")?;
            Ok(())
        }
        let tmp = tempfile::tempdir().unwrap();
        let err = ecrit_dans_chemin_invalide(tmp.path()).unwrap_err();
        assert!(matches!(err, CfgStoreError::Io(_)));
        let io_msg = match &err {
            CfgStoreError::Io(io) => io.to_string(),
            _ => String::new(),
        };
        assert_eq!(
            format!("{err}"),
            format!("VNL-CFG-004: I/O error: {io_msg}")
        );
        let source = std::error::Error::source(&err).expect("source Some");
        assert_eq!(source.to_string(), io_msg);
    }

    #[test]
    fn source_std_error_absente_sauf_variante_io() {
        let ro = CfgStoreError::ReadOnly;
        assert!(std::error::Error::source(&ro).is_none());
        let io = CfgStoreError::Io(std::io::Error::other("x"));
        let source = std::error::Error::source(&io).expect("source Some");
        assert_eq!(source.to_string(), "x");
    }

    #[test]
    fn marqueurs_vnl_cfg_uniques_dans_le_fichier() {
        let src = include_str!("error.rs");
        let hors_tests = src.split("#[cfg(test)]").next().unwrap_or_default();
        let mut total = 0;
        for n in 1..=10 {
            let gabarit = format!("#[error(\"VNL-CFG-{n:03}:");
            let occurrences = hors_tests.matches(gabarit.as_str()).count();
            assert_eq!(occurrences, 1, "marqueur VNL-CFG-{n:03}");
            total += occurrences;
        }
        assert_eq!(hors_tests.matches("#[error(\"VNL-CFG-").count(), total);
    }

    #[test]
    fn payload_rendu_tel_quel_y_compris_nom_vide() {
        let chemin = "/dosier avec espace «çà» 'quote'";
        let cfg = CfgStoreError::Config(chemin.to_string());
        assert_eq!(
            format!("{cfg}"),
            format!("VNL-CFG-001: Configuration error: {chemin}")
        );
        let vide = CfgStoreError::InvalidName(String::new());
        assert_eq!(format!("{vide}"), "VNL-CFG-005: Invalid name: ");
    }

    #[test]
    fn payload_portant_deja_un_marqueur_produit_un_double_marqueur() {
        let err = CfgStoreError::Config("VNL-LLM-001: LLM provider error: x".to_string());
        assert_eq!(
            format!("{err}"),
            "VNL-CFG-001: Configuration error: VNL-LLM-001: LLM provider error: x"
        );
    }
}
