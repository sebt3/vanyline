use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use vanyline_lib::event::{ChatEvent, ChatTurnResult, EventSink};
use vanyline_lib::session::run_agent_turn;
use vanyline_lib::store::ConfigStore;

use crate::{config, config_store::CliConfigStore, store};

struct StdoutSink;

#[async_trait]
impl EventSink for StdoutSink {
    async fn emit(&self, event: ChatEvent) {
        match event {
            ChatEvent::Token { content } => {
                print!("{content}");
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
            ChatEvent::ToolCall { name, args, .. } => {
                print!("\n[tool] {name}({args})\n");
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
            ChatEvent::Done => println!(),
            ChatEvent::Error { code, message } => eprintln!("[{code}] {message}"),
            _ => {}
        }
    }
}

pub async fn run(message: Option<String>, agent: Option<String>, conversation: Option<Uuid>) {
    config::ensure_config_dir();
    if let Some(msg) = message {
        run_one_shot(&msg, agent, conversation).await;
        return;
    }
    run_repl(agent, conversation).await;
}

async fn run_one_shot(user_msg: &str, agent: Option<String>, conversation: Option<Uuid>) {
    let (mut conv, agent_name, is_new) = resolve_context(agent, conversation).await;
    if is_new {
        println!("Session: {}", conv.id);
    }

    let ctx = build_session_context();
    let workspace_context = read_workspace_context();

    match process_turn(&conv, &agent_name, &ctx, workspace_context.as_deref(), user_msg).await {
        Ok(result) => {
            conv.messages.push(vanyline_lib::Message {
                role: "user".to_string(),
                content: user_msg.to_string(),
                tool_calls: None,
            });
            conv.messages.push(result_to_assistant_message(result));
            store::save_conversation(&conv).ok();
        }
        Err(_) => {
            std::process::exit(1);
        }
    }
}

async fn run_repl(agent: Option<String>, conversation: Option<Uuid>) {
    let (mut conv, agent_name, _) = resolve_context(agent, conversation).await;
    println!("vanyline REPL (Ctrl-D to exit)");
    println!("Agent: {agent_name}");
    if let Some(title) = &conv.title {
        println!("Conversation: {} ({})", title, conv.id);
    } else {
        println!("Conversation: {}", conv.id);
    }
    println!();

    let ctx = build_session_context();
    let workspace_context = read_workspace_context();

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

        match process_turn(&conv, &agent_name, &ctx, workspace_context.as_deref(), &input).await {
            Ok(result) => {
                conv.messages.push(vanyline_lib::Message {
                    role: "user".to_string(),
                    content: input.clone(),
                    tool_calls: None,
                });
                conv.messages.push(result_to_assistant_message(result));
                store::save_conversation(&conv).ok();
            }
            Err(_) => break,
        }
    }
    println!();
}

/// Construit le `SessionContext` de la session CLI — UNE FOIS par exécution
/// (`run_one_shot`/`run_repl`), pas par tour : `store`/`sink`/`local_tools`/
/// `subagent_depth_max` ne changent jamais au sein d'une session CLI.
fn build_session_context() -> vanyline_lib::session::SessionContext {
    vanyline_lib::session::SessionContext {
        store: Arc::new(CliConfigStore::new(config::config_dir())),
        sink: Arc::new(StdoutSink),
        local_tools: crate::tools::local_tools_map(),
        subagent_depth_max: 1,
    }
}

/// Contenu d'AGENTS.md dans le répertoire courant, si présent — c'est le
/// `workspace_context` optionnel de `run_agent_turn` (design harness-core,
/// section "Session engine", étape 4 de l'assemblage du prompt).
fn read_workspace_context() -> Option<String> {
    std::fs::read_to_string("AGENTS.md").ok()
}

async fn process_turn(
    conv: &vanyline_lib::Conversation,
    agent_name: &str,
    ctx: &vanyline_lib::session::SessionContext,
    workspace_context: Option<&str>,
    user_msg: &str,
) -> Result<ChatTurnResult, vanyline_lib::VnyError> {
    let history: Vec<rig_core::message::Message> = conv
        .messages
        .iter()
        .filter_map(|m| {
            if m.role == "user" {
                Some(rig_core::message::Message::user(m.content.clone()))
            } else if m.role == "assistant" {
                Some(rig_core::message::Message::assistant(m.content.clone()))
            } else {
                None
            }
        })
        .collect();

    run_agent_turn(ctx, agent_name, history, user_msg, workspace_context).await
}

/// Convertit le résultat d'un tour (`event::ChatTurnResult`, tool_calls avec
/// `id`) en `types::Message` à persister dans la conversation CLI
/// (`types::ToolCall`, PAS d'`id` — champ abandonné à la persistance, la
/// corrélation call/result n'a de sens que pendant le tour lui-même).
fn result_to_assistant_message(result: ChatTurnResult) -> vanyline_lib::Message {
    let tool_calls: Vec<vanyline_lib::ToolCall> = result.tool_calls.iter().map(|tc| {
        vanyline_lib::ToolCall {
            name: tc.name.clone(),
            arguments: tc.arguments.clone(),
            result: tc.result.clone(),
        }
    }).collect();
    vanyline_lib::Message {
        role: "assistant".to_string(),
        content: result.response_text,
        tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
    }
}

async fn resolve_context(
    agent: Option<String>,
    conversation: Option<Uuid>,
) -> (vanyline_lib::Conversation, String, bool) {
    let config_store = CliConfigStore::new(config::config_dir());

    let agents = config_store.list_agents().await.unwrap_or_default();
    let default_agent_name = config_store.default_agent().await.ok().flatten();

    let agent_name = agent.or(default_agent_name).unwrap_or_else(|| {
        if agents.len() == 1 {
            agents[0].name.clone()
        } else {
            eprintln!("No agent specified. Use --agent or set a default agent.");
            eprintln!("Available agents:");
            for a in &agents {
                println!("  {}", a.name);
            }
            std::process::exit(1);
        }
    });

    // Validate agent exists
    if !agents.iter().any(|a| a.name == agent_name) {
        eprintln!("Agent not found: {agent_name}");
        std::process::exit(1);
    }

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
                    agent: Some(agent_name.clone()),
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
                agent: Some(agent_name.clone()),
                title: None,
                messages: Vec::new(),
            },
            true,
        )
    };

    (conv, agent_name, is_new)
}
