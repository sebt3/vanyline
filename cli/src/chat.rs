use std::sync::Arc;

use async_trait::async_trait;

use vanyline_lib::domain::{McpSelection, McpServer, McpTransport};
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

pub async fn run(
    message: Option<String>,
    agent: Option<String>,
    continue_active: bool,
    toolbox_mcp_url: Option<String>,
    timeout_secs: Option<u64>,
    json: bool,
    model: Option<String>,
) {
    config::ensure_config_dir();
    if !json {
        print_workspace_sources().await;
    }
    if let Some(msg) = message {
        run_one_shot(
            &msg,
            agent,
            continue_active,
            toolbox_mcp_url,
            timeout_secs,
            json,
            model,
        )
        .await;
        return;
    }
    run_repl(agent, continue_active, toolbox_mcp_url).await;
}

async fn run_one_shot(
    user_msg: &str,
    agent: Option<String>,
    continue_active: bool,
    toolbox_mcp_url: Option<String>,
    timeout_secs: Option<u64>,
    json: bool,
    model: Option<String>,
) {
    let (mut conv, agent_name, is_new) = resolve_context(agent, continue_active).await;
    if is_new && !json {
        println!("Session: {}", conv.id);
    }

    let ctx = build_session_context(
        toolbox_mcp_url.as_deref(),
        json,
        model.as_deref(),
        conv.todo.clone(),
    );
    let workspace_context = read_workspace_context();

    let turn = process_turn(
        &conv,
        &agent_name,
        &ctx,
        workspace_context.as_deref(),
        user_msg,
    );
    let result = match timeout_secs {
        Some(secs) if secs > 0 => {
            match tokio::time::timeout(std::time::Duration::from_secs(secs), turn).await {
                Ok(inner) => inner,
                Err(_) => {
                    eprintln!("[VNL-CLI-001] run timed out after {secs} seconds");
                    std::process::exit(1);
                }
            }
        }
        _ => turn.await,
    };
    match result {
        Ok(result) => {
            conv.messages.push(vanyline_lib::Message {
                role: "user".to_string(),
                content: user_msg.to_string(),
                tool_calls: None,
            });
            conv.messages.push(result_to_assistant_message(result));
            conv.todo = read_todo_state(&ctx.todo_state);
            store::save_conversation(&conv).ok();
            if !json {
                print_git_diff_stat();
            }
        }
        Err(_) => {
            std::process::exit(1);
        }
    }
}

async fn run_repl(agent: Option<String>, continue_active: bool, toolbox_mcp_url: Option<String>) {
    let (mut conv, agent_name, _) = resolve_context(agent, continue_active).await;
    println!("vanyline REPL (Ctrl-D to exit)");
    println!("Agent: {agent_name}");
    if let Some(title) = &conv.title {
        println!("Conversation: {} ({})", title, conv.id);
    } else {
        println!("Conversation: {}", conv.id);
    }
    println!();

    let ctx = build_session_context(toolbox_mcp_url.as_deref(), false, None, conv.todo.clone());
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

        match process_turn(
            &conv,
            &agent_name,
            &ctx,
            workspace_context.as_deref(),
            &input,
        )
        .await
        {
            Ok(result) => {
                conv.messages.push(vanyline_lib::Message {
                    role: "user".to_string(),
                    content: input.clone(),
                    tool_calls: None,
                });
                conv.messages.push(result_to_assistant_message(result));
                conv.todo = read_todo_state(&ctx.todo_state);
                store::save_conversation(&conv).ok();
            }
            Err(_) => break,
        }
    }
    println!();
}

