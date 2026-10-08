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
}
