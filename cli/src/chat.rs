use std::sync::Arc;

use async_trait::async_trait;

use vanyline_lib::domain::{McpSelection, McpServer, McpTransport};
use vanyline_lib::event::{ChatEvent, ChatTurnResult, EventSink};
use vanyline_lib::session::run_agent_turn;
use vanyline_lib::store::ConfigStore;

use vanyline_cfgstore::layers::{config_entry_source, file_entry_source, skill_entry_source};

use crate::{config, store};

struct StdoutSink;

#[async_trait]
impl EventSink for StdoutSink {
    // tooling.sdd D6a — canal rendu contracté : chat.sdd §Handles « Rendu
    // @StdoutSink » (et Scenarios `stdout_sink_*`) — @Token nu + flush,
    // @ToolCall `[tool] {name}({args})`, @Done ligne vide sur stdout ; @Error
    // `[{code}] {message}` sur stderr. stdout/stderr EST le produit (§Returns :
    // contrat observable = {stdout, stderr, code de sortie}).
    #[allow(clippy::print_stdout, clippy::print_stderr)]
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
    toolbox: Option<crate::ToolboxTarget>,
    timeout_secs: Option<u64>,
    json: bool,
    model: Option<String>,
) {
    config::ensure_config_dir();
    if let Some(msg) = message {
        if !json {
            print_workspace_sources().await;
        }
        run_one_shot(
            &msg,
            agent,
            continue_active,
            toolbox,
            timeout_secs,
            json,
            model,
        )
        .await;
        return;
    }
    run_interactive(agent, continue_active, toolbox).await;
}

/// Mode interactif = TUI ratatui (`chat.sdd` §Must « REPL INTERACTIF = TUI »,
/// `tui/mod.sdd`) : exige stdin ET stdout terminaux, sinon message exact et
/// sortie code 2 (le one-shot `run <message>` reste le mode scriptable).
/// L'ancien REPL texte (bannière, prompt `> `, `StdoutSink` interactif) est
/// supprimé ; sources du workspace, agent et conversation passent au TUI
/// (en-tête et première `Notice`), le `git diff` de clôture devient la barre
/// d'état du TUI.
async fn run_interactive(
    agent: Option<String>,
    continue_active: bool,
    toolbox: Option<crate::ToolboxTarget>,
) {
    // tooling.sdd D6a — terminaison contractée : chat.sdd §Must « REPL
    // INTERACTIF = TUI » — message exact puis exit(2) hors terminal.
    #[allow(clippy::print_stderr)]
    if !(std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::io::IsTerminal::is_terminal(&std::io::stdout()))
    {
        eprintln!(
            "vanyline: interactive chat needs a terminal; use `vanyline run <message>` for scripts"
        );
        std::process::exit(2);
    }
    let (conv, agent_name, _) = resolve_context(agent, continue_active).await;
    let workspace_context = read_workspace_context();
    let workspace_summary = workspace_summary_lines().await;
    let git_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| config::discover_workspace_root(&cwd));
    // Le sink de ce contexte est remplacé par le `TuiSink` à l'entrée de la
    // boucle (`tui/mod.sdd` Exposes) ; le modèle est `None` et aucun timeout
    // n'est codé en dur en mode interactif (chat.sdd §Handles).
    let ctx = build_turn_context(toolbox.as_ref(), false, None, conv.todo.clone()).await;
    let session = crate::tui::TuiSession {
        ctx,
        conversation: conv,
        agent: agent_name,
        workspace_context,
        workspace_summary,
        git_root,
        toolbox: toolbox.map(std::sync::Arc::new),
        // souris (molette) activée par défaut ; `--no-mouse` reste ouvert
        // (tui/mod.sdd Tasks `[?]`) faute de drapeau dans la grammaire clap.
        mouse: true,
    };
    // tooling.sdd D6a — terminaison contractée : erreur du chemin TUI affichée
    // APRÈS restauration du terminal (la garde RAII du TUI est tombée), puis
    // exit(1) comme les autres terminaisons du CLI.
    #[allow(clippy::print_stderr)]
    if let Err(e) = crate::tui::run_tui(session).await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

