use std::fmt::Display;
use std::io::Write;
use std::path::Path;

use clap::Subcommand;
use kube::ResourceExt;
use vanyline_lib::k8s::VnlK8sClient;

use crate::chat::GlobalOptions;
use crate::exit_code::Outcome;
use crate::host::layers::Layers;
use crate::target::{self, Settings, TargetError};

/// Client K8s et réglages d'une commande d'objets (`target.sdd`, `objets#FR-8`).
pub(crate) struct Connexion {
    /// Client K8s, namespace d'application résolu.
    pub(crate) client: VnlK8sClient,
    /// Réglages du niveau utilisateur (`defaults.owner`, `defaults.namespace`).
    pub(crate) settings: Settings,
}

/// Ouvre la connexion : réglages utilisateur, namespace d'application (`--namespace`, `defaults.namespace`, puis le
/// kubeconfig), client K8s. Une erreur est écrite sur la sortie d'erreur et rendue comme issue.
pub(crate) async fn connexion(global: &GlobalOptions) -> Result<Connexion, Outcome> {
    let home = dirs::home_dir().unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let layers = Layers::discover(&home, &cwd, global.repo.as_deref(), Path::exists);
    let settings = Settings::load(&layers).map_err(echec)?;
    let kubeconfig = kube::Config::infer()
        .await
        .ok()
        .map(|config| config.default_namespace);
    let namespace = target::resolve_namespace(
        global.namespace.as_deref(),
        &settings,
        kubeconfig.as_deref(),
    );
    let client = VnlK8sClient::discover(Some(namespace))
        .await
        .map_err(echec)?;
    Ok(Connexion { client, settings })
}

/// Owner de la commande : `--owner`, puis `defaults.owner`. Son absence est une erreur d'usage (code 2, `AC-7`).
pub(crate) fn owner_de(global: &GlobalOptions, settings: &Settings) -> Result<String, Outcome> {
    target::resolve_owner(global.owner.as_deref(), settings).map_err(|erreur| match erreur {
        TargetError::Usage(message) => {
            ecrire_erreur(&message);
            Outcome::Usage
        }
        autre => echec(autre),
    })
}

/// Écrit une erreur sur la sortie d'erreur et rend l'issue d'échec (code 1).
pub(crate) fn echec(erreur: impl Display) -> Outcome {
    ecrire_erreur(&erreur.to_string());
    Outcome::Failure
}

/// Issue d'une erreur de spécification (`apply`, `edit`) : usage (code 2) sans `$EDITOR`, échec (code 1) sinon.
pub(crate) fn echec_spec(erreur: crate::spec_edit::SpecError) -> Outcome {
    use crate::spec_edit::SpecError;
    let issue = match erreur {
        SpecError::Usage(_) => Outcome::Usage,
        _ => Outcome::Failure,
    };
    ecrire_erreur(&match erreur {
        SpecError::Invalid(cause) | SpecError::Immutable(cause) | SpecError::Usage(cause) => cause,
    });
    issue
}

/// Ligne de résultat d'un `apply` ou d'un `edit` ; « redémarrage requis » quand l'état de la sandbox le porte.
pub(crate) fn rendre_apply(
    sortie: &mut impl Write,
    libelle: &str,
    nom: &str,
    issue: crate::spec_edit::ApplyOutcome,
) {
    use crate::spec_edit::ApplyOutcome;
    match issue {
        ApplyOutcome::Created => {
            let _ = writeln!(sortie, "Created {libelle}: {nom}");
        }
        ApplyOutcome::Updated { redemarrage_requis } => {
            let _ = writeln!(sortie, "Updated {libelle}: {nom}");
            if redemarrage_requis {
                let _ = writeln!(sortie, "redémarrage requis");
            }
        }
        ApplyOutcome::Unchanged => {
            let _ = writeln!(sortie, "Unchanged {libelle}: {nom}");
        }
    }
}

