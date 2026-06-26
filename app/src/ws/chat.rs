use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, State, WebSocketUpgrade, ws::WebSocket},
    response::IntoResponse,
};
use rig_core::completion::{CompletionModel, GetTokenUsage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    api::conversations::get_or_create_user,
    auth::middleware::AuthUser,
    db::models::{Agent as DbAgent, LlmProvider, McpServer},
    error::AppError,
    AppState,
};

#[derive(Deserialize)]
struct ClientMessage {
    r#type: String,
    content: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Token { content: String },
    ToolCall { name: String, args: serde_json::Value },
    Done { message_id: Uuid },
    Error { code: String, message: String },
}

#[derive(Clone)]
enum CollectedMessage {
    Token(String),
    ToolCall(String, serde_json::Value),
    Error(String, String),
}

struct CollectingSink {
    messages: parking_lot::Mutex<Vec<CollectedMessage>>,
}

impl CollectingSink {
    fn new() -> Self {
        Self {
            messages: parking_lot::Mutex::new(Vec::new()),
        }
    }

    async fn flush(&self, socket: &mut WebSocket) {
        let msgs: Vec<CollectedMessage> = self.messages.lock().clone();
        for msg in msgs.iter() {
            let server_msg = match msg {
                CollectedMessage::Token(content) => ServerMessage::Token {
                    content: content.clone(),
                },
                CollectedMessage::ToolCall(name, args) => ServerMessage::ToolCall {
                    name: name.clone(),
                    args: args.clone(),
                },
                CollectedMessage::Error(code, message) => ServerMessage::Error {
                    code: code.clone(),
                    message: message.clone(),
                },
            };
            let text = serde_json::to_string(&server_msg).unwrap_or_default();
            let _ = socket.send(axum::extract::ws::Message::Text(text.into())).await;
        }
    }

