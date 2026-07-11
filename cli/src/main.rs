mod chat;
mod config;
mod config_store;
mod store;

mod tools;

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
    let config_store = config_store::CliConfigStore::new(config::config_dir());
    use vanyline_lib::store::ConfigStore;

    match cmd {
        List => {
            let agents = config_store.list_agents().await.unwrap_or_default();
            let default_name = config_store.default_agent().await.ok().flatten();
            if agents.is_empty() {
                println!("No agents configured.");
            } else {
                for a in &agents {
                    let marker = if a.name == default_name.as_deref().unwrap_or("") {
                        " (default)"
                    } else {
                        ""
                    };
                    println!("  {}{}", a.name, marker);
                    if let Some(ref desc) = a.description {
                        println!("      {}", desc);
                    }
                }
            }
        }
        SetDefault { name } => {
            let agents = config_store.list_agents().await.unwrap_or_default();
            if !agents.iter().any(|a| a.name == name) {
                eprintln!("agent not found: {name}");
                std::process::exit(1);
            }
            config_store.set_default_agent_name(&name).expect("failed to set default agent");
            println!("Default agent set to: {name}");
        }
    }
}

async fn run_provider(cmd: provider::Commands) {
    use provider::Commands::*;
    let config_store = config_store::CliConfigStore::new(config::config_dir());
    use vanyline_lib::store::ConfigStore;

    match cmd {
        List => {
            let providers = config_store.list_providers().await.unwrap_or_default();
            if providers.is_empty() {
                println!("No LLM providers configured.");
            } else {
                for p in &providers {
                    println!("  {} | {:?}", p.name, p.provider_type);
                }
            }
        }
    }
}
