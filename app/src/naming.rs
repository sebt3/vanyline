//! Nommage des objets Kubernetes de l'app : fonctions pures, sans I/O — contrat : `naming.sdd`.
#![allow(dead_code)] // naming.sdd Tasks : aucun consommateur de production avant le premier lot ; à retirer avec lui.

use thiserror::Error;

/// Longueur maximale d'un label DNS (RFC 1123) — `naming.sdd`, Exposes.
pub const MAX_DNS_LABEL: usize = 63;

/// Erreurs de nommage — identifiants `VNL-NAM-NNN` (`naming.sdd`, Exposes).
#[derive(Debug, Error, PartialEq)]
pub enum NamingError {
    /// Valeur qui n'est pas un label DNS valide (`VNL-NAM-001`).
    #[error(
        "VNL-NAM-001: invalid {field} « {value} » (lowercase alphanumerics and '-', must start and end with an alphanumeric)"
    )]
    Invalid {
        /// Nom du champ refusé (`project` ou `name`).
        field: &'static str,
        /// Valeur refusée, telle que reçue.
        value: String,
    },
    /// Nom construit plus long que `MAX_DNS_LABEL` (`VNL-NAM-002`).
    #[error("VNL-NAM-002: name too long ({len} > {max})")]
    TooLong {
        /// Longueur du nom construit.
        len: usize,
        /// Longueur maximale autorisée.
        max: usize,
    },
}

/// Vérifie qu'une valeur est un label DNS (RFC 1123) ; ne contrôle pas la longueur.
///
/// # Errors
/// `NamingError::Invalid` si la valeur est vide, contient autre chose que `[a-z0-9-]`,
/// ou ne commence/finit pas par un alphanumérique.
pub fn validate_label(field: &'static str, value: &str) -> Result<(), NamingError> {
    let chars_ok = value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let edges_ok = value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && value
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if chars_ok && edges_ok {
        Ok(())
    } else {
        Err(NamingError::Invalid {
            field,
            value: value.to_string(),
        })
    }
}

/// Construit le `metadata.name` déterministe `"{project}-{name}"` d'une sandbox.
///
/// # Errors
/// `Invalid` (champ `project` puis `name`) si l'un des deux n'est pas un label DNS valide ;
/// `TooLong` si le résultat dépasse `MAX_DNS_LABEL` (jamais de troncature).
pub fn sandbox_cr_name(project: &str, name: &str) -> Result<String, NamingError> {
    validate_label("project", project)?;
    validate_label("name", name)?;
    let len = project.len().saturating_add(1).saturating_add(name.len());
    if len > MAX_DNS_LABEL {
        return Err(NamingError::TooLong {
            len,
            max: MAX_DNS_LABEL,
        });
    }
    Ok(format!("{project}-{name}"))
}

/// Nom affiché d'une CR adoptée : retire le préfixe `"{project}-"` si le reste est non vide,
/// sinon retourne `cr_name` tel quel.
pub fn display_name(project: &str, cr_name: &str) -> String {
    let prefix = format!("{project}-");
    match cr_name.strip_prefix(prefix.as_str()) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => cr_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: `nom_technique_deterministe`
    #[test]
    fn nom_technique_deterministe() {
        assert_eq!(sandbox_cr_name("demo", "main"), Ok("demo-main".to_string()));
    }

    /// Scenario: `meme_nom_dans_deux_projets`
    #[test]
    fn meme_nom_dans_deux_projets() {
        let a = sandbox_cr_name("a", "main").expect("valid");
        let b = sandbox_cr_name("b", "main").expect("valid");
        assert_eq!(a, "a-main");
        assert_eq!(b, "b-main");
        assert_ne!(a, b);
    }

    /// Scenario: `nom_invalide_refuse`
    #[test]
    fn nom_invalide_refuse() {
        for name in ["Main", "ma_sandbox", "-x", "x-", "", "a b"] {
            match sandbox_cr_name("demo", name) {
                Err(e @ NamingError::Invalid { field: "name", .. }) => {
                    assert!(e.to_string().starts_with("VNL-NAM-001"), "{name:?} : {e}");
                }
                other => panic!("{name:?} doit être refusé en Invalid name, got {other:?}"),
            }
        }
    }

    /// Scenario: `projet_invalide_refuse_en_premier`
    #[test]
    fn projet_invalide_refuse_en_premier() {
        match sandbox_cr_name("Demo", "Main") {
            Err(NamingError::Invalid { field, .. }) => assert_eq!(field, "project"),
            other => panic!("attendu Invalid project, got {other:?}"),
        }
    }

    /// Scenario: `longueur_maximale`
    #[test]
    fn longueur_maximale() {
        // "aaaaaaaaaa" + "-" + 52 caractères = 63
        let project = "a".repeat(10);
        let ok_name = "b".repeat(52);
        let cr = sandbox_cr_name(&project, &ok_name).expect("63 caractères acceptés");
        assert_eq!(cr.len(), 63);

        // 53 caractères ⇒ 64 : refus, sans troncature
        let long_name = "b".repeat(53);
        match sandbox_cr_name(&project, &long_name) {
            Err(e @ NamingError::TooLong { len: 64, max: 63 }) => {
                assert!(e.to_string().starts_with("VNL-NAM-002"), "{e}");
            }
            other => panic!("attendu TooLong 64/63, got {other:?}"),
        }
    }

    /// Scenario: `ambiguite_de_concatenation_assumee`
    #[test]
    fn ambiguite_de_concatenation_assumee() {
        assert_eq!(sandbox_cr_name("a", "b-c"), Ok("a-b-c".to_string()));
        assert_eq!(sandbox_cr_name("a-b", "c"), Ok("a-b-c".to_string()));
    }

    /// Scenario: `nom_affiche_d_une_cr_adoptee`
    #[test]
    fn nom_affiche_d_une_cr_adoptee() {
        assert_eq!(display_name("demo", "demo-main"), "main");
        assert_eq!(display_name("demo", "autre"), "autre");
        assert_eq!(display_name("demo", "demo-"), "demo-");
    }
}
