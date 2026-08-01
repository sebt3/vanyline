mod chat;
mod config;
mod config_check;
mod fs_store;
mod store;

mod tools;

mod mcp_cmd;
mod model_cmd;
mod owner_cmd;
mod project_cmd;
mod skill_cmd;
mod toolset_cmd;
mod sandbox_cmd;

mod rpc;

use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

#[derive(Parser)]
#[command(name = "vanyline", version, about = "CLI for vanyline LLM chat")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    /// Agent name to use (REPL or `run`)
    #[arg(short, long, global = true)]
    agent: Option<String>,
    /// Continue the active conversation instead of starting a new one
    #[arg(short = 'c', long = "continue", global = true)]
    continue_active: bool,
    /// Namespace K8s target (owner/project/sandbox). Overrides `defaults.namespace`.
    #[arg(short, long, global = true)]
    namespace: Option<String>,
    /// Nom de la sandbox a utiliser comme toolbox d'inference (remplace les
    /// local_tools par les tools MCP de la sandbox). Surcharge
    /// `defaults.toolbox`.
    #[arg(long, global = true)]
    toolbox: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// One-shot message to an LLM agent
    Run { message: String },
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
    /// Manage toolsets
    #[command(subcommand)]
    Toolsets(toolset_cmd::Commands),
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
    /// Validate configuration (both layers)
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

mod config_cmd {
    use clap::Subcommand;

    #[derive(Subcommand)]
    pub enum Commands {
        /// Validate configuration (both layers)
        Check,
    }
}

#[tokio::main]
async fn main() {
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
            let toolbox_mcp_url =
                resolve_toolbox_mcp_url(cli.toolbox.clone(), cli.namespace.clone()).await;
            chat::run(None, cli.agent, cli.continue_active, toolbox_mcp_url).await
        }
        Some(Commands::Run { message }) => {
            let toolbox_mcp_url =
                resolve_toolbox_mcp_url(cli.toolbox.clone(), cli.namespace.clone()).await;
            chat::run(Some(message), cli.agent, cli.continue_active, toolbox_mcp_url).await
        }
        Some(Commands::Conversations(cmd)) => run_conversation(cmd).await,
        Some(Commands::Agents(cmd)) => run_agent(cmd).await,
        Some(Commands::Providers(cmd)) => run_provider(cmd).await,
        Some(Commands::Models(cmd)) => run_models(cmd).await,
        Some(Commands::Toolsets(cmd)) => run_toolsets(cmd).await,
        Some(Commands::Skills(cmd)) => run_skills(cmd).await,
        Some(Commands::Mcp(cmd)) => run_mcp(cmd).await,
        Some(Commands::Config(cmd)) => run_config(cmd).await,
        Some(Commands::Serve { stdio }) => {
            if stdio {
                rpc::run_stdio_server().await;
            } else {
                eprintln!("vanyline serve: only --stdio is currently supported");
                std::process::exit(1);
            }
        }
        Some(Commands::K8sOwner(cmd)) => run_owner_k8s(cmd, cli.namespace).await,
        Some(Commands::K8sProject(cmd)) => run_project_k8s(cmd, cli.namespace).await,
        Some(Commands::K8sSandbox(cmd)) => run_sandbox_k8s(cmd, cli.namespace).await,
    }
}

async fn run_conversation(cmd: conversation::Commands) {
    use conversation::Commands::*;
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
                        i + 1,
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
            };
            store::save_conversation(&conv).expect("failed to save conversation");
            println!("Created conversation: {}", id);
        }
        Show { reference } => {
            let convs = store::list_conversations().unwrap_or_default();
            let id =
                store::resolve_conversation_reference(&convs, &reference).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
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
            store::delete_conversation(&id).expect("failed to delete conversation");
            println!("Deleted conversation: {}", id);
        }
        Set { reference } => {
            let convs = store::list_conversations().unwrap_or_default();
            let id =
                store::resolve_conversation_reference(&convs, &reference).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
            store::get_conversation(&id).expect("conversation not found");
            store::set_active_conversation(&id).expect("failed to set active conversation");
            println!("Active conversation: {}", id);
        }
    }
}

