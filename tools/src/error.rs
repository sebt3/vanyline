use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolsError {
    #[error("VNL-TLS-001: file not found: {path}{hint}")]
    FileNotFound { path: String, hint: String },
    // `hint` : ", parent directory contains: [a, b, c]" si le parent existe,
    // ", parent directory does not exist either" sinon. Toujours actionnable.
    #[error("VNL-TLS-002: not a file: {0} (it is a directory — use list_directory)")]
    NotAFile(String),

    #[error("VNL-TLS-003: not a directory: {0} (it is a file — use read_file)")]
    NotADirectory(String),

    #[error("VNL-TLS-004: permission denied: {0}")]
    PermissionDenied(String),

    #[error("VNL-TLS-005: invalid argument {name}: {reason}")]
    InvalidArgument { name: String, reason: String },

    #[error(
        "VNL-TLS-006: string not found in {path}: no occurrence of the provided old_string{hint}"
    )]
    EditNoMatch { path: String, hint: String },
    // `hint` : ". Closest line in file: '<ligne>'" — aide le modèle à corriger
    // son old_string (indentation, espaces).
    #[error("VNL-TLS-007: {count} occurrences of old_string in {path} — pass replace_all=true or provide a more specific old_string")]
    EditAmbiguous { path: String, count: usize },

    #[error("VNL-TLS-008: command timed out after {0}s (partial output kept)")]
    CommandTimeout(u64),

    #[error("VNL-TLS-009: I/O error on {path}: {source}")]
    Io {
        path: String,
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
