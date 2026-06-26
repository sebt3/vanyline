use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{config::ensure_config_dir, store};

struct StdoutSink;

#[async_trait]
impl vanyline_lib::ChatSink for StdoutSink {
    async fn send_token(&self, content: &str) {
        print!("{content}");
        std::io::Write::flush(&mut std::io::stdout()).ok();
    }

    async fn send_tool_call(&self, name: &str, args: &serde_json::Value) {
        print!("\n[tool] {name}({args})\n");
        std::io::Write::flush(&mut std::io::stdout()).ok();
    }

    async fn send_done(&self) {
        println!();
    }

    async fn send_error(&self, code: &str, message: &str) {
        eprintln!("[{code}] {message}");
    }
}

pub async fn run(message: Option<String>, agent: Option<Uuid>, conversation: Option<Uuid>) {
    ensure_config_dir();

    if let Some(msg) = message {
        run_one_shot(&msg, agent, conversation).await;
        return;
    }

    run_repl(agent, conversation).await;
}

async fn run_one_shot(user_msg: &str, agent: Option<Uuid>, conversation: Option<Uuid>) {
    let (mut conv, agent_config, is_new) = resolve_context(agent, conversation).await;
    if is_new {
        println!("Session: {}", conv.id);
    }
    if let Ok(response) = process_turn(&mut conv, &agent_config, user_msg).await {
        conv.messages.push(vanyline_lib::Message {
            role: "assistant".to_string(),
            content: response,
        });
        store::save_conversation(&conv).ok();
    } else {
        std::process::exit(1);
    }
}

async fn run_repl(agent: Option<Uuid>, conversation: Option<Uuid>) {
    let (mut conv, agent_config, _) = resolve_context(agent, conversation).await;
    println!("vanyline REPL (Ctrl-D to exit)");
    println!("Agent: {}", agent_config.name);
    if let Some(title) = &conv.title {
        println!("Conversation: {} ({})", title, conv.id);
    } else {
        println!("Conversation: {}", conv.id);
    }
    println!();

    loop {
        let mut input = String::new();
        print!("> ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        if std::io::stdin().read_line(&mut input).unwrap_or(0) == 0 {
            break;
        }
        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }

        conv.messages.push(vanyline_lib::Message {
            role: "user".to_string(),
            content: input.clone(),
        });

        match process_turn(&mut conv, &agent_config, &input).await {
            Ok(response) => {
                conv.messages.push(vanyline_lib::Message {
                    role: "assistant".to_string(),
                    content: response,
                });
                store::save_conversation(&conv).ok();
            }
            Err(_) => break,
        }
    }
    println!();
}

async fn process_turn(
    conv: &mut vanyline_lib::Conversation,
    agent_config: &vanyline_lib::Agent,
    user_msg: &str,
) -> Result<String, vanyline_lib::VnyError> {
    let history: Vec<rig_core::message::Message> = conv
        .messages
        .iter()
        .map(|m| rig_core::message::Message::user(m.content.clone()))
        .collect();

    let sink = Arc::new(StdoutSink);
    let mcp_servers = &agent_config.mcp_servers;

    let provider = resolve_provider_owned(agent_config)?;
    let model_name = agent_config
        .model
        .as_deref()
        .or(provider.default_model.as_deref())
        .ok_or(vanyline_lib::VnyError::NoModelConfigured)?;

    let response_text = match provider.provider_type.as_str() {
        "ollama" => {
            let model = vanyline_lib::build_ollama_model(&provider, model_name)?;
            vanyline_lib::run_chat_turn(sink, agent_config, mcp_servers, model, history, user_msg)
                .await?
        }
        "openai-compatible" => {
            let model = vanyline_lib::build_openai_compat_model(&provider, model_name)?;
            vanyline_lib::run_chat_turn(sink, agent_config, mcp_servers, model, history, user_msg)
                .await?
        }
        other => {
            return Err(vanyline_lib::VnyError::UnknownProviderType(
                other.to_string(),
            ))
        }
    };

    Ok(response_text)
}

fn resolve_provider_owned(
    agent: &vanyline_lib::Agent,
) -> Result<vanyline_lib::LlmProvider, vanyline_lib::VnyError> {
    let providers = store::list_providers().map_err(|_| vanyline_lib::VnyError::NoProviderConfigured)?;

    if let Some(pid) = agent.llm_provider_id {
        providers
            .into_iter()
            .find(|p| p.id == pid)
            .ok_or(vanyline_lib::VnyError::LlmProviderNotFound)
    } else {
        providers
            .into_iter()
            .find(|p| !p.name.is_empty())
            .ok_or(vanyline_lib::VnyError::NoProviderConfigured)
    }
}

async fn resolve_context(
    agent: Option<Uuid>,
    conversation: Option<Uuid>,
) -> (vanyline_lib::Conversation, vanyline_lib::Agent, bool) {
    let agents = store::list_agents().unwrap_or_default();
    let default_agent_id = store::get_default_agent_id().ok();

    let agent_id = agent.or(default_agent_id).unwrap_or_else(|| {
        if agents.len() == 1 {
            agents[0].id
        } else {
            eprintln!("No agent specified. Use --agent or set a default agent.");
            eprintln!("Available agents:");
            for a in &agents {
                println!("  {} | {}", a.id, a.name);
            }
            std::process::exit(1);
        }
    });

    let agent_config = agents
        .iter()
        .find(|a| a.id == agent_id)
        .unwrap_or_else(|| {
            eprintln!("Agent not found: {agent_id}");
            std::process::exit(1);
        })
        .clone();

    let (conv, is_new) = if let Some(cid) = conversation {
        (
            store::get_conversation(&cid).unwrap_or_else(|_| {
                eprintln!("Conversation not found: {cid}");
                std::process::exit(1);
            }),
            false,
        )
    } else if let Some(active_id) = store::get_active_conversation() {
        match store::get_conversation(&active_id) {
            Ok(existing) => (existing, false),
            Err(_) => (
                vanyline_lib::Conversation {
                    id: uuid::Uuid::new_v4(),
                    agent_id: Some(agent_id),
                    title: None,
                    messages: Vec::new(),
                },
                true,
            ),
        }
    } else {
        (
            vanyline_lib::Conversation {
                id: uuid::Uuid::new_v4(),
                agent_id: Some(agent_id),
                title: None,
                messages: Vec::new(),
            },
            true,
        )
    };

    (conv, agent_config, is_new)
}
