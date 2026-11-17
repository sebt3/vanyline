//! Racine du binaire `vanyline` (crate `cli`, package cargo `vanyline`) : point
//! d'entrée du processus, grammaire clap complète et handlers `run_*` qui rendent
//! la sortie stdout/stderr et fixent les codes de sortie de la quasi-totalité des
//! commandes — contrat complet : `main.sdd` §Purpose.

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
mod config;
// Ancien `config check` (cfgstore) : remplacé par `config_cmd` (E9). Retiré avec `vanyline-cfgstore` (lot 6).
#[allow(dead_code)]
mod config_check;
mod store;

// Hôte lymaille (host/mod.sdd) : aucun consommateur avant les Solutions chat et batch.
#[allow(dead_code)]
mod host;

// Codes de sortie (exit_code.sdd) : aucun consommateur avant les Solutions run et script.
#[allow(dead_code)]
mod exit_code;

// Ouverture de session lymaille (session_open.sdd) : aucun consommateur avant les Solutions chat et run.
#[allow(dead_code)]
mod session_open;

// Commande `run` (run.sdd) : dispatch dans `main`.
mod run;

// Owner, namespace et réglages utilisateur (target.sdd) : consommé par chat et run ; `resolve_namespace`
// attend les commandes d'objets (E9, suite). Retirer le allow quand il n'y a plus de code mort.
#[allow(dead_code)]
mod target;

mod tools;

// Synchronisation de `.vanyline/` (sync/mod.sdd), dispatch dans `main` (sync/command.sdd).
mod sync;

mod config_cmd;
mod mcp_cmd;
mod model_cmd;
mod owner_cmd;
mod project_cmd;
mod sandbox_cmd;
mod secret_cmd;
mod sessions_cmd;
mod skill_cmd;
// Spécification complète des CRD (apply/edit, E9) : `parse_spec` et `check_immutable` ; apply/edit en attente
// d'un remplacement du client K8s. Retirer le allow quand un consommateur existe.
#[allow(dead_code)]
mod spec_edit;
mod toolset_cmd;

mod rpc;
mod tui;

use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;
use vanyline_cfgstore::layers::{config_entry_source, file_entry_source, skill_entry_source};

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
    /// Projects et les Sandboxes. Requis par les commandes project/sandbox et
    /// par `--toolbox` (UN OWNER = UN NAMESPACE, 2026-10-04).
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
    /// Manage conversations
    #[command(subcommand)]
    Conversations(conversation::Commands),
    /// Manage agents
    #[command(subcommand)]
    Agents(agent::Commands),
    /// Manage LLM providers
    #[command(subcommand)]
    Providers(provider::Commands),
    /// Manage model profiles
    #[command(subcommand)]
    Models(model_cmd::Commands),
    /// Lister les toolsets, promouvoir un brouillon (`toolset_cmd.sdd`)
    #[command(name = "toolset", subcommand)]
    Toolset(toolset_cmd::Commands),
    /// Enregistrer un secret dans le niveau utilisateur (`secret_cmd.sdd`)
    #[command(subcommand)]
    Secret(secret_cmd::Commands),
    /// Manage skills
    #[command(subcommand)]
    Skills(skill_cmd::Commands),
    /// Manage MCP servers
    #[command(subcommand)]
    Mcp(mcp_cmd::Commands),
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
    /// Run a JSON-RPC 2.0 server (for the VS Code extension, or any programmatic client)
    Serve {
        /// Use stdio transport (the only transport supported in v1)
        #[arg(long)]
        stdio: bool,
    },
}

mod conversation {
    use clap::Subcommand;

    #[derive(Subcommand)]
    pub enum Commands {
        /// List conversations
        List,
        /// Create a new conversation
        New {
            /// Agent name to use
            #[arg(short, long)]
            agent: Option<String>,
            /// Title for the conversation
            #[arg(short, long)]
            title: Option<String>,
        },
        /// Show conversation messages
        Show { reference: String },
        /// Delete a conversation
        Delete { reference: String },
        /// Set active conversation
        Set { reference: String },
    }
}

mod agent {
    use clap::Subcommand;

    #[derive(Subcommand)]
    pub enum Commands {
        /// List available agents
        List,
        /// Set default agent
        SetDefault { name: String },
        /// Show resolved detail for an agent (model → provider, toolsets expanded)
        Show { name: String },
    }
}

mod provider {
    use clap::Subcommand;

