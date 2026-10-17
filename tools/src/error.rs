use thiserror::Error;

/// Erreurs des outils, à identifiants stables `VNL-TLS-001..009`.
#[derive(Debug, Error)]
pub enum ToolsError {
    /// VNL-TLS-001 : chemin inexistant ; `hint` documente le contenu du
    /// parent quand il est listable.
    #[error("VNL-TLS-001: file not found: {path}{hint}")]
    FileNotFound {
        /// Chemin cherché.
        path: String,
        /// Suffixe actionnable (« parent directory contains: […] » ou
        /// « parent does not exist either »), vide si sans objet.
        hint: String,
    },
    // `hint` : ", parent directory contains: [a, b, c]" si le parent existe,
    // ", parent directory does not exist either" sinon. Toujours actionnable.
    /// VNL-TLS-002 : le chemin est un répertoire, pas un fichier.
    #[error("VNL-TLS-002: not a file: {0} (it is a directory — use list_directory)")]
    NotAFile(String),

    /// VNL-TLS-003 : le chemin est un fichier, pas un répertoire.
    #[error("VNL-TLS-003: not a directory: {0} (it is a file — use read_file)")]
    NotADirectory(String),

    /// VNL-TLS-004 : permission refusée sur le chemin.
    #[error("VNL-TLS-004: permission denied: {0}")]
    PermissionDenied(String),

    /// VNL-TLS-005 : argument invalide.
    #[error("VNL-TLS-005: invalid argument {name}: {reason}")]
    InvalidArgument {
        /// Nom de l'argument fautif.
        name: String,
        /// Motif du rejet.
        reason: String,
    },

