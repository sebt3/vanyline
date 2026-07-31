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
use uuid::Uuid;

use vanyline_lib::event::{ChatEvent, EventSink, ToolCallRecord};
use vanyline_lib::session::SessionContext;

use crate::{
    api::conversations::get_or_create_user,
    auth::middleware::AuthUser,
    config_store::PgConfigStore,
    error::AppError,
    AppState,
};

#[derive(Deserialize)]
struct ClientMessage {
    r#type: String,
    content: String,
}

/// Pont EventSink -> canal mpsc : chaque événement est poussé sur `tx` dès
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
    Path(conversation_id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, user, conversation_id))
}

async fn handle_socket(socket: WebSocket, state: AppState, user: AuthUser, conversation_id: Uuid) {
    if let Err(e) = run_socket(socket, state, user, conversation_id).await {
        tracing::error!("ws chat error: {e}");
    }
}

/// Tente d'acquérir le verrou busy pour `conversation_id` : `true` si
/// acquis (aucun tour actif pour cette conversation, insertion faite),
/// `false` si déjà occupée (l'appelant doit renvoyer une erreur busy sans
/// spawn). Extrait de `run_socket` pour être testable sans WebSocket/DB
/// (R4).
fn try_acquire_busy(busy: &Mutex<HashSet<Uuid>>, conversation_id: Uuid) -> bool {
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
    busy: Arc<Mutex<HashSet<Uuid>>>,
    conversation_id: Uuid,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.busy.lock().unwrap().remove(&self.conversation_id);
    }
}

async fn run_socket(
    socket: WebSocket,
    state: AppState,
    user: AuthUser,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let user_id = db_user.id;

    let conv = sqlx::query_as::<_, crate::db::models::Conversation>(
        "SELECT * FROM conversations WHERE id = $1",
    )
    .bind(conversation_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::ConversationNotFound)?;

    if conv.user_id != db_user.id {
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

        let agent_id = match conv.agent_id {
            Some(id) => id,
            None => {
                send_error(&tx, "VNL-AGT-001", "No agent assigned to conversation");
                continue;
            }
        };

        // R4 : verrou busy par conversation — un tour ne bloque plus la
        // lecture du socket (Close honoré pendant un tour actif).
        if !try_acquire_busy(&state.busy, conversation_id) {
            send_error(&tx, "VNL-WS-001", "A turn is already in progress for this conversation");
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

/// Persistance (R9) : le message user est enregistré AVANT l'appel à
/// `run_agent_turn` (il a bien été envoyé, qu'importe l'issue du tour) ;
/// le message assistant seulement APRÈS un tour réussi (le `?` sur
/// `run_agent_turn` empêche d'atteindre le persist_message final en cas
/// d'échec). C'est cette sémantique qui fait référence — le RPC
/// (`cli/src/rpc/handlers.rs`) est aligné dessus séparément.
async fn handle_message(
    tx: &mpsc::UnboundedSender<ChatEvent>,
    state: &AppState,
    conversation_id: Uuid,
    agent_id: Uuid,
    user_id: Uuid,
    user_msg: &str,
) -> Result<(), AppError> {
    let agent_name: String = sqlx::query_scalar("SELECT name FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::AgentNotFound)?;

    let history = load_history(state, conversation_id).await?;

    persist_message(state, conversation_id, "user", user_msg, None).await?;

    let sink = Arc::new(ChannelSink { tx: tx.clone() });
    let ctx = SessionContext {
        store: Arc::new(PgConfigStore::new(state.pool.clone(), user_id)),
        sink,
        local_tools: HashMap::new(),
        subagent_depth_max: 1,
    };

    let result = vanyline_lib::session::run_agent_turn(&ctx, &agent_name, history, user_msg, None)
        .await?;

    let tool_calls = tool_calls_for_persistence(&result.tool_calls);
    persist_message(
        state,
        conversation_id,
        "assistant",
        &result.response_text,
        if tool_calls.is_empty() { None } else { Some(tool_calls) },
    )
    .await?;

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

async fn load_history(
    state: &AppState,
    conversation_id: Uuid,
) -> Result<Vec<rig_core::message::Message>, AppError> {
    let messages = sqlx::query_as::<_, crate::db::models::Message>(
        "SELECT * FROM messages WHERE conversation_id = $1 ORDER BY created_at ASC",
    )
    .bind(conversation_id)
    .fetch_all(&state.pool)
    .await?;

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
    conversation_id: Uuid,
    role: &str,
    content: &str,
    tool_calls: Option<Vec<vanyline_lib::ToolCall>>,
) -> Result<Uuid, AppError> {
    let payload = serde_json::json!({
        "role": role,
        "content": content,
        "tool_calls": tool_calls,
    });
    let id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO messages (conversation_id, role, payload)
           VALUES ($1, $2, $3)
           RETURNING id"#,
    )
    .bind(conversation_id)
    .bind(role)
    .bind(&payload)
    .fetch_one(&state.pool)
    .await?;

    sqlx::query("UPDATE conversations SET updated_at = NOW() WHERE id = $1")
        .bind(conversation_id)
        .execute(&state.pool)
        .await?;

    Ok(id)
}

#[cfg(test)]
mod tests {
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
        let conv = Uuid::new_v4();
        assert!(try_acquire_busy(&busy, conv));
        assert!(!try_acquire_busy(&busy, conv));
    }

    #[test]
    fn try_acquire_busy_different_conversations_independent() {
        let busy = Mutex::new(HashSet::new());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert!(try_acquire_busy(&busy, a));
        assert!(try_acquire_busy(&busy, b));
    }

    #[test]
    fn busy_guard_removes_conversation_on_drop() {
        let busy = Arc::new(Mutex::new(HashSet::new()));
        let conv = Uuid::new_v4();
        busy.lock().unwrap().insert(conv);
        {
            let _guard = BusyGuard { busy: busy.clone(), conversation_id: conv };
            assert!(busy.lock().unwrap().contains(&conv));
        }
        assert!(!busy.lock().unwrap().contains(&conv));
    }
}