/// Lignes du résumé des sources du workspace pour la première `Notice` du TUI
/// (`chat.sdd` §Must « sources passées au TUI ») — mêmes filtres et même
/// en-tête que `print_workspace_sources`, en `Vec<String>` au lieu du rendu.
async fn workspace_summary_lines() -> Vec<String> {
    let store = crate::discover_fs_store();
    let Some(ws_dir) = store.layers().workspace_dir.clone() else {
        return Vec::new();
    };
    let lines = workspace_source_summary(&store).await;
    if lines.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!("Sources workspace ({}):", ws_dir.display())];
    out.extend(lines);
    out
}

async fn run_one_shot(
    user_msg: &str,
    agent: Option<String>,
    continue_active: bool,
    toolbox: Option<crate::ToolboxTarget>,
    timeout_secs: Option<u64>,
    json: bool,
    model: Option<String>,
) {
    let (mut conv, agent_name, is_new) = resolve_context(agent, continue_active).await;
    // tooling.sdd D6a — chat.sdd §Returns « Rendu one-shot lisible » + §Must
    // littéraux : `Session: {id}` uniquement si conversation neuve ET non
    // @json (Scenario `one_shot_ligne_session_saufen_json`).
    #[allow(clippy::print_stdout)]
    if is_new && !json {
        println!("Session: {}", conv.id);
    }

    // Jeton du toolbox PAR TOUR (décision 2026-10-08) : le one-shot n'a
    // qu'un tour, son bearer est posé ici.
    let ctx = build_turn_context(toolbox.as_ref(), json, model.as_deref(), conv.todo.clone()).await;
    let workspace_context = read_workspace_context();

    let turn = process_turn(
        &conv,
        &agent_name,
        &ctx,
        workspace_context.as_deref(),
        user_msg,
    );
    // tooling.sdd D6a — chat.sdd §Raises : timeout atteint ⇒ stderr exact
    // `[VNL-CLI-001] run timed out after {secs} seconds` puis exit(1)
    // (Scenario `one_shot_timeout_tue_et_sort_en_1`).
    #[allow(clippy::print_stderr)]
    let result = match timeout_secs {
        Some(secs) if secs > 0 => {
            if let Ok(inner) =
                tokio::time::timeout(std::time::Duration::from_secs(secs), turn).await
            {
                inner
            } else {
                eprintln!("[VNL-CLI-001] run timed out after {secs} seconds");
                std::process::exit(1);
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

// L'ancien REPL texte (run_repl : bannière, prompt `> `, lecture de lignes,
// @StdoutSink interactif) est supprimé par l'ALIGNEMENT TUI (chat.sdd §Must
// « REPL INTERACTIF = TUI ratatui », `tui/mod.sdd`) — voir @run_interactive.

/// Lit l'état todo courant du handle partagé — même logique de récupération
/// sur poison que `lib/src/builtin/todo.rs` (`unwrap_or_else(|e| e.into_inner())`) :
/// un panic dans un tool call ne doit pas empêcher de persister l'état posé
/// avant le panic.
pub(crate) fn read_todo_state(state: &std::sync::Mutex<Option<String>>) -> Option<String> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Construit le `SessionContext` de la session CLI. `toolbox` :
/// `Some((url, jeton))` -> `local_tools` vide + la sandbox injectee comme serveur
/// MCP nomme "toolbox" dans `extra_mcp` (design "Toolbox en inference" de
/// ws12-sandbox-clients), avec l'en-tete `authorization: Bearer {jeton}` pose
/// sur le serveur (jamais de headers vides, decision Sebastien 2026-10-08,
/// cf. `sandbox/src/auth.sdd`) ; `None` -> comportement inchange (`local_tools`
/// du CLI, `extra_mcp` vide). Le paramètre `json` choisit le sink de sortie :
/// `JsonSink` (ligne JSON par événement) ou `StdoutSink` (sortie lisible).
/// `todo_seed` : valeur optionnelle issue de `Conversation.todo` pour le
/// resume d'une conversation existante (`-c/--continue`).
fn build_session_context(
    toolbox: Option<(&str, &str)>,
    json: bool,
    model_override: Option<&str>,
    todo_seed: Option<String>,
) -> vanyline_lib::session::SessionContext {
    let sink: Arc<dyn vanyline_lib::event::EventSink> = if json {
        Arc::new(JsonSink)
    } else {
        Arc::new(StdoutSink)
    };
    match toolbox {
        Some((url, token)) => vanyline_lib::session::SessionContext {
            store: Arc::new(crate::discover_fs_store()),
            sink,
            local_tools: std::collections::HashMap::new(),
            subagent_depth_max: 1,
            extra_mcp: vec![(
                McpServer {
                    name: "toolbox".to_string(),
                    transport: McpTransport::HttpStreamable,
                    url: url.to_string(),
                    headers: std::collections::BTreeMap::from([(
                        "authorization".to_string(),
                        format!("Bearer {token}"),
                    )]),
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

/// Fenêtre de vie minimale (secondes) acceptée sur le jeton d'Owner présenté
/// au toolbox pour un tour — même valeur que la marge du cache lib
/// (`OWNER_TOKEN_REFRESH_MARGIN_SECS`, décision Sébastien 2026-10-08). Sous
/// cette fenêtre : warn factuel puis continuation avec ce jeton — jamais un
/// blocage de session, jamais une re-demande en boucle.
const TOOLBOX_TOKEN_MIN_SECS: i64 = 300;

/// Construit le `SessionContext` d'un tour : toolbox actif -> jeton d'Owner
/// frais demandé à l'apiserver (cache lib, au plus un `TokenRequest` par
/// ~10 min de session) et bearer du tour posé sur le serveur toolbox ; sans
/// toolbox -> construction directe. Appelé par tour (le one-shot : son tour
/// unique ; le REPL : avant chaque message traité), jamais une fois pour la
/// session — les headers rmcp sont figés à la connexion, la fenêtre de vie se
/// regagne donc à la reprise du contexte (décision 2026-10-08).
async fn build_turn_context(
    toolbox: Option<&crate::ToolboxTarget>,
    json: bool,
    model_override: Option<&str>,
    todo_seed: Option<String>,
) -> vanyline_lib::session::SessionContext {
    match toolbox {
        Some(target) => {
            let token = owner_token_for_turn(target).await;
            build_session_context(
                Some((target.url.as_str(), token.as_str())),
                json,
                model_override,
                todo_seed,
            )
        }
        None => build_session_context(None, json, model_override, todo_seed),
    }
}

/// Variante ouverte au TUI (`tui/mod.rs`) : même construction tour par tour,
/// mais l'échec de `owner_token` remonte un message au lieu de terminer le
/// processus — dans l'écran alternatif, seule la garde RAII du TUI parle au
/// terminal ; c'est la boucle qui rend l'erreur visible (`Notice`), puis
/// l'appelant qui termine après restauration (`tui/mod.sdd` §Must not).
pub(crate) async fn try_build_turn_context(
    toolbox: Option<&crate::ToolboxTarget>,
    json: bool,
    model_override: Option<&str>,
    todo_seed: Option<String>,
) -> Result<vanyline_lib::session::SessionContext, String> {
    match toolbox {
        Some(target) => {
            let token = owner_token_checked(target).await?;
            Ok(build_session_context(
                Some((target.url.as_str(), token.as_str())),
                json,
                model_override,
                todo_seed,
            ))
        }
        None => Ok(build_session_context(None, json, model_override, todo_seed)),
    }
}

/// Obtient le jeton d'Owner du tour pour la cible `target` et vérifie sa
/// fenêtre de vie : `exp` lu dans le JWT décodé SANS vérifier la signature ;
/// sous `TOOLBOX_TOKEN_MIN_SECS` (ou `exp` absent/illisible — traité comme
/// expiré) un `tracing::warn!` factuel est émis puis on poursuit avec ce
/// jeton. Le jeton et l'en-tête ne sont JAMAIS loggés. Un échec de
/// `owner_token` (Owner sans SA, cluster injoignable) est une erreur
/// fatale du chemin toolbox : message sur stderr puis exit(1), comme les
/// autres terminaisons du CLI.
async fn owner_token_for_turn(target: &crate::ToolboxTarget) -> String {
    // tooling.sdd D6a — terminaison standard du CLI : main.sdd §Raises
    // « Code 1, sur stderr, message = @Display de l'erreur (aucun reformatage
    // local) » — échec fatal de @owner_token sur le chemin toolbox (chat.sdd
    // §Must « JETON DU TOOLBOX PAR TOUR », doc in-line : message sur stderr
    // puis exit(1) comme les autres terminaisons).
    #[allow(clippy::print_stderr)]
    match owner_token_checked(target).await {
        Ok(token) => token,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

/// Obtient le jeton d'Owner du tour pour la cible `target` et vérifie sa
/// fenêtre de vie : `exp` lu dans le JWT décodé SANS vérifier la signature ;
/// sous `TOOLBOX_TOKEN_MIN_SECS` (ou `exp` absent/illisible — traité comme
/// expiré) un `tracing::warn!` factuel est émis puis on poursuit avec ce
/// jeton. Le jeton et l'en-tête ne sont JAMAIS loggés. Un échec de
/// `owner_token` (Owner sans SA, cluster injoignable) remonte le message
/// @Display de l'erreur en `Err` — la terminaison (stderr + exit(1), ou
/// `Notice` visible dans le TUI) appartient à l'appelant.
async fn owner_token_checked(target: &crate::ToolboxTarget) -> Result<String, String> {
    let jeton = target
        .client
        .owner_token(&target.owner)
        .await
        .map_err(|e| e.to_string())?;
    match jwt_exp_remaining(&jeton.token) {
        Some(remaining) if remaining >= TOOLBOX_TOKEN_MIN_SECS => {}
        Some(remaining) => tracing::warn!(
            "jeton toolbox : fenêtre de vie restante {remaining} s sous le minimum de \
             {TOOLBOX_TOKEN_MIN_SECS} s (marge du cache contredite — dérive d'horloge ?) ; \
             poursuite avec ce jeton"
        ),
        None => tracing::warn!(
            "jeton toolbox : claim exp absent ou illisible — traité comme expiré ; \
             poursuite avec ce jeton"
        ),
    }
    Ok(jeton.token)
}

/// Décode une chaîne base64url (alphabet URL, rembourrage absent ou
/// ignoré). `None` sur tout caractère hors alphabet ou longueur de bloc
/// finale impossible (1 caractère seul).
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c.checked_sub(b'A')?),
            b'a'..=b'z' => Some(c.checked_sub(b'a')?.checked_add(26)?),
            b'0'..=b'9' => Some(c.checked_sub(b'0')?.checked_add(52)?),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let input = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(input.len());
    for chunk in input.as_bytes().chunks(4) {
        // 1 caractère seul = longueur invalide.
        if chunk.len() < 2 {
            return None;
        }
        let mut vals = [0u8; 4];
        for (slot, c) in vals.iter_mut().zip(chunk) {
            *slot = value(*c)?;
        }
        let merged: u32 = (u32::from(vals[0]) << 18)
            | (u32::from(vals[1]) << 12)
            | (u32::from(vals[2]) << 6)
            | u32::from(vals[3]);
        // 2 caractères -> 1 octet, 3 -> 2, 4 -> 3.
        out.push(u8::try_from((merged >> 16) & 0xFF).ok()?);
        if chunk.len() >= 3 {
            out.push(u8::try_from((merged >> 8) & 0xFF).ok()?);
        }
        if chunk.len() >= 4 {
            out.push(u8::try_from(merged & 0xFF).ok()?);
        }
    }
    Some(out)
}

/// Lit la fenêtre de vie restante (secondes avant le claim `exp`) d'un JWT
/// d'Owner, SANS vérifier la signature — lecture d'un jeton émis par
/// l'apiserver, jamais une vérification de credential. `None` si le jeton
/// n'est pas un JWT à trois segments, si le segment central n'est pas du
/// base64url JSON, ou si `exp` est absent ou non-entier. Le jeton n'est
/// jamais loggé.
fn jwt_exp_remaining(token: &str) -> Option<i64> {
    let mut parts = token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let payload = base64url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    let exp = claims.get("exp")?.as_i64()?;
    exp.checked_sub(chrono::Utc::now().timestamp())
}

/// Sink de sortie structurée : sérialise chaque `ChatEvent` en une ligne JSON
/// (`#[serde(tag = "type", rename_all = "snake_case")]`, cf. `event.rs`). Les
/// erreurs vont sur stderr (même convention que `StdoutSink`), tout le reste en
/// JSON sur stdout.
struct JsonSink;

#[async_trait]
impl EventSink for JsonSink {
    // tooling.sdd D6a — canal rendu contracté : chat.sdd §Exposes/§Handles
    // « Rendu @JsonSink » + §Must « Réserver tout rendu non JSONL sur stdout au
    // cas @json @false » — toute variante d'événement sérialisée puis rendue en
    // JSONL sur stdout, @Error en texte `[{code}] {message}` sur stderr (jamais
    // en JSON). Scenarios `json_sink_*`.
    #[allow(clippy::print_stdout, clippy::print_stderr)]
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

/// Exécute un tour : historique filtré (rôles `user`/`assistant` seuls, texte
/// seul — `chat.sdd` §Must) puis `run_agent_turn`. Ouvert au TUI, qui lance
/// chaque tour dans sa tâche dédiée (`tui/mod.sdd` §Handles « Tour »).
pub(crate) async fn process_turn(
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

/// Convertit le résultat d'un tour (`event::ChatTurnResult`, `tool_calls` avec
/// `id`) en `types::Message` à persister dans la conversation CLI
/// (`types::ToolCall`, PAS d'`id` — champ abandonné à la persistance, la
/// corrélation call/result n'a de sens que pendant le tour lui-même).
pub(crate) fn result_to_assistant_message(result: ChatTurnResult) -> vanyline_lib::Message {
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
/// `file_entry_source`/`skill_entry_source`/
/// `config_entry_source`, déjà utilisés par les commandes `list`,
/// tâche 04a). Vide si `layers.workspace_dir` est `None`, ou si aucun kind
/// n'a d'entrée workspace-sourcée. L'appelant ajoute l'en-tête avec le
/// chemin — cette fonction ne fait que la liste des lignes de détail, pour
/// rester testable sans capturer stdout.
async fn workspace_source_summary(
    store: &vanyline_cfgstore::fs_store::FsConfigStore,
) -> Vec<String> {
    let layers = store.layers();
    if layers.workspace_dir.is_none() {
        return Vec::new();
    }

    let mut lines = Vec::new();

    let agents = store.list_agents().await.unwrap_or_default();
    let names: Vec<&str> = agents
        .iter()
        .filter(|a| file_entry_source(layers, "agents", "md", &a.name) == "workspace")
        .map(|a| a.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  agents: {}", names.join(", ")));
    }

    let toolsets = store.list_toolsets().await.unwrap_or_default();
    let names: Vec<&str> = toolsets
        .iter()
        .filter(|t| file_entry_source(layers, "toolsets", "yaml", &t.name) == "workspace")
        .map(|t| t.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  toolsets: {}", names.join(", ")));
    }

    let skills = store.list_skills().await.unwrap_or_default();
    let names: Vec<&str> = skills
        .iter()
        .filter(|s| skill_entry_source(layers, &s.name) == "workspace")
        .map(|s| s.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  skills: {}", names.join(", ")));
    }

    let mcp_servers = store.list_mcp_servers().await.unwrap_or_default();
    let names: Vec<&str> = mcp_servers
        .iter()
        .filter(|s| config_entry_source(layers, &s.name, |r| &r.mcp) == "workspace")
        .map(|s| s.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  mcp: {}", names.join(", ")));
    }

    let models = store.list_models().await.unwrap_or_default();
    let names: Vec<&str> = models
        .iter()
        .filter(|m| config_entry_source(layers, &m.name, |r| &r.models) == "workspace")
        .map(|m| m.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  models: {}", names.join(", ")));
    }

    let providers = store.list_providers().await.unwrap_or_default();
    let names: Vec<&str> = providers
        .iter()
        .filter(|p| config_entry_source(layers, &p.name, |r| &r.providers) == "workspace")
        .map(|p| p.name.as_str())
        .collect();
    if !names.is_empty() {
        lines.push(format!("  providers: {}", names.join(", ")));
    }

    lines
}

/// Affiche l'en-tête + les lignes de `workspace_source_summary`, rien si
/// cette dernière retourne une liste vide.
// tooling.sdd D6a — canal rendu contracté : chat.sdd §Returns —
// @print_workspace_sources préfixe de `Sources workspace ({racine}):` et ne rend
// RIEN (en-tête compris) si la couche est absente ou sans ligne (§Must
// littéraux, Scenario `sources_workspace_en_tete_et_ordre_des_kinds`).
#[allow(clippy::print_stdout)]
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
// tooling.sdd D6a — canal rendu contracté : chat.sdd §Returns « Rendu one-shot
// lisible » — la sortie de @git_diff_stat, uniquement après sauvegarde du tour
// réussi et uniquement en non-@json (§Handles @print_git_diff_stat, Scenario
// `git_diff_stat_rend_le_stat_des_modifications`).
#[allow(clippy::print_stdout)]
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

    // tooling.sdd D6a — chat.sdd §Raises « @resolve_context » : stderr exact
    // `No agent specified. Use --agent or set a default agent.` puis
    // `Available agents:` (stderr) puis les noms sur STDOUT (une ligne
    // `  {name}` chacun) + exit(1) — mélange stdout/stderr contracté tel quel,
    // en attente du [?] chat.sdd Tasks l.586 (tout stderr ou tout stdout ?).
    #[allow(clippy::print_stdout, clippy::print_stderr)]
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

    // tooling.sdd D6a — chat.sdd §Raises « @resolve_context » : agent absent de
    // @list_agents ⇒ stderr exact `Agent not found: {agent_name}` + exit(1)
    // (Scenario `agent_inconnu_sort_en_1_avec_message`).
    #[allow(clippy::print_stderr)]
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

    // tooling.sdd D6a — chat.sdd §Handles « Reprise -c » : pointeur absent/
    // corrompu ⇒ stdout `No active conversation found, starting a new one.` et
    // conversation neuve (jamais d'erreur ; §Must littéraux). Sa pollution du
    // flux JSONL en mode @json est ouverte au [?] chat.sdd Tasks l.594 — le
    // contrat stdout reste celui-ci.
    #[allow(clippy::print_stdout)]
    let (conv, is_new) = if continue_active {
        if let Some(existing) =
            store::get_active_conversation().and_then(|id| store::get_conversation(&id).ok())
        {
            (existing, false)
        } else {
            println!("No active conversation found, starting a new one.");
            (new_conversation(), true)
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
    use vanyline_cfgstore::fs_store::FsConfigStore;
    use vanyline_cfgstore::layers::Layers;

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
        assert_eq!(result, [] as [std::string::String; 0]);
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
        assert_eq!(result, [] as [std::string::String; 0]);
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
        assert_eq!(result.len(), 2);
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

    // 7. Scenario: mode_toolbox_remplace_les_tools_locaux — local_tools vide,
    // serveur toolbox @HttpStreamable, headers [(`authorization`,
    // `Bearer jeton-de-test`)] (jamais vides, décision 2026-10-08), sélection
    // totale, profondeur 1, `model_override`/`todo_state` reflètent les arguments.
    #[test]
    fn mode_toolbox_remplace_les_tools_locaux() {
        let global_dir = tempdir().unwrap();
        let layers = Layers {
            global_dir: global_dir.path().to_path_buf(),
            workspace_dir: None,
        };
        let _store = FsConfigStore::new(layers);
        let ctx = build_session_context(
            Some(("http://sbx:3000/mcp", "jeton-de-test")),
            false,
            Some("modele-de-test"),
            Some("[{\"content\":\"todo-de-test\"}]".to_string()),
        );
        assert!(ctx.local_tools.is_empty());
        assert_eq!(ctx.extra_mcp.len(), 1);
        let (server, selection) = &ctx.extra_mcp[0];
        assert_eq!(server.name, "toolbox");
        assert_eq!(server.url, "http://sbx:3000/mcp");
        assert!(matches!(server.transport, McpTransport::HttpStreamable));
        assert_eq!(
            server.headers.get("authorization").map(String::as_str),
            Some("Bearer jeton-de-test"),
            "l'en-tete authorization Bearer du tour doit être posé (jamais de headers vides)"
        );
        assert_eq!(selection.server, "toolbox");
        assert_eq!(selection.tools, [] as [std::string::String; 0]);
        assert_eq!(ctx.subagent_depth_max, 1);
        assert_eq!(
            ctx.model_override.as_deref(),
            Some("modele-de-test"),
            "model_override doit refléter l'argument"
        );
        assert_eq!(
            ctx.todo_state.lock().unwrap().clone(),
            Some("[{\"content\":\"todo-de-test\"}]".to_string()),
            "todo_state doit être seedé depuis l'argument todo_seed"
        );
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
            Some(("http://sandbox-demo.dev.svc:3000/mcp", "jeton-de-test")),
            true,
            None,
            None,
        );
        assert!(ctx.local_tools.is_empty());
        assert_eq!(ctx.extra_mcp.len(), 1);
        let (server, selection) = &ctx.extra_mcp[0];
        assert_eq!(server.name, "toolbox");
        assert_eq!(server.url, "http://sandbox-demo.dev.svc:3000/mcp");
        assert_eq!(
            server.headers.get("authorization").map(String::as_str),
            Some("Bearer jeton-de-test")
        );
        assert_eq!(selection.server, "toolbox");
        assert_eq!(selection.tools, [] as [std::string::String; 0]);
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

    // ── TOOLBOX BEARER par tour (2026-10-08) ──

    /// Encodage base64url sans rembourrage — uniquement pour fabriquer les
    /// JWT de test (la production ne fait que décoder).
    fn b64url_encode(data: &[u8]) -> String {
        const ALPHA: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            let n: u32 = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
            // 1 octet → 2 caractères, 2 → 3, 3 → 4.
            let out_len = chunk.len().saturating_add(1);
            for &shift in [18u32, 12, 6, 0].iter().take(out_len) {
                let idx = usize::try_from((n >> shift) & 63).unwrap();
                out.push(char::from(ALPHA[idx]));
            }
        }
        out
    }

    /// Fabrique un JWT de test dont la signature n'a aucune signification —
    /// `jwt_exp_remaining` lit le payload SANS jamais vérifier la signature.
    fn make_test_jwt(payload: &serde_json::Value) -> String {
        let header = b64url_encode(br#"{"alg":"ES256","kid":"cle-de-test"}"#);
        let body = b64url_encode(serde_json::to_vec(payload).unwrap().as_slice());
        let sig = b64url_encode(b"signature-jamais-verifiee");
        format!("{header}.{body}.{sig}")
    }

    /// Scenario: `fenetre_de_vie_lue_dans_le_claim_exp` — JWT fabriqué avec
    /// `exp = now + 700` : la vie restante rendue est dans `695..=705`
    /// (tolérance d'horloge, pas de re-vérification).
    #[test]
    fn fenetre_de_vie_lue_dans_le_claim_exp() {
        let exp = chrono::Utc::now().timestamp().saturating_add(700);
        let jwt = make_test_jwt(&serde_json::json!({ "sub": "alice", "exp": exp }));
        let remaining = jwt_exp_remaining(&jwt).expect("un JWT avec exp doit être décodé");
        assert!(
            (695..=705).contains(&remaining),
            "vie restante attendue dans 695..=705, obtenue: {remaining}"
        );
    }

    /// Scenario: `fenetre_de_vie_lue_dans_le_claim_exp` (bras négatif) —
    /// sans `exp`, non-JWT, payload illisible ou `exp` non-entier : `None`
    /// (le contrat du tour le traite comme expiré : warn + continuation).
    #[test]
    fn fenetre_de_vie_absente_ou_illisible_rend_none() {
        // sans claim exp
        let jwt = make_test_jwt(&serde_json::json!({ "sub": "alice" }));
        assert_eq!(jwt_exp_remaining(&jwt), None);
        // non-JWT (pas trois segments)
        assert_eq!(jwt_exp_remaining("pas-un-jwt"), None);
        // segment central ni base64url, ni JSON
        assert_eq!(jwt_exp_remaining("aaa.!!!pas-du-base64!!!.ccc"), None);
        // exp non entier
        let jwt = make_test_jwt(&serde_json::json!({ "exp": "tard" }));
        assert_eq!(jwt_exp_remaining(&jwt), None);
    }
}
