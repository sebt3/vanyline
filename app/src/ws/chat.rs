use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::{
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;

use vanyline_lib::domain::{McpSelection, McpServer, McpTransport};
use vanyline_lib::event::{ChatEvent, EventSink, ToolCallRecord};
use vanyline_lib::session::SessionContext;

use miryad_core::auth::AuthUser;
use miryad_core::users::resolve_user;
use sea_orm::{
    ActiveValue::NotSet, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
};

use crate::{AppState, config_store::PgConfigStore, error::AppError};

#[derive(Deserialize)]
struct ClientMessage {
    r#type: String,
    content: String,
}

/// Pont `EventSink` -> canal mpsc : chaque événement est poussé sur `tx` dès
/// qu'il est émis, sans attendre la fin du tour. Remplace l'ancien
/// `CollectingSink` (bufferisait tout un tour avant le premier octet
/// envoyé) — la tâche `forward_events` (une par connexion, pas une par
/// tour) draine le canal et écrit sur le socket au fil de l'eau.
struct ChannelSink {
    tx: mpsc::UnboundedSender<ChatEvent>,
}

#[async_trait::async_trait]
impl EventSink for ChannelSink {
    async fn emit(&self, event: ChatEvent) {
        let _ = self.tx.send(event);
    }
}

pub async fn ws_chat_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(conversation_id): Path<i32>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, user, conversation_id))
}

async fn handle_socket(socket: WebSocket, state: AppState, user: AuthUser, conversation_id: i32) {
    if let Err(e) = run_socket(socket, state, user, conversation_id).await {
        tracing::error!("ws chat error: {e}");
    }
}

/// Tente d'acquérir le verrou busy pour `conversation_id` : `true` si
/// acquis (aucun tour actif pour cette conversation, insertion faite),
/// `false` si déjà occupée (l'appelant doit renvoyer une erreur busy sans
/// spawn). Extrait de `run_socket` pour être testable sans WebSocket/DB
/// (R4).
#[allow(clippy::unwrap_used)] // mutex empoisonne = etat deja corrompu ailleurs, panic attendu
fn try_acquire_busy(busy: &Mutex<HashSet<i32>>, conversation_id: i32) -> bool {
    let mut guard = busy.lock().unwrap();
    if guard.contains(&conversation_id) {
        false
    } else {
        guard.insert(conversation_id);
        true
    }
}

/// Nettoie `busy` à la fin d'un tour (spawné), même en cas de panique dans
/// la tâche — `Drop` est synchrone, cohérent avec `busy: Mutex` (pas
/// `tokio::sync::Mutex`). Même pattern que `BusyGuard` côté RPC.
struct BusyGuard {
    busy: Arc<Mutex<HashSet<i32>>>,
    conversation_id: i32,
}

impl Drop for BusyGuard {
    #[allow(clippy::unwrap_used)] // Drop::drop ne peut pas retourner Result ; mutex empoisonne = panic attendu
    fn drop(&mut self) {
        self.busy.lock().unwrap().remove(&self.conversation_id);
    }
}

async fn run_socket(
    socket: WebSocket,
    state: AppState,
    user: AuthUser,
    conversation_id: i32,
) -> Result<(), AppError> {
    let db = &state.auth.db;
    let principal_user = resolve_user(db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let user_id = principal_user.id;

    let conv = crate::db::entities::conversations::Entity::find_by_id(conversation_id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or(AppError::ConversationNotFound)?;

    if conv.owner_id != user_id {
        return Err(AppError::ConversationAccessDenied);
    }

    let (ws_sink, mut ws_stream) = socket.split();
    let (tx, rx) = mpsc::unbounded_channel::<ChatEvent>();
    let forward_handle = tokio::spawn(forward_events(rx, ws_sink));

    while let Some(Ok(msg)) = ws_stream.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };

        let client_msg: ClientMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if client_msg.r#type != "message" {
            continue;
        }

        let agent_id = if let Some(id) = conv.agent_id {
            id
        } else {
            send_error(&tx, "VNL-AGT-001", "No agent assigned to conversation");
            continue;
        };

        // R4 : verrou busy par conversation — un tour ne bloque plus la
        // lecture du socket (Close honoré pendant un tour actif).
        if !try_acquire_busy(&state.busy, conversation_id) {
            send_error(
                &tx,
                "VNL-WS-001",
                "A turn is already in progress for this conversation",
            );
            continue;
        }

        let spawn_state = state.clone();
        let spawn_tx = tx.clone();
        let content = client_msg.content.clone();
        tokio::spawn(async move {
            let _guard = BusyGuard {
                busy: spawn_state.busy.clone(),
                conversation_id,
            };
            let result = handle_message(
                &spawn_tx,
                &spawn_state,
                conversation_id,
                conv.context_id,
                agent_id,
                user_id,
                &content,
            )
            .await;
            if let Err(e) = result {
                send_error(&spawn_tx, "VNL-LLM-001", &e.to_string());
            }
        });
    }

    drop(tx);
    let _ = forward_handle.await;
    Ok(())
}