/// Lit l'état todo courant du handle partagé — même logique de récupération
/// sur poison que `lib/src/builtin/todo.rs` (`unwrap_or_else(|e| e.into_inner())`) :
/// un panic dans un tool call ne doit pas empêcher de persister l'état posé
/// avant le panic.
fn read_todo_state(state: &std::sync::Mutex<Option<String>>) -> Option<String> {
    state.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Construit le `SessionContext` de la session CLI. `toolbox_mcp_url` :
/// `Some(url)` -> `local_tools` vide + la sandbox injectee comme serveur
/// MCP nomme "toolbox" dans `extra_mcp` (design "Toolbox en inference" de
/// ws12-sandbox-clients) ; `None` -> comportement inchange (local_tools du
/// CLI, `extra_mcp` vide). Le paramètre `json` choisit le sink de sortie :
/// `JsonSink` (ligne JSON par événement) ou `StdoutSink` (sortie lisible).
/// `todo_seed` : valeur optionnelle issue de `Conversation.todo` pour le
/// resume d'une conversation existante (`-c/--continue`).
fn build_session_context(
    toolbox_mcp_url: Option<&str>,
    json: bool,
    model_override: Option<&str>,
    todo_seed: Option<String>,
) -> vanyline_lib::session::SessionContext {
    let sink: Arc<dyn vanyline_lib::event::EventSink> = if json {
        Arc::new(JsonSink)
    } else {
        Arc::new(StdoutSink)
    };
    match toolbox_mcp_url {
        Some(url) => vanyline_lib::session::SessionContext {
            store: Arc::new(crate::discover_fs_store()),
            sink,
            local_tools: std::collections::HashMap::new(),
            subagent_depth_max: 1,
            extra_mcp: vec![(
                McpServer {
                    name: "toolbox".to_string(),
                    transport: McpTransport::HttpStreamable,
                    url: url.to_string(),
                    headers: Default::default(),
                },
                McpSelection {
                    server: "toolbox".to_string(),
                    tools: vec![],
                },
            )],
            model_override: model_override.map(str::to_string),
            todo_state: Arc::new(std::sync::Mutex::new(todo_seed)),
        },
        None => vanyline_lib::session::SessionContext {
            store: Arc::new(crate::discover_fs_store()),
            sink,
            local_tools: crate::tools::local_tools_map(),
            subagent_depth_max: 1,
            extra_mcp: Vec::new(),
            model_override: model_override.map(str::to_string),
            todo_state: Arc::new(std::sync::Mutex::new(todo_seed)),
        },
    }
}

/// Sink de sortie structurée : sérialise chaque `ChatEvent` en une ligne JSON
/// (`#[serde(tag = "type", rename_all = "snake_case")]`, cf. `event.rs`). Les
/// erreurs vont sur stderr (même convention que `StdoutSink`), tout le reste en
/// JSON sur stdout.
struct JsonSink;

#[async_trait]
impl EventSink for JsonSink {
    async fn emit(&self, event: ChatEvent) {
        match event {
            ChatEvent::Error { code, message } => eprintln!("[{code}] {message}"),
            _ => {
                if let Ok(line) = serde_json::to_string(&event) {
                    println!("{line}");
                }
            }
        }
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
    let tool_calls: Vec<vanyline_lib::ToolCall> = result
        .tool_calls
        .iter()
        .map(|tc| vanyline_lib::ToolCall {
            name: tc.name.clone(),
            arguments: tc.arguments.clone(),
            result: tc.result.clone(),
        })
        .collect();
    vanyline_lib::Message {
        role: "assistant".to_string(),
        content: result.response_text,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
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
    let names: Vec<&str> = agents
        .iter()
        .filter(|a| config::file_entry_source(layers, "agents", "md", &a.name) == "workspace")
        .map(|a| a.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  agents: {}", names.join(", ")));
    }

    let toolsets = store.list_toolsets().await.unwrap_or_default();
    let names: Vec<&str> = toolsets
        .iter()
        .filter(|t| config::file_entry_source(layers, "toolsets", "yaml", &t.name) == "workspace")
        .map(|t| t.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  toolsets: {}", names.join(", ")));
    }

    let skills = store.list_skills().await.unwrap_or_default();
    let names: Vec<&str> = skills
        .iter()
        .filter(|s| config::skill_entry_source(layers, &s.name) == "workspace")
        .map(|s| s.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  skills: {}", names.join(", ")));
    }

    let mcp_servers = store.list_mcp_servers().await.unwrap_or_default();
    let names: Vec<&str> = mcp_servers
        .iter()
        .filter(|s| config::config_entry_source(layers, &s.name, |r| &r.mcp) == "workspace")
        .map(|s| s.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  mcp: {}", names.join(", ")));
    }

    let models = store.list_models().await.unwrap_or_default();
    let names: Vec<&str> = models
        .iter()
        .filter(|m| config::config_entry_source(layers, &m.name, |r| &r.models) == "workspace")
        .map(|m| m.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  models: {}", names.join(", ")));
    }

    let providers = store.list_providers().await.unwrap_or_default();
    let names: Vec<&str> = providers
        .iter()
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
    let Some(ws_dir) = store.layers().workspace_dir.clone() else {
        return;
    };
    let lines = workspace_source_summary(&store).await;
    if lines.is_empty() {
        return;
    }
    println!("Sources workspace ({}):", ws_dir.display());
    for line in &lines {
        println!("{line}");
    }
}

/// Reproduit le `git diff --stat` du wrapper llm-exec : exécute `git diff
/// --stat` dans `root` et renvoie le texte de sortie (trimé), ou `None` si
/// `root` n'est pas un dépôt git, si `git` échoue, ou si le diff est vide.
/// Séparée de l'affichage pour rester testable sans capturer stdout.
fn git_diff_stat(root: &std::path::Path) -> Option<String> {
    if !root.join(".git").exists() {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["diff", "--stat"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// Résout la racine workspace depuis le cwd courant puis affiche
/// `git diff --stat`. Silencieux si le cwd n'est pas dans un dépôt git,
/// si `git` échoue, ou si le diff est vide.
fn print_git_diff_stat() {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let Some(root) = config::discover_workspace_root(&cwd) else {
        return;
    };
    if let Some(text) = git_diff_stat(&root) {
        println!("{text}");
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
        todo: None,
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

    // 6. build_session_context_none_uses_local_tools
    #[test]
    fn build_session_context_none_uses_local_tools() {
        let global_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: None,
        };
        let _store = FsConfigStore::new(layers);
        let ctx = build_session_context(None, false, None, None);
        assert!(!ctx.local_tools.is_empty());
        assert!(ctx.local_tools.contains_key("read_file"));
        assert!(ctx.extra_mcp.is_empty());
    }

    // 7. build_session_context_toolbox_empties_local_tools
    #[test]
    fn build_session_context_toolbox_empties_local_tools() {
        let global_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: None,
        };
        let _store = FsConfigStore::new(layers);
        let ctx = build_session_context(
            Some("http://sandbox-demo.dev.svc:3000/mcp"),
            false,
            None,
            None,
        );
        assert!(ctx.local_tools.is_empty());
        assert_eq!(ctx.extra_mcp.len(), 1);
        let (server, selection) = &ctx.extra_mcp[0];
        assert_eq!(server.name, "toolbox");
        assert_eq!(server.url, "http://sandbox-demo.dev.svc:3000/mcp");
        assert_eq!(selection.server, "toolbox");
        assert!(selection.tools.is_empty());
    }

    // 8. build_session_context_none_json
    #[test]
    fn build_session_context_none_json() {
        let global_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: None,
        };
        let _store = FsConfigStore::new(layers);
        let ctx = build_session_context(None, true, None, None);
        assert!(!ctx.local_tools.is_empty());
        assert!(ctx.local_tools.contains_key("read_file"));
        assert!(ctx.extra_mcp.is_empty());
        // Same logic as non-json: local_tools peupés, extra_mcp vide
    }

    // 9. build_session_context_toolbox_json
    #[test]
    fn build_session_context_toolbox_json() {
        let global_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: None,
        };
        let _store = FsConfigStore::new(layers);
        let ctx = build_session_context(
            Some("http://sandbox-demo.dev.svc:3000/mcp"),
            true,
            None,
            None,
        );
        assert!(ctx.local_tools.is_empty());
        assert_eq!(ctx.extra_mcp.len(), 1);
        let (server, selection) = &ctx.extra_mcp[0];
        assert_eq!(server.name, "toolbox");
        assert_eq!(server.url, "http://sandbox-demo.dev.svc:3000/mcp");
        assert_eq!(selection.server, "toolbox");
        assert!(selection.tools.is_empty());
        // Même logique que le cas non-json : local_tools vides, toolbox dans extra_mcp
    }

    // git_diff_stat_non_repo_returns_none
    #[test]
    fn git_diff_stat_non_repo_returns_none() {
        let tmp = tempdir().unwrap();
        assert_eq!(git_diff_stat(tmp.path()), None);
    }

    // git_diff_stat_clean_repo_returns_none
    #[test]
    fn git_diff_stat_clean_repo_returns_none() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(ok.success());
        assert_eq!(git_diff_stat(&repo), None);
    }

    // git_diff_stat_with_changes_returns_summary
    #[test]
    fn git_diff_stat_with_changes_returns_summary() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        };
        assert!(git(&["init", "-q"]));
        assert!(git(&["config", "user.email", "test@example.com"]));
        assert!(git(&["config", "user.name", "test"]));
        assert!(git(&["add", "a.txt"]));
        assert!(git(&["commit", "-q", "-m", "init"]));
        std::fs::write(repo.join("a.txt"), "hello world\n").unwrap();
        let out = git_diff_stat(&repo).unwrap();
        assert!(out.contains("a.txt"));
    }

    // 13. build_session_context_seeds_todo_from_conversation
    #[test]
    fn build_session_context_seeds_todo_from_conversation() {
        let global_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: None,
        };
        let _store = FsConfigStore::new(layers);
        let ctx =
            build_session_context(None, false, None, Some("[{\"content\":\"x\"}]".to_string()));
        assert_eq!(
            ctx.todo_state.lock().unwrap().clone(),
            Some("[{\"content\":\"x\"}]".to_string())
        );
    }

    // 14. build_session_context_no_seed_when_none
    #[test]
    fn build_session_context_no_seed_when_none() {
        let global_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: None,
        };
        let _store = FsConfigStore::new(layers);
        let ctx = build_session_context(None, false, None, None);
        assert!(ctx.todo_state.lock().unwrap().is_none());
    }

    // 15. read_todo_state_returns_value
    #[test]
    fn read_todo_state_returns_value() {
        let m = std::sync::Mutex::new(Some("x".to_string()));
        assert_eq!(read_todo_state(&m), Some("x".to_string()));
    }

    // 16. read_todo_state_returns_none
    #[test]
    fn read_todo_state_returns_none() {
        let m = std::sync::Mutex::new(None);
        assert_eq!(read_todo_state(&m), None);
    }
}