async fn run_agent(cmd: agent::Commands) {
    use agent::Commands::*;
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
                    let source = config::file_entry_source(store.layers(), "agents", "md", &a.name);
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
            config::set_default_agent(store.layers(), &name).unwrap_or_else(|e| {
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
            let source = config::file_entry_source(store.layers(), "agents", "md", &agent.name);
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

async fn run_provider(cmd: provider::Commands) {
    use provider::Commands::*;
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
                    let source =
                        config::config_entry_source(store.layers(), &p.name, |raw| &raw.providers);
                    println!("  {} | {} | {:?}", p.name, source, p.provider_type);
                }
            }
        }
    }
}

pub(crate) fn discover_fs_store() -> fs_store::FsConfigStore {
    let cwd = std::env::current_dir().unwrap_or_else(|e| {
        eprintln!("Failed to read current directory: {e}");
        std::process::exit(1);
    });
    fs_store::FsConfigStore::new(config::Layers::discover(&cwd))
}

async fn run_config(cmd: config_cmd::Commands) {
    use config_cmd::Commands::*;
    match cmd {
        Check => {
            let store = discover_fs_store();
            let problems = config_check::check_config(&store).await;
            if problems.is_empty() {
                println!("Config OK — no problems found.");
            } else {
                for p in &problems {
                    println!("  {p}");
                }
                std::process::exit(1);
            }
        }
    }
}

async fn run_models(cmd: model_cmd::Commands) {
    use model_cmd::Commands::*;
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
                let source =
                    config::config_entry_source(store.layers(), &m.name, |raw| &raw.models);
                println!("  {} | {} | {} / {}", m.name, source, m.provider, m.model);
            }
        }
    }
}

async fn run_toolsets(cmd: toolset_cmd::Commands) {
    use toolset_cmd::Commands::*;
    use vanyline_lib::store::ConfigStore;
    let store = discover_fs_store();
    match cmd {
        List => {
            let toolsets = store.list_toolsets().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if toolsets.is_empty() {
                println!("No toolsets configured.");
                return;
            }
            for t in &toolsets {
                let source = config::file_entry_source(store.layers(), "toolsets", "yaml", &t.name);
                let summary = t.description.as_deref().unwrap_or("-");
                println!("  {} | {} | {}", t.name, source, summary);
            }
        }
    }
}

async fn run_skills(cmd: skill_cmd::Commands) {
    use skill_cmd::Commands::*;
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
                let source = config::skill_entry_source(store.layers(), &s.name);
                println!("  {} | {} | {}", s.name, source, s.description);
            }
        }
    }
}

async fn run_mcp(cmd: mcp_cmd::Commands) {
    use mcp_cmd::Commands::*;
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
                let source = config::config_entry_source(store.layers(), &s.name, |raw| &raw.mcp);
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
async fn discover_k8s_client(namespace_flag: Option<String>) -> vanyline_lib::k8s::VnlK8sClient {
    let store = discover_fs_store();
    let namespace = namespace_flag.or_else(|| config::configured_namespace(store.layers()));
    vanyline_lib::k8s::VnlK8sClient::discover(namespace).await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    })
}

/// Resout l'URL MCP de la toolbox pour ce lancement, ou `None` si aucune
/// toolbox n'est demandee (ni `--toolbox`, ni `defaults.toolbox`) — dans
/// ce cas, PAS d'appel K8s du tout (coherent avec "les commandes non-K8s
/// continuent de fonctionner", meme principe que `discover_k8s_client`
/// mais applique ici au chemin chat par defaut).
async fn resolve_toolbox_mcp_url(
    toolbox_flag: Option<String>,
    namespace: Option<String>,
) -> Option<String> {
    let store = discover_fs_store();
    let name = toolbox_flag.or_else(|| config::configured_toolbox(store.layers()))?;
    let client = discover_k8s_client(namespace).await;
    Some(client.sandbox_mcp_url(&name).await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    }))
}

