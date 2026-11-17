//! `vanyline toolset list|promote` : lister les toolsets, et promouvoir un brouillon en toolset actif.
//! Contrat : `toolset_cmd.sdd`. La promotion est l'action humaine hors session (`hote#DEC-1`).

use std::io::Write;
use std::path::Path;

use clap::{Subcommand, ValueEnum};
use lymaille::ToolsetStore as _;

use crate::chat::GlobalOptions;
use crate::exit_code::Outcome;
use crate::host::layers::{Category, Layers, Level};
use crate::host::toolset_store::FileToolsets;
use crate::session_open::RENDU_MAX_CARACTERES;

/// Niveau de promotion, en ligne de commande (`--level`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum NiveauArg {
    /// `$HOME/.config/vanyline/`.
    User,
    /// `.vanyline/` du dépôt courant.
    Shared,
}

impl NiveauArg {
    /// Niveau de l'hôte correspondant.
    #[must_use]
    pub const fn niveau(self) -> Level {
        match self {
            Self::User => Level::User,
            Self::Shared => Level::Shared,
        }
    }
}

/// Sous-commandes de `toolset` (grammaire : `toolset_cmd.sdd`).
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// Liste les toolsets actifs et les brouillons, des deux niveaux
    List {
        /// Sortie JSON (tableau d'objets)
        #[arg(long)]
        json: bool,
    },
    /// Promeut un brouillon nommé en toolset actif
    Promote {
        /// Nom du brouillon
        nom: String,
        /// Niveau où le toolset devient actif
        #[arg(long, value_enum, default_value_t = NiveauArg::User)]
        level: NiveauArg,
        /// Confirme la promotion ; sans lui, les exigences sont listées et rien n'est promu
        #[arg(long)]
        yes: bool,
    },
}

/// Exécute la sous-commande `toolset` et rend son issue.
pub async fn run_toolset(commande: Commands, global: &GlobalOptions) -> Outcome {
    let home = dirs::home_dir().unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let layers = Layers::discover(&home, &cwd, global.repo.as_deref(), std::path::Path::exists);
    let mut sortie = std::io::stdout();
    match commande {
        Commands::List { json } => lister(&layers, json, &mut sortie),
        Commands::Promote { nom, level, yes } => {
            promouvoir(&layers, &nom, level.niveau(), yes, &mut sortie).await
        }
    }
}

/// Marqueur d'un brouillon absent au niveau de la promotion.
const SANS_BROUILLON: &str = "VNL-CLI-TSC-001";
/// Marqueur d'un brouillon refusé par l'hôte (lecture ou validation).
const BROUILLON_REFUSE: &str = "VNL-CLI-TSC-002";
/// Marqueur d'une promotion refusée à l'écriture.
const PROMOTION: &str = "VNL-CLI-TSC-003";

/// Nom du niveau, tel qu'il s'affiche et se passe à `--level`.
const fn nom_niveau(niveau: Level) -> &'static str {
    match niveau {
        Level::User => "user",
        Level::Shared => "shared",
    }
}

/// Une ligne de listage : niveau, état (`actif` ou `brouillon`) et nom.
struct Ligne {
    niveau: &'static str,
    etat: &'static str,
    nom: String,
}

