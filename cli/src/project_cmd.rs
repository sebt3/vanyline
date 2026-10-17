use clap::Subcommand;

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// List K8s Projects
    List,
    /// Show a single Project
    Show { name: String },
    /// Create a Project
    Create {
        name: String,
        #[arg(long = "repo-url")]
        repo_url: String,
        #[arg(long)]
        default_branch: Option<String>,
        #[arg(long)]
        storage_size: Option<String>,
        #[arg(long)]
        storage_class: Option<String>,
        #[arg(long)]
        storage_access_mode: Option<String>,
        #[arg(long)]
        git_secret: Option<String>,
        /// Répétable : --cache cargo --cache pnpm
        #[arg(long = "cache")]
        caches: Vec<String>,
        #[arg(long)]
        fetch_interval: Option<String>,
    },
    /// Delete a Project
    Delete { name: String },
}

#[cfg(test)]
mod tests {
    use crate::{Cli, Commands as RootCommands};
    use clap::Parser;
    use clap::error::ErrorKind;

    use super::Commands as ProjectCmd;

    /// Parse `args` via le `Cli` racine et extrait la sous-commande `project`.
    fn project(args: &[&str]) -> Option<ProjectCmd> {
        match Cli::try_parse_from(args).ok()?.command {
            Some(RootCommands::K8sProject(cmd)) => Some(cmd),
            _ => None,
        }
    }

    // Scenario: `project list` se parse sans argument
    #[test]
    fn project_list_se_parse_sans_argument() {
        assert!(matches!(
            project(&["vanyline", "project", "list"]),
            Some(ProjectCmd::List)
        ));
    }

    // Scenario: `project show` parse le positionnel name
    #[test]
    fn project_show_parse_le_positionnel_name() {
        assert!(matches!(
            project(&["vanyline", "project", "show", "mon-projet"]),
            Some(ProjectCmd::Show { name }) if name == "mon-projet"
        ));
    }

    // Scenario: `project show` sans name est rejeté
    #[test]
    fn project_show_sans_name_est_rejete() {
        assert!(Cli::try_parse_from(["vanyline", "project", "show"]).is_err());
    }

    // Scenario: `project` seul est rejeté
    #[test]
    fn project_seul_est_rejete() {
        assert!(Cli::try_parse_from(["vanyline", "project"]).is_err());
    }

    // Scenario: `project create` minimal avec les deux options requises
    #[test]
    fn project_create_minimal_avec_les_deux_options_requises() {
        let cli = Cli::try_parse_from([
            "vanyline",
            "project",
            "create",
            "depot",
            "--owner",
            "alice",
            "--repo-url",
            "https://example.invalid/depot.git",
        ])
        .ok();
        // `--owner` est un flag global de `Cli` (pas un champ de `Create`).
        assert_eq!(cli.as_ref().and_then(|c| c.owner.as_deref()), Some("alice"));
        match cli.and_then(|c| c.command) {
            Some(RootCommands::K8sProject(ProjectCmd::Create {
                name,
                repo_url,
                default_branch,
                storage_size,
                storage_class,
                storage_access_mode,
                git_secret,
                caches,
                fetch_interval,
            })) => {
                assert_eq!(name, "depot");
                assert_eq!(repo_url, "https://example.invalid/depot.git");
                assert_eq!(default_branch, None);
                assert_eq!(storage_size, None);
                assert_eq!(storage_class, None);
                assert_eq!(storage_access_mode, None);
                assert_eq!(git_secret, None);
                assert!(caches.is_empty());
                assert_eq!(fetch_interval, None);
            }
            _ => panic!("expected K8sProject(Create)"),
        }
    }

    // Scenario: `project create` sans `--repo-url` est rejeté
    #[test]
    fn project_create_sans_repo_url_est_rejete() {
        assert!(
            Cli::try_parse_from(["vanyline", "project", "create", "depot", "--owner", "alice"])
                .is_err()
        );
    }

