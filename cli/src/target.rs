//! Owner, namespace et réglages utilisateur des commandes d'objets.
//! Contrat : `target.sdd`.

use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

use crate::host::files;
use crate::host::layers::Layers;

/// Nom du fichier de réglages, au niveau utilisateur (`target.sdd`).
const CONFIG: &str = "config.yaml";
/// Namespace par défaut de Kubernetes, quand aucune source ne le fournit (`FR-8`).
const NAMESPACE_PAR_DEFAUT: &str = "default";
/// Marqueur d'un fichier de réglages invalide.
const SETTINGS: &str = "VNL-CLI-TGT-001";
/// Marqueur d'un Owner absent.
const OWNER_ABSENT: &str = "VNL-CLI-TGT-002";
/// Marqueur d'un namespace d'Owner refusé.
const NAMESPACE_REFUSE: &str = "VNL-CLI-TGT-003";

/// Forme du fichier de réglages : seule la section `defaults` est lue.
#[derive(Deserialize, Default)]
struct Fichier {
    #[serde(default)]
    defaults: Defaut,
}

/// Section `defaults` du fichier de réglages.
#[derive(Deserialize, Default)]
struct Defaut {
    owner: Option<String>,
    namespace: Option<String>,
}

/// Première valeur non vide parmi les sources, dans l'ordre donné.
fn premiere_non_vide<'a>(sources: &[Option<&'a str>]) -> Option<&'a str> {
    sources
        .iter()
        .flatten()
        .copied()
        .find(|valeur| !valeur.is_empty())
}

/// Réglages du niveau utilisateur (`$HOME/.config/vanyline/config.yaml`), `defaults.*` (`target.sdd`, `@Settings`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// `defaults.owner`.
    pub owner: Option<String>,
    /// `defaults.namespace`.
    pub namespace: Option<String>,
}

/// Erreur de résolution des réglages ou de l'Owner (`target.sdd`, `@TargetError`).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TargetError {
    /// Usage invalide : aucun Owner, ou nom d'Owner refusé (code 2).
    #[error("{0}")]
    Usage(String),
    /// Fichier de réglages illisible ou invalide, avec son chemin et sa cause.
    #[error("{chemin}: {cause}")]
    Settings {
        /// Chemin du fichier de réglages.
        chemin: PathBuf,
        /// Cause, avec son marqueur.
        cause: String,
    },
    /// Namespace d'Owner invalide, refusé avant tout appel au cluster (`BR-2`).
    #[error("{0}")]
    Namespace(String),
}

impl Settings {
    /// Lit le niveau utilisateur seulement ; fichier absent ⇒ réglages vides.
    ///
    /// # Errors
    ///
    /// Returns [`TargetError::Settings`] when the file exists but cannot be read or is not valid.
    pub fn load(layers: &Layers) -> Result<Self, TargetError> {
        let chemin = layers.user().join(CONFIG);
        let texte = match files::read_text(&chemin) {
            Ok(Some(texte)) => texte,
            Ok(None) => return Ok(Self::default()),
            Err(erreur) => {
                return Err(TargetError::Settings {
                    chemin,
                    cause: erreur.to_string(),
                });
            }
        };
        if texte.trim().is_empty() {
            return Ok(Self::default());
        }
        let fichier: Fichier =
            yaml_serde::from_str(&texte).map_err(|erreur| TargetError::Settings {
                chemin,
                cause: format!("{SETTINGS}: {erreur}"),
            })?;
        Ok(Self {
            owner: fichier.defaults.owner,
            namespace: fichier.defaults.namespace,
        })
    }
}

/// Owner d'une commande : `--owner`, puis `defaults.owner`, sinon `TargetError::Usage` (`AC-7`).
///
/// # Errors
///
/// Returns [`TargetError::Usage`] when neither source gives an Owner.
pub fn resolve_owner(option: Option<&str>, settings: &Settings) -> Result<String, TargetError> {
    premiere_non_vide(&[option, settings.owner.as_deref()])
        .map(str::to_owned)
        .ok_or_else(|| {
            TargetError::Usage(format!(
                "{OWNER_ABSENT}: owner requis : passer --owner <nom> ou définir defaults.owner dans le fichier de réglages utilisateur"
            ))
        })
}

/// Namespace d'application : `--namespace`, puis `defaults.namespace`, puis le namespace du kubeconfig (`FR-8`).
#[must_use]
pub fn resolve_namespace(
    option: Option<&str>,
    settings: &Settings,
    kubeconfig: Option<&str>,
) -> String {
    premiere_non_vide(&[option, settings.namespace.as_deref(), kubeconfig])
        .unwrap_or(NAMESPACE_PAR_DEFAUT)
        .to_owned()
}