/// Draine `rx` et écrit chaque `ChatEvent` sur le socket dès réception —
/// c'est ce qui rend le streaming réel. Se termine quand `tx` est dropped
/// (fin de `run_socket`) ou si l'écriture échoue (client déconnecté).
async fn forward_events(
    mut rx: mpsc::UnboundedReceiver<ChatEvent>,
    mut sink: SplitSink<WebSocket, Message>,
) {
    while let Some(event) = rx.recv().await {
        let text = serde_json::to_string(&event).unwrap_or_default();
        if sink.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }
}

fn send_error(tx: &mpsc::UnboundedSender<ChatEvent>, code: &str, message: &str) {
    let _ = tx.send(ChatEvent::Error {
        code: code.to_string(),
        message: message.to_string(),
    });
}

fn send_tool_unavailable(tx: &mpsc::UnboundedSender<ChatEvent>, server: &str, reason: &str) {
    let _ = tx.send(ChatEvent::ToolUnavailable {
        server: server.to_string(),
        reason: reason.to_string(),
    });
}

/// Résout `extra_mcp` à partir du contexte de la conversation. Aujourd'hui,
/// seul `kind = "sandbox"` produit un serveur MCP (l'URL de la sandbox
/// elle-même, comme le fait `--toolbox` côté CLI) — les autres `kind` (à
/// venir, cf. docs/features/chat-app-fonctionnel.md) n'ont pas encore de
/// toolset associé et retournent une liste vide, silencieusement (ce n'est
/// pas une panne, juste pas encore implémenté). Un échec de résolution
/// (sandbox absente, K8s injoignable) est non bloquant pour le tour : signalé
/// via `ChatEvent::ToolUnavailable` plutôt que d'échouer le tour entier.
///
/// `context.data.sandbox_name` vient du client (posé à la création de la
/// conversation, cf. `ChatContextInput`) — sans le scoping owner ci-dessous,
/// n'importe quel utilisateur authentifié pourrait faire résoudre les tools
/// MCP d'une sandbox appartenant à quelqu'un d'autre en la nommant dans le
/// contexte. Même vérification que `api::sandboxes::get_sandbox`
/// (project.spec.owner == owner de l'utilisateur), pas de raccourci ici.
async fn resolve_extra_mcp(
    state: &AppState,
    tx: &mpsc::UnboundedSender<ChatEvent>,
    context_id: i32,
    user_id: i32,
) -> Result<Vec<(McpServer, McpSelection)>, AppError> {
    let db = &state.auth.db;
    let context = crate::db::entities::chat_contexts::Entity::find_by_id(context_id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::InternalError("VNL-DB-007: context not found".into()))?;

    if context.kind != "sandbox" {
        return Ok(Vec::new());
    }

    let Some(sandbox_name) = context.data.get("sandbox_name").and_then(|v| v.as_str()) else {
        send_tool_unavailable(
            tx,
            "sandbox",
            "contexte sandbox invalide (sandbox_name absent)",
        );
        return Ok(Vec::new());
    };

    let owner = match crate::api::owners::resolve_owner_name(state, user_id).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            send_tool_unavailable(
                tx,
                sandbox_name,
                "aucun owner K8s associé à cet utilisateur",
            );
            return Ok(Vec::new());
        }
        Err(e) => {
            send_tool_unavailable(tx, sandbox_name, &e.to_string());
            return Ok(Vec::new());
        }
    };

    let client = match crate::k8s::client(state).await {
        Ok(c) => c,
        Err(e) => {
            send_tool_unavailable(tx, sandbox_name, &e.to_string());
            return Ok(Vec::new());
        }
    };

    let sandbox = match client.get_sandbox(sandbox_name).await {
        Ok(s) => s,
        Err(e) => {
            send_tool_unavailable(tx, sandbox_name, &e.to_string());
            return Ok(Vec::new());
        }
    };
    let project = match client.get_project(&sandbox.spec.project).await {
        Ok(p) => p,
        Err(e) => {
            send_tool_unavailable(tx, sandbox_name, &e.to_string());
            return Ok(Vec::new());
        }
    };
    if project.spec.owner != owner {
        send_tool_unavailable(
            tx,
            sandbox_name,
            "sandbox hors du périmètre de cet utilisateur",
        );
        return Ok(Vec::new());
    }

    let url = match client.sandbox_mcp_url(sandbox_name).await {
        Ok(url) => url,
        Err(e) => {
            send_tool_unavailable(tx, sandbox_name, &e.to_string());
            return Ok(Vec::new());
        }
    };

    Ok(vec![(
        McpServer {
            name: "sandbox".to_string(),
            transport: McpTransport::HttpStreamable,
            url,
            headers: Default::default(),
        },
        McpSelection {
            server: "sandbox".to_string(),
            tools: vec![],
        },
    )])
}