async fn run_owner_k8s(cmd: owner_cmd::Commands, namespace: Option<String>) {
    use owner_cmd::Commands::*;
    use kube::ResourceExt;
    let client = discover_k8s_client(namespace).await;
    match cmd {
        List => {
            let owners = client.list_owners().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if owners.is_empty() {
                println!("No owners found.");
            } else {
                for o in &owners {
                    println!(
                        "  {} | {} | {}",
                        o.name_any(),
                        o.status.as_ref().and_then(|s| s.pvc_name.as_deref()).unwrap_or("-"),
                        o.status.as_ref().and_then(|s| s.service_account.as_deref()).unwrap_or("-")
                    );
                }
            }
        }
        Show { name } => {
            let owner = client.get_owner(&name).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Owner: {}", owner.name_any());
            println!(
                "  existingPvc: {}",
                owner.spec.existing_pvc.as_deref().unwrap_or("-")
            );
            println!(
                "  homeSize: {}",
                owner.spec.home_size.as_deref().unwrap_or("-")
            );
            println!(
                "  homeStorageClass: {}",
                owner.spec.home_storage_class.as_deref().unwrap_or("-")
            );
            if let Some(pd) = &owner.spec.project_defaults {
                println!("  projectDefaults:");
                println!(
                    "    storageSize: {}",
                    pd.storage_size.as_deref().unwrap_or("-")
                );
                println!(
                    "    storageClass: {}",
                    pd.storage_class.as_deref().unwrap_or("-")
                );
            } else {
                println!("  projectDefaults: -");
            }
            println!("Status:");
            match &owner.status {
                Some(status) => {
                    println!(
                        "  pvcName: {}",
                        status.pvc_name.as_deref().unwrap_or("-")
                    );
                    println!(
                        "  serviceAccount: {}",
                        status.service_account.as_deref().unwrap_or("-")
                    );
                    if status.conditions.is_empty() {
                        println!("  conditions: -");
                    } else {
                        println!("  conditions:");
                        for c in &status.conditions {
                            println!(
                                "    - {} status={} message={}",
                                c.type_, c.status, c.message
                            );
                        }
                    }
                }
                None => println!("  (not yet reconciled)"),
            }
        }
        Create {
            name,
            existing_pvc,
            home_size,
            home_storage_class,
            project_default_storage_size,
            project_default_storage_class,
        } => {
            let project_defaults = if project_default_storage_size.is_some()
                || project_default_storage_class.is_some()
            {
                Some(vanyline_crds::ProjectDefaults {
                    storage_size: project_default_storage_size,
                    storage_class: project_default_storage_class,
                })
            } else {
                None
            };
            let spec = vanyline_crds::OwnerSpec {
                existing_pvc,
                home_size,
                home_storage_class,
                project_defaults,
            };
            let _owner = client.create_owner(&name, spec).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Created owner: {name}");
        }
        Delete { name } => {
            client.delete_owner(&name).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Deleted owner: {name}");
        }
    }
}

