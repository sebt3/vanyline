//! Spécifications complètes des CRD Project et Sandbox : lecture, et refus des modifications immuables.
//! Contrat : `spec_edit.sdd`. Périmètre de ce lot : `parse_spec` et `check_immutable` ; `apply` et `edit` attendent un
//! remplacement (PUT) du client K8s de la lib, qui n'existe pas encore (compte rendu E9).

use serde_json::Value;
use vanyline_crds::{ProjectSpec, SandboxSpec};

/// Marqueur d'une spécification illisible, inconnue ou d'un type qui ne correspond pas à la cible.
const INVALID: &str = "VNL-CLI-SPC-001";
/// Marqueur d'un champ immuable modifié (refus avant tout appel au cluster).
const IMMUTABLE: &str = "VNL-CLI-SPC-002";

/// Type d'objet géré par `apply` et `edit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `Project`.
    Project,
    /// `Sandbox`.
    Sandbox,
}

/// Spécification complète d'un objet, typée par son CRD.
#[derive(Debug, Clone)]
pub enum ObjectSpec {
    /// Spécification d'un Project.
    Project(ProjectSpec),
    /// Spécification d'une Sandbox.
    Sandbox(SandboxSpec),
}

/// Refus de la spécification : la cause nomme le champ ou la ligne en cause, sans valeur de secret.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpecError {
    /// Spécification illisible, champ inconnu, ou type de spécification incompatible.
    #[error("{0}")]
    Invalid(String),
    /// Modification d'un champ immuable : la cause nomme le champ.
    #[error("{0}")]
    Immutable(String),
}

/// Lit une spécification complète (YAML ou JSON) pour `kind`. Un champ inconnu est refusé, nommé par son chemin.
///
/// # Errors
///
/// Returns [`SpecError::Invalid`] when the text is not a valid specification of `kind`.
pub fn parse_spec(kind: Kind, texte: &str) -> Result<ObjectSpec, SpecError> {
    let valeur: Value = yaml_serde::from_str(texte).map_err(|erreur| {
        SpecError::Invalid(format!("{INVALID}: spécification illisible: {erreur}"))
    })?;
    let (spec, relue) = match kind {
        Kind::Project => {
            let spec: ProjectSpec = typer(&valeur)?;
            let relue = serde_json::to_value(&spec).map_err(|erreur| invalide(&erreur))?;
            (ObjectSpec::Project(spec), relue)
        }
        Kind::Sandbox => {
            let spec: SandboxSpec = typer(&valeur)?;
            let relue = serde_json::to_value(&spec).map_err(|erreur| invalide(&erreur))?;
            (ObjectSpec::Sandbox(spec), relue)
        }
    };
    match champ_inconnu(&valeur, &relue, "") {
        Some(chemin) => Err(SpecError::Invalid(format!(
            "{INVALID}: champ inconnu `{chemin}`"
        ))),
        None => Ok(spec),
    }
}

/// Désérialise la spécification typée ; une erreur de type nomme le champ fautif par le message de `serde`.
fn typer<T: serde::de::DeserializeOwned>(valeur: &Value) -> Result<T, SpecError> {
    serde_json::from_value(valeur.clone()).map_err(|erreur| invalide(&erreur))
}

/// Erreur d'une spécification refusée par la désérialisation.
fn invalide(erreur: &serde_json::Error) -> SpecError {
    SpecError::Invalid(format!("{INVALID}: spécification invalide: {erreur}"))
}