    // Scenario: le flag de repo est `--repo-url` et pas `--repo_url`
    #[test]
    fn le_flag_de_repo_est_repo_url_et_pas_repo_underscore() {
        assert_eq!(
            Cli::try_parse_from([
                "vanyline",
                "project",
                "create",
                "depot",
                "--owner",
                "alice",
                "--repo_url",
                "r"
            ])
            .err()
            .map(|e| e.kind()),
            Some(ErrorKind::UnknownArgument)
        );
    }

    // Scenario: `project create` accepte la totalité des flags
    #[test]
    fn project_create_accepte_la_totalite_des_flags() {
        match project(&[
            "vanyline",
            "project",
            "create",
            "depot",
            "--owner",
            "alice",
            "--repo-url",
            "r",
            "--default-branch",
            "main",
            "--storage-size",
            "10Gi",
            "--storage-class",
            "fast",
            "--storage-access-mode",
            "ReadWriteMany",
            "--git-secret",
            "deploy-key",
            "--fetch-interval",
            "30m",
        ]) {
            Some(ProjectCmd::Create {
                name,
                repo_url,
                default_branch,
                storage_size,
                storage_class,
                storage_access_mode,
                git_secret,
                caches,
                fetch_interval,
            }) => {
                assert_eq!(name, "depot");
                assert_eq!(repo_url, "r");
                assert_eq!(default_branch.as_deref(), Some("main"));
                assert_eq!(storage_size.as_deref(), Some("10Gi"));
                assert_eq!(storage_class.as_deref(), Some("fast"));
                assert_eq!(storage_access_mode.as_deref(), Some("ReadWriteMany"));
                assert_eq!(git_secret.as_deref(), Some("deploy-key"));
                assert!(caches.is_empty());
                assert_eq!(fetch_interval.as_deref(), Some("30m"));
            }
            _ => panic!("expected K8sProject(Create)"),
        }
    }

    // Scenario: `--cache` est répétable et préserve l'ordre
    #[test]
    fn cache_est_repetable_et_preserve_l_ordre() {
        match project(&[
            "vanyline",
            "project",
            "create",
            "depot",
            "--owner",
            "a",
            "--repo-url",
            "r",
            "--cache",
            "cargo",
            "--cache",
            "pnpm",
        ]) {
            Some(ProjectCmd::Create { caches, .. }) => {
                assert_eq!(caches, vec!["cargo".to_string(), "pnpm".to_string()]);
            }
            _ => panic!("expected K8sProject(Create)"),
        }
    }

    // Scenario: `--existing-pvc-name` n'existe plus
    #[test]
    fn existing_pvc_name_n_existe_plus() {
        assert_eq!(
            Cli::try_parse_from([
                "vanyline",
                "project",
                "create",
                "depot",
                "--owner",
                "a",
                "--repo-url",
                "r",
                "--existing-pvc-name",
                "x",
            ])
            .err()
            .map(|e| e.kind()),
            Some(ErrorKind::UnknownArgument)
        );
    }

    // Scenario: `--storage-size` ne valide pas la valeur
    #[test]
    fn storage_size_ne_valide_pas_la_valeur() {
        match project(&[
            "vanyline",
            "project",
            "create",
            "depot",
            "--owner",
            "a",
            "--repo-url",
            "r",
            "--storage-size",
            "pas-une-taille",
        ]) {
            Some(ProjectCmd::Create { storage_size, .. }) => {
                assert_eq!(storage_size.as_deref(), Some("pas-une-taille"));
            }
            _ => panic!("expected K8sProject(Create)"),
        }
    }

    // Scenario: `project delete` parse le positionnel name
    #[test]
    fn project_delete_parse_le_positionnel_name() {
        assert!(matches!(
            project(&["vanyline", "project", "delete", "mon-projet"]),
            Some(ProjectCmd::Delete { name }) if name == "mon-projet"
        ));
    }

    // Scenario: une sous-commande inconnue est rejetée
    #[test]
    fn une_sous_commande_inconnue_est_rejetee() {
        assert!(Cli::try_parse_from(["vanyline", "project", "get-all"]).is_err());
    }
}