/// Namespace de l'Owner, calculé par le nommage de `vanyline-crds` (`BR-2`).
///
/// # Errors
///
/// Returns [`TargetError::Namespace`] when the name would not be a valid DNS label.
pub fn owner_namespace(app_namespace: &str, owner: &str) -> Result<String, TargetError> {
    vanyline_crds::owner_namespace_name(app_namespace, owner)
        .map_err(|cause| TargetError::Namespace(format!("{NAMESPACE_REFUSE}: {cause}")))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Settings, TargetError, owner_namespace, resolve_namespace, resolve_owner};
    use crate::host::layers::Layers;

    fn reglages(owner: Option<&str>, namespace: Option<&str>) -> Settings {
        Settings {
            owner: owner.map(str::to_owned),
            namespace: namespace.map(str::to_owned),
        }
    }

    /// Scenario: `owner par l'option puis par le réglage`.
    #[test]
    fn owner_par_l_option_puis_par_le_reglage() {
        let settings = reglages(Some("b"), None);
        assert_eq!(resolve_owner(Some("a"), &settings), Ok("a".to_owned()));
        assert_eq!(resolve_owner(None, &settings), Ok("b".to_owned()));
    }

    /// Scenario: `aucun owner` : l'erreur d'usage nomme `--owner` et `defaults.owner`.
    #[test]
    fn aucun_owner_nomme_les_deux_moyens() {
        match resolve_owner(None, &Settings::default()) {
            Err(TargetError::Usage(message)) => {
                assert!(message.contains("--owner"), "{message}");
                assert!(message.contains("defaults.owner"), "{message}");
            }
            autre => panic!("une erreur d'usage est attendue, obtenu {autre:?}"),
        }
    }

    /// Scenario: `namespace par ordre de priorité` : option, puis réglage, puis kubeconfig.
    #[test]
    fn namespace_par_ordre_de_priorite() {
        let avec_reglage = reglages(None, Some("y"));
        assert_eq!(resolve_namespace(Some("x"), &avec_reglage, Some("z")), "x");
        assert_eq!(resolve_namespace(None, &avec_reglage, Some("z")), "y");
        assert_eq!(
            resolve_namespace(None, &Settings::default(), Some("z")),
            "z"
        );
    }

    /// Sans option, sans réglage ni kubeconfig nommé : le namespace par défaut de Kubernetes (choix documenté).
    #[test]
    fn namespace_sans_source_est_default() {
        assert_eq!(
            resolve_namespace(None, &Settings::default(), None),
            "default"
        );
    }

    /// Scenario: `nom d'owner invalide` : refus avant tout appel au cluster.
    #[test]
    fn nom_d_owner_invalide_est_refuse() {
        let trop_long = "a".repeat(60);
        assert!(matches!(
            owner_namespace("vanyline-app", &trop_long),
            Err(TargetError::Namespace(_))
        ));
        assert!(matches!(
            owner_namespace("vanyline-app", ""),
            Err(TargetError::Namespace(_))
        ));
        assert!(matches!(
            owner_namespace("vanyline-app", "Alice"),
            Err(TargetError::Namespace(_))
        ));
    }

    #[test]
    fn namespace_d_owner_valide_est_calcule_par_crds() {
        assert_eq!(
            owner_namespace("vanyline-app", "alice"),
            Ok("vanyline-app-alice".to_owned())
        );
    }

    /// Scenario : lecture du seul niveau utilisateur. Un fichier au niveau partagé n'est jamais lu.
    #[test]
    fn seul_le_niveau_utilisateur_est_lu() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let depot = tempfile::tempdir().expect("dépôt temporaire");
        let partage = depot.path().join(".vanyline");
        std::fs::create_dir_all(&partage).expect("niveau partagé");
        std::fs::write(partage.join("config.yaml"), "defaults:\n  owner: intrus\n")
            .expect("écriture du niveau partagé");
        let layers = Layers::discover(home.path(), Path::new("/"), Some(depot.path()), |_| false);

        let lus = Settings::load(&layers).expect("aucun fichier utilisateur : réglages vides");

        assert_eq!(lus, Settings::default());
    }

    /// Scenario : réglages lus depuis `defaults` du niveau utilisateur.
    #[test]
    fn reglages_utilisateur_lus() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let dossier = home.path().join(".config").join("vanyline");
        std::fs::create_dir_all(&dossier).expect("niveau utilisateur");
        std::fs::write(
            dossier.join("config.yaml"),
            "defaults:\n  owner: alice\n  namespace: vanyline-app\n",
        )
        .expect("config.yaml utilisateur");
        let layers = Layers::discover(home.path(), home.path(), None, |_| false);

        let lus = Settings::load(&layers).expect("réglages valides");

        assert_eq!(lus, reglages(Some("alice"), Some("vanyline-app")));
    }

    /// Fichier de réglages invalide : `TargetError::Settings` avec son chemin.
    #[test]
    fn reglages_invalides_nomment_le_chemin() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let dossier = home.path().join(".config").join("vanyline");
        std::fs::create_dir_all(&dossier).expect("niveau utilisateur");
        std::fs::write(dossier.join("config.yaml"), "defaults: [pas, une, map\n")
            .expect("config.yaml invalide");
        let layers = Layers::discover(home.path(), home.path(), None, |_| false);

        match Settings::load(&layers) {
            Err(TargetError::Settings { chemin, cause }) => {
                assert!(chemin.ends_with("config.yaml"), "{}", chemin.display());
                assert!(!cause.is_empty());
            }
            autre => panic!("une erreur de réglages est attendue, obtenu {autre:?}"),
        }
    }
}
