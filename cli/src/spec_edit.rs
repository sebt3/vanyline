//! Spécifications complètes des CRD Project et Sandbox : lecture, refus des modifications immuables, application
//! (`apply`) et édition (`edit`). Contrat : `spec_edit.sdd`. Le client K8s passe par le seam [`SpecCluster`].

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use serde_json::Value;
use vanyline_crds::{ProjectSpec, Sandbox, SandboxSpec};

/// Marqueur d'une spécification illisible, inconnue ou d'un type qui ne correspond pas à la cible.
const INVALID: &str = "VNL-CLI-SPC-001";
/// Marqueur d'un champ immuable modifié (refus avant tout appel au cluster).
const IMMUTABLE: &str = "VNL-CLI-SPC-002";
/// Marqueur d'une édition impossible faute d'éditeur (`$EDITOR`).
const USAGE: &str = "VNL-CLI-SPC-003";
/// Marqueur d'une condition de la sandbox qui signale un redémarrage requis.
const RESTART_PENDING: &str = "RestartPending";

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
    /// Édition impossible : aucun éditeur configuré (`$EDITOR`).
    #[error("{0}")]
    Usage(String),
}

/// Issue d'un `apply` ou d'un `edit` (`spec_edit.sdd`, `@ApplyOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// L'objet n'existait pas : il est créé.
    Created,
    /// L'objet a été mis à jour ; `redemarrage_requis` quand l'état de la sandbox le porte (jamais de redémarrage par la CLI).
    Updated {
        /// Vrai si la condition `RestartPending` est posée.
        redemarrage_requis: bool,
    },
    /// La spécification est identique : aucune écriture.
    Unchanged,
}

/// Objet lu dans le cluster : sa spécification et son signalement de redémarrage.
#[derive(Debug, Clone)]
pub struct Lu {
    /// Spécification complète lue.
    pub spec: ObjectSpec,
    /// Vrai si la condition `RestartPending` est posée (sandbox uniquement).
    pub redemarrage_requis: bool,
}

/// Futur boxé rendu par le seam (même patron que `host/owner_auth.rs`).
pub type Futur<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Lectures et écritures du cluster nécessaires à `apply` et `edit` (seam). Implémenté par le client K8s de la lib.
pub trait SpecCluster: Send + Sync {
    /// Lit l'objet `nom` de l'Owner `owner` ; `None` s'il n'existe pas.
    fn lire<'a>(
        &'a self,
        kind: Kind,
        owner: &'a str,
        nom: &'a str,
    ) -> Futur<'a, Result<Option<Lu>, SpecError>>;
    /// Applique la spécification complète `voulu` à l'objet `nom` (server-side apply) et rend l'objet écrit.
    fn appliquer<'a>(
        &'a self,
        kind: Kind,
        owner: &'a str,
        nom: &'a str,
        voulu: &'a ObjectSpec,
    ) -> Futur<'a, Result<Lu, SpecError>>;
}

/// Éditeur de fichier (seam de `edit` : `$EDITOR` en production, un faux en test).
pub trait Editor: Send + Sync {
    /// Ouvre `fichier` dans l'éditeur et rend quand l'édition est terminée.
    ///
    /// # Errors
    ///
    /// Returns [`SpecError::Usage`] when no editor is configured.
    fn editer(&self, fichier: &Path) -> Result<(), SpecError>;
}

/// `$EDITOR` lu au démarrage de la commande ; absent ou vide : [`SpecError::Usage`] à l'édition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvEditor {
    commande: Option<String>,
}

impl EnvEditor {
    /// Éditeur désigné par `$EDITOR`.
    #[must_use]
    pub fn depuis_env() -> Self {
        Self::avec(std::env::var("EDITOR").ok())
    }

    /// Éditeur désigné par `commande` (`None` ou vide : aucun éditeur).
    #[must_use]
    pub fn avec(commande: Option<String>) -> Self {
        Self {
            commande: commande.filter(|texte| !texte.trim().is_empty()),
        }
    }
}

