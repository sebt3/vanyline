//! Racine du binaire `vanyline` (crate `cli`, package cargo `vanyline`) : point
//! d'entrée du processus, grammaire clap et dispatch vers les modules de commande.
//! Aucune orchestration ici — contrat complet : `main.sdd` §Purpose.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::assert_is_empty,
        unsafe_code
    )
)]

mod chat;

// Hôte lymaille (host/mod.sdd) : aucun consommateur avant les Solutions chat et batch.
#[allow(dead_code)]
mod host;

// Codes de sortie (exit_code.sdd) : aucun consommateur avant les Solutions run et script.
#[allow(dead_code)]
mod exit_code;

// Ouverture de session lymaille (session_open.sdd) : aucun consommateur avant les Solutions chat et run.
#[allow(dead_code)]
mod session_open;

// Résumé de compaction par le fournisseur de la session (summarizer.sdd) : consommé par session_open.
#[allow(dead_code)]
mod summarizer;

// Commande `run` (run.sdd) : dispatch dans `main`.
mod run;

// Commande `script` (script.sdd) : dispatch dans `main`.
mod script;

// Owner, namespace et réglages utilisateur (target.sdd) : consommé par chat et run ; `resolve_namespace`
// attend les commandes d'objets (E9, suite). Retirer le allow quand il n'y a plus de code mort.
#[allow(dead_code)]
mod target;

// Synchronisation de `.vanyline/` (sync/mod.sdd), dispatch dans `main` (sync/command.sdd).
mod sync;

mod config_cmd;
mod owner_cmd;
mod project_cmd;
mod sandbox_cmd;
mod secret_cmd;
mod sessions_cmd;
// Spécification complète des CRD (apply/edit, E9) : `parse_spec` et `check_immutable` ; apply/edit en attente
// d'un remplacement du client K8s. Retirer le allow quand un consommateur existe.
#[allow(dead_code)]
mod spec_edit;
mod toolset_cmd;

mod tui;

use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

