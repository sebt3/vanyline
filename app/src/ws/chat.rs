use axum::{
    extract::{Path, State, WebSocketUpgrade, ws::WebSocket},
    response::IntoResponse,
};
use futures::StreamExt;
use rig_core::{
    agent::{Agent, MultiTurnStreamItem, Text},
    completion::{CompletionModel, GetTokenUsage},
    message::Message,
    streaming::{StreamedAssistantContent, StreamingChat},
};
use rmcp::{serve_client, transport::StreamableHttpClientTransport};
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

        let result = handle_message(&mut socket, &state, conversation_id, agent_id, &client_msg.content).await;
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

    match provider.provider_type.as_str() {
        "ollama" => {
            let model = crate::llm::client::build_ollama_model(&provider, &model_name)?;
            run_chat_turn(socket, state, conversation_id, user_msg, &agent_config, &mcp_servers, model, history).await
        }
        "openai-compatible" => {
            let model = crate::llm::client::build_openai_compat_model(&provider, &model_name)?;
            run_chat_turn(socket, state, conversation_id, user_msg, &agent_config, &mcp_servers, model, history).await
        }
        other => Err(AppError::LlmError(format!(
            "VNL-LLM-005: unknown provider type: {other}"
        ))),
    }
}

async fn load_history(
    state: &AppState,
    conversation_id: Uuid,
) -> Result<Vec<Message>, AppError> {
    let messages = sqlx::query_as::<_, crate::db::models::Message>(
        "SELECT * FROM messages WHERE conversation_id = $1 ORDER BY created_at ASC",
    )
    .bind(conversation_id)
    .fetch_all(&state.pool)
    .await?;

    let history = messages
        .into_iter()
        .filter_map(|m| serde_json::from_value(m.payload).ok())
        .collect();
    Ok(history)
}

async fn connect_mcp_server(
    server: &McpServer,
) -> Result<(Vec<rmcp::model::Tool>, rmcp::service::ServerSink), AppError> {
    match server.server_type.as_str() {
        "http-streamable" => {
            let transport = StreamableHttpClientTransport::from_uri(server.url.as_str());
            let running = serve_client((), transport).await.map_err(|e| {
                AppError::McpError(format!("VNL-MCP-005: connect to {}: {e}", server.name))
            })?;
            let server_sink = running.peer().clone();
            let tools = running.list_all_tools().await.map_err(|e| {
                AppError::McpError(format!("VNL-MCP-006: list tools from {}: {e}", server.name))
            })?;
            Ok((tools, server_sink))
        }
        "sse" => Err(AppError::McpError(
            "VNL-MCP-004: SSE transport not yet implemented".to_string(),
        )),
        other => Err(AppError::McpError(format!(
            "VNL-MCP-003: unknown server type: {other}"
        ))),
    }
}

async fn run_chat_turn<M>(
    socket: &mut WebSocket,
    state: &AppState,
    conversation_id: Uuid,
    user_msg: &str,
    agent_config: &DbAgent,
    mcp_servers: &[McpServer],
    model: M,
    history: Vec<Message>,
) -> Result<(), AppError>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: GetTokenUsage,
{
    let system_prompt = &agent_config.system_prompt;

    let mut connections: Vec<(Vec<rmcp::model::Tool>, rmcp::service::ServerSink)> = Vec::new();
    for server in mcp_servers {
        match connect_mcp_server(server).await {
            Ok(pair) => connections.push(pair),
            Err(e) => tracing::warn!("skipping MCP server {}: {e}", server.name),
        }
    }

    let agent: Agent<M> = if connections.is_empty() {
        rig_core::agent::AgentBuilder::new(model)
            .preamble(system_prompt)
            .build()
    } else {
        let mut iter = connections.into_iter();
        let (first_tools, first_sink) = iter.next().unwrap();
        let mut builder = rig_core::agent::AgentBuilder::new(model)
            .preamble(system_prompt)
            .rmcp_tools(first_tools, first_sink);
        for (tools, sink) in iter {
            builder = builder.rmcp_tools(tools, sink);
        }
        builder.build()
    };

    stream_agent_response(socket, state, conversation_id, user_msg, agent, history).await
}

async fn stream_agent_response<M>(
    socket: &mut WebSocket,
    state: &AppState,
    conversation_id: Uuid,
    user_msg: &str,
    agent: Agent<M>,
    history: Vec<Message>,
) -> Result<(), AppError>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: GetTokenUsage,
{
    persist_message(state, conversation_id, "user", user_msg).await?;

    let mut stream = agent.stream_chat(user_msg, history).await;
    let mut response_text = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                Text { text, .. },
            ))) => {
                response_text.push_str(&text);
                let msg = serde_json::to_string(&ServerMessage::Token { content: text })
                    .unwrap_or_default();
                let _ = socket
                    .send(axum::extract::ws::Message::Text(msg.into()))
                    .await;
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::ToolCall { tool_call, .. },
            )) => {
                let msg = serde_json::to_string(&ServerMessage::ToolCall {
                    name: tool_call.function.name.clone(),
                    args: tool_call.function.arguments.clone(),
                })
                .unwrap_or_default();
                let _ = socket
                    .send(axum::extract::ws::Message::Text(msg.into()))
                    .await;
            }
            Ok(MultiTurnStreamItem::FinalResponse(_)) => break,
            Err(e) => return Err(AppError::LlmError(format!("VNL-LLM-001: {e}"))),
            _ => {}
        }
    }

    let msg_id = persist_message(state, conversation_id, "assistant", &response_text).await?;

    let done =
        serde_json::to_string(&ServerMessage::Done { message_id: msg_id }).unwrap_or_default();
    let _ = socket
        .send(axum::extract::ws::Message::Text(done.into()))
        .await;

    Ok(())
}

async fn persist_message(
    state: &AppState,
    conversation_id: Uuid,
    role: &str,
    content: &str,
) -> Result<Uuid, AppError> {
    let payload = serde_json::json!({ "role": role, "content": content });
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
