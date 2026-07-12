use std::sync::Arc;

use async_trait::async_trait;

use vanyline_lib::event::{ChatEvent, ChatTurnResult, EventSink};
use vanyline_lib::session::run_agent_turn;
use vanyline_lib::store::ConfigStore;

use crate::{config, store};

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

pub async fn run(message: Option<String>, agent: Option<String>, continue_active: bool) {
    config::ensure_config_dir();
    print_workspace_sources().await;
    if let Some(msg) = message {
        run_one_shot(&msg, agent, continue_active).await;
        return;
    }
    run_repl(agent, continue_active).await;
}

async fn run_one_shot(user_msg: &str, agent: Option<String>, continue_active: bool) {
    let (mut conv, agent_name, is_new) = resolve_context(agent, continue_active).await;
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

async fn run_repl(agent: Option<String>, continue_active: bool) {
    let (mut conv, agent_name, _) = resolve_context(agent, continue_active).await;
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
        store: Arc::new(crate::discover_fs_store()),
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

/// Calcule les lignes "sources workspace" à afficher au lancement — une
/// ligne par kind qui a au moins une entrée workspace-sourcée (via
/// `config::file_entry_source`/`config::skill_entry_source`/
/// `config::config_entry_source`, déjà utilisés par les commandes `list`,
/// tâche 04a). Vide si `layers.workspace_dir` est `None`, ou si aucun kind
/// n'a d'entrée workspace-sourcée. L'appelant ajoute l'en-tête avec le
/// chemin — cette fonction ne fait que la liste des lignes de détail, pour
/// rester testable sans capturer stdout.
async fn workspace_source_summary(store: &crate::fs_store::FsConfigStore) -> Vec<String> {
    let layers = store.layers();
    if layers.workspace_dir.is_none() {
        return Vec::new();
    }

    let mut lines = Vec::new();

    let agents = store.list_agents().await.unwrap_or_default();
    let names: Vec<&str> = agents.iter()
        .filter(|a| config::file_entry_source(layers, "agents", "md", &a.name) == "workspace")
        .map(|a| a.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  agents: {}", names.join(", ")));
    }

    let toolsets = store.list_toolsets().await.unwrap_or_default();
    let names: Vec<&str> = toolsets.iter()
        .filter(|t| config::file_entry_source(layers, "toolsets", "yaml", &t.name) == "workspace")
        .map(|t| t.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  toolsets: {}", names.join(", ")));
    }

    let skills = store.list_skills().await.unwrap_or_default();
    let names: Vec<&str> = skills.iter()
        .filter(|s| config::skill_entry_source(layers, &s.name) == "workspace")
        .map(|s| s.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  skills: {}", names.join(", ")));
    }

    let mcp_servers = store.list_mcp_servers().await.unwrap_or_default();
    let names: Vec<&str> = mcp_servers.iter()
        .filter(|s| config::config_entry_source(layers, &s.name, |r| &r.mcp) == "workspace")
        .map(|s| s.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  mcp: {}", names.join(", ")));
    }

    let models = store.list_models().await.unwrap_or_default();
    let names: Vec<&str> = models.iter()
        .filter(|m| config::config_entry_source(layers, &m.name, |r| &r.models) == "workspace")
        .map(|m| m.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  models: {}", names.join(", ")));
    }

    let providers = store.list_providers().await.unwrap_or_default();
    let names: Vec<&str> = providers.iter()
        .filter(|p| config::config_entry_source(layers, &p.name, |r| &r.providers) == "workspace")
        .map(|p| p.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  providers: {}", names.join(", ")));
    }

    lines
}

/// Affiche l'en-tête + les lignes de `workspace_source_summary`, rien si
/// cette dernière retourne une liste vide.
async fn print_workspace_sources() {
    let store = crate::discover_fs_store();
    let Some(ws_dir) = store.layers().workspace_dir.clone() else { return };
    let lines = workspace_source_summary(&store).await;
    if lines.is_empty() {
        return;
    }
    println!("Sources workspace ({}):", ws_dir.display());
    for line in &lines {
        println!("{line}");
    }
}

async fn resolve_context(
    agent: Option<String>,
    continue_active: bool,
) -> (vanyline_lib::Conversation, String, bool) {
    let config_store = crate::discover_fs_store();

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

    if !agents.iter().any(|a| a.name == agent_name) {
        eprintln!("Agent not found: {agent_name}");
        std::process::exit(1);
    }

    let new_conversation = || vanyline_lib::Conversation {
        id: uuid::Uuid::new_v4(),
        agent: Some(agent_name.clone()),
        title: None,
        messages: Vec::new(),
    };

    let (conv, is_new) = if continue_active {
        match store::get_active_conversation().and_then(|id| store::get_conversation(&id).ok()) {
            Some(existing) => (existing, false),
            None => {
                println!("No active conversation found, starting a new one.");
                (new_conversation(), true)
            }
        }
    } else {
        (new_conversation(), true)
    };

    (conv, agent_name, is_new)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;
    use crate::config::Layers;
    use crate::fs_store::FsConfigStore;

    fn write_config_yaml(dir: &std::path::Path, content: &str) {
        let path = dir.join("config.yaml");
        let mut f = std::fs::File::create(&path)
            .unwrap_or_else(|e| panic!("Failed to create config.yaml at {}: {e}", path.display()));
        f.write_all(content.as_bytes())
            .unwrap_or_else(|e| panic!("Failed to write config.yaml at {}: {e}", path.display()));
    }

    // 1. no_workspace_layer_yields_empty_summary
    #[tokio::test]
    async fn no_workspace_layer_yields_empty_summary() {
        let global_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: None,
        };
        let store = FsConfigStore::new(layers);
        let result = workspace_source_summary(&store).await;
        assert!(result.is_empty());
    }

    // 2. workspace_layer_with_no_overrides_yields_empty_summary
    #[tokio::test]
    async fn workspace_layer_with_no_overrides_yields_empty_summary() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);
        let result = workspace_source_summary(&store).await;
        assert!(result.is_empty());
    }

    // 3. workspace_agent_reported
    #[tokio::test]
    async fn workspace_agent_reported() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        // Global: no agents
        std::fs::create_dir_all(workspace_dir.path().join("agents")).unwrap();
        std::fs::write(
            workspace_dir.path().join("agents").join("build.md"),
            "---\nmodel: test\n---\nbuild agent\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);
        let result = workspace_source_summary(&store).await;
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("agents: build"));
    }

    // 4. workspace_toolset_and_mcp_reported
    #[tokio::test]
    async fn workspace_toolset_and_mcp_reported() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        // Workspace: toolset + mcp config
        std::fs::create_dir_all(workspace_dir.path().join("toolsets")).unwrap();
        std::fs::write(
            workspace_dir.path().join("toolsets").join("grafana.yaml"),
            "tools:\n  mcp:\n    internal:\n      - metric_query\n",
        )
        .unwrap();
        write_config_yaml(
            workspace_dir.path(),
            "mcp:\n  internal:\n    type: http-streamable\n    url: http://localhost:3000\n",
        );
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);
        let result = workspace_source_summary(&store).await;
        assert!(result.len() == 2);
        let mcp_line = result.iter().find(|l| l.contains("mcp")).unwrap();
        assert!(mcp_line.contains("internal"));
        let ts_line = result.iter().find(|l| l.contains("toolsets")).unwrap();
        assert!(ts_line.contains("grafana"));
    }

    // 5. multiple_workspace_agents_joined_on_one_line
    #[tokio::test]
    async fn multiple_workspace_agents_joined_on_one_line() {
        let global_dir = tempdir().unwrap();
        let workspace_dir = tempdir().unwrap();
        std::fs::create_dir_all(workspace_dir.path().join("agents")).unwrap();
        std::fs::write(
            workspace_dir.path().join("agents").join("build.md"),
            "---\nmodel: test\n---\nbuild\n",
        )
        .unwrap();
        std::fs::write(
            workspace_dir.path().join("agents").join("debug.md"),
            "---\nmodel: test\n---\ndebug\n",
        )
        .unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: Some(workspace_dir.path().to_path_buf()),
        };
        let store = FsConfigStore::new(layers);
        let result = workspace_source_summary(&store).await;
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("agents:"));
        assert!(result[0].contains("build"));
        assert!(result[0].contains("debug"));
        assert!(result[0].contains("build, debug") || result[0].contains("debug, build"));
        // Not two separate lines
        let agent_lines = result.iter().filter(|l| l.contains("agents:")).count();
        assert_eq!(agent_lines, 1);
    }
}