/// Persistance (R9) : le message user est enregistré AVANT l'appel à
/// `run_agent_turn` (il a bien été envoyé, qu'importe l'issue du tour) ;
/// le message assistant seulement APRÈS un tour réussi (le `?` sur
/// `run_agent_turn` empêche d'atteindre le `persist_message` final en cas
/// d'échec). C'est cette sémantique qui fait référence — le RPC
/// (`cli/src/rpc/handlers.rs`) est aligné dessus séparément.
#[allow(clippy::unwrap_used)] // mutex empoisonne = etat deja corrompu ailleurs, panic attendu
async fn handle_message(
    tx: &mpsc::UnboundedSender<ChatEvent>,
    state: &AppState,
    conversation_id: i32,
    context_id: i32,
    agent_id: i32,
    user_id: i32,
    user_msg: &str,
) -> Result<(), AppError> {
    let db = &state.auth.db;
    let agent_name: String = crate::db::entities::agents::Entity::find_by_id(agent_id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .map(|a| a.name)
        .ok_or(AppError::AgentNotFound)?;

    let history = load_history(state, conversation_id).await?;

    persist_message(state, conversation_id, user_id, "user", user_msg, None).await?;

    let todo_initial: Option<String> =
        crate::db::entities::conversations::Entity::find_by_id(conversation_id)
            .one(db)
            .await
            .map_err(AppError::from)?
            .and_then(|conv| conv.todo);

    let extra_mcp = resolve_extra_mcp(state, tx, context_id, user_id).await?;

    let sink = Arc::new(ChannelSink { tx: tx.clone() });
    let ctx = SessionContext {
        store: Arc::new(PgConfigStore::new(state.auth.db.clone(), user_id)),
        sink,
        local_tools: HashMap::new(),
        subagent_depth_max: 1,
        extra_mcp,
        model_override: None,
        todo_state: Arc::new(std::sync::Mutex::new(todo_initial.clone())),
    };

    let result =
        vanyline_lib::session::run_agent_turn(&ctx, &agent_name, history, user_msg, None).await?;

    let tool_calls = tool_calls_for_persistence(&result.tool_calls);
    persist_message(
        state,
        conversation_id,
        user_id,
        "assistant",
        &result.response_text,
        if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
    )
    .await?;

    let current_todo = ctx.todo_state.lock().unwrap().clone();
    if let Some(todo) = todo_to_persist(current_todo, todo_initial) {
        let mut active: crate::db::entities::conversations::ActiveModel =
            crate::db::entities::conversations::Entity::find_by_id(conversation_id)
                .one(db)
                .await
                .map_err(AppError::from)?
                .ok_or(AppError::ConversationNotFound)?
                .into_active_model();
        active.todo = Set(Some(todo));
        crate::db::entities::conversations::Entity::update(active)
            .exec(db)
            .await
            .map_err(AppError::from)?;
    }

    Ok(())
}

/// `ChatTurnResult.tool_calls` (`ToolCallRecord`, avec `id` — sert à la
/// corrélation ToolCall/ToolResult dans le flux d'événements, cf.
/// `lib/src/event.rs`) -> `vanyline_lib::ToolCall` (sans `id` — forme de
/// persistance existante, `messages.payload`, inchangée par cette tâche).
/// Pure, testable sans réseau ni DB.
fn tool_calls_for_persistence(records: &[ToolCallRecord]) -> Vec<vanyline_lib::ToolCall> {
    records
        .iter()
        .map(|r| vanyline_lib::ToolCall {
            name: r.name.clone(),
            arguments: r.arguments.clone(),
            result: r.result.clone(),
        })
        .collect()
}

/// Renvoie la valeur todo à persister après un tour : `Some(final)` si l'état
/// a été modifié par rapport au seed `initial` ET si le résultat est non-NULL.
/// `None` sinon (aucun changement, ou résultat NULL) — on ne NULL-e jamais un état
/// antérieur par un update systématique.
fn todo_to_persist(current: Option<String>, initial: Option<String>) -> Option<String> {
    match current {
        Some(_) if current != initial => current,
        _ => None,
    }
}

async fn load_history(
    state: &AppState,
    conversation_id: i32,
) -> Result<Vec<rig_core::message::Message>, AppError> {
    let db = &state.auth.db;
    let messages = crate::db::entities::messages::Entity::find()
        .filter(crate::db::entities::messages::Column::ConversationId.eq(conversation_id))
        .order_by_asc(crate::db::entities::messages::Column::CreatedAt)
        .all(db)
        .await
        .map_err(AppError::from)?;

    let history: Vec<rig_core::message::Message> = messages
        .into_iter()
        .filter_map(|m| {
            serde_json::from_value::<vanyline_lib::Message>(m.payload)
                .ok()
                .and_then(|msg| match msg.role.as_str() {
                    "user" => Some(rig_core::message::Message::user(msg.content)),
                    "assistant" => Some(rig_core::message::Message::assistant(msg.content)),
                    _ => None,
                })
        })
        .collect();
    Ok(history)
}

async fn persist_message(
    state: &AppState,
    conversation_id: i32,
    user_id: i32,
    role: &str,
    content: &str,
    tool_calls: Option<Vec<vanyline_lib::ToolCall>>,
) -> Result<i32, AppError> {
    let db = &state.auth.db;
    let payload = serde_json::json!({
        "role": role,
        "content": content,
        "tool_calls": tool_calls,
    });
    let active: crate::db::entities::messages::ActiveModel =
        crate::db::entities::messages::ActiveModel {
            id: NotSet,
            owner_id: Set(user_id),
            conversation_id: Set(conversation_id),
            role: Set(role.to_string()),
            payload: Set(payload),
            created_at: Set(chrono::Utc::now()),
        };
    let res = crate::db::entities::messages::Entity::insert(active)
        .exec(db)
        .await
        .map_err(AppError::from)?;
    let id = res.last_insert_id as i32;

    let mut conv_active: crate::db::entities::conversations::ActiveModel =
        crate::db::entities::conversations::Entity::find_by_id(conversation_id)
            .one(db)
            .await
            .map_err(AppError::from)?
            .ok_or(AppError::ConversationNotFound)?
            .into_active_model();
    conv_active.updated_at = Set(chrono::Utc::now());
    crate::db::entities::conversations::Entity::update(conv_active)
        .exec(db)
        .await
        .map_err(AppError::from)?;

    Ok(id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn tool_calls_for_persistence_drops_id_keeps_rest() {
        let records = vec![
            ToolCallRecord {
                id: "call-1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "x"}),
                result: Some("42".to_string()),
            },
            ToolCallRecord {
                id: "call-2".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "a.txt"}),
                result: None,
            },
        ];
        let calls = tool_calls_for_persistence(&records);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].arguments, serde_json::json!({"q": "x"}));
        assert_eq!(calls[0].result, Some("42".to_string()));
        assert_eq!(calls[1].name, "read_file");
        assert_eq!(calls[1].result, None);
    }

    #[test]
    fn tool_calls_for_persistence_empty() {
        let calls = tool_calls_for_persistence(&[]);
        assert!(calls.is_empty());
    }

    #[test]
    fn try_acquire_busy_first_call_succeeds_second_fails() {
        let busy = Mutex::new(HashSet::new());
        let conv = 1;
        assert!(try_acquire_busy(&busy, conv));
        assert!(!try_acquire_busy(&busy, conv));
    }

    #[test]
    fn try_acquire_busy_different_conversations_independent() {
        let busy = Mutex::new(HashSet::new());
        let a = 1;
        let b = 2;
        assert!(try_acquire_busy(&busy, a));
        assert!(try_acquire_busy(&busy, b));
    }

    #[test]
    fn busy_guard_removes_conversation_on_drop() {
        let busy = Arc::new(Mutex::new(HashSet::new()));
        let conv = 1;
        busy.lock().unwrap().insert(conv);
        {
            let _guard = BusyGuard {
                busy: busy.clone(),
                conversation_id: conv,
            };
            assert!(busy.lock().unwrap().contains(&conv));
        }
        assert!(!busy.lock().unwrap().contains(&conv));
    }

    #[test]
    fn todo_to_persist_changed_some_from_none_returns_some() {
        assert_eq!(
            todo_to_persist(Some("new todo".to_string()), None),
            Some("new todo".to_string())
        );
    }

    #[test]
    fn todo_to_persist_unchanged_returns_none() {
        assert_eq!(
            todo_to_persist(Some("x".to_string()), Some("x".to_string())),
            None
        );
    }

    #[test]
    fn todo_to_persist_changed_to_none_does_not_null_previous() {
        assert_eq!(todo_to_persist(None, Some("old".to_string())), None);
    }

    #[test]
    fn todo_to_persist_unchanged_none_is_none() {
        assert_eq!(todo_to_persist(None, None), None);
    }
}