impl Editor for EnvEditor {
    fn editer(&self, fichier: &Path) -> Result<(), SpecError> {
        let Some(commande) = &self.commande else {
            return Err(SpecError::Usage(format!(
                "{USAGE}: $EDITOR absent : définir EDITOR pour éditer la spécification"
            )));
        };
        let mut morceaux = commande.split_whitespace();
        let programme = morceaux
            .next()
            .ok_or_else(|| SpecError::Usage(format!("{USAGE}: $EDITOR vide")))?;
        let statut = std::process::Command::new(programme)
            .args(morceaux)
            .arg(fichier)
            .status()
            .map_err(|erreur| {
                SpecError::Invalid(format!(
                    "{INVALID}: éditeur `{programme}` non lancé: {erreur}"
                ))
            })?;
        if statut.success() {
            Ok(())
        } else {
            Err(SpecError::Invalid(format!(
                "{INVALID}: éditeur `{programme}` en échec"
            )))
        }
    }
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
pub fn check_immutable(
    nom_existant: &str,
    existant: &ObjectSpec,
    nom_voulu: &str,
    voulu: &ObjectSpec,
) -> Result<(), SpecError> {
    if nom_existant != nom_voulu {
        return Err(SpecError::Immutable(format!(
            "{IMMUTABLE}: champ `nom` immuable"
        )));
    }
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

/// Vrai si la Sandbox porte la condition `RestartPending` à `True` (`vanyline.sdd`).
#[must_use]
pub fn redemarrage_requis(sandbox: &Sandbox) -> bool {
    sandbox.status.as_ref().is_some_and(|status| {
        status
            .conditions
            .iter()
            .any(|c| c.type_ == RESTART_PENDING && c.status == "True")
    })
}

/// Lit un fichier `apply -f` : un `name` de premier niveau et la spécification complète du reste.
///
/// # Errors
///
/// Returns [`SpecError::Invalid`] when `name` is missing or not a string, or when the rest is not a valid spec.
pub fn lire_fichier_apply(kind: Kind, texte: &str) -> Result<(String, ObjectSpec), SpecError> {
    let mut valeur: Value = yaml_serde::from_str(texte)
        .map_err(|erreur| SpecError::Invalid(format!("{INVALID}: fichier illisible: {erreur}")))?;
    let nom = valeur
        .as_object_mut()
        .and_then(|objet| objet.remove("name"))
        .ok_or_else(|| SpecError::Invalid(format!("{INVALID}: champ `name` absent du fichier")))?;
    let nom = nom
        .as_str()
        .ok_or_else(|| SpecError::Invalid(format!("{INVALID}: champ `name` non textuel")))?
        .to_owned();
    let reste = serde_json::to_string(&valeur).map_err(|erreur| invalide(&erreur))?;
    Ok((nom, parse_spec(kind, &reste)?))
}

/// Applique `voulu` à l'objet `nom` de l'Owner `owner` (`spec_edit.sdd`, `@apply`).
///
/// Lit d'abord l'existant : un champ immuable modifié est refusé AVANT toute écriture ; une spécification identique
/// rend `Unchanged` sans écrire.
///
/// # Errors
///
/// Returns [`SpecError::Immutable`] for a changed immutable field, [`SpecError::Invalid`] for a cluster failure.
pub async fn apply(
    client: &impl SpecCluster,
    kind: Kind,
    owner: &str,
    nom: &str,
    voulu: &ObjectSpec,
) -> Result<ApplyOutcome, SpecError> {
    match client.lire(kind, owner, nom).await? {
        None => {
            client.appliquer(kind, owner, nom, voulu).await?;
            Ok(ApplyOutcome::Created)
        }
        Some(lu) => {
            check_immutable(nom, &lu.spec, nom, voulu)?;
            if valeur(&lu.spec) == valeur(voulu) {
                return Ok(ApplyOutcome::Unchanged);
            }
            let ecrit = client.appliquer(kind, owner, nom, voulu).await?;
            Ok(ApplyOutcome::Updated {
                redemarrage_requis: ecrit.redemarrage_requis,
            })
        }
    }
}

/// `edit` avec le dossier temporaire du système (`spec_edit.sdd`, `@edit`).
///
/// # Errors
///
/// Comme [`editer_dans`].
pub async fn edit(
    client: &impl SpecCluster,
    kind: Kind,
    owner: &str,
    nom: &str,
    editeur: &impl Editor,
) -> Result<ApplyOutcome, SpecError> {
    editer_dans(&std::env::temp_dir(), client, kind, owner, nom, editeur).await
}

/// `edit` avec le fichier temporaire dans `dossier` : la spécification existante est ouverte dans `editeur`, puis
/// validée et appliquée. En cas d'erreur après l'édition, le fichier est CONSERVÉ et son chemin est rendu.
///
/// # Errors
///
/// Returns [`SpecError::Usage`] without editor, [`SpecError::Invalid`] when the object does not exist or the saved text is
/// not valid (the path of the kept file is in the cause).
pub async fn editer_dans(
    dossier: &Path,
    client: &impl SpecCluster,
    kind: Kind,
    owner: &str,
    nom: &str,
    editeur: &impl Editor,
) -> Result<ApplyOutcome, SpecError> {
    let existant = client
        .lire(kind, owner, nom)
        .await?
        .ok_or_else(|| SpecError::Invalid(format!("{INVALID}: objet `{nom}` introuvable")))?;
    let chemin = dossier.join(format!(
        "vanyline-edit-{}-{}.yaml",
        if kind == Kind::Project {
            "project"
        } else {
            "sandbox"
        },
        std::process::id()
    ));
    let texte = yaml_serde::to_string(&valeur(&existant.spec))
        .map_err(|erreur| SpecError::Invalid(format!("{INVALID}: sérialisation: {erreur}")))?;
    std::fs::write(&chemin, texte).map_err(|erreur| {
        SpecError::Invalid(format!(
            "{INVALID}: écriture de {}: {erreur}",
            chemin.display()
        ))
    })?;
    if let Err(erreur) = editeur.editer(&chemin) {
        if matches!(erreur, SpecError::Usage(_)) {
            let _ = std::fs::remove_file(&chemin);
        }
        return Err(erreur);
    }
    let voulu = match std::fs::read_to_string(&chemin)
        .map_err(|erreur| SpecError::Invalid(format!("{INVALID}: lecture: {erreur}")))
        .and_then(|texte| parse_spec(kind, &texte))
    {
        Ok(voulu) => voulu,
        Err(erreur) => return Err(conserve(erreur, &chemin)),
    };
    match apply(client, kind, owner, nom, &voulu).await {
        Ok(issue) => {
            let _ = std::fs::remove_file(&chemin);
            Ok(issue)
        }
        Err(erreur) => Err(conserve(erreur, &chemin)),
    }
}

/// Valeur JSON d'une spécification, pour comparer deux spécifications sans tenir compte de leur type Rust.
fn valeur(spec: &ObjectSpec) -> Value {
    match spec {
        ObjectSpec::Project(p) => serde_json::to_value(p).unwrap_or(Value::Null),
        ObjectSpec::Sandbox(s) => serde_json::to_value(s).unwrap_or(Value::Null),
    }
}

/// Erreur d'une spécification refusée après l'édition : le chemin du fichier conservé est ajouté à la cause.
fn conserve(erreur: SpecError, chemin: &Path) -> SpecError {
    let suffixe = format!(" (fichier conservé : {})", chemin.display());
    match erreur {
        SpecError::Invalid(cause) => SpecError::Invalid(format!("{cause}{suffixe}")),
        SpecError::Immutable(cause) => SpecError::Immutable(format!("{cause}{suffixe}")),
        autre @ SpecError::Usage(_) => autre,
    }
}

/// Lecture d'une CR : `NotFound` est une absence, toute autre erreur une cause sans jeton ni URL.
fn lecture<T>(resultat: Result<T, vanyline_lib::VnyError>) -> Result<Option<T>, SpecError> {
    match resultat {
        Ok(valeur) => Ok(Some(valeur)),
        Err(vanyline_lib::VnyError::K8sNotFound(_)) => Ok(None),
        Err(erreur) => Err(SpecError::Invalid(format!(
            "VNL-CLI-SPC-004: lecture refusée: {erreur}"
        ))),
    }
}

impl SpecCluster for vanyline_lib::k8s::VnlK8sClient {
    fn lire<'a>(
        &'a self,
        kind: Kind,
        owner: &'a str,
        nom: &'a str,
    ) -> Futur<'a, Result<Option<Lu>, SpecError>> {
        Box::pin(async move {
            match kind {
                Kind::Project => Ok(lecture(self.get_project(owner, nom).await)?.map(|p| Lu {
                    spec: ObjectSpec::Project(p.spec),
                    redemarrage_requis: false,
                })),
                Kind::Sandbox => Ok(lecture(self.get_sandbox(owner, nom).await)?.map(|s| {
                    let redemarrage_requis = redemarrage_requis(&s);
                    Lu {
                        spec: ObjectSpec::Sandbox(s.spec),
                        redemarrage_requis,
                    }
                })),
            }
        })
    }