    #[derive(Subcommand)]
    pub enum Commands {
        /// List LLM providers
        List,
    }
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
// main.sdd (Must : aucune logique) : le dispatch est une table, une branche par famille de commandes. L'allow
// reste jusqu'au lot 6, qui retire les branches des commandes anciennes.
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
            return std::process::ExitCode::from(2);
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
            return exit_code::from_outcome(chat::run_chat(args, global).await);
        }
        Some(Commands::Sessions(commande)) => {
            return exit_code::from_outcome(sessions_cmd::run_sessions(
                commande,
                cli.repo.as_deref(),
            ));
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
            return exit_code::from_outcome(run::run_run(args, global).await);
        }
        Some(Commands::Conversations(cmd)) => run_conversation(cmd),
        Some(Commands::Agents(cmd)) => run_agent(cmd).await,
        Some(Commands::Providers(cmd)) => run_provider(cmd).await,
        Some(Commands::Models(cmd)) => run_models(cmd).await,
        Some(Commands::Toolset(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            return exit_code::from_outcome(toolset_cmd::run_toolset(cmd, &global).await);
        }
        Some(Commands::Secret(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            return exit_code::from_outcome(secret_cmd::run_secret(cmd, &global));
        }
        Some(Commands::Skills(cmd)) => run_skills(cmd).await,
        Some(Commands::Mcp(cmd)) => run_mcp(cmd).await,
        Some(Commands::Config(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            return exit_code::from_outcome(config_cmd::run_config(cmd, &global).await);
        }
        // tooling.sdd D6b — terminaison contractée : main.sdd §Handles « `serve`
        // nu → stderr `vanyline serve: only --stdio is currently supported` +
        // exit 1 » et §Raises (message exact en Handles).
        #[allow(clippy::print_stderr)]
        Some(Commands::Serve { stdio }) => {
            if stdio {
                rpc::run_stdio_server().await;
            } else {
                eprintln!("vanyline serve: only --stdio is currently supported");
                std::process::exit(1);
            }
        }
        Some(Commands::K8sOwner(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            return exit_code::from_outcome(owner_cmd::run_owner(cmd, &global).await);
        }
        Some(Commands::K8sProject(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            return exit_code::from_outcome(project_cmd::run_project(cmd, &global).await);
        }
        Some(Commands::Sync(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            return exit_code::from_outcome(sync::command::run_sync(cmd, &global).await);
        }
        Some(Commands::K8sSandbox(cmd)) => {
            let global =
                options_globales(cli.owner.clone(), cli.repo.clone(), cli.namespace.clone());
            return exit_code::from_outcome(sandbox_cmd::run_sandbox(cmd, &global).await);
        }
    }
    std::process::ExitCode::SUCCESS
}

// tooling.sdd D6b — canal rendu contracté : §Returns « Handlers de config »
// (sortie texte uniquement sur stdout hors erreurs) + §Handles « Rendu
// `conversations list` », « Rendu `conversations show` » et messages §Returns
// `Created/Deleted/Active conversation: {uuid}` ; référence irrésolue → message
// @Display sur stderr + exit 1 (§Raises « référence de conversation irrésolue »).
// Non-async mécanique : ce handler n'attendait rien (zéro `.await`), le rendu est
// identique.
#[allow(clippy::print_stdout, clippy::print_stderr)]
fn run_conversation(cmd: conversation::Commands) {
    use conversation::Commands::{Delete, List, New, Set, Show};
    match cmd {
        List => {
            let convs = store::list_conversations().unwrap_or_default();
            if convs.is_empty() {
                println!("No conversations.");
            } else {
                for (i, c) in convs.iter().enumerate() {
                    let agent_label = c.agent.as_deref().unwrap_or("(none)");
                    let title = c.title.as_deref().unwrap_or("(untitled)");
                    println!(
                        "  [{}] {} | {} | {} messages | {}",
                        i.saturating_add(1),
                        c.id,
                        title,
                        c.messages.len(),
                        agent_label
                    );
                }
            }
        }
        New { agent, title } => {
            let id = uuid::Uuid::new_v4();
            let conv = vanyline_lib::Conversation {
                id,
                agent,
                title,
                messages: Vec::new(),
                todo: None,
            };
            // tooling.sdd D6b — panic code 101 contracté : main.sdd §Raises
            // « Code 101 (panic par @expect — divergence documentée, jamais
            // nettoyée) » pour `failed to save conversation` ; convergence
            // [?] main.sdd l.547 non soldée.
            #[allow(clippy::expect_used)]
            store::save_conversation(&conv).expect("failed to save conversation");
            println!("Created conversation: {id}");
        }
        Show { reference } => {
            let convs = store::list_conversations().unwrap_or_default();
            let id =
                store::resolve_conversation_reference(&convs, &reference).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
            // tooling.sdd D6b — panic code 101 contracté : §Raises Code 101,
            // `conversation not found` (course listing↔rechargement) ; [?] l.547.
            #[allow(clippy::expect_used)]
            let conv = store::get_conversation(&id).expect("conversation not found");
            for msg in &conv.messages {
                println!("[{}] {}", msg.role, msg.content);
            }
        }
        Delete { reference } => {
            let convs = store::list_conversations().unwrap_or_default();
            let id =
                store::resolve_conversation_reference(&convs, &reference).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
            // tooling.sdd D6b — panic code 101 contracté : §Raises Code 101,
            // `failed to delete conversation` ; [?] l.547.
            #[allow(clippy::expect_used)]
            store::delete_conversation(&id).expect("failed to delete conversation");
            println!("Deleted conversation: {id}");
        }
        Set { reference } => {
            let convs = store::list_conversations().unwrap_or_default();
            let id =
                store::resolve_conversation_reference(&convs, &reference).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
            // tooling.sdd D6b — panic code 101 contracté : §Raises Code 101,
            // `conversation not found` (course) ; [?] l.547.
            #[allow(clippy::expect_used)]
            store::get_conversation(&id).expect("conversation not found");
            // tooling.sdd D6b — panic code 101 contracté : §Raises Code 101,
            // `failed to set active conversation` ; [?] l.547.
            #[allow(clippy::expect_used)]
            store::set_active_conversation(&id).expect("failed to set active conversation");
            println!("Active conversation: {id}");
        }
    }
}

// tooling.sdd D6b — une seule fonction par famille contractée : §Exposes
// « Handlers privés de rendu/dispatch (contrat de ce fichier) : … @run_agent »
// et §Structure « les handlers `run_*` ».
#[allow(clippy::too_many_lines)]
// tooling.sdd D6b — canal rendu contracté : §Handles « Rendu `agents list` »,
// « Rendu `agents show {name}` » (littéraux exacts) ; `Default agent set to:`
// §Returns ; §Raises « erreur store sur … `agents list`, `agents show`,
// `agents set-default` (écriture) » et `agent not found: {name}` sur stderr
// + exit 1.
#[allow(clippy::print_stdout, clippy::print_stderr)]
async fn run_agent(cmd: agent::Commands) {
    use agent::Commands::{List, SetDefault, Show};
    use vanyline_lib::store::ConfigStore;
    let store = discover_fs_store();

    match cmd {
        List => {
            let agents = store.list_agents().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let default_name = store.default_agent().await.ok().flatten();
            if agents.is_empty() {
                println!("No agents configured.");
            } else {
                for a in &agents {
                    let source = file_entry_source(store.layers(), "agents", "md", &a.name);
                    let summary = a.description.as_deref().unwrap_or("-");
                    let marker = if a.name == default_name.as_deref().unwrap_or("") {
                        " (default)"
                    } else {
                        ""
                    };
                    println!("  {} | {} | {}{}", a.name, source, summary, marker);
                }
            }
        }
        SetDefault { name } => {
            let agents = store.list_agents().await.unwrap_or_default();
            if !agents.iter().any(|a| a.name == name) {
                eprintln!("agent not found: {name}");
                std::process::exit(1);
            }
            store
                .set_default_agent(vanyline_cfgstore::store::Layer::Global, &name)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
            println!("Default agent set to: {name}");
        }
        Show { name } => {
            let agent = store.get_agent(&name).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });

            println!("Agent: {}", agent.name);
            let source = file_entry_source(store.layers(), "agents", "md", &agent.name);
            println!("  Source: {source}");
            if let Some(desc) = &agent.description {
                println!("  Description: {desc}");
            }
            println!("  Mode: {:?}", agent.mode);

            match store.get_model(&agent.model).await {
                Ok(model) => match store.get_provider(&model.provider).await {
                    Ok(provider) => println!(
                        "  Model: {} -> provider '{}' ({:?}, {}), model '{}'",
                        model.name,
                        provider.name,
                        provider.provider_type,
                        provider.endpoint,
                        model.model
                    ),
                    Err(_) => println!(
                        "  Model: {} -> provider '{}' (unknown)",
                        model.name, model.provider
                    ),
                },
                Err(_) => println!("  Model: {} (unknown)", agent.model),
            }

            println!("  Skills: {:?}", agent.skills);

            if agent.toolsets.is_empty() {
                println!("  Toolsets: (none)");
            } else {
                println!("  Toolsets:");
                for ts_name in &agent.toolsets {
                    match store.get_toolset(ts_name).await {
                        Ok(t) => {
                            let local = if t.local_tools.is_empty() {
                                "-".to_string()
                            } else {
                                t.local_tools.join(", ")
                            };
                            let mcp = if t.mcp.is_empty() {
                                "-".to_string()
                            } else {
                                t.mcp
                                    .iter()
                                    .map(|s| {
                                        let tools = if s.tools.is_empty() {
                                            "*".to_string()
                                        } else {
                                            s.tools.join(", ")
                                        };
                                        format!("{}: {}", s.server, tools)
                                    })
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            };
                            println!("    - {} : local=[{}], mcp=[{}]", t.name, local, mcp);
                        }
                        Err(_) => println!("    - {ts_name} (unknown)"),
                    }
                }
            }

            println!("  System prompt:");
            for line in agent.system_prompt.lines() {
                println!("    {line}");
            }
        }
    }
}

// tooling.sdd D6b — canal rendu contracté : §Handles « Rendu `providers list` »
// ; erreur store sur stderr + exit 1 (§Raises « erreur store sur
// `agents/providers/models/toolsets/skills/mcp list` »).
#[allow(clippy::print_stdout, clippy::print_stderr)]
async fn run_provider(cmd: provider::Commands) {
    use provider::Commands::List;
    use vanyline_lib::store::ConfigStore;
    let store = discover_fs_store();

    match cmd {
        List => {
            let providers = store.list_providers().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if providers.is_empty() {
                println!("No LLM providers configured.");
            } else {
                for p in &providers {
                    let source = config_entry_source(store.layers(), &p.name, |raw| &raw.providers);
                    println!("  {} | {} | {:?}", p.name, source, p.provider_type);
                }
            }
        }
    }
}

pub(crate) fn discover_fs_store() -> vanyline_cfgstore::fs_store::FsConfigStore {
    // tooling.sdd D6b — terminaison contractée : main.sdd §Raises « `Failed to
    // read current directory: {e}` dans @discover_fs_store » (stderr + exit 1).
    #[allow(clippy::print_stderr)]
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Failed to read current directory: {e}");
        std::process::exit(1);
    });
    vanyline_cfgstore::fs_store::FsConfigStore::new(config::discover_layers(&cwd))
}