async fn run_project_k8s(cmd: project_cmd::Commands, namespace: Option<String>) {
    use project_cmd::Commands::*;
    use kube::ResourceExt;
    let client = discover_k8s_client(namespace).await;
    match cmd {
        List => {
            let projects = client.list_projects().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if projects.is_empty() {
                println!("No projects found.");
            } else {
                for p in &projects {
                    let cloned = p.status.as_ref().map(|s| s.cloned).unwrap_or(false);
                    println!(
                        "  {} | owner={} | {} | cloned={}",
                        p.name_any(), p.spec.owner, p.spec.repo_url, cloned
                    );
                }
            }
        }
        Show { name } => {
            let project = client.get_project(&name).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Project: {}", project.name_any());
            println!("  owner: {}", project.spec.owner);
            println!("  repoUrl: {}", project.spec.repo_url);
            println!("  defaultBranch: {}", project.spec.default_branch.as_deref().unwrap_or("-"));
            match &project.spec.existing_pvc {
                Some(pvc) => println!("  existingPvc: {} (subPath={})", pvc.name, pvc.sub_path.as_deref().unwrap_or("-")),
                None => println!("  existingPvc: -"),
            }
            println!("  storageSize: {}", project.spec.storage_size.as_deref().unwrap_or("-"));
            println!("  storageClass: {}", project.spec.storage_class.as_deref().unwrap_or("-"));
            println!("  gitSecret: {}", project.spec.git_secret.as_deref().unwrap_or("-"));
            match &project.spec.caches {
                Some(c) if !c.is_empty() => println!("  caches: {}", c.join(", ")),
                _ => println!("  caches: -"),
            }
            println!("  fetchInterval: {}", project.spec.fetch_interval.as_deref().unwrap_or("-"));
            println!("Status:");
            match &project.status {
                Some(status) => {
                    println!("  pvcName: {}", status.pvc_name.as_deref().unwrap_or("-"));
                    println!("  cloned: {}", status.cloned);
                    if status.worktrees.is_empty() {
                        println!("  worktrees: -");
                    } else {
                        println!("  worktrees: {}", status.worktrees.join(", "));
                    }
                    if status.conditions.is_empty() {
                        println!("  conditions: -");
                    } else {
                        println!("  conditions:");
                        for c in &status.conditions {
                            println!("    - {} status={} message={}", c.type_, c.status, c.message);
                        }
                    }
                }
                None => println!("  (not yet reconciled)"),
            }
        }
        Create {
            name,
            owner,
            repo_url,
            default_branch,
            existing_pvc_name,
            existing_pvc_subpath,
            storage_size,
            storage_class,
            git_secret,
            caches,
            fetch_interval,
        } => {
            let existing_pvc = existing_pvc_name.map(|pvc_name| vanyline_crds::PvcRef {
                name: pvc_name,
                sub_path: existing_pvc_subpath,
            });
            let spec = vanyline_crds::ProjectSpec {
                owner,
                repo_url,
                default_branch,
                existing_pvc,
                storage_size,
                storage_class,
                git_secret,
                caches: if caches.is_empty() { None } else { Some(caches) },
                fetch_interval,
            };
            client.create_project(&name, spec).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Created project: {name}");
        }
        Delete { name } => {
            client.delete_project(&name).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Deleted project: {name}");
        }
    }
}

async fn run_sandbox_k8s(cmd: sandbox_cmd::Commands, namespace: Option<String>) {
    use sandbox_cmd::Commands::*;
    use kube::ResourceExt;
    let client = discover_k8s_client(namespace).await;
    match cmd {
        List => {
            let sandboxes = client.list_sandboxes().await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            if sandboxes.is_empty() {
                println!("No sandboxes found.");
            } else {
                for s in &sandboxes {
                    let phase = s
                        .status
                        .as_ref()
                        .and_then(|st| st.phase.as_deref())
                        .unwrap_or("-");
                    println!(
                        "  {} | project={} | branch={} | phase={}",
                        s.name_any(), s.spec.project, s.spec.branch, phase
                    );
                }
            }
        }
        Show { name } => {
            let sandbox = client.get_sandbox(&name).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Sandbox: {}", sandbox.name_any());
            println!("  project: {}", sandbox.spec.project);
            println!("  branch: {}", sandbox.spec.branch);
            println!("  image: {}", sandbox.spec.image.as_deref().unwrap_or("-"));
            if sandbox.spec.toolchains.is_empty() {
                println!("  toolchains: -");
            } else {
                println!("  toolchains:");
                for tc in &sandbox.spec.toolchains {
                    println!("    - {} = {}", tc.name, tc.image);
                }
            }
            println!("Status:");
            match &sandbox.status {
                Some(status) => {
                    println!("  phase: {}", status.phase.as_deref().unwrap_or("-"));
                    println!("  service: {}", status.service.as_deref().unwrap_or("-"));
                    if status.conditions.is_empty() {
                        println!("  conditions: -");
                    } else {
                        println!("  conditions:");
                        for c in &status.conditions {
                            println!("    - {} status={} message={}", c.type_, c.status, c.message);
                        }
                    }
                }
                None => println!("  (not yet reconciled)"),
            }
        }
        Create {
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
            };
            client.create_sandbox(&name, spec).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Created sandbox: {name}");
        }
        Delete { name } => {
            client.delete_sandbox(&name).await.unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Deleted sandbox: {name}");
        }
    }
}