/// `toolset list` : actifs et brouillons de tous les niveaux, avec leur niveau et leur état.
///
/// Un toolset illisible rend son erreur seule et fait échouer la commande ; les autres lignes sont listées.
pub fn lister(layers: &Layers, json: bool, sortie: &mut impl Write) -> Outcome {
    let mut lignes = Vec::new();
    let mut echec = false;
    for entree in FileToolsets::new(layers, RENDU_MAX_CARACTERES).list_active() {
        match entree {
            Ok(entree) => lignes.push(Ligne {
                niveau: nom_niveau(entree.niveau),
                etat: "actif",
                nom: entree.valeur.name().to_owned(),
            }),
            Err(erreur) => {
                ecrire_erreur(&format!("{erreur}"));
                echec = true;
            }
        }
    }
    // Les brouillons se listent par leurs dossiers : `DraftEntry` ne porte pas le niveau.
    for niveau in [Level::Shared, Level::User] {
        let Some(racine) = layers.dir(niveau, Category::ToolsetDrafts) else {
            continue;
        };
        match noms_de_dossiers(&racine) {
            Ok(noms) => lignes.extend(noms.into_iter().map(|nom| Ligne {
                niveau: nom_niveau(niveau),
                etat: "brouillon",
                nom,
            })),
            Err(erreur) => {
                ecrire_erreur(&erreur);
                echec = true;
            }
        }
    }
    if json {
        let tableau: Vec<serde_json::Value> = lignes
            .iter()
            .map(|ligne| {
                serde_json::json!({"nom": ligne.nom, "niveau": ligne.niveau, "etat": ligne.etat})
            })
            .collect();
        let _ = writeln!(sortie, "{}", serde_json::Value::Array(tableau));
    } else {
        for ligne in &lignes {
            let _ = writeln!(sortie, "{}\t{}\t{}", ligne.etat, ligne.niveau, ligne.nom);
        }
    }
    if echec {
        Outcome::Failure
    } else {
        Outcome::Success
    }
}

/// Noms des sous-dossiers de `racine` ; un dossier absent n'a aucun nom.
fn noms_de_dossiers(racine: &Path) -> Result<Vec<String>, String> {
    let lecture = match std::fs::read_dir(racine) {
        Ok(lecture) => lecture,
        Err(erreur) if erreur.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(erreur) => return Err(format!("{}: {erreur}", racine.display())),
    };
    let mut noms = Vec::new();
    for entree in lecture {
        let entree = entree.map_err(|erreur| format!("{}: {erreur}", racine.display()))?;
        let chemin = entree.path();
        if chemin.is_dir()
            && let Some(nom) = chemin.file_name().and_then(|nom| nom.to_str())
        {
            noms.push(nom.to_owned());
        }
    }
    noms.sort();
    Ok(noms)
}