// tooling.sdd D6b — canal rendu contracté : §Handles « Rendu `models list` » ;
// erreur store sur stderr + exit 1 (§Raises « erreur store sur
// `agents/providers/models/toolsets/skills/mcp list` »).
#[allow(clippy::print_stdout, clippy::print_stderr)]
async fn run_models(cmd: model_cmd::Commands) {
    use model_cmd::Commands::List;
    use vanyline_lib::store::ConfigStore;
    let store = discover_fs_store();
    match cmd {
        List => {
            let models = store.list_models().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if models.is_empty() {
                println!("No models configured.");
                return;
            }
            for m in &models {
                let source = config_entry_source(store.layers(), &m.name, |raw| &raw.models);
                println!("  {} | {} | {} / {}", m.name, source, m.provider, m.model);
            }
        }
    }
}

// tooling.sdd D6b — canal rendu contracté : §Handles « Rendu `skills list` » ;
// erreur store sur stderr + exit 1 (§Raises « erreur store sur
// `agents/providers/models/toolsets/skills/mcp list` »).
#[allow(clippy::print_stdout, clippy::print_stderr)]
async fn run_skills(cmd: skill_cmd::Commands) {
    use skill_cmd::Commands::List;
    use vanyline_lib::store::ConfigStore;
    let store = discover_fs_store();
    match cmd {
        List => {
            let skills = store.list_skills().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if skills.is_empty() {
                println!("No skills configured.");
                return;
            }
            for s in &skills {
                let source = skill_entry_source(store.layers(), &s.name);
                println!("  {} | {} | {}", s.name, source, s.description);
            }
        }
    }
}

