use clap::Subcommand;

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
