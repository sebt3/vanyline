mod chat;
mod config;
mod store;

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
        /// Agent ID to use
        #[arg(short, long)]
        agent: Option<uuid::Uuid>,
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
    use uuid::Uuid;

    #[derive(Subcommand)]
    pub enum Commands {
        /// List conversations
        List,
        /// Create a new conversation
        New {
            /// Agent ID to use
            #[arg(short, long)]
            agent: Option<Uuid>,
            /// Title for the conversation
            #[arg(short, long)]
            title: Option<String>,
        },
        /// Show conversation messages
        Show { id: Uuid },
        /// Delete a conversation
        Delete { id: Uuid },
        /// Set active conversation
        Set { id: Uuid },
    }
}

mod agent {
    use clap::Subcommand;
    use uuid::Uuid;

    #[derive(Subcommand)]
    pub enum Commands {
        /// List available agents
        List,
        /// Set default agent
        SetDefault { id: Uuid },
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
                    let agents = store::list_agents().unwrap_or_default();
                    let agent_label = c
                        .agent_id
                        .and_then(|aid| agents.iter().find(|a| a.id == aid).map(|a| a.name.clone()))
                        .unwrap_or_default();
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
                agent_id: agent,
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
    match cmd {
        List => {
            let agents = store::list_agents().unwrap_or_default();
            let default_id = store::get_default_agent_id().unwrap_or_default();
            if agents.is_empty() {
                println!("No agents configured.");
            } else {
                for a in &agents {
                    let marker = if a.id == default_id { " (default)" } else { "" };
                    println!("  {} | {}{}", a.id, a.name, marker);
                    if let Some(ref desc) = a.description {
                        println!("      {}", desc);
                    }
                }
            }
        }
        SetDefault { id } => {
            store::list_agents()
                .unwrap_or_default()
                .iter()
                .find(|a| a.id == id)
                .expect("agent not found");
            store::set_default_agent_id(&id).expect("failed to set default agent");
            println!("Default agent set to: {}", id);
        }
    }
}

async fn run_provider(cmd: provider::Commands) {
    use provider::Commands::*;
    match cmd {
        List => {
            let providers = store::list_providers().unwrap_or_default();
            if providers.is_empty() {
                println!("No LLM providers configured.");
            } else {
                for p in &providers {
                    println!(
                        "  {} | {} | {} | model: {}",
                        p.id,
                        p.name,
                        p.provider_type,
                        p.default_model.as_deref().unwrap_or("(none)")
                    );
                }
            }
        }
    }
}
