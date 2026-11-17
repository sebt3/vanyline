use std::io::Write;

use clap::Subcommand;
use kube::ResourceExt;
use vanyline_crds::Sandbox;
use vanyline_lib::k8s::VnlK8sClient;

use crate::chat::GlobalOptions;
use crate::exit_code::Outcome;
use crate::owner_cmd::{connexion, echec, owner_de};

/// Condition de la Sandbox qui signale qu'un changement de spec attend un redémarrage du pod (`vanyline.sdd`).
const RESTART_PENDING: &str = "RestartPending";

/// Vrai si la Sandbox porte la condition `RestartPending` à `True` : la spec courante n'est pas encore active.
fn redemarrage_requis(sandbox: &Sandbox) -> bool {
    sandbox.status.as_ref().is_some_and(|status| {
        status
            .conditions
            .iter()
            .any(|c| c.type_ == RESTART_PENDING && c.status == "True")
    })
}

/// Exécute `sandbox` et rend son issue : l'Owner est résolu avant tout appel au cluster (`target.sdd`).
pub async fn run_sandbox(commande: Commands, global: &GlobalOptions) -> Outcome {
    let connexion = match connexion(global).await {
        Ok(connexion) => connexion,
        Err(issue) => return issue,
    };
    let owner = match owner_de(global, &connexion.settings) {
        Ok(owner) => owner,
        Err(issue) => return issue,
    };
    match executer(&connexion.client, &owner, commande).await {
        Ok(()) => Outcome::Success,
        Err(issue) => issue,
    }
}

