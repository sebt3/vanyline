//! `vanyline secret set <nom>` : enregistre un secret dans `$HOME/.config/vanyline/secrets/secrets.yaml`.
//! Contrat : `secret_cmd.sdd`. La valeur ne passe jamais en argument ; elle est lue sur l'entrée standard, ou saisie
//! sans écho sur un terminal.

use std::io::{IsTerminal, Read, Write};

use clap::Subcommand;

use crate::chat::GlobalOptions;
use crate::exit_code::Outcome;
use crate::host::layers::Layers;
use crate::host::secrets::{SecretValue, Secrets};

/// Marqueur d'un nom de secret invalide (code 2, rien écrit).
const NOM: &str = "VNL-CLI-SEC-001";
/// Marqueur d'une valeur vide (code 2, rien écrit).
const VIDE: &str = "VNL-CLI-SEC-002";
/// Marqueur d'une lecture de la valeur impossible.
const LECTURE: &str = "VNL-CLI-SEC-003";

/// Sous-commandes de `secret` (grammaire : `secret_cmd.sdd`).
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// Enregistre un secret ; la valeur vient de l'entrée standard, jamais d'un argument
    Set {
        /// Nom du secret (lettres, chiffres, `-`, `_`)
        nom: String,
    },
}

/// Exécute la sous-commande `secret` et rend son issue.
pub fn run_secret(commande: Commands, global: &GlobalOptions) -> Outcome {
    let home = dirs::home_dir().unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let layers = Layers::discover(&home, &cwd, global.repo.as_deref(), std::path::Path::exists);
    match commande {
        Commands::Set { nom } => {
            let terminal = std::io::stdin().is_terminal();
            set_from(&layers, &nom, std::io::stdin().lock(), terminal)
        }
    }
}

/// Vérifie le nom : non vide, lettres, chiffres, `-` et `_` seulement.
///
/// # Errors
///
/// Returns the message to print when the name is refused.
pub fn valider_nom(nom: &str) -> Result<(), String> {
    let valide = !nom.is_empty()
        && nom
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valide {
        Ok(())
    } else {
        Err(format!(
            "{NOM}: nom de secret invalide `{nom}` (lettres, chiffres, `-` et `_` seulement)"
        ))
    }
}

/// Valeur lue sur l'entrée : un saut de ligne final est retiré ; une valeur vide est refusée (code 2).
///
/// # Errors
///
/// Returns the outcome to give when the input is empty (`Usage`) or unreadable (`Failure`), after printing its cause.
pub fn lire_valeur(mut entree: impl Read) -> Result<String, Outcome> {
    let mut texte = String::new();
    if let Err(erreur) = entree.read_to_string(&mut texte) {
        ecrire_erreur(&format!(
            "{LECTURE}: lecture de la valeur impossible: {erreur}"
        ));
        return Err(Outcome::Failure);
    }
    let sans_fin = texte
        .strip_suffix("\r\n")
        .or_else(|| texte.strip_suffix('\n'))
        .unwrap_or(&texte);
    valeur_non_vide(sans_fin)
}

/// Refuse une valeur vide (code 2), sans la rendre ni la journaliser.
fn valeur_non_vide(valeur: &str) -> Result<String, Outcome> {
    if valeur.is_empty() {
        ecrire_erreur(&format!("{VIDE}: valeur vide, rien n'est enregistré"));
        Err(Outcome::Usage)
    } else {
        Ok(valeur.to_owned())
    }
}

/// Saisie sans écho sur le terminal, demandée deux fois (refus si elles diffèrent).
fn saisie_terminal() -> Result<String, Outcome> {
    let saisie = dialoguer::Password::new()
        .with_prompt("Valeur du secret")
        .with_confirmation("Confirmer la valeur", "Les deux valeurs diffèrent")
        .interact();
    match saisie {
        Ok(valeur) => valeur_non_vide(&valeur),
        Err(erreur) => {
            ecrire_erreur(&format!("{LECTURE}: saisie impossible: {erreur}"));
            Err(Outcome::Failure)
        }
    }
}

