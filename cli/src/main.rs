mod chat;
mod config;
mod config_check;
mod config_store;
mod fs_store;
mod store;

mod tools;

mod model_cmd;
mod toolset_cmd;
mod skill_cmd;
mod mcp_cmd;

use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

#[derive(Parser)]
#[command(name = "vanyline", version, about = "CLI for vanyline LLM chat")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Chat with an LLM agent (REPL or one-shot)
    Chat {
        /// One-shot message (skip REPL if provided)
        message: Option<String>,
        /// Agent name to use
        #[arg(short, long)]
        agent: Option<String>,
        /// Conversation ID to continue
        #[arg(short, long)]
        conversation: Option<uuid::Uuid>,
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
    /// Manage toolsets
    #[command(subcommand)]
    Toolsets(toolset_cmd::Commands),
    /// Manage skills
    #[command(subcommand)]
    Skills(skill_cmd::Commands),
    /// Manage MCP servers
    #[command(subcommand)]
    Mcp(mcp_cmd::Commands),
    /// Validate configuration (both layers)
    #[command(subcommand)]
    Config(config_cmd::Commands),
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
        Show { id: uuid::Uuid },
        /// Delete a conversation
        Delete { id: uuid::Uuid },
        /// Set active conversation
        Set { id: uuid::Uuid },
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
        Commands::Chat {
            message,
            agent,
            conversation,
        } => chat::run(message, agent, conversation).await,
        Commands::Conversations(cmd) => run_conversation(cmd).await,
        Commands::Agents(cmd) => run_agent(cmd).await,
        Commands::Providers(cmd) => run_provider(cmd).await,
        Commands::Models(cmd) => run_models(cmd).await,
        Commands::Toolsets(cmd) => run_toolsets(cmd).await,
        Commands::Skills(cmd) => run_skills(cmd).await,
        Commands::Mcp(cmd) => run_mcp(cmd).await,
        Commands::Config(cmd) => run_config(cmd).await,
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
                for c in &convs {
                    let agent_label = c.agent.as_deref().unwrap_or("(none)");
                    let title = c.title.as_deref().unwrap_or("(untitled)");
                    println!(
                        "  {} | {} | {} messages | {}",
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
        Show { id } => {
            let conv = store::get_conversation(&id).expect("conversation not found");
            for msg in &conv.messages {
                println!("[{}] {}", msg.role, msg.content);
            }
        }
        Delete { id } => {
            store::delete_conversation(&id).expect("failed to delete conversation");
            println!("Deleted conversation: {}", id);
        }
        Set { id } => {
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
                    let source = config::config_entry_source(store.layers(), &p.name, |raw| &raw.providers);
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
                let source = config::config_entry_source(store.layers(), &m.name, |raw| &raw.models);
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
