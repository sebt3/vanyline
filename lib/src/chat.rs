use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use rig_core::{
    agent::{Agent, MultiTurnStreamItem, Text},
    completion::{CompletionModel, GetTokenUsage},
    message::Message,
    streaming::{StreamedAssistantContent, StreamingChat},
};

use crate::error::VnyError;
use crate::types::{Agent as AgentConfig, McpServer};

#[async_trait]
pub trait ChatSink: Send + Sync {
    async fn send_token(&self, content: &str);
    async fn send_tool_call(&self, name: &str, args: &serde_json::Value);
    async fn send_done(&self);
    async fn send_error(&self, code: &str, message: &str);
}

pub async fn run_chat_turn<M, S>(
    sink: Arc<S>,
    agent_config: &AgentConfig,
    mcp_servers: &[McpServer],
    model: M,
    history: Vec<Message>,
    user_msg: &str,
) -> Result<String, VnyError>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: GetTokenUsage,
    S: ChatSink + 'static,
{
    let system_prompt = &agent_config.system_prompt;

    let agent: Agent<M> = if let Some(handle) = crate::connect_mcp_servers_prefixed(mcp_servers).await {
        rig_core::agent::AgentBuilder::new(model)
            .preamble(system_prompt)
            .tool_server_handle(handle)
            .build()
    } else {
        rig_core::agent::AgentBuilder::new(model)
            .preamble(system_prompt)
            .build()
    };

    stream_agent_response(sink, agent, history, user_msg).await
}

async fn stream_agent_response<M, S>(
    sink: Arc<S>,
    agent: Agent<M>,
    history: Vec<Message>,
    user_msg: &str,
) -> Result<String, VnyError>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: GetTokenUsage,
    S: ChatSink + 'static,
{
    let mut stream = agent.stream_chat(user_msg, history).await;
    let mut response_text = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                Text { text, .. },
            ))) => {
                response_text.push_str(&text);
                sink.send_token(&text).await;
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::ToolCall { tool_call, .. },
            )) => {
                sink.send_tool_call(&tool_call.function.name, &tool_call.function.arguments).await;
            }
            Ok(MultiTurnStreamItem::FinalResponse(_)) => break,
            Err(e) => return Err(VnyError::LlmError(format!("{}", e))),
            _ => {}
        }
    }

    sink.send_done().await;
    Ok(response_text)
}