    fn collected_tool_calls(&self) -> Vec<vanyline_lib::ToolCall> {
        self.messages
            .lock()
            .iter()
            .filter_map(|msg| {
                if let CollectedMessage::ToolCall(name, args) = msg {
                    Some(vanyline_lib::ToolCall {
                        name: name.clone(),
                        arguments: args.clone(),
                        result: None,
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

#[async_trait]
impl vanyline_lib::ChatSink for CollectingSink {
    async fn send_token(&self, content: &str) {
        self.messages
            .lock()
            .push(CollectedMessage::Token(content.to_string()));
    }

    async fn send_tool_call(&self, name: &str, args: &serde_json::Value) {
        self.messages.lock().push(CollectedMessage::ToolCall(
            name.to_string(),
            args.clone(),
        ));
    }

    async fn send_done(&self) {}

    async fn send_error(&self, code: &str, message: &str) {
        self.messages.lock().push(CollectedMessage::Error(
            code.to_string(),
            message.to_string(),
        ));
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

async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    user: AuthUser,
    conversation_id: Uuid,
) {
    if let Err(e) = run_socket(socket, state, user, conversation_id).await {
        tracing::error!("ws chat error: {e}");
    }
}

async fn run_socket(
    mut socket: WebSocket,
    state: AppState,
    user: AuthUser,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;

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

    while let Some(Ok(msg)) = socket.recv().await {
        let text = match msg {
            axum::extract::ws::Message::Text(t) => t,
            axum::extract::ws::Message::Close(_) => break,
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
                send_error(&mut socket, "VNL-AGT-001", "No agent assigned to conversation").await;
                continue;
            }
        };

        let result =
            handle_message(&mut socket, &state, conversation_id, agent_id, &client_msg.content)
                .await;
        if let Err(e) = result {
            send_error(&mut socket, "VNL-LLM-001", &e.to_string()).await;
        }
    }
    Ok(())
}

async fn send_error(socket: &mut WebSocket, code: &str, message: &str) {
    let msg = serde_json::to_string(&ServerMessage::Error {
        code: code.to_string(),
        message: message.to_string(),
    })
    .unwrap_or_default();
    let _ = socket.send(axum::extract::ws::Message::Text(msg.into())).await;
}

async fn handle_message(
    socket: &mut WebSocket,
    state: &AppState,
    conversation_id: Uuid,
    agent_id: Uuid,
    user_msg: &str,
) -> Result<(), AppError> {
    let agent_row = sqlx::query_as::<_, crate::db::models::AgentRow>(
        "SELECT * FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::AgentNotFound)?;

    let mcp_servers: Vec<McpServer> = sqlx::query_as::<_, McpServer>(
        r#"SELECT m.* FROM mcp_servers m
           JOIN agent_mcp_servers ams ON ams.mcp_server_id = m.id
           WHERE ams.agent_id = $1 ORDER BY m.name"#,
    )
    .bind(agent_id)
    .fetch_all(&state.pool)
    .await?;

    let provider: Option<LlmProvider> = if let Some(pid) = agent_row.llm_provider_id {
        sqlx::query_as::<_, LlmProvider>("SELECT * FROM llm_providers WHERE id = $1")
            .bind(pid)
            .fetch_optional(&state.pool)
            .await?
    } else {
        sqlx::query_as::<_, LlmProvider>(
            "SELECT * FROM llm_providers WHERE is_default = TRUE LIMIT 1",
        )
        .fetch_optional(&state.pool)
        .await?
    };

    let provider = provider.ok_or_else(|| {
        AppError::LlmError("VNL-LLM-007: no LLM provider configured".to_string())
    })?;

    let model_name = agent_row
        .model
        .as_deref()
        .or(provider.default_model.as_deref())
        .ok_or_else(|| AppError::LlmError("VNL-LLM-008: no model configured".to_string()))?
        .to_string();

    let agent_config = DbAgent {
        id: agent_row.id,
        name: agent_row.name,
        description: agent_row.description,
        system_prompt: agent_row.system_prompt,
        llm_provider_id: agent_row.llm_provider_id,
        model: agent_row.model,
        mcp_servers: mcp_servers.clone(),
        created_at: agent_row.created_at,
        updated_at: agent_row.updated_at,
    };

    let history = load_history(state, conversation_id).await?;

    persist_message(state, conversation_id, "user", user_msg, None).await?;

    let sink = Arc::new(CollectingSink::new());

    let result = match provider.provider_type.as_str() {
        "ollama" => {
            let model = crate::llm::client::build_ollama_model(&provider, &model_name)?;
            run_chat_with_sink(sink.clone(), &agent_config, &mcp_servers, model, history, user_msg).await?
        }
        "openai-compatible" => {
            let model = crate::llm::client::build_openai_compat_model(&provider, &model_name)?;
            run_chat_with_sink(sink.clone(), &agent_config, &mcp_servers, model, history, user_msg).await?
        }
        other => {
            return Err(AppError::LlmError(format!(
                "VNL-LLM-005: unknown provider type: {other}"
            )))
        }
    };

    sink.flush(socket).await;

    let tool_calls = sink.collected_tool_calls();
    let msg_id = persist_message(
        state,
        conversation_id,
        "assistant",
        &result.response_text,
        if tool_calls.is_empty() { None } else { Some(tool_calls) },
    ).await?;

    let done = serde_json::to_string(&ServerMessage::Done { message_id: msg_id }).unwrap_or_default();
    let _ = socket.send(axum::extract::ws::Message::Text(done.into())).await;

    Ok(())
}

async fn run_chat_with_sink<M>(
    sink: Arc<CollectingSink>,
    agent_config: &DbAgent,
    mcp_servers: &[McpServer],
    model: M,
    history: Vec<rig_core::message::Message>,
    user_msg: &str,
) -> Result<vanyline_lib::ChatTurnResult, AppError>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: GetTokenUsage,
{
    let lib_agent = to_lib_agent(agent_config);
    let lib_mcp_servers: Vec<vanyline_lib::McpServer> = mcp_servers
        .iter()
        .map(to_lib_mcp)
        .collect();

    let result = vanyline_lib::run_chat_turn(
        sink,
        &lib_agent,
        &lib_mcp_servers,
        model,
        history,
        user_msg,
    )
    .await?;

    Ok(result)
}

fn to_lib_agent(a: &DbAgent) -> vanyline_lib::Agent {
    vanyline_lib::Agent {
        id: a.id,
        name: a.name.clone(),
        description: a.description.clone(),
        system_prompt: a.system_prompt.clone(),
        llm_provider_id: a.llm_provider_id,
        model: a.model.clone(),
        mcp_servers: Vec::new(),
    }
}

fn to_lib_mcp(m: &McpServer) -> vanyline_lib::McpServer {
    vanyline_lib::McpServer {
        id: m.id,
        name: m.name.clone(),
        server_type: m.server_type.clone(),
        url: m.url.clone(),
        headers: m.headers.clone(),
    }
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
                .and_then(|msg| {
                    match msg.role.as_str() {
                        "user" => Some(rig_core::message::Message::user(msg.content)),
                        "assistant" => Some(rig_core::message::Message::assistant(msg.content)),
                        _ => None,
                    }
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