    /// VNL-TLS-006 : aucun endroit du fichier ne correspond à `old_string`.
    #[error(
        "VNL-TLS-006: string not found in {path}: no occurrence of the provided old_string{hint}"
    )]
    EditNoMatch {
        /// Chemin du fichier édité.
        path: String,
        // `hint` : ". Closest line in file: '<ligne>'" — aide le modèle à corriger
        // son old_string (indentation, espaces).
        /// Indice de la ligne la plus proche (Levenshtein), vide si fichier vide.
        hint: String,
    },
    /// VNL-TLS-007 : `old_string` ambigu — plusieurs occurrences sans
    /// `replace_all`.
    #[error(
        "VNL-TLS-007: {count} occurrences of old_string in {path} — pass replace_all=true or provide a more specific old_string"
    )]
    EditAmbiguous {
        /// Chemin du fichier édité.
        path: String,
        /// Nombre d'occurrences trouvées.
        count: usize,
    },

    /// VNL-TLS-008 : commande interrompue après expiration du timeout
    /// (sortie partielle conservée côté appelant).
    #[error("VNL-TLS-008: command timed out after {0}s (partial output kept)")]
    CommandTimeout(u64),

    /// VNL-TLS-009 : erreur d'E/S sous-jacente sur un chemin.
    #[error("VNL-TLS-009: I/O error on {path}: {source}")]
    Io {
        /// Chemin (ou descripteur d'opération) concerné.
        path: String,
        /// Erreur d'E/S d'origine.
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_present() {
        let file_not_found = ToolsError::FileNotFound {
            path: "/foo/bar".into(),
            hint: ", parent directory contains: [a, b]".into(),
        };
        assert!(file_not_found.to_string().contains("VNL-TLS-001"));

        let not_a_file = ToolsError::NotAFile("/foo".into());
        assert!(not_a_file.to_string().contains("VNL-TLS-002"));

        let not_a_dir = ToolsError::NotADirectory("/foo".into());
        assert!(not_a_dir.to_string().contains("VNL-TLS-003"));

        let perm = ToolsError::PermissionDenied("/foo".into());
        assert!(perm.to_string().contains("VNL-TLS-004"));

        let invalid = ToolsError::InvalidArgument {
            name: "arg".into(),
            reason: "bad value".into(),
        };
        assert!(invalid.to_string().contains("VNL-TLS-005"));

        let no_match = ToolsError::EditNoMatch {
            path: "/foo".into(),
            hint: ". Closest line in file: 'xyz'".into(),
        };
        assert!(no_match.to_string().contains("VNL-TLS-006"));

        let ambiguous = ToolsError::EditAmbiguous {
            path: "/foo".into(),
            count: 3,
        };
        assert!(ambiguous.to_string().contains("VNL-TLS-007"));

        let timeout = ToolsError::CommandTimeout(30);
        assert!(timeout.to_string().contains("VNL-TLS-008"));

        let io = ToolsError::Io {
            path: "/foo".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        };
        assert!(io.to_string().contains("VNL-TLS-009"));
    }

    /// Scenario `display_exhaustif_des_neuf_marqueurs_vnl_tls`.
    #[test]
    fn display_exhaustif_des_neuf_marqueurs_vnl_tls() {
        let cases: Vec<(ToolsError, &str)> = vec![
            (
                ToolsError::FileNotFound {
                    path: "/foo".into(),
                    hint: String::new(),
                },
                "VNL-TLS-001: file not found: /foo",
            ),
            (
                ToolsError::NotAFile("/foo".into()),
                "VNL-TLS-002: not a file: /foo (it is a directory — use list_directory)",
            ),
            (
                ToolsError::NotADirectory("/foo".into()),
                "VNL-TLS-003: not a directory: /foo (it is a file — use read_file)",
            ),
            (
                ToolsError::PermissionDenied("/foo".into()),
                "VNL-TLS-004: permission denied: /foo",
            ),
            (
                ToolsError::InvalidArgument {
                    name: "offset".into(),
                    reason: "file is empty".into(),
                },
                "VNL-TLS-005: invalid argument offset: file is empty",
            ),
            (
                ToolsError::EditNoMatch {
                    path: "/foo".into(),
                    hint: String::new(),
                },
                "VNL-TLS-006: string not found in /foo: no occurrence of the provided old_string",
            ),
            (
                ToolsError::EditAmbiguous {
                    path: "/foo".into(),
                    count: 3,
                },
                "VNL-TLS-007: 3 occurrences of old_string in /foo — pass replace_all=true or provide a more specific old_string",
            ),
            (
                ToolsError::CommandTimeout(30),
                "VNL-TLS-008: command timed out after 30s (partial output kept)",
            ),
            (
                ToolsError::Io {
                    path: "/foo".into(),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
                },
                "VNL-TLS-009: I/O error on /foo: nope",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    /// Scenario `file_not_found_concatene_le_hint_sans_separateur`.
    #[test]
    fn file_not_found_concatene_le_hint_sans_separateur() {
        let with_contents = ToolsError::FileNotFound {
            path: "/foo/bar".into(),
            hint: ", parent directory contains: [a, b]".into(),
        };
        assert_eq!(
            with_contents.to_string(),
            "VNL-TLS-001: file not found: /foo/bar, parent directory contains: [a, b]"
        );

        let not_exist = ToolsError::FileNotFound {
            path: "/foo/bar".into(),
            hint: ", parent directory does not exist either".into(),
        };
        assert_eq!(
            not_exist.to_string(),
            "VNL-TLS-001: file not found: /foo/bar, parent directory does not exist either"
        );

        let empty = ToolsError::FileNotFound {
            path: "/foo/bar".into(),
            hint: String::new(),
        };
        assert_eq!(empty.to_string(), "VNL-TLS-001: file not found: /foo/bar");
    }

    /// Scenario `edit_no_match_avec_et_sans_hint_proche`.
    #[test]
    fn edit_no_match_avec_et_sans_hint_proche() {
        let with_hint = ToolsError::EditNoMatch {
            path: "/foo".into(),
            hint: ". Closest line in file: 'xyz'".into(),
        };
        assert_eq!(
            with_hint.to_string(),
            "VNL-TLS-006: string not found in /foo: no occurrence of the provided old_string. Closest line in file: 'xyz'"
        );

        let without_hint = ToolsError::EditNoMatch {
            path: "/foo".into(),
            hint: String::new(),
        };
        assert_eq!(
            without_hint.to_string(),
            "VNL-TLS-006: string not found in /foo: no occurrence of the provided old_string"
        );
    }

    /// Scenario `edit_ambiguous_rend_le_nombre_et_la_parade`.
    #[test]
    fn edit_ambiguous_rend_le_nombre_et_la_parade() {
        let err = ToolsError::EditAmbiguous {
            path: "/foo".into(),
            count: 3,
        };
        assert_eq!(
            err.to_string(),
            "VNL-TLS-007: 3 occurrences of old_string in /foo — pass replace_all=true or provide a more specific old_string"
        );
    }

    /// Scenario `command_timeout_affiche_la_duree_en_secondes`.
    #[test]
    fn command_timeout_affiche_la_duree_en_secondes() {
        assert_eq!(
            ToolsError::CommandTimeout(30).to_string(),
            "VNL-TLS-008: command timed out after 30s (partial output kept)"
        );
    }

    /// Scenario `source_std_error_absente_sauf_variante_io`.
    #[test]
    fn source_std_error_absente_sauf_variante_io() {
        use std::error::Error as _;

        let perm = ToolsError::PermissionDenied("/foo".into());
        assert!(perm.source().is_none());

        let io = ToolsError::Io {
            path: "/foo".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        };
        let src = io.source();
        assert!(src.is_some());
        assert_eq!(src.map(ToString::to_string).as_deref(), Some("nope"));
    }

    /// Scenario `io_inliste_le_display_de_la_cause`.
    #[test]
    fn io_inliste_le_display_de_la_cause() {
        use std::error::Error as _;

        let io = ToolsError::Io {
            path: "/foo".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        };
        assert_eq!(io.to_string(), "VNL-TLS-009: I/O error on /foo: nope");
        assert_eq!(
            io.source().map(ToString::to_string).as_deref(),
            Some("nope")
        );
    }

    /// Scenario `pas_de_conversion_automatique_depuis_std_io_error`.
    /// Substitut à trybuild : contrôle source sur la partie production du fichier.
    #[test]
    fn pas_de_conversion_automatique_depuis_std_io_error() {
        let src = include_str!("error.rs");
        let (production, _) = src
            .split_once("#[cfg(test)]")
            .expect("marqueur cfg(test) présent");
        assert!(!production.contains("#[from]"));
        assert!(!production.contains("impl From"));
        assert!(!production.contains("impl std::convert::From"));
    }

    /// Scenario `marqueurs_tls_uniques_et_non_dupliques`.
    #[test]
    fn marqueurs_tls_uniques_et_non_dupliques() {
        let src = include_str!("error.rs");
        let (production, _) = src
            .split_once("#[cfg(test)]")
            .expect("marqueur cfg(test) présent");
        for n in 1..=9 {
            // Motif restreint au gabarit `#[error("VNL-TLS-00n:`, pour ne pas
            // compter les commentaires doc qui citent aussi le marqueur.
            let marker = format!("\"VNL-TLS-00{n}:");
            assert_eq!(
                production.matches(marker.as_str()).count(),
                1,
                "gabarit {marker} attendu une seule fois hors tests"
            );
        }
    }

    /// Scenario `payload_rendu_tel_quel_sans_validation`.
    #[test]
    fn payload_rendu_tel_quel_sans_validation() {
        let chemin = "/dosier avec espace «çà» 'quote'";
        assert_eq!(
            ToolsError::NotAFile(chemin.into()).to_string(),
            format!("VNL-TLS-002: not a file: {chemin} (it is a directory — use list_directory)")
        );
        let ambiguous = ToolsError::EditAmbiguous {
            path: "/foo".into(),
            count: 0,
        };
        assert_eq!(
            ambiguous.to_string(),
            "VNL-TLS-007: 0 occurrences of old_string in /foo — pass replace_all=true or provide a more specific old_string"
        );
    }
}