/// `secret set` avec la valeur lue sur `entree` ; `terminal` dit si la saisie doit être demandée sans écho.
pub fn set_from(layers: &Layers, nom: &str, entree: impl Read, terminal: bool) -> Outcome {
    if let Err(message) = valider_nom(nom) {
        ecrire_erreur(&message);
        return Outcome::Usage;
    }
    let valeur = if terminal {
        saisie_terminal()
    } else {
        lire_valeur(entree)
    };
    let valeur = match valeur {
        Ok(valeur) => valeur,
        Err(issue) => return issue,
    };
    match Secrets::new(layers).set(nom, SecretValue::new(valeur)) {
        Ok(()) => {
            let _ = writeln!(std::io::stdout(), "secret {nom} enregistré");
            Outcome::Success
        }
        Err(erreur) => {
            // `HostError` ne porte jamais la valeur (`host/secrets.rs`).
            ecrire_erreur(&format!("VNL-CLI-SEC-004: {erreur}"));
            Outcome::Failure
        }
    }
}

/// Message d'erreur sur la sortie d'erreur ; une écriture impossible n'a pas d'issue utile.
fn ecrire_erreur(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use tempfile::TempDir;

    use super::{set_from, valider_nom};
    use crate::exit_code::Outcome;
    use crate::host::layers::Layers;
    use crate::host::secrets::Secrets;

    fn niveaux(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    fn fichier(home: &Path) -> std::path::PathBuf {
        home.join(".config/vanyline/secrets/secrets.yaml")
    }

    fn lu(layers: &Layers, nom: &str) -> Option<String> {
        Secrets::new(layers)
            .resolve(nom)
            .ok()
            .map(|v| v.expose().to_owned())
    }

    // Scenario: valeur sur l'entrée standard
    #[test]
    fn valeur_sur_l_entree_standard() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        let sortie = set_from(&layers, "api", &b"valeur\n"[..], false);
        assert_eq!(sortie, Outcome::Success);
        assert_eq!(lu(&layers, "api").as_deref(), Some("valeur"));
        let mode = fs::metadata(fichier(home.path()))
            .expect("fichier")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // Scenario: second secret
    #[test]
    fn second_secret_conserve_le_premier() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        assert_eq!(
            set_from(&layers, "api", &b"un"[..], false),
            Outcome::Success
        );
        assert_eq!(
            set_from(&layers, "autre", &b"deux"[..], false),
            Outcome::Success
        );
        assert_eq!(lu(&layers, "api").as_deref(), Some("un"));
        assert_eq!(lu(&layers, "autre").as_deref(), Some("deux"));
    }

    // Scenario: nom invalide ou valeur vide
    #[test]
    fn nom_invalide_ou_valeur_vide_usage_et_rien_ecrit() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        assert_eq!(
            set_from(&layers, "../x", &b"valeur"[..], false),
            Outcome::Usage
        );
        assert_eq!(set_from(&layers, "api", &b""[..], false), Outcome::Usage);
        assert!(!fichier(home.path()).exists());
    }

    // Scenario: droits trop larges
    #[test]
    fn droits_trop_larges_refus_fichier_inchange() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        assert_eq!(
            set_from(&layers, "api", &b"valeur"[..], false),
            Outcome::Success
        );
        let avant = fs::read_to_string(fichier(home.path())).expect("lecture");
        fs::set_permissions(fichier(home.path()), fs::Permissions::from_mode(0o644))
            .expect("chmod");
        assert_eq!(
            set_from(&layers, "autre", &b"deux"[..], false),
            Outcome::Failure
        );
        let apres = fs::read_to_string(fichier(home.path())).expect("lecture");
        assert_eq!(avant, apres);
    }

    // Contrat valider_nom (lettres, chiffres, - et _)
    #[test]
    fn noms_acceptes_et_refuses() {
        assert!(valider_nom("api-key_2").is_ok());
        assert!(valider_nom("").is_err());
        assert!(valider_nom("../x").is_err());
        assert!(valider_nom("a b").is_err());
    }
}
