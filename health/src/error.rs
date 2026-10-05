//! Erreurs du crate `vanyline-health` — domaine `HLT` de la convention « messages d'erreur
//! avec identifiant unique ».

use thiserror::Error;

/// Erreurs du domaine « santé du code » — convention « messages d'erreur avec identifiant
/// unique » (`VNL-HLT-NNN`, même famille que `VNL-TLS-*`, `VNL-CFG-*`). Le `Display` est la
/// seule forme visible au fil : les consommateurs le stringifient.
#[derive(Debug, Error)]
pub enum HealthError {
    /// (`VNL-HLT-001`) Enregistrement `lcov` invalide — `line` est le numéro de ligne
    /// 1-indexé du texte.
    #[error("VNL-HLT-001: invalid lcov at line {line}: {reason}")]
    InvalidLcov {
        /// Ligne 1-indexée du texte où l'erreur a été détectée.
        line: usize,
        /// Détail lisible.
        reason: String,
    },
    /// (`VNL-HLT-002`) Diff git invalide.
    #[error("VNL-HLT-002: invalid diff: {reason}")]
    InvalidDiff {
        /// Détail lisible.
        reason: String,
    },
    /// (`VNL-HLT-003`) Vérification mal configurée ou sortie inexploitable.
    #[error("VNL-HLT-003: invalid check '{id}': {reason}")]
    InvalidCheck {
        /// Identifiant de la vérification.
        id: String,
        /// Détail lisible.
        reason: String,
    },
    /// (`VNL-HLT-004`) Impossible de démarrer une vérification.
    #[error("VNL-HLT-004: cannot start check '{id}': {source}")]
    Spawn {
        /// Identifiant de la vérification.
        id: String,
        /// Erreur d'exécution sous-jacente.
        #[source]
        source: std::io::Error,
    },
    /// (`VNL-HLT-005`) Vérification tuée après son délai.
    #[error("VNL-HLT-005: check '{id}' timed out after {secs}s")]
    Timeout {
        /// Identifiant de la vérification.
        id: String,
        /// Délai expiré, en secondes.
        secs: u64,
    },
    /// (`VNL-HLT-006`) Erreur d'E/S sur un rapport.
    #[error("VNL-HLT-006: report I/O error on {path}: {source}")]
    ReportIo {
        /// Chemin du rapport.
        path: String,
        /// Erreur d'E/S sous-jacente.
        #[source]
        source: std::io::Error,
    },
    /// (`VNL-HLT-007`) Référence git invalide.
    #[error("VNL-HLT-007: invalid git ref: {value}")]
    InvalidRef {
        /// Valeur refusée.
        value: String,
    },
    /// (`VNL-HLT-008`) Commande git en échec.
    #[error("VNL-HLT-008: git failed: {reason}")]
    Git {
        /// Détail lisible.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::HealthError;

    /// Scenario `display_porte_le_marqueur` — chaque variante construite avec des champs
    /// d'exemple rend un `Display` commençant par son marqueur `VNL-HLT-NNN: ` exact.
    #[test]
    fn display_porte_le_marqueur() {
        let cases: Vec<(HealthError, &str)> = vec![
            (
                HealthError::InvalidLcov {
                    line: 3,
                    reason: "boom".to_string(),
                },
                "VNL-HLT-001: invalid lcov at line 3: boom",
            ),
            (
                HealthError::InvalidDiff {
                    reason: "boom".to_string(),
                },
                "VNL-HLT-002: invalid diff: boom",
            ),
            (
                HealthError::InvalidCheck {
                    id: "clippy".to_string(),
                    reason: "boom".to_string(),
                },
                "VNL-HLT-003: invalid check 'clippy': boom",
            ),
            (
                HealthError::Spawn {
                    id: "clippy".to_string(),
                    source: std::io::Error::other("nope"),
                },
                "VNL-HLT-004: cannot start check 'clippy': nope",
            ),
            (
                HealthError::Timeout {
                    id: "clippy".to_string(),
                    secs: 30,
                },
                "VNL-HLT-005: check 'clippy' timed out after 30s",
            ),
            (
                HealthError::ReportIo {
                    path: "/tmp/r.json".to_string(),
                    source: std::io::Error::other("nope"),
                },
                "VNL-HLT-006: report I/O error on /tmp/r.json: nope",
            ),
            (
                HealthError::InvalidRef {
                    value: "a b".to_string(),
                },
                "VNL-HLT-007: invalid git ref: a b",
            ),
            (
                HealthError::Git {
                    reason: "boom".to_string(),
                },
                "VNL-HLT-008: git failed: boom",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    /// Scenario `erreur_io_chainee` — `source()` sur une variante `Spawn` retourne l'erreur
    /// I/O d'origine.
    #[test]
    fn erreur_io_chainee() {
        let inner = std::io::Error::other("cause");
        let err = HealthError::Spawn {
            id: "clippy".to_string(),
            source: inner,
        };
        let src = <dyn std::error::Error>::source(&err).expect("Spawn doit chaîner sa source");
        assert_eq!(src.to_string(), "cause");
    }
}