#[derive(Parser)]
#[command(name = "vanyline", version, about = "CLI for vanyline LLM chat")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    /// Dépôt dont le `.vanyline/` est le niveau partagé (`main.sdd`, `--repo`).
    #[arg(long, global = true)]
    repo: Option<std::path::PathBuf>,
    /// Namespace K8s target (owner/project/sandbox). Overrides `defaults.namespace`.
    /// Désigne le NAMESPACE D'APPLICATION (CR Owner/Application) ; le namespace
    /// de chaque Owner en est dérivé (`<app-ns>-<owner>`, UN OWNER = UN
    /// NAMESPACE 2026-10-04).
    #[arg(short, long, global = true)]
    namespace: Option<String>,
    /// Nom de l'Owner K8s dont le namespace (`<app-ns>-<owner>`) porte les
    /// Projects et les Sandboxes. Requis par les commandes qui parlent au
    /// cluster (UN OWNER = UN NAMESPACE, 2026-10-04 ; `--toolbox` est retiré).
    #[arg(long, global = true)]
    owner: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Chat interactif plein écran avec l'agent d'un projet
    Chat {
        /// Projet du contexte de travail
        #[arg(long)]
        context: String,
        /// Sandbox de travail (worktree lu pour le compteur de fichiers modifiés)
        #[arg(long)]
        sandbox: Option<String>,
        /// Reprendre la dernière session du projet
        #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
        continue_: bool,
        /// Reprendre une session précise
        #[arg(long)]
        resume: Option<String>,
    },
    /// Lister ou supprimer les sessions
    #[command(subcommand)]
    Sessions(sessions_cmd::Commands),
    /// Un tour de l'agent d'un projet, sans TUI (`run.sdd`)
    Run {
        /// Message du tour ; `-` lit l'entrée standard en entier
        message: String,
        /// Projet du contexte de travail
        #[arg(long)]
        context: String,
        /// Sandbox de travail
        #[arg(long)]
        sandbox: Option<String>,
        /// Reprendre la dernière session du projet
        #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
        continue_: bool,
        /// Reprendre une session précise
        #[arg(long)]
        resume: Option<String>,
        /// Profil de modèle du catalogue (`-m`)
        #[arg(short = 'm', long = "model")]
        model: Option<String>,
        /// Délai du tour, en secondes (absent ou 0 : aucun délai)
        #[arg(short = 't', long = "timeout")]
        timeout: Option<u64>,
        /// Sortie JSONL : un événement de session par ligne
        #[arg(long)]
        json: bool,
    },
    /// Exécuter un fichier Rhai sur une sandbox, sans LLM ni session (`script.sdd`)
    Script {
        /// Fichier Rhai à exécuter
        file: std::path::PathBuf,
        /// Projet du contexte de travail
        #[arg(long)]
        context: String,
        /// Sandbox de travail (obligatoire)
        #[arg(long)]
        sandbox: String,
        /// Paramètre `clé=valeur`, lu par le script avec `param("clé")` (répétable)
        #[arg(long = "param")]
        params: Vec<String>,
        /// Délai de l'exécution, en secondes (absent ou 0 : aucun délai)
        #[arg(short = 't', long = "timeout")]
        timeout: Option<u64>,
        /// Sortie JSON : un objet `{status, text}` sur la sortie standard
        #[arg(long)]
        json: bool,
    },
    /// Lister les toolsets, promouvoir un brouillon (`toolset_cmd.sdd`)
    #[command(name = "toolset", subcommand)]
    Toolset(toolset_cmd::Commands),
    /// Enregistrer un secret dans le niveau utilisateur (`secret_cmd.sdd`)
    #[command(subcommand)]
    Secret(secret_cmd::Commands),
    /// Manage K8s Owners
    #[command(name = "owner", subcommand)]
    K8sOwner(owner_cmd::Commands),
    /// Manage K8s Projects
    #[command(name = "project", subcommand)]
    K8sProject(project_cmd::Commands),
    /// Manage K8s Sandboxes
    #[command(name = "sandbox", subcommand)]
    K8sSandbox(sandbox_cmd::Commands),
    /// Copier `.vanyline/` vers la sandbox (`push`) ou depuis la sandbox (`pull`) (`sync/command.sdd`)
    #[command(subcommand)]
    Sync(sync::command::Commands),
    /// Valider les fichiers de configuration des deux niveaux (`config_cmd.sdd`)
    #[command(subcommand)]
    Config(config_cmd::Commands),
}

/// Options globales d'une commande qui parle au cluster ou au niveau de configuration (`main.sdd`, `@Cli`).
fn options_globales(
    owner: Option<String>,
    repo: Option<std::path::PathBuf>,
    namespace: Option<String>,
) -> chat::GlobalOptions {
    chat::GlobalOptions {
        owner,
        repo,
        namespace,
    }
}

