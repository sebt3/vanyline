use crate::rpc::protocol::*;

use serde_json::Value;

pub struct ServerState {
    pub initialized: bool,
    /// Passe à `true` quand `shutdown` a été traité avec succès — la boucle
    /// appelante (`rpc::run_stdio_server`) doit alors arrêter de lire stdin
    /// APRÈS avoir envoyé la réponse.
    pub shutdown_requested: bool,
}

impl ServerState {
    pub fn new() -> Self {
        Self { initialized: false, shutdown_requested: false }
    }
}

/// Traite UNE ligne ndjson reçue sur stdin et retourne la ligne de réponse à
/// écrire sur stdout (déjà sérialisée en JSON, SANS le `\n` final — c'est
/// l'appelant qui l'ajoute). Retourne toujours `Some` dans cette tâche
/// (aucune méthode n'est une notification pure) — le type `Option<String>`
/// est conservé pour la tâche 3 (notifications `chat/event`, qui n'ont pas de
/// réponse RPC directe et empruntent un chemin différent, pas celui-ci).
///
/// Règle d'ordre de validation (STRICTE, dans cet ordre) :
/// 1. La ligne parse comme `JsonRpcRequest` valide ? Sinon -> erreur
///    `VNL-RPC-000`, code JSON-RPC `PARSE_ERROR`, id = `Value::Null` (on ne
///    peut pas extraire l'id d'un JSON invalide ou mal formé).
/// 2. `method == "initialize"` ? Toujours autorisé (même si déjà initialisé
///    — réinitialiser écrase l'état, pas une erreur dans cette tâche).
///    Sinon, si `!state.initialized` -> erreur `VNL-RPC-001`, code
///    `SERVER_ERROR`.
/// 3. Si initialisé et méthode == "shutdown" -> traiter.
/// 4. Sinon (méthode inconnue de cette tâche) -> erreur `VNL-RPC-004`, code
///    `METHOD_NOT_FOUND`.
pub async fn handle_line(state: &mut ServerState, line: &str) -> Option<String> {
    let request: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(req) => req,
        Err(_) => {
            return Some(
                serde_json::to_string(&JsonRpcResponse::error(
                    Value::Null,
                    jsonrpc_code::PARSE_ERROR,
                    "Parse error: invalid JSON-RPC request",
                    vnl_code::MALFORMED_REQUEST,
                ))
                .expect("JSON serialize response"),
            );
        }
    };

    let id = request.id.unwrap_or(Value::Null);

    // "initialize" is always allowed
    if request.method == "initialize" {
        return Some(
            serde_json::to_string(&handle_initialize(state, id, request.params))
                .expect("JSON serialize response"),
        );
    }

    // Must be initialized for any other method
    if !state.initialized {
        return Some(
            serde_json::to_string(&JsonRpcResponse::error(
                id,
                jsonrpc_code::SERVER_ERROR,
                "Server not initialized — call initialize first",
                vnl_code::NOT_INITIALIZED,
            ))
            .expect("JSON serialize response"),
        );
    }

    // Dispatch on known methods
    if request.method == "shutdown" {
        return Some(
            serde_json::to_string(&handle_shutdown(state, id))
                .expect("JSON serialize response"),
        );
    }

    // Unknown method (already initialized)
    Some(
        serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::METHOD_NOT_FOUND,
            format!("Method not found: {}", request.method),
            vnl_code::METHOD_NOT_FOUND,
        ))
        .expect("JSON serialize response"),
    )
}

/// `initialize` : valide `protocol_version == PROTOCOL_VERSION`. Si mismatch,
/// NE MET PAS `state.initialized = true` et retourne l'erreur
/// `VNL-RPC-003` (code JSON-RPC `SERVER_ERROR`) — le message d'erreur inclut
/// la version reçue et la version attendue. Si `params` ne désérialise pas
/// en `InitializeParams` (ex. `protocolVersion` absent ou mauvais type),
/// traiter comme `VNL-RPC-000` (requête malformée), PAS `VNL-RPC-003`.
///
/// Succès : `state.initialized = true`, résultat `InitializeResult` avec
/// `server_version` = `env!("CARGO_PKG_VERSION")`, `workspace_root` et
/// `default_agent` = `None` dans cette tâche (résolus à la tâche 2, une fois
/// `FsConfigStore` branché ici).
fn handle_initialize(state: &mut ServerState, id: Value, params: serde_json::Value) -> JsonRpcResponse {
    let initialize_params: InitializeParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => {
            return JsonRpcResponse::error(
                id,
                jsonrpc_code::PARSE_ERROR,
                "Malformed request: params could not be deserialized as InitializeParams",
                vnl_code::MALFORMED_REQUEST,
            );
        }
    };

    if initialize_params.protocol_version != PROTOCOL_VERSION {
        return JsonRpcResponse::error(
            id,
            jsonrpc_code::SERVER_ERROR,
            format!(
                "Unknown protocol version: got {}, expected {}",
                initialize_params.protocol_version, PROTOCOL_VERSION
            ),
            vnl_code::UNKNOWN_PROTOCOL_VERSION,
        );
    }

    *state = ServerState::new();
    state.initialized = true;
    JsonRpcResponse::success(
        id,
        serde_json::to_value(InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            workspace_root: None,
            default_agent: None,
        })
        .expect("serialize InitializeResult"),
    )
}