// tooling.sdd D6b — canal rendu contracté : §Handles « Rendu `mcp list` » ;
// erreur store sur stderr + exit 1 (§Raises « erreur store sur
// `agents/providers/models/toolsets/skills/mcp list` »).
#[allow(clippy::print_stdout, clippy::print_stderr)]
async fn run_mcp(cmd: mcp_cmd::Commands) {
    use mcp_cmd::Commands::List;
    use vanyline_lib::store::ConfigStore;
    let store = discover_fs_store();
    match cmd {
        List => {
            let servers = store.list_mcp_servers().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if servers.is_empty() {
                println!("No MCP servers configured.");
                return;
            }
            for s in &servers {
                let source = config_entry_source(store.layers(), &s.name, |raw| &raw.mcp);
                println!("  {} | {} | {:?} {}", s.name, source, s.transport, s.url);
            }
        }
    }
}

/// Construit le client K8s pour les commandes owner/project/sandbox.
/// Précédence namespace : `--namespace` (flag global) > `defaults.namespace`
/// (config.yaml, fusionné) > namespace du contexte kubeconfig courant
/// (résolu par `VnlK8sClient::discover` elle-même si les deux premiers
/// sont absents). Erreur `VNL-K8S-001` propre si aucun cluster/kubeconfig
/// n'est joignable — affichée et le process quitte en erreur ; le reste du
/// CLI (chat, agents, config...) n'est pas affecté par cette fonction, elle
/// n'est appelée que par les commandes K8s.
#[cfg(test)]
mod tests {
    use super::*;

    // Grammaire de `run` (run.sdd, main.sdd) : `--context` obligatoire, `-c` et `--resume` exclusifs.

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
}
