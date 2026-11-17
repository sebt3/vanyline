//! `vanyline config check` : valide les fichiers de configuration des deux niveaux par les stores de l'hôte.
//! Contrat : `config_cmd.sdd`. N'écrit rien et ne résout aucun secret.

use std::collections::BTreeSet;
use std::io::Write;

use clap::Subcommand;
use lymaille::{DefaultSecretDetector, DraftStore as _};

use crate::chat::GlobalOptions;
use crate::exit_code::Outcome;
use crate::host::catalog_store::FileCatalog;
use crate::host::context_store::FileContexts;
use crate::host::layers::Layers;
use crate::host::toolset_store::FileToolsets;
use crate::session_open::RENDU_MAX_CARACTERES;

/// Sous-commandes de `config` (grammaire : `config_cmd.sdd`).
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// Valide les fichiers de configuration des deux niveaux, et les références entre eux
    Check,
}

/// Exécute la sous-commande `config` et rend son issue.
pub async fn run_config(commande: Commands, global: &GlobalOptions) -> Outcome {
    let home = dirs::home_dir().unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let layers = Layers::discover(&home, &cwd, global.repo.as_deref(), std::path::Path::exists);
    match commande {
        Commands::Check => {
            let problemes = verifier(&layers).await;
            let mut sortie = std::io::stdout();
            if problemes.is_empty() {
                let _ = writeln!(sortie, "configuration valide");
                Outcome::Success
            } else {
                for probleme in &problemes {
                    let _ = writeln!(sortie, "{probleme}");
                }
                Outcome::Failure
            }
        }
    }
}

/// Lignes de problèmes (catégorie, chemin ou nom, cause). Vide = configuration valide.
///
/// Lit chaque catégorie des deux niveaux par les stores de l'hôte, puis croise les références : agent vers profil,
/// profil vers provider. Un provider qui porte une valeur ressemblant à un secret est signalé. Ne résout aucun secret.
pub async fn verifier(layers: &Layers) -> Vec<String> {
    let mut problemes = Vec::new();
    let catalogue = FileCatalog::new(layers);

    let mut providers = BTreeSet::new();
    for entree in catalogue.list_providers() {
        match entree {
            Ok(entree) => {
                let provider = entree.valeur;
                if !provider.check(&DefaultSecretDetector::default()) {
                    problemes.push(format!(
                        "providers: `{}` porte une valeur ressemblant à un secret",
                        provider.name()
                    ));
                }
                providers.insert(provider.name().to_owned());
            }
            Err(erreur) => problemes.push(format!("providers: {erreur}")),
        }
    }

    let mut profils = BTreeSet::new();
    for entree in catalogue.list_profiles() {
        match entree {
            Ok(entree) => {
                let profil = entree.valeur;
                if !providers.contains(profil.provider()) {
                    problemes.push(format!(
                        "profiles: `{}` référence le provider inconnu `{}`",
                        profil.name(),
                        profil.provider()
                    ));
                }
                profils.insert(profil.name().to_owned());
            }
            Err(erreur) => problemes.push(format!("profiles: {erreur}")),
        }
    }

    for entree in catalogue.list_agents() {
        match entree {
            Ok(entree) => {
                let agent = entree.valeur;
                if let Some(profil) = agent.profile()
                    && !profils.contains(profil)
                {
                    problemes.push(format!(
                        "agents: `{}` référence le profil inconnu `{profil}`",
                        agent.id()
                    ));
                }
            }
            Err(erreur) => problemes.push(format!("agents: {erreur}")),
        }
    }

    let toolsets = FileToolsets::new(layers, RENDU_MAX_CARACTERES);
    for entree in toolsets.list_active() {
        if let Err(erreur) = entree {
            problemes.push(format!("toolsets: {erreur}"));
        }
    }
    match toolsets.list_drafts().await {
        Ok(_) => {}
        Err(erreur) => problemes.push(format!("toolsets-drafts: {erreur}")),
    }

    let contextes = FileContexts::new(layers);
    for entree in contextes.list_types() {
        if let Err(erreur) = entree {
            problemes.push(format!("context-types: {erreur}"));
        }
    }
    for entree in contextes.list_contexts() {
        if let Err(erreur) = entree {
            problemes.push(format!("contexts: {erreur}"));
        }
    }

    problemes
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use lymaille::{ModelProfile, Provider, ProviderLevel};
    use tempfile::TempDir;

    use super::verifier;
    use crate::host::catalog_store::FileCatalog;
    use crate::host::layers::{Layers, Level};

    fn niveaux(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    // Scenario: configuration saine
    #[tokio::test]
    async fn configuration_saine_sans_probleme() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        let catalogue = FileCatalog::new(&layers);
        let provider = Provider::new(
            "distant".to_owned(),
            "openai".to_owned(),
            "https://exemple.invalide/v1".to_owned(),
            Some("cle-nom".to_owned()),
            ProviderLevel::User,
        )
        .expect("provider de référence");
        let profil = ModelProfile::new(
            "rapide".to_owned(),
            "distant".to_owned(),
            "modele-x".to_owned(),
        );
        catalogue
            .put_provider_at(Level::User, &provider)
            .expect("écriture provider");
        catalogue
            .put_profile_at(Level::User, &profil)
            .expect("écriture profil");
        assert!(verifier(&layers).await.is_empty());
    }

    // Scenario: fichier invalide et référence inconnue
    #[tokio::test]
    async fn fichier_invalide_et_reference_inconnue_listes_avec_chemin() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        let dossier = home.path().join(".config/vanyline/profiles");
        fs::create_dir_all(&dossier).expect("dossier");
        fs::write(dossier.join("casse.yaml"), "name: [\n").expect("fichier cassé");
        let problemes = verifier(&layers).await;
        assert!(
            problemes.iter().any(|p| p.contains("casse.yaml")),
            "le fichier cassé doit être listé avec son chemin : {problemes:?}"
        );
    }

    // Contrat : référence profil -> provider inconnue
    #[tokio::test]
    async fn profil_vers_provider_inconnu_liste() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        let profil = ModelProfile::new(
            "rapide".to_owned(),
            "absent".to_owned(),
            "modele-x".to_owned(),
        );
        FileCatalog::new(&layers)
            .put_profile_at(Level::User, &profil)
            .expect("écriture profil");
        let problemes = verifier(&layers).await;
        assert!(
            problemes.iter().any(|p| p.contains("absent")),
            "la référence inconnue doit être nommée : {problemes:?}"
        );
    }

    // Contrat (Handles) : un provider qui porte une valeur ressemblant à un secret est signalé, nommé par son fichier
    #[tokio::test]
    async fn provider_qui_porte_un_secret_est_signale() {
        let home = TempDir::new().expect("tempdir");
        let layers = niveaux(home.path());
        let dossier = home.path().join(".config/vanyline/providers");
        fs::create_dir_all(&dossier).expect("dossier");
        fs::write(
            dossier.join("fuite.yaml"),
            "name: fuite\nkind: openai\naddress: https://exemple.invalide/v1\nauth: sk-abc123def456ghi789\nlevel: user\n",
        )
        .expect("fichier provider");
        let problemes = verifier(&layers).await;
        assert!(
            problemes.iter().any(|p| p.contains("fuite.yaml")),
            "le provider fautif doit être signalé par son fichier : {problemes:?}"
        );
    }

    // Contrat : sans niveau partagé, seul le niveau utilisateur est vérifié, sans erreur
    #[tokio::test]
    async fn hors_depot_pas_d_erreur() {
        let home = TempDir::new().expect("tempdir");
        assert!(verifier(&niveaux(home.path())).await.is_empty());
    }
}