/// `shutdown` : positionne `state.shutdown_requested = true`, retourne
/// `result: null`. Ne désérialise pas `params` (ignoré, cf. table du design :
/// `shutdown` n'a pas de params).
fn handle_shutdown(state: &mut ServerState, id: Value) -> JsonRpcResponse {
    state.shutdown_requested = true;
    JsonRpcResponse::success(id, Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::test;

    fn make_request_json(id: impl Into<Value>, method: &str, params: Option<serde_json::Value>) -> String {
        let mut map = serde_json::Map::new();
        map.insert("jsonrpc".into(), Value::String("2.0".into()));
        map.insert("id".into(), id.into());
        map.insert("method".into(), Value::String(method.into()));
        if let Some(p) = params {
            map.insert("params".into(), p);
        }
        serde_json::to_string(&Value::Object(map)).expect("serialize request")
    }

    #[test]
    async fn initialize_success() {
        let mut state = ServerState::new();
        let line = make_request_json(1, "initialize", Some(json!({"protocolVersion": 1})));
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        // No error
        assert!(resp.error.is_none(), "response should have no error");
        // Result should have protocolVersion == 1 and non-empty serverVersion
        let r = resp.result.as_ref().expect("result should be Some");
        assert_eq!(r["protocolVersion"], 1);
        let server_version = r["serverVersion"].as_str().expect("serverVersion should be a string");
        assert!(!server_version.is_empty(), "serverVersion should not be empty");
        // State should be initialized
        assert!(state.initialized);
    }

    #[test]
    async fn initialize_wrong_version() {
        let mut state = ServerState::new();
        let line = make_request_json(2, "initialize", Some(json!({"protocolVersion": 99})));
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-003");
        // State should NOT be initialized
        assert!(!state.initialized);
    }

    #[test]
    async fn initialize_missing_protocol_version() {
        let mut state = ServerState::new();
        let line = make_request_json(3, "initialize", Some(json!({})));
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-000");
    }

    #[test]
    async fn method_before_initialize_rejected() {
        let mut state = ServerState::new();
        let line = make_request_json(4, "shutdown", None);
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-001");
    }

    #[test]
    async fn unknown_method_after_initialize() {
        let mut state = ServerState::new();
        // First initialize
        let init_line = make_request_json(5, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // Then try unknown method
        let unknown_line = make_request_json(6, "config/agents", None);
        let result = handle_line(&mut state, &unknown_line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-004");
    }

    #[test]
    async fn shutdown_after_initialize() {
        let mut state = ServerState::new();
        // First initialize
        let init_line = make_request_json(7, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;

        // Then shutdown
        let shutdown_line = make_request_json(8, "shutdown", None);
        let result = handle_line(&mut state, &shutdown_line).await.expect("response");

        // Wire format check first: `result: null` must actually be on the wire
        // (design doc: shutdown -> null). Checked on the raw string because
        // `Option<Value>`'s Deserialize collapses a JSON `null` into `None`
        // (indistinguishable from an absent field) — a round-tripped struct
        // can't tell the two apart, so the raw JSON is the only way to assert
        // the wire behavior directly.
        assert!(result.contains("\"result\":null"));

        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");
        assert!(resp.error.is_none());
        assert_eq!(resp.result, None);
        assert!(state.shutdown_requested);
    }

    #[test]
    async fn malformed_json_line() {
        let mut state = ServerState::new();
        let line = "not json at all";
        let result = handle_line(&mut state, line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-000");
        assert_eq!(resp.id, Value::Null);
    }

    #[test]
    async fn id_preserved_in_response() {
        // Test with string id for success
        let mut state = ServerState::new();
        let init_line = make_request_json("abc", "initialize", Some(json!({"protocolVersion": 1})));
        let result = handle_line(&mut state, &init_line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        assert_eq!(resp.id, Value::String("abc".into()));

        // Test with string id for error
        let mut state2 = ServerState::new();
        let error_line = make_request_json("xyz", "config/agents", None);
        let result2 = handle_line(&mut state2, &error_line).await;
        assert!(result2.is_some());
        let resp2: JsonRpcResponse = serde_json::from_str(&result2.unwrap()).expect("parse response");
        assert!(resp2.error.is_some());
        assert_eq!(resp2.id, Value::String("xyz".into()));
    }
}