/// Écrit une erreur sur la sortie d'erreur ; une écriture impossible n'a pas d'issue utile.
fn ecrire_erreur(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// `toolset promote` : sans `yes`, liste les exigences et rend `Usage` sans rien promouvoir.
///
/// # Errors
///
/// Rend `Failure` pour un brouillon absent, refusé par la validation, ou non promu à l'écriture.
pub async fn promouvoir(
    layers: &Layers,
    nom: &str,
    niveau: Level,
    yes: bool,
    sortie: &mut impl Write,
) -> Outcome {
    let stock = FileToolsets::new(layers, RENDU_MAX_CARACTERES);
    let brouillon = match stock.get_draft(nom).await {
        Ok(Some(brouillon)) => brouillon,
        Ok(None) => {
            ecrire_erreur(&format!("{SANS_BROUILLON}: aucun brouillon nommé `{nom}`"));
            return Outcome::Failure;
        }
        Err(erreur) => {
            ecrire_erreur(&format!("{BROUILLON_REFUSE}: {erreur}"));
            return Outcome::Failure;
        }
    };
    if !yes {
        let _ = writeln!(
            sortie,
            "promotion de `{nom}` au niveau {} : exigences demandées",
            nom_niveau(niveau)
        );
        for requis in brouillon.type_requirements().keys() {
            let _ = writeln!(sortie, "  type de contexte : {requis}");
        }
        for role in brouillon.roles().keys() {
            let _ = writeln!(sortie, "  rôle : {role}");
        }
        for dependance in brouillon.dependencies() {
            let _ = writeln!(sortie, "  dépendance : {dependance}");
        }
        let _ = writeln!(sortie, "relancer avec --yes pour promouvoir");
        return Outcome::Usage;
    }
    match stock.promote(niveau, nom) {
        Ok(()) => {
            let _ = writeln!(
                sortie,
                "toolset `{nom}` promu au niveau {}",
                nom_niveau(niveau)
            );
            Outcome::Success
        }
        Err(erreur) => {
            ecrire_erreur(&format!("{PROMOTION}: {erreur}"));
            Outcome::Failure
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use lymaille::{Skill, Toolset, ToolsetStore as _};
    use tempfile::TempDir;
    use vynil_core::Semver;

    use super::{NiveauArg, lister, promouvoir};
    use crate::exit_code::Outcome;
    use crate::host::layers::{Layers, Level};
    use crate::host::toolset_store::FileToolsets;

    /// Plafond de rendu des tests (même valeur que `toolset_store`).
    const CAP: u64 = 262_144;

    fn niveaux(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    fn racine(home: &Path) -> PathBuf {
        home.join(".config").join("vanyline")
    }

    /// Pose `toolset` comme brouillon au niveau utilisateur (écriture par l'agent, `hote#DEC-1`).
    async fn poser_brouillon(layers: &Layers, toolset: &Toolset) {
        FileToolsets::new(layers, CAP)
            .put_draft(toolset)
            .await
            .expect("brouillon valide");
    }

    /// Toolset valide nommé `nom`, avec un skill `revue`.
    fn toolset(nom: &str) -> Toolset {
        let base = Toolset::new(
            nom.to_owned(),
            "Outils de test".to_owned(),
            Semver::parse("1.0.0").expect("version"),
            "fn ping() { 1 }".to_owned(),
        )
        .expect("toolset valide");
        let skill = Skill::new(
            "revue".to_owned(),
            "Relit une modification".to_owned(),
            "Corps\n\nsuite\n".to_owned(),
        )
        .expect("skill valide");
        base.with_skill(skill).expect("skill attaché")
    }

    // Scenario: promotion confirmée
    #[tokio::test]
    async fn promotion_confirmee_rend_actif_et_supprime_le_brouillon() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        poser_brouillon(&layers, &toolset("outils")).await;
        let sortie = promouvoir(&layers, "outils", Level::User, true, &mut Vec::new()).await;
        assert_eq!(sortie, Outcome::Success);
        assert!(
            racine(home.path())
                .join("toolsets/outils/toolset.yaml")
                .is_file()
        );
        assert!(!racine(home.path()).join("toolsets-drafts/outils").exists());
    }

    // Scenario: sans confirmation
    #[tokio::test]
    async fn sans_confirmation_liste_les_exigences_et_rend_usage() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        poser_brouillon(&layers, &toolset("outils")).await;
        let mut sortie: Vec<u8> = Vec::new();
        assert_eq!(
            promouvoir(&layers, "outils", Level::User, false, &mut sortie).await,
            Outcome::Usage
        );
        let texte = String::from_utf8_lossy(&sortie);
        assert!(
            texte.contains("exigences"),
            "les exigences doivent être listées : {texte}"
        );
        assert!(racine(home.path()).join("toolsets-drafts/outils").exists());
        assert!(!racine(home.path()).join("toolsets/outils").exists());
    }

    // Scenario: brouillon invalide
    #[tokio::test]
    async fn brouillon_invalide_rend_echec_et_actif_inchange() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        let actif = racine(home.path()).join("toolsets/outils");
        fs::create_dir_all(&actif).expect("actif");
        fs::write(actif.join("toolset.yaml"), "nom: ancien\n").expect("actif écrit");
        let brouillon = racine(home.path()).join("toolsets-drafts/outils");
        fs::create_dir_all(&brouillon).expect("brouillon");
        fs::write(brouillon.join("toolset.yaml"), "{{{ cassé").expect("brouillon écrit");
        fs::write(brouillon.join("module.rhai"), "").expect("module");
        let avant = fs::read_to_string(actif.join("toolset.yaml")).expect("lecture");
        let sortie = promouvoir(&layers, "outils", Level::User, true, &mut Vec::new()).await;
        assert_eq!(sortie, Outcome::Failure);
        assert_eq!(
            fs::read_to_string(actif.join("toolset.yaml")).expect("lecture"),
            avant
        );
    }

    // Contrat `toolset list` : rend Success même sans toolset
    #[test]
    fn list_vide_reussit() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        assert_eq!(lister(&layers, false, &mut Vec::new()), Outcome::Success);
    }

    // Contrat `--level` : défaut user
    #[test]
    fn niveau_par_defaut_est_user() {
        assert_eq!(NiveauArg::User.niveau(), Level::User);
        assert_eq!(NiveauArg::Shared.niveau(), Level::Shared);
    }
}