/// Exécution des sous-commandes `sandbox` dans le namespace de l'Owner ; le premier échec rend son issue.
// objets (`dsd/cli/objets.md`, « Conception ») : un dispatch par famille de commandes, sans logique propre.
#[allow(clippy::too_many_lines)]
async fn executer(client: &VnlK8sClient, owner: &str, commande: Commands) -> Result<(), Outcome> {
    let mut sortie = std::io::stdout();
    match commande {
        Commands::List => {
            let sandboxes = client.list_sandboxes(owner).await.map_err(echec)?;
            if sandboxes.is_empty() {
                let _ = writeln!(sortie, "No sandboxes found.");
            } else {
                for s in &sandboxes {
                    let phase = s
                        .status
                        .as_ref()
                        .and_then(|st| st.phase.as_deref())
                        .unwrap_or("-");
                    let _ = writeln!(
                        sortie,
                        "  {} | project={} | branch={} | phase={}",
                        s.name_any(),
                        s.spec.project,
                        s.spec.branch,
                        phase
                    );
                }
            }
        }
        Commands::Show { name } => {
            let sandbox = client.get_sandbox(owner, &name).await.map_err(echec)?;
            let _ = writeln!(sortie, "Sandbox: {}", sandbox.name_any());
            let _ = writeln!(sortie, "  project: {}", sandbox.spec.project);
            let _ = writeln!(sortie, "  branch: {}", sandbox.spec.branch);
            let _ = writeln!(
                sortie,
                "  image: {}",
                sandbox.spec.image.as_deref().unwrap_or("-")
            );
            if sandbox.spec.toolchains.is_empty() {
                let _ = writeln!(sortie, "  toolchains: -");
            } else {
                let _ = writeln!(sortie, "  toolchains:");
                for tc in &sandbox.spec.toolchains {
                    let _ = writeln!(sortie, "    - {} = {}", tc.name, tc.image);
                }
            }
            let _ = writeln!(sortie, "Status:");
            if redemarrage_requis(&sandbox) {
                let _ = writeln!(sortie, "  redémarrage requis");
            }
            match &sandbox.status {
                Some(status) => {
                    let _ = writeln!(
                        sortie,
                        "  phase: {}",
                        status.phase.as_deref().unwrap_or("-")
                    );
                    let _ = writeln!(
                        sortie,
                        "  service: {}",
                        status.service.as_deref().unwrap_or("-")
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
            project,
            branch,
            toolchains,
            image,
        } => {
            let spec = vanyline_crds::SandboxSpec {
                project,
                branch,
                toolchains,
                image,
                resources: None,
                egress: Vec::new(),
                suspended: false,
            };
            client
                .create_sandbox(owner, &name, spec)
                .await
                .map_err(echec)?;
            let _ = writeln!(sortie, "Created sandbox: {name}");
        }
        Commands::Delete { name } => {
            client.delete_sandbox(owner, &name).await.map_err(echec)?;
            let _ = writeln!(sortie, "Deleted sandbox: {name}");
        }
        Commands::Stop { name } => {
            client
                .set_sandbox_suspended(owner, &name, true)
                .await
                .map_err(echec)?;
            let _ = writeln!(sortie, "Stopped sandbox: {name}");
        }
        Commands::Start { name } => {
            client
                .set_sandbox_suspended(owner, &name, false)
                .await
                .map_err(echec)?;
            let _ = writeln!(sortie, "Started sandbox: {name}");
        }
    }
    Ok(())
}

/// Parse `NAME=IMAGE` (ex. `rust=rust:slim-trixie`) en `Toolchain` avec un
/// env vide — le controller applique un preset d'environnement connu pour
/// les noms `rust`/`node` quand `env` est vide (cf. `resolve_toolchain_env`
/// cote controller), donc laisser `env` vide ici est le comportement voulu,
/// pas un oubli.
///
/// `split_once` prend le premier `=` : les images Docker n'utilisent pas `=`
/// dans leur reference, donc c'est l'unique `=` attendu et le comportement est
/// correct.
fn parse_toolchain(s: &str) -> Result<vanyline_crds::Toolchain, String> {
    let (name, image) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid --toolchain '{s}', expected NAME=IMAGE"))?;
    if name.is_empty() || image.is_empty() {
        return Err(format!("invalid --toolchain '{s}', expected NAME=IMAGE"));
    }
    Ok(vanyline_crds::Toolchain {
        name: name.to_string(),
        image: image.to_string(),
        env: std::collections::BTreeMap::default(),
        lsp: None,
    })
}

#[derive(Subcommand)]
pub enum Commands {
    /// List K8s Sandboxes
    List,
    /// Show a single Sandbox
    Show { name: String },
    /// Create a Sandbox
    Create {
        name: String,
        #[arg(long)]
        project: String,
        #[arg(long)]
        branch: String,
        /// Repetable : --toolchain rust=rust:slim-trixie --toolchain node=node:trixie-slim
        #[arg(long = "toolchain", value_parser = parse_toolchain)]
        toolchains: Vec<vanyline_crds::Toolchain>,
        /// Image du serveur sandbox. Omis: defaut du controller (env `SANDBOX_IMAGE`).
        #[arg(long)]
        image: Option<String>,
    },
    /// Delete a Sandbox
    Delete { name: String },
    /// Stop a Sandbox (suspend the pod without deleting the resource)
    Stop { name: String },
    /// Start a previously stopped Sandbox
    Start { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Commands as RootCommands};
    use clap::Parser;

    #[test]
    fn parse_toolchain_valid() {
        let tc = parse_toolchain("rust=rust:slim-trixie").unwrap();
        assert_eq!(tc.name, "rust");
        assert_eq!(tc.image, "rust:slim-trixie");
        assert!(tc.env.is_empty());
    }

    #[test]
    fn parse_toolchain_missing_equals_errors() {
        let err = parse_toolchain("rust").unwrap_err();
        assert!(err.contains("invalid --toolchain"));
    }

    #[test]
    fn parse_toolchain_empty_name_errors() {
        let err = parse_toolchain("=rust:slim").unwrap_err();
        assert!(err.contains("invalid --toolchain"));
    }

    #[test]
    fn parse_toolchain_empty_image_errors() {
        let err = parse_toolchain("rust=").unwrap_err();
        assert!(err.contains("invalid --toolchain"));
    }

    // Scenario: parse_toolchain_valide_donne_env_vide_et_lsp_none
    #[test]
    fn parse_toolchain_valide_donne_env_vide_et_lsp_none() {
        let tc = parse_toolchain("rust=rust:slim-trixie").unwrap();
        assert_eq!(tc.name, "rust");
        assert_eq!(tc.image, "rust:slim-trixie");
        assert!(tc.env.is_empty());
        assert!(tc.lsp.is_none());
    }

    // Scenario: parse_toolchain_sans_signe_egale_refuse (message cite la valeur brute)
    #[test]
    fn parse_toolchain_sans_signe_egale_refuse_cite_la_valeur() {
        assert_eq!(
            parse_toolchain("rust").unwrap_err(),
            "invalid --toolchain 'rust', expected NAME=IMAGE"
        );
    }

    // Scenario: parse_toolchain_nom_vide_refuse
    #[test]
    fn parse_toolchain_nom_vide_refuse() {
        assert_eq!(
            parse_toolchain("=rust:slim").unwrap_err(),
            "invalid --toolchain '=rust:slim', expected NAME=IMAGE"
        );
    }

    // Scenario: parse_toolchain_image_vide_refuse
    #[test]
    fn parse_toolchain_image_vide_refuse() {
        assert_eq!(
            parse_toolchain("rust=").unwrap_err(),
            "invalid --toolchain 'rust=', expected NAME=IMAGE"
        );
    }

    // Scenario: parse_toolchain_ne_garde_que_le_premier_signe_egale
    #[test]
    fn parse_toolchain_ne_garde_que_le_premier_signe_egale() {
        let tc = parse_toolchain("rust=b=c").unwrap();
        assert_eq!(tc.name, "rust");
        assert_eq!(tc.image, "b=c");
    }

    // Scenario: parse_toolchain_accepte_les_espaces_sans_trim
    #[test]
    fn parse_toolchain_accepte_les_espaces_sans_trim() {
        let tc = parse_toolchain(" rust = img ").unwrap();
        assert_eq!(tc.name, " rust ");
        assert_eq!(tc.image, " img ");
    }

    // Contrat `sandbox show` (FR-5, AC-5) : « redémarrage requis » quand la condition RestartPending est à True
    #[test]
    fn redemarrage_requis_suit_la_condition_restart_pending() {
        let spec = vanyline_crds::SandboxSpec {
            project: "depot".to_owned(),
            branch: "main".to_owned(),
            toolchains: Vec::new(),
            image: None,
            resources: None,
            egress: Vec::new(),
            suspended: false,
        };
        let mut sandbox = vanyline_crds::Sandbox::new("depot-main", spec);
        assert!(!super::redemarrage_requis(&sandbox));
        // Statut lu comme le cluster le renvoie (pas de type de condition importé dans ce crate).
        let statut = |valeur: &str| -> vanyline_crds::SandboxStatus {
            serde_json::from_value(serde_json::json!({
                "conditions": [{
                    "type": "RestartPending",
                    "status": valeur,
                    "lastTransitionTime": "2026-01-01T00:00:00Z",
                    "reason": "",
                    "message": ""
                }]
            }))
            .expect("statut de sandbox valide")
        };
        sandbox.status = Some(statut("False"));
        assert!(!super::redemarrage_requis(&sandbox));
        sandbox.status = Some(statut("True"));
        assert!(super::redemarrage_requis(&sandbox));
    }

    fn sandbox(args: &[&str]) -> Option<Commands> {
        match Cli::try_parse_from(args).ok()?.command {
            Some(RootCommands::K8sSandbox(cmd)) => Some(cmd),
            _ => None,
        }
    }

    // Scenario: sandbox_sous_commande_nue_refusee
    #[test]
    fn sandbox_sous_commande_nue_refusee() {
        assert!(Cli::try_parse_from(["vanyline", "sandbox"]).is_err());
    }

    // Scenario: sandbox_list_se_parse_avec_zero_argument
    #[test]
    fn sandbox_list_se_parse_avec_zero_argument() {
        assert!(matches!(
            sandbox(&["vanyline", "sandbox", "list"]),
            Some(Commands::List)
        ));
    }

    // Scenario: sandbox_renommage_utilisateur_verrouille
    #[test]
    fn sandbox_renommage_utilisateur_verrouille() {
        assert!(Cli::try_parse_from(["vanyline", "sandbox", "list"]).is_ok());
        assert!(Cli::try_parse_from(["vanyline", "k8s-sandbox", "list"]).is_err());
    }

    // Scenario: show_delete_stop_start_exigent_un_nom
    #[test]
    fn show_delete_stop_start_exigent_un_nom() {
        for sub in ["show", "delete", "stop", "start"] {
            assert!(
                Cli::try_parse_from(["vanyline", "sandbox", sub]).is_err(),
                "{sub} sans nom doit échouer"
            );
        }
        assert!(matches!(
            sandbox(&["vanyline", "sandbox", "show", "sb1"]),
            Some(Commands::Show { name }) if name == "sb1"
        ));
    }

    // Scenario: create_exige_project_et_branch
    #[test]
    fn create_exige_project_et_branch() {
        assert!(Cli::try_parse_from(["vanyline", "sandbox", "create", "sb1"]).is_err());
        match sandbox(&[
            "vanyline",
            "sandbox",
            "create",
            "sb1",
            "--project",
            "app",
            "--branch",
            "main",
        ]) {
            Some(Commands::Create {
                project,
                branch,
                toolchains,
                image,
                ..
            }) => {
                assert_eq!(project, "app");
                assert_eq!(branch, "main");
                assert!(toolchains.is_empty());
                assert_eq!(image, None);
            }
            _ => panic!("expected Create"),
        }
    }

    // Scenario: toolchain_flag_au_long_singulier_verrouille
    #[test]
    fn toolchain_flag_au_long_singulier_verrouille() {
        assert!(
            Cli::try_parse_from([
                "vanyline",
                "sandbox",
                "create",
                "sb1",
                "--project",
                "app",
                "--branch",
                "main",
                "--toolchain",
                "rust=rust:slim-trixie",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "vanyline",
                "sandbox",
                "create",
                "sb1",
                "--project",
                "app",
                "--branch",
                "main",
                "--toolchains",
                "rust=rust:slim-trixie",
            ])
            .is_err()
        );
    }

    // Scenario: toolchain_repete_accumule_dans_lordre_du_argv
    #[test]
    fn toolchain_repete_accumule_dans_lordre_du_argv() {
        match sandbox(&[
            "vanyline",
            "sandbox",
            "create",
            "sb1",
            "--project",
            "app",
            "--branch",
            "main",
            "--toolchain",
            "rust=rust:slim-trixie",
            "--toolchain",
            "node=node:trixie-slim",
        ]) {
            Some(Commands::Create { toolchains, .. }) => {
                assert_eq!(toolchains.len(), 2);
                assert_eq!(toolchains[0].name, "rust");
                assert_eq!(toolchains[1].name, "node");
                for tc in &toolchains {
                    assert!(tc.env.is_empty());
                    assert!(tc.lsp.is_none());
                }
            }
            _ => panic!("expected Create"),
        }
    }

    // Scenario: toolchain_valeur_invalide_fait_echouer_la_parse
    #[test]
    fn toolchain_valeur_invalide_fait_echouer_la_parse() {
        assert_eq!(
            Cli::try_parse_from([
                "vanyline",
                "sandbox",
                "create",
                "sb1",
                "--project",
                "app",
                "--branch",
                "main",
                "--toolchain",
                "nope",
            ])
            .err()
            .map(|e| e.kind()),
            Some(clap::error::ErrorKind::ValueValidation)
        );
    }

    // Scenario: create_ne_valide_aucune_valeur
    #[test]
    fn create_ne_valide_aucune_valeur() {
        match sandbox(&[
            "vanyline",
            "sandbox",
            "create",
            "sb 1!",
            "--project",
            "",
            "--branch",
            "??",
            "--image",
            "banane",
        ]) {
            Some(Commands::Create {
                name,
                project,
                branch,
                image,
                ..
            }) => {
                assert_eq!(name, "sb 1!");
                assert_eq!(project, "");
                assert_eq!(branch, "??");
                assert_eq!(image.as_deref(), Some("banane"));
            }
            _ => panic!("expected Create"),
        }
    }

    // Scenario: sandbox_flag_image_repete_refuse
    #[test]
    fn sandbox_flag_image_repete_refuse() {
        let err = Cli::try_parse_from([
            "vanyline",
            "sandbox",
            "create",
            "sb1",
            "--project",
            "app",
            "--branch",
            "main",
            "--image",
            "a",
            "--image",
            "b",
        ])
        .err();
        assert_eq!(
            err.as_ref().map(clap::Error::kind),
            Some(clap::error::ErrorKind::ArgumentConflict)
        );
        assert!(err.is_some_and(|e| e.to_string().contains("cannot be used multiple times")));
    }

    // Scenario: aide_sandbox_documente_variants_et_flags
    #[test]
    fn aide_sandbox_documente_variants_et_flags() {
        use clap::Subcommand;
        let mut cmd = <Commands as Subcommand>::augment_subcommands(clap::Command::new("sandbox"));
        let help = cmd.render_help().to_string();
        for needle in [
            "List K8s Sandboxes",
            "Show a single Sandbox",
            "Create a Sandbox",
            "Delete a Sandbox",
            "Stop a Sandbox (suspend the pod without deleting the resource)",
            "Start a previously stopped Sandbox",
        ] {
            assert!(
                help.contains(needle),
                "aide sandbox sans « {needle} »:\n{help}"
            );
        }
        // Les flags vivent sur `create` : leur aide se lit sur la sous-commande.
        let create_help = cmd
            .find_subcommand_mut("create")
            .expect("sous-commande create")
            .render_help()
            .to_string();
        for needle in [
            "--project",
            "--branch",
            "--toolchain",
            "--image",
            "Repetable",
        ] {
            assert!(
                create_help.contains(needle),
                "aide sandbox create sans « {needle} »:\n{create_help}"
            );
        }
    }
}