/// Écrit un message sur la sortie d'erreur ; une écriture impossible n'a pas d'issue utile.
fn ecrire_erreur(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Exécute `owner` et rend son issue (`objets` : `owner list|show|create|delete`). Pas d'`apply` ni d'`edit` (`objets#OI-1`).
///
/// Helpers partagés avec `project` et `sandbox` (`pub(crate)`) : `connexion`, `owner_de`, `echec`, `echec_spec`, `rendre_apply`.
pub async fn run_owner(commande: Commands, global: &GlobalOptions) -> Outcome {
    let connexion = match connexion(global).await {
        Ok(connexion) => connexion,
        Err(issue) => return issue,
    };
    match executer(&connexion.client, commande).await {
        Ok(()) => Outcome::Success,
        Err(issue) => issue,
    }
}

/// Exécution des sous-commandes `owner` sur le client ; le premier échec rend son issue.
// objets (`dsd/cli/objets.md`, « Conception ») : un dispatch par famille de commandes, sans logique propre.
#[allow(clippy::too_many_lines)]
async fn executer(client: &VnlK8sClient, commande: Commands) -> Result<(), Outcome> {
    let mut sortie = std::io::stdout();
    match commande {
        Commands::List => {
            let owners = client.list_owners().await.map_err(echec)?;
            if owners.is_empty() {
                let _ = writeln!(sortie, "No owners found.");
            } else {
                for o in &owners {
                    let _ = writeln!(
                        sortie,
                        "  {} | {} | {}",
                        o.name_any(),
                        o.status
                            .as_ref()
                            .and_then(|s| s.pvc_name.as_deref())
                            .unwrap_or("-"),
                        o.status
                            .as_ref()
                            .and_then(|s| s.service_account.as_deref())
                            .unwrap_or("-")
                    );
                }
            }
        }
        Commands::Show { name } => {
            let owner = client.get_owner(&name).await.map_err(echec)?;
            let _ = writeln!(sortie, "Owner: {}", owner.name_any());
            let _ = writeln!(
                sortie,
                "  homeSize: {}",
                owner.spec.home_size.as_deref().unwrap_or("-")
            );
            let _ = writeln!(
                sortie,
                "  homeStorageClass: {}",
                owner.spec.home_storage_class.as_deref().unwrap_or("-")
            );
            if let Some(pd) = &owner.spec.project_defaults {
                let _ = writeln!(sortie, "  projectDefaults:");
                let _ = writeln!(
                    sortie,
                    "    storageSize: {}",
                    pd.storage_size.as_deref().unwrap_or("-")
                );
                let _ = writeln!(
                    sortie,
                    "    storageClass: {}",
                    pd.storage_class.as_deref().unwrap_or("-")
                );
            } else {
                let _ = writeln!(sortie, "  projectDefaults: -");
            }
            let _ = writeln!(sortie, "Status:");
            match &owner.status {
                Some(status) => {
                    let _ = writeln!(
                        sortie,
                        "  pvcName: {}",
                        status.pvc_name.as_deref().unwrap_or("-")
                    );
                    let _ = writeln!(
                        sortie,
                        "  serviceAccount: {}",
                        status.service_account.as_deref().unwrap_or("-")
                    );
                    if status.conditions.is_empty() {
                        let _ = writeln!(sortie, "  conditions: -");
                    } else {
                        let _ = writeln!(sortie, "  conditions:");
                        for c in &status.conditions {
                            let _ = writeln!(
                                sortie,
                                "    - {} status={} message={}",
                                c.type_, c.status, c.message
                            );
                        }
                    }
                }
                None => {
                    let _ = writeln!(sortie, "  (not yet reconciled)");
                }
            }
        }
        Commands::Create {
            name,
            home_size,
            home_storage_class,
            home_access_mode,
            project_default_storage_size,
            project_default_storage_class,
            project_default_storage_access_mode,
        } => {
            let project_defaults = if project_default_storage_size.is_some()
                || project_default_storage_class.is_some()
                || project_default_storage_access_mode.is_some()
            {
                Some(vanyline_crds::ProjectDefaults {
                    storage_size: project_default_storage_size,
                    storage_class: project_default_storage_class,
                    storage_access_mode: project_default_storage_access_mode,
                })
            } else {
                None
            };
            let spec = vanyline_crds::OwnerSpec {
                home_size,
                home_storage_class,
                home_access_mode,
                project_defaults,
                application_ref: None,
                egress: Vec::new(),
            };
            client.create_owner(&name, spec).await.map_err(echec)?;
            let _ = writeln!(sortie, "Created owner: {name}");
        }
        Commands::Delete { name } => {
            client.delete_owner(&name).await.map_err(echec)?;
            let _ = writeln!(sortie, "Deleted owner: {name}");
        }
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum Commands {
    /// List K8s Owners
    List,
    /// Show a single Owner
    Show { name: String },
    /// Create an Owner
    Create {
        name: String,
        #[arg(long)]
        home_size: Option<String>,
        #[arg(long)]
        home_storage_class: Option<String>,
        /// Mode d'accès du PVC home. Omis: "`ReadWriteMany`" (défaut historique).
        #[arg(long)]
        home_access_mode: Option<String>,
        /// Taille par défaut appliquée aux futurs Projects de cet Owner.
        #[arg(long)]
        project_default_storage_size: Option<String>,
        #[arg(long)]
        project_default_storage_class: Option<String>,
        #[arg(long)]
        project_default_storage_access_mode: Option<String>,
    },
    /// Delete an Owner
    Delete { name: String },
}

#[cfg(test)]
mod tests {
    use crate::{Cli, Commands as RootCommands};
    use clap::error::ErrorKind;
    use clap::{Parser, Subcommand};

    use super::Commands as OwnerCmd;

    /// Parse `args` via le `Cli` racine et extrait la sous-commande `owner`.
    fn owner(args: &[&str]) -> Option<OwnerCmd> {
        match Cli::try_parse_from(args).ok()?.command {
            Some(RootCommands::K8sOwner(cmd)) => Some(cmd),
            _ => None,
        }
    }

    fn parse_err_kind(args: &[&str]) -> Option<ErrorKind> {
        Cli::try_parse_from(args).err().map(|e| e.kind())
    }

    // Scenario: owner_list_se_parse_avec_zero_argument
    #[test]
    fn owner_list_se_parse_avec_zero_argument() {
        assert!(matches!(
            owner(&["vanyline", "owner", "list"]),
            Some(OwnerCmd::List)
        ));
    }

    // Scenario: owner_sous_commande_nue_refusee
    #[test]
    fn owner_sous_commande_nue_refusee() {
        assert!(Cli::try_parse_from(["vanyline", "owner"]).is_err());
    }

    // Scenario: show_et_delete_exigent_un_nom
    #[test]
    fn show_et_delete_exigent_un_nom() {
        assert!(Cli::try_parse_from(["vanyline", "owner", "show"]).is_err());
        assert!(Cli::try_parse_from(["vanyline", "owner", "delete"]).is_err());
        assert!(matches!(
            owner(&["vanyline", "owner", "show", "alice"]),
            Some(OwnerCmd::Show { name }) if name == "alice"
        ));
    }

    // Scenario: owner_renommage_utilisateur_verrouille
    #[test]
    fn owner_renommage_utilisateur_verrouille() {
        assert!(Cli::try_parse_from(["vanyline", "owner", "list"]).is_ok());
        assert!(Cli::try_parse_from(["vanyline", "k8s-owner", "list"]).is_err());
    }

    // Scenario: create_sans_flag_donne_tous_options_none
    #[test]
    fn create_sans_flag_donne_tous_options_none() {
        match owner(&["vanyline", "owner", "create", "alice"]) {
            Some(OwnerCmd::Create {
                name,
                home_size,
                home_storage_class,
                home_access_mode,
                project_default_storage_size,
                project_default_storage_class,
                project_default_storage_access_mode,
            }) => {
                assert_eq!(name, "alice");
                assert_eq!(home_size, None);
                assert_eq!(home_storage_class, None);
                assert_eq!(home_access_mode, None);
                assert_eq!(project_default_storage_size, None);
                assert_eq!(project_default_storage_class, None);
                assert_eq!(project_default_storage_access_mode, None);
            }
            _ => panic!("expected Create"),
        }
    }

    // Scenario: create_accepte_les_six_flags_kebab_case
    #[test]
    fn create_accepte_les_six_flags_kebab_case() {
        match owner(&[
            "vanyline",
            "owner",
            "create",
            "alice",
            "--home-size",
            "2Gi",
            "--home-storage-class",
            "cephfs",
            "--home-access-mode",
            "ReadWriteOnce",
            "--project-default-storage-size",
            "5Gi",
            "--project-default-storage-class",
            "local-path",
            "--project-default-storage-access-mode",
            "ReadWriteOnce",
        ]) {
            Some(OwnerCmd::Create {
                home_size,
                home_storage_class,
                home_access_mode,
                project_default_storage_size,
                project_default_storage_class,
                project_default_storage_access_mode,
                ..
            }) => {
                assert_eq!(home_size.as_deref(), Some("2Gi"));
                assert_eq!(home_storage_class.as_deref(), Some("cephfs"));
                assert_eq!(home_access_mode.as_deref(), Some("ReadWriteOnce"));
                assert_eq!(project_default_storage_size.as_deref(), Some("5Gi"));
                assert_eq!(project_default_storage_class.as_deref(), Some("local-path"));
                assert_eq!(
                    project_default_storage_access_mode.as_deref(),
                    Some("ReadWriteOnce")
                );
            }
            _ => panic!("expected Create"),
        }
    }

    // Scenario: create_rejette_les_flags_underscore
    #[test]
    fn create_rejette_les_flags_underscore() {
        assert_eq!(
            parse_err_kind(&["vanyline", "owner", "create", "alice", "--home_size", "2Gi"]),
            Some(ErrorKind::UnknownArgument)
        );
    }

    // Scenario: create_transplante_les_valeurs_sans_validation
    #[test]
    fn create_transplante_les_valeurs_sans_validation() {
        match owner(&[
            "vanyline",
            "owner",
            "create",
            "alice",
            "--home-size",
            "banane",
            "--home-access-mode",
            "PasUnMode",
        ]) {
            Some(OwnerCmd::Create {
                home_size,
                home_access_mode,
                ..
            }) => {
                assert_eq!(home_size.as_deref(), Some("banane"));
                assert_eq!(home_access_mode.as_deref(), Some("PasUnMode"));
            }
            _ => panic!("expected Create"),
        }
    }

    // Scenario: existing_pvc_n_existe_plus
    #[test]
    fn existing_pvc_n_existe_plus() {
        assert_eq!(
            parse_err_kind(&[
                "vanyline",
                "owner",
                "create",
                "alice",
                "--existing-pvc",
                "x"
            ]),
            Some(ErrorKind::UnknownArgument)
        );
    }

    // Scenario: positionnel_supplementaire_refuse
    #[test]
    fn positionnel_supplementaire_refuse() {
        assert!(Cli::try_parse_from(["vanyline", "owner", "show", "alice", "bob"]).is_err());
    }

    // Scenario: aide_owner_documente_variants_et_flags
    #[test]
    fn aide_owner_documente_variants_et_flags() {
        let mut cmd = <OwnerCmd as Subcommand>::augment_subcommands(clap::Command::new("owner"));
        let help = cmd.render_help().to_string();
        for needle in [
            "List K8s Owners",
            "Show a single Owner",
            "Create an Owner",
            "Delete an Owner",
        ] {
            assert!(
                help.contains(needle),
                "aide owner sans « {needle} »:\n{help}"
            );
        }
        // Les flags vivent sur `create` : leur aide se lit sur la sous-commande.
        let create_help = cmd
            .find_subcommand_mut("create")
            .expect("sous-commande create")
            .render_help()
            .to_string();
        for needle in [
            "--home-access-mode",
            "--project-default-storage-size",
            "Mode d'accès du PVC home",
        ] {
            assert!(
                create_help.contains(needle),
                "aide owner create sans « {needle} »:\n{create_help}"
            );
        }
    }

    // Scenario: owner_flag_repete_en_home_size_refuse
    #[test]
    fn owner_flag_repete_en_home_size_refuse() {
        let err = Cli::try_parse_from([
            "vanyline",
            "owner",
            "create",
            "alice",
            "--home-size",
            "1Gi",
            "--home-size",
            "2Gi",
        ])
        .err();
        assert_eq!(
            err.as_ref().map(clap::Error::kind),
            Some(ErrorKind::ArgumentConflict)
        );
        assert!(err.is_some_and(|e| e.to_string().contains("cannot be used multiple times")));
    }
}