#[tokio::main]
// main.sdd (Must : aucune logique) : le dispatch est une table, une branche par famille de commandes. La limite de
// longueur est dépassée d'une branche (`script`, script.sdd) : la table reste une table.
#[allow(clippy::too_many_lines)]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vanyline=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            use clap::CommandFactory;
            let _ = Cli::command().print_help();
            std::process::ExitCode::from(2)
        }
        Some(Commands::Chat {
            context,
            sandbox,
            continue_,
            resume,
        }) => {
            let args = chat::ChatArgs {
                context,
                sandbox,
                continue_,
                resume,
            };
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(chat::run_chat(args, global).await)
        }
        Some(Commands::Sessions(commande)) => {
            exit_code::from_outcome(sessions_cmd::run_sessions(commande, cli.repo.as_deref()))
        }
        Some(Commands::Run {
            message,
            context,
            sandbox,
            continue_,
            resume,
            model,
            timeout,
            json,
        }) => {
            let args = run::RunArgs {
                message,
                context,
                sandbox,
                continue_,
                resume,
                model,
                timeout,
                json,
            };
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(run::run_run(args, global).await)
        }
        Some(Commands::Script {
            file,
            context,
            sandbox,
            params,
            timeout,
            json,
        }) => {
            let args = script::ScriptArgs {
                file,
                context,
                sandbox,
                params,
                timeout,
                json,
            };
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(script::run_script(args, global).await)
        }
        Some(Commands::Toolset(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(toolset_cmd::run_toolset(cmd, &global).await)
        }
        Some(Commands::Secret(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(secret_cmd::run_secret(cmd, &global))
        }
        Some(Commands::Config(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(config_cmd::run_config(cmd, &global).await)
        }
        Some(Commands::K8sOwner(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(owner_cmd::run_owner(cmd, &global).await)
        }
        Some(Commands::K8sProject(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(project_cmd::run_project(cmd, &global).await)
        }
        Some(Commands::Sync(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(sync::command::run_sync(cmd, &global).await)
        }
        Some(Commands::K8sSandbox(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            exit_code::from_outcome(sandbox_cmd::run_sandbox(cmd, &global).await)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Grammaire de `run` (run.sdd, main.sdd) : `--context` obligatoire, `-c` et `--resume` exclusifs.

    // Grammaire de `script` (script.sdd, main.sdd) : `--context` et `--sandbox` obligatoires, `--param` répétable.
    #[test]
    fn script_parses_with_params_and_global_owner() {
        let cli = Cli::try_parse_from([
            "vanyline",
            "--owner",
            "alice",
            "script",
            "f.rhai",
            "--context",
            "p",
            "--sandbox",
            "s",
            "--param",
            "a=1",
            "--param",
            "b=2",
            "-t",
            "5",
            "--json",
        ])
        .unwrap();
        assert_eq!(cli.owner.as_deref(), Some("alice"));
        match cli {
            Cli {
                command:
                    Some(Commands::Script {
                        file,
                        context,
                        sandbox,
                        params,
                        timeout,
                        json,
                    }),
                ..
            } => {
                assert_eq!(file, std::path::PathBuf::from("f.rhai"));
                assert_eq!(context, "p");
                assert_eq!(sandbox, "s");
                assert_eq!(params, vec!["a=1".to_owned(), "b=2".to_owned()]);
                assert_eq!(timeout, Some(5));
                assert!(json);
            }
            _ => panic!("expected Commands::Script"),
        }
    }

    #[test]
    fn script_exige_le_sandbox() {
        assert!(
            Cli::try_parse_from(["vanyline", "script", "f.rhai", "--context", "p"]).is_err(),
            "la sandbox est obligatoire pour script"
        );
    }

    #[test]
    fn run_timeout_parses() {
        let cli =
            Cli::try_parse_from(["vanyline", "run", "hello", "--context", "p", "-t", "5"]).unwrap();
        match cli {
            Cli {
                command:
                    Some(Commands::Run {
                        message,
                        context,
                        model,
                        timeout,
                        json,
                        ..
                    }),
                ..
            } => {
                assert_eq!(message, "hello");
                assert_eq!(context, "p");
                assert_eq!(model, None);
                assert_eq!(timeout, Some(5));
                assert!(!json);
            }
            _ => panic!("expected Commands::Run"),
        }
    }

    #[test]
    fn run_json_flag_parses() {
        let cli =
            Cli::try_parse_from(["vanyline", "run", "hello", "--context", "p", "--json"]).unwrap();
        match cli {
            Cli {
                command:
                    Some(Commands::Run {
                        message,
                        timeout,
                        json,
                        ..
                    }),
                ..
            } => {
                assert_eq!(message, "hello");
                assert_eq!(timeout, None);
                assert!(json);
            }
            _ => panic!("expected Commands::Run"),
        }
    }

    #[test]
    fn run_defaults_no_flags() {
        let cli = Cli::try_parse_from(["vanyline", "run", "hello", "--context", "p"]).unwrap();
        match cli {
            Cli {
                command:
                    Some(Commands::Run {
                        message,
                        sandbox,
                        continue_,
                        resume,
                        model,
                        timeout,
                        json,
                        ..
                    }),
                ..
            } => {
                assert_eq!(message, "hello");
                assert_eq!(sandbox, None);
                assert!(!continue_);
                assert_eq!(resume, None);
                assert_eq!(model, None);
                assert_eq!(timeout, None);
                assert!(!json);
            }
            _ => panic!("expected Commands::Run"),
        }
    }

    #[test]
    fn run_rejects_unknown_flag() {
        let result = Cli::try_parse_from(["vanyline", "run", "hello", "--context", "p", "--nope"]);
        assert!(result.is_err());
    }

    #[test]
    fn run_model_flag_parses() {
        let cli = Cli::try_parse_from(["vanyline", "run", "hello", "--context", "p", "-m", "qwen"])
            .unwrap();
        match cli {
            Cli {
                command: Some(Commands::Run { message, model, .. }),
                ..
            } => {
                assert_eq!(message, "hello");
                assert_eq!(model, Some("qwen".to_string()));
            }
            _ => panic!("expected Commands::Run"),
        }
    }

    #[test]
    fn run_exige_le_contexte() {
        let result = Cli::try_parse_from(["vanyline", "run", "hello"]);
        assert!(result.is_err(), "--context est obligatoire");
    }

    #[test]
    fn run_continue_et_resume_exclusifs() {
        let result = Cli::try_parse_from([
            "vanyline",
            "run",
            "hello",
            "--context",
            "p",
            "-c",
            "--resume",
            "s-1",
        ]);
        assert!(result.is_err(), "-c et --resume s'excluent");
    }

    // Grammaire E9 (`main.sdd`) : `secret set`, `config check`, `toolset list|promote`
    #[test]
    fn grammaire_secret_config_toolset() {
        assert!(matches!(
            Cli::try_parse_from(["vanyline", "secret", "set", "api"]).ok().and_then(|c| c.command),
            Some(Commands::Secret(secret_cmd::Commands::Set { nom })) if nom == "api"
        ));
        assert!(matches!(
            Cli::try_parse_from(["vanyline", "config", "check"])
                .ok()
                .and_then(|c| c.command),
            Some(Commands::Config(config_cmd::Commands::Check))
        ));
        assert!(matches!(
            Cli::try_parse_from(["vanyline", "toolset", "list", "--json"])
                .ok()
                .and_then(|c| c.command),
            Some(Commands::Toolset(toolset_cmd::Commands::List {
                json: true
            }))
        ));
        assert!(matches!(
            Cli::try_parse_from(["vanyline", "toolset", "promote", "x", "--yes"])
                .ok()
                .and_then(|c| c.command),
            Some(Commands::Toolset(toolset_cmd::Commands::Promote {
                yes: true,
                ..
            }))
        ));
    }

    // Grammaire E9 : la valeur d'un secret n'est jamais un argument ; `toolsets` (cfgstore) est retiré
    #[test]
    fn secret_set_refuse_la_valeur_en_argument_et_toolsets_retire() {
        assert!(Cli::try_parse_from(["vanyline", "secret", "set", "api", "valeur"]).is_err());
        assert!(Cli::try_parse_from(["vanyline", "toolsets", "list"]).is_err());
    }

    #[test]
    fn run_message_tiret_lit_l_entree() {
        let cli = Cli::try_parse_from(["vanyline", "run", "-", "--context", "p"]).unwrap();
        match cli {
            Cli {
                command: Some(Commands::Run { message, .. }),
                ..
            } => assert_eq!(message, "-"),
            _ => panic!("expected Commands::Run"),
        }
    }

    // Scenario « commandes retirées » (`main.sdd`) : clap rejette les anciennes commandes et l'option `--toolbox`.
    #[test]
    fn commandes_retirees_rejetees() {
        for args in [
            vec!["vanyline", "conversations", "list"],
            vec!["vanyline", "agents", "list"],
            vec!["vanyline", "providers", "list"],
            vec!["vanyline", "models", "list"],
            vec!["vanyline", "skills", "list"],
            vec!["vanyline", "mcp", "list"],
            vec!["vanyline", "serve", "--stdio"],
            vec!["vanyline", "chat", "--context", "p", "--toolbox", "s"],
        ] {
            assert!(
                Cli::try_parse_from(args.clone()).is_err(),
                "{args:?} doit être rejeté"
            );
        }
    }
}
