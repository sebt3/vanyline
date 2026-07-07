use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;

// ── JSON-RPC 2.0 types ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    pub(crate) fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(crate) fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// ── MCP POST /mcp ─────────────────────────────────────────────────────────────

pub async fn handle(
    State(_state): State<AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    // Notifications have no id and expect HTTP 202, no body
    if req.id.is_none() {
        metrics::counter!("mcp_requests_total", "method" => "notification").increment(1);
        return StatusCode::ACCEPTED.into_response();
    }

    metrics::counter!("mcp_requests_total", "method" => req.method.clone()).increment(1);

    let response = match req.method.as_str() {
        "initialize" => handle_initialize(req.id, req.params),
        "notifications/initialized" => {
            // Should not happen (no id), but handle gracefully
            return StatusCode::ACCEPTED.into_response();
        }
        "tools/list" => handle_tools_list(req.id),
        "tools/call" => handle_tools_call(req.id, req.params),
        _ => JsonRpcResponse::err(req.id, -32601, format!("Method not found: {}", req.method)),
    };

    Json(response).into_response()
}

pub(crate) fn handle_initialize(_id: Option<Value>, _params: Value) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        _id,
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": env!("CARGO_PKG_NAME"),
                "version": env!("CARGO_PKG_VERSION"),
            }
        }),
    )
}

pub(crate) fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        serde_json::json!({
            "tools": [
                {
                    "name": "ping",
                    "description": "Health-check tool: replies pong",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                }
            ]
        }),
    )
}

fn handle_tools_call(id: Option<Value>, params: Value) -> JsonRpcResponse {
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return JsonRpcResponse::err(id, -32602, "Missing tool name in params");
        }
    };

    match name {
        "ping" => JsonRpcResponse::ok(
            id,
            serde_json::json!({
                "content": [
                    {
                        "type": "text",
                        "text": "pong"
                    }
                ],
                "isError": false
            }),
        ),
        unknown => JsonRpcResponse::err(id, -32602, format!("Unknown tool: {unknown}")),
    }
}

// ── GET /.well-known/oauth-protected-resource (RFC 9728) ─────────────────────

pub async fn oauth_metadata(State(state): State<AppState>) -> Json<Value> {
    let resource = state
        .config
        .public_url
        .as_deref()
        .unwrap_or(crate::config::DEFAULT_PUBLIC_URL);

    let auth_servers: Vec<_> = state
        .config
        .oidc_issuer
        .iter()
        .map(|s| s.as_str())
        .collect();

    Json(serde_json::json!({
        "resource": resource,
        "authorization_servers": auth_servers,
        "bearer_methods_supported": ["header"],
    }))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_correct_protocol_version() {
        let resp = handle_initialize(Some(serde_json::json!(1)), Value::Null);
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["serverInfo"]["name"].is_string());
        assert!(result["serverInfo"]["version"].is_string());
    }

    #[test]
    fn initialize_echoes_request_id() {
        let id = serde_json::json!("req-abc");
        let resp = handle_initialize(Some(id.clone()), Value::Null);
        assert_eq!(resp.id.unwrap(), id);
    }

    #[test]
    fn tools_list_contains_ping() {
        let resp = handle_tools_list(Some(serde_json::json!(1)));
        let result = resp.result.unwrap();
        let tools = &result["tools"];
        assert!(tools.as_array().unwrap().len() == 1);
        assert_eq!(tools[0]["name"], "ping");
        assert_eq!(tools[0]["description"], "Health-check tool: replies pong");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn tools_call_ping_returns_pong() {
        let params = serde_json::json!({ "name": "ping", "arguments": {} });
        let resp = handle_tools_call(Some(serde_json::json!(1)), params);
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["text"], "pong");
        assert_eq!(result["isError"], false);
    }

    #[test]
    fn tools_call_unknown_tool_returns_error() {
        let params = serde_json::json!({ "name": "nope", "arguments": {} });
        let resp = handle_tools_call(Some(serde_json::json!(1)), params);
        let error = resp.error.unwrap();
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, "Unknown tool: nope");
    }

    #[test]
    fn tools_call_missing_name_returns_error() {
        let params = serde_json::json!({ "arguments": {} });
        let resp = handle_tools_call(Some(serde_json::json!(1)), params);
        let error = resp.error.unwrap();
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, "Missing tool name in params");
    }

    #[test]
    fn rpc_ok_has_no_error() {
        let resp = JsonRpcResponse::ok(Some(serde_json::json!(42)), serde_json::json!("data"));
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), "data");
        assert_eq!(resp.jsonrpc, "2.0");
    }

    #[test]
    fn rpc_err_has_no_result() {
        let resp = JsonRpcResponse::err(Some(serde_json::json!(42)), -32601, "not found");
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "not found");
    }

    #[test]
    fn rpc_err_code_method_not_found() {
        let resp = JsonRpcResponse::err(None, -32601, "Method not found: foo");
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