/// Premier champ de `entree` absent de `sortie` (la forme relue de la spécification typée), par son chemin.
///
/// `serde` ignore les champs inconnus ; la relecture ne retrouve donc que les champs que le CRD connaît. Une valeur
/// `null` vaut absence.
fn champ_inconnu(entree: &Value, sortie: &Value, chemin: &str) -> Option<String> {
    match (entree, sortie) {
        (Value::Object(attendus), Value::Object(relus)) => {
            for (cle, valeur) in attendus {
                if valeur.is_null() {
                    continue;
                }
                let sous = if chemin.is_empty() {
                    cle.clone()
                } else {
                    format!("{chemin}.{cle}")
                };
                match relus.get(cle) {
                    None => return Some(sous),
                    Some(relu) => {
                        if let Some(trouve) = champ_inconnu(valeur, relu, &sous) {
                            return Some(trouve);
                        }
                    }
                }
            }
            None
        }
        (Value::Array(attendus), Value::Array(relus)) => {
            for (indice, valeur) in attendus.iter().enumerate() {
                let sous = format!("{chemin}[{indice}]");
                match relus.get(indice) {
                    None => return Some(sous),
                    Some(relu) => {
                        if let Some(trouve) = champ_inconnu(valeur, relu, &sous) {
                            return Some(trouve);
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Refuse la modification d'un champ immuable : le projet d'une Sandbox, l'Owner d'un Project. L'erreur nomme le champ.
///
/// # Errors
///
/// Returns [`SpecError::Immutable`] naming the field, or [`SpecError::Invalid`] when the two specs are of different kinds.
pub fn check_immutable(existant: &ObjectSpec, voulu: &ObjectSpec) -> Result<(), SpecError> {
    match (existant, voulu) {
        (ObjectSpec::Sandbox(avant), ObjectSpec::Sandbox(apres)) => {
            if avant.project == apres.project {
                Ok(())
            } else {
                Err(SpecError::Immutable(format!(
                    "{IMMUTABLE}: champ `project` immuable (sandbox)"
                )))
            }
        }
        (ObjectSpec::Project(avant), ObjectSpec::Project(apres)) => {
            if avant.owner == apres.owner {
                Ok(())
            } else {
                Err(SpecError::Immutable(format!(
                    "{IMMUTABLE}: champ `owner` immuable (project)"
                )))
            }
        }
        _ => Err(SpecError::Invalid(format!(
            "{INVALID}: spécifications de types différents"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, ObjectSpec, SpecError, check_immutable, parse_spec};

    const SANDBOX_YAML: &str = "project: depot\nbranch: main\negress: []\nsuspended: false\n";

    fn sandbox(texte: &str) -> ObjectSpec {
        match parse_spec(Kind::Sandbox, texte) {
            Ok(spec) => spec,
            Err(erreur) => panic!("spec de sandbox refusée à tort : {erreur}"),
        }
    }

    fn project(texte: &str) -> ObjectSpec {
        match parse_spec(Kind::Project, texte) {
            Ok(spec) => spec,
            Err(erreur) => panic!("spec de project refusée à tort : {erreur}"),
        }
    }

    // Contrat @parse_spec (YAML ou JSON, spécification complète) — non un Scenario de la spec
    #[test]
    fn lit_une_specification_yaml_ou_json() {
        assert!(matches!(
            parse_spec(Kind::Sandbox, SANDBOX_YAML),
            Ok(ObjectSpec::Sandbox(_))
        ));
        let json = r#"{"project":"depot","branch":"main"}"#;
        assert!(matches!(
            parse_spec(Kind::Sandbox, json),
            Ok(ObjectSpec::Sandbox(_))
        ));
        let spec = project("owner: alice\nrepoUrl: https://example.invalid/d.git\n");
        assert!(matches!(spec, ObjectSpec::Project(_)));
    }

    // Contrat @parse_spec (champs inconnus refusés, nommés)
    #[test]
    fn champ_inconnu_refuse_nomme() {
        let texte = "project: depot\nbranch: main\nbranchh: x\n";
        match parse_spec(Kind::Sandbox, texte) {
            Err(SpecError::Invalid(cause)) => {
                assert!(cause.contains("branchh"), "cause: {cause}");
            }
            autre => panic!("attendu Invalid nommant branchh, reçu {autre:?}"),
        }
    }

    // Contrat @parse_spec (valeur null = absence)
    #[test]
    fn valeur_null_ignoree() {
        let texte = "project: depot\nbranch: main\nimage: null\n";
        assert!(matches!(
            parse_spec(Kind::Sandbox, texte),
            Ok(ObjectSpec::Sandbox(_))
        ));
    }

    // Contrat @parse_spec (type incompatible refusé)
    #[test]
    fn type_incompatible_refuse() {
        assert!(matches!(
            parse_spec(Kind::Project, SANDBOX_YAML),
            Err(SpecError::Invalid(_))
        ));
    }

    // Contrat @check_immutable / FR-4 (projet d une sandbox, nommé) — Scenario « champ immuable refusé » partiel
    #[test]
    fn projet_de_sandbox_immuable() {
        let existant = sandbox(SANDBOX_YAML);
        let voulu = sandbox("project: autre\nbranch: main\n");
        match check_immutable(&existant, &voulu) {
            Err(SpecError::Immutable(cause)) => {
                assert!(cause.contains("project"), "cause: {cause}");
            }
            autre => panic!("attendu Immutable nommant project, reçu {autre:?}"),
        }
    }

    // Contrat @check_immutable (branche modifiable, CON-2)
    #[test]
    fn branche_de_sandbox_modifiable() {
        let existant = sandbox(SANDBOX_YAML);
        let voulu = sandbox("project: depot\nbranch: dev\n");
        assert_eq!(check_immutable(&existant, &voulu), Ok(()));
    }

    // Contrat @check_immutable (owner de project immuable, nommé)
    #[test]
    fn owner_de_project_immuable() {
        let existant = project("owner: alice\nrepoUrl: https://example.invalid/d.git\n");
        let voulu = project("owner: bob\nrepoUrl: https://example.invalid/d.git\n");
        match check_immutable(&existant, &voulu) {
            Err(SpecError::Immutable(cause)) => {
                assert!(cause.contains("owner"), "cause: {cause}");
            }
            autre => panic!("attendu Immutable nommant owner, reçu {autre:?}"),
        }
    }

    // Contrat @check_immutable (kinds différents refusés)
    #[test]
    fn kinds_differents_refuses() {
        let existant = sandbox(SANDBOX_YAML);
        let voulu = project("owner: alice\nrepoUrl: https://example.invalid/d.git\n");
        assert!(matches!(
            check_immutable(&existant, &voulu),
            Err(SpecError::Invalid(_))
        ));
    }
}