    fn appliquer<'a>(
        &'a self,
        kind: Kind,
        owner: &'a str,
        nom: &'a str,
        voulu: &'a ObjectSpec,
    ) -> Futur<'a, Result<Lu, SpecError>> {
        Box::pin(async move {
            match (kind, voulu) {
                (Kind::Project, ObjectSpec::Project(spec)) => {
                    let p = self
                        .apply_project(owner, nom, spec)
                        .await
                        .map_err(|erreur| {
                            SpecError::Invalid(format!(
                                "VNL-CLI-SPC-005: écriture refusée: {erreur}"
                            ))
                        })?;
                    Ok(Lu {
                        spec: ObjectSpec::Project(p.spec),
                        redemarrage_requis: false,
                    })
                }
                (Kind::Sandbox, ObjectSpec::Sandbox(spec)) => {
                    let s = self
                        .apply_sandbox(owner, nom, spec)
                        .await
                        .map_err(|erreur| {
                            SpecError::Invalid(format!(
                                "VNL-CLI-SPC-005: écriture refusée: {erreur}"
                            ))
                        })?;
                    let redemarrage_requis = redemarrage_requis(&s);
                    Ok(Lu {
                        spec: ObjectSpec::Sandbox(s.spec),
                        redemarrage_requis,
                    })
                }
                _ => Err(SpecError::Invalid(format!(
                    "{INVALID}: type de spécification incompatible"
                ))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::{
        ApplyOutcome, Editor, EnvEditor, Futur, Kind, Lu, ObjectSpec, SpecCluster, SpecError,
        apply, check_immutable, editer_dans, lire_fichier_apply, parse_spec,
    };

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
        match check_immutable("depot", &existant, "depot", &voulu) {
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
        assert_eq!(check_immutable("depot", &existant, "depot", &voulu), Ok(()));
    }

    // Contrat @check_immutable (owner de project immuable, nommé)
    #[test]
    fn owner_de_project_immuable() {
        let existant = project("owner: alice\nrepoUrl: https://example.invalid/d.git\n");
        let voulu = project("owner: bob\nrepoUrl: https://example.invalid/d.git\n");
        match check_immutable("depot", &existant, "depot", &voulu) {
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
            check_immutable("depot", &existant, "depot", &voulu),
            Err(SpecError::Invalid(_))
        ));
    }

    /// Faux cluster : un objet optionnel, un compteur d'écritures, et le signalement de redémarrage servi.
    struct Faux {
        existant: Mutex<Option<Lu>>,
        redemarrage: bool,
        ecritures: Mutex<usize>,
    }

    impl Faux {
        fn vide() -> Self {
            Self {
                existant: Mutex::new(None),
                redemarrage: false,
                ecritures: Mutex::new(0),
            }
        }
        fn avec(lu: Lu, redemarrage: bool) -> Self {
            Self {
                existant: Mutex::new(Some(lu)),
                redemarrage,
                ecritures: Mutex::new(0),
            }
        }
        fn ecrits(&self) -> usize {
            *self.ecritures.lock().expect("compteur")
        }
    }

    impl SpecCluster for Faux {
        fn lire<'a>(
            &'a self,
            _kind: Kind,
            _owner: &'a str,
            _nom: &'a str,
        ) -> Futur<'a, Result<Option<Lu>, SpecError>> {
            let valeur = self.existant.lock().expect("objet").clone();
            Box::pin(async move { Ok(valeur) })
        }
        fn appliquer<'a>(
            &'a self,
            _kind: Kind,
            _owner: &'a str,
            _nom: &'a str,
            voulu: &'a ObjectSpec,
        ) -> Futur<'a, Result<Lu, SpecError>> {
            Box::pin(async move {
                let mut compteur = self.ecritures.lock().expect("compteur");
                *compteur = compteur.saturating_add(1);
                drop(compteur);
                let lu = Lu {
                    spec: voulu.clone(),
                    redemarrage_requis: self.redemarrage,
                };
                *self.existant.lock().expect("objet") = Some(lu.clone());
                Ok(lu)
            })
        }
    }

    /// Faux éditeur : `Some(texte)` remplace le fichier, `None` le laisse tel quel.
    struct Editeur {
        contenu: Option<String>,
    }

    impl Editor for Editeur {
        fn editer(&self, fichier: &Path) -> Result<(), SpecError> {
            if let Some(texte) = &self.contenu {
                std::fs::write(fichier, texte).expect("écriture du faux éditeur");
            }
            Ok(())
        }
    }

    fn sandbox_avec(suspended: bool) -> ObjectSpec {
        sandbox(&format!(
            "project: depot\nbranch: main\nsuspended: {suspended}\n"
        ))
    }

    fn lu(spec: ObjectSpec) -> Lu {
        Lu {
            spec,
            redemarrage_requis: false,
        }
    }

    // Scenario: apply crée puis met à jour
    #[tokio::test]
    async fn apply_cree_puis_met_a_jour() {
        let client = Faux::vide();
        let v1 = sandbox_avec(false);
        let v2 = sandbox_avec(true);
        assert_eq!(
            apply(&client, Kind::Sandbox, "alice", "depot-main", &v1).await,
            Ok(ApplyOutcome::Created)
        );
        assert_eq!(
            apply(&client, Kind::Sandbox, "alice", "depot-main", &v2).await,
            Ok(ApplyOutcome::Updated {
                redemarrage_requis: false
            })
        );
        assert_eq!(
            apply(&client, Kind::Sandbox, "alice", "depot-main", &v2).await,
            Ok(ApplyOutcome::Unchanged)
        );
        assert_eq!(
            client.ecrits(),
            2,
            "un troisième apply identique n'écrit pas"
        );
    }

    // Scenario: champ immuable refusé (projet de sandbox) : aucune requête d'écriture
    #[tokio::test]
    async fn champ_immuable_refuse_avant_ecriture() {
        let client = Faux::avec(lu(sandbox_avec(false)), false);
        let voulu = sandbox("project: autre\nbranch: main\n");
        match apply(&client, Kind::Sandbox, "alice", "depot-main", &voulu).await {
            Err(SpecError::Immutable(cause)) => {
                assert!(cause.contains("project"), "cause: {cause}");
            }
            autre => panic!("attendu Immutable nommant project, reçu {autre:?}"),
        }
        assert_eq!(client.ecrits(), 0);
    }

    // Scenario: redémarrage requis signalé ; la CLI n'appelle que l'écriture, jamais un redémarrage
    #[tokio::test]
    async fn redemarrage_requis_signale() {
        let client = Faux::avec(lu(sandbox_avec(false)), true);
        assert_eq!(
            apply(
                &client,
                Kind::Sandbox,
                "alice",
                "depot-main",
                &sandbox_avec(true)
            )
            .await,
            Ok(ApplyOutcome::Updated {
                redemarrage_requis: true
            })
        );
        assert_eq!(client.ecrits(), 1);
    }

    // Scenario: edit conserve la saisie invalide, chemin du fichier rendu
    #[tokio::test]
    async fn edit_conserve_la_saisie_invalide() {
        let dossier = TempDir::new().expect("tempdir");
        let client = Faux::avec(lu(sandbox_avec(false)), false);
        let editeur = Editeur {
            contenu: Some("project: [\n".to_owned()),
        };
        match editer_dans(
            dossier.path(),
            &client,
            Kind::Sandbox,
            "alice",
            "depot-main",
            &editeur,
        )
        .await
        {
            Err(SpecError::Invalid(cause)) => {
                let restes: Vec<_> = std::fs::read_dir(dossier.path())
                    .expect("dossier")
                    .collect();
                assert_eq!(restes.len(), 1, "le fichier temporaire est conservé");
                let nom = restes[0].as_ref().expect("entrée").file_name();
                assert!(
                    cause.contains(nom.to_string_lossy().as_ref()),
                    "cause: {cause}"
                );
            }
            autre => panic!("attendu Invalid avec le chemin, reçu {autre:?}"),
        }
        assert_eq!(client.ecrits(), 0);
    }

    // Scenario: edit sans EDITOR
    #[tokio::test]
    async fn edit_sans_editor_est_une_erreur_d_usage() {
        let dossier = TempDir::new().expect("tempdir");
        let client = Faux::avec(lu(sandbox_avec(false)), false);
        let editeur = EnvEditor::avec(None);
        assert!(matches!(
            editer_dans(
                dossier.path(),
                &client,
                Kind::Sandbox,
                "alice",
                "depot-main",
                &editeur
            )
            .await,
            Err(SpecError::Usage(_))
        ));
    }

    // Must : un edit sans changement est Unchanged, sans écriture
    #[tokio::test]
    async fn edit_sans_changement_est_unchanged() {
        let dossier = TempDir::new().expect("tempdir");
        let client = Faux::avec(lu(sandbox_avec(false)), false);
        let editeur = Editeur { contenu: None };
        assert_eq!(
            editer_dans(
                dossier.path(),
                &client,
                Kind::Sandbox,
                "alice",
                "depot-main",
                &editeur
            )
            .await,
            Ok(ApplyOutcome::Unchanged)
        );
        assert_eq!(client.ecrits(), 0);
    }

    // Contrat @edit : une modification saisie est appliquée (Updated)
    #[tokio::test]
    async fn edit_modifie_puis_applique() {
        let dossier = TempDir::new().expect("tempdir");
        let client = Faux::avec(lu(sandbox_avec(false)), false);
        let editeur = Editeur {
            contenu: Some("project: depot\nbranch: main\nsuspended: true\n".to_owned()),
        };
        assert_eq!(
            editer_dans(
                dossier.path(),
                &client,
                Kind::Sandbox,
                "alice",
                "depot-main",
                &editeur
            )
            .await,
            Ok(ApplyOutcome::Updated {
                redemarrage_requis: false
            })
        );
        assert_eq!(client.ecrits(), 1);
    }

    // Contrat @edit : objet absent (lecture préalable)
    #[tokio::test]
    async fn edit_objet_absent_est_refuse() {
        let dossier = TempDir::new().expect("tempdir");
        let client = Faux::vide();
        let editeur = Editeur { contenu: None };
        assert!(matches!(
            editer_dans(
                dossier.path(),
                &client,
                Kind::Sandbox,
                "alice",
                "depot-main",
                &editeur
            )
            .await,
            Err(SpecError::Invalid(_))
        ));
    }

    // Contrat (FR-4, CON-2) : le nom est immuable, nommé
    #[test]
    fn nom_immuable_nomme() {
        let spec = sandbox_avec(false);
        match check_immutable("depot-main", &spec, "depot-autre", &spec) {
            Err(SpecError::Immutable(cause)) => assert!(cause.contains("nom"), "cause: {cause}"),
            autre => panic!("attendu Immutable nommant nom, reçu {autre:?}"),
        }
    }

    // Contrat `apply -f` : `name` de premier niveau, spécification du reste
    #[test]
    fn fichier_apply_extrait_le_nom() {
        let (nom, spec) = lire_fichier_apply(
            Kind::Sandbox,
            "name: depot-main\nproject: depot\nbranch: main\n",
        )
        .expect("fichier valide");
        assert_eq!(nom, "depot-main");
        assert!(matches!(spec, ObjectSpec::Sandbox(_)));
        assert!(matches!(
            lire_fichier_apply(Kind::Sandbox, "project: depot\nbranch: main\n"),
            Err(SpecError::Invalid(_))
        ));
    }
}
