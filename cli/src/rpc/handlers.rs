use crate::rpc::protocol::*;

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;
use vanyline_lib::event::{ChatEvent, EventSink};
use vanyline_lib::store::ConfigStore;

use crate::store;

pub struct ServerState {
    pub initialized: bool,
    pub shutdown_requested: bool,
    pub store: Option<Arc<crate::fs_store::FsConfigStore>>,
    /// Sender du canal d'écriture unique de `rpc::mod` — cloné dans toute
    /// tâche spawnée qui a besoin d'écrire (réponse finale OU notification
    /// `chat/event`, tâche 03b).
    #[allow(dead_code)] // utilisé par la tâche 03b (chat/send)
    pub tx: mpsc::UnboundedSender<String>,
    /// Conversations avec un tour actif — vérifié/inséré de façon atomique
    /// avant de spawner un tour (tâche 03b) ; PAS utilisé par cette tâche
    /// au-delà de sa définition et de son reset sur `initialize`.
    pub busy: Arc<Mutex<HashSet<Uuid>>>,
    /// Compteur `seq` par conversation pour les notifications `chat/event`
    /// (tâche 03b) — défini ici pour que `RpcEventSink` (cette tâche)
    /// puisse déjà l'utiliser.
    pub seq: Arc<Mutex<HashMap<Uuid, u64>>>,
}

impl ServerState {
    pub fn new(tx: mpsc::UnboundedSender<String>) -> Self {
        Self {
            initialized: false,
            shutdown_requested: false,
            store: None,
            tx,
            busy: Arc::new(Mutex::new(HashSet::new())),
            seq: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Résout la racine de config à partir du `workspace` optionnel
/// d'`initialize` — PAS le cwd du process (design : "c'est l'extension qui
/// la connaît (workspace folder VS Code), pas le cwd"). Fallback sur le
/// cwd uniquement si `workspace` est `None` (usage CLI direct / tests).
fn resolve_layers(workspace: Option<&str>) -> crate::config::Layers {
    let root = workspace
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    crate::config::Layers::discover(&root)
}

/// Convertit une opération de config (`Result<T, VnyError>`) en réponse
/// JSON-RPC : `Ok` → succès (sérielisé en Value), `Err` → erreur
/// `VNL-RPC-006` ("config read error", message = format!("{}", err)).
fn config_error_response<T: serde::Serialize>(
    id: Value,
    result: Result<T, vanyline_lib::VnyError>,
) -> JsonRpcResponse {
    match result {
        Ok(v) => {
            JsonRpcResponse::success(id, serde_json::to_value(v).expect("serialize config list"))
        }
        Err(e) => {
            JsonRpcResponse::error(
                id,
                jsonrpc_code::SERVER_ERROR,
                format!("{e}"),
                vnl_code::CONFIG_ERROR,
            )
        }
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
            serde_json::to_string(&handle_initialize(state, id, request.params).await)
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
    let store = state.store.as_ref().expect("initialized implies store = Some");
    match request.method.as_str() {
        "shutdown" => {
            state.shutdown_requested = true;
            Some(
                serde_json::to_string(&shutdown_response(id)).expect("JSON serialize response"),
            )
        }
        "config/agents" => {
            let store_clone = store.clone();
            Some(handle_config_list(id, async {
                store_clone.list_agents().await
            }).await)
        }
        "config/models" => {
            let store_clone = store.clone();
            Some(handle_config_list(id, async {
                store_clone.list_models().await
            }).await)
        }
        "config/toolsets" => {
            let store_clone = store.clone();
            Some(handle_config_list(id, async {
                store_clone.list_toolsets().await
            }).await)
        }
        "config/skills" => {
            let store_clone = store.clone();
            Some(handle_config_list(id, async {
                store_clone.list_skills().await
            }).await)
        }
        "conversations/list" => Some(handle_conversations_list(id)),
        "conversations/get" => Some(handle_conversations_get(id, request.params)),
        "conversations/create" => Some(handle_conversations_create(id, request.params)),
        "conversations/delete" => Some(handle_conversations_delete(id, request.params)),
        "chat/cancel" => Some(handle_chat_cancel(id, request.params)),
        _ => Some(
            serde_json::to_string(&JsonRpcResponse::error(
                id,
                jsonrpc_code::METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
                vnl_code::METHOD_NOT_FOUND,
            ))
            .expect("JSON serialize response"),
        ),
    }
}

/// Helper async : reçoit un future produisant la liste typée d'une méthode
/// `config/*` (`Vec<Agent>`, `Vec<ModelProfile>`, ...), l'exécute et
/// convertit le résultat en réponse JSON-RPC sérialisée.
async fn handle_config_list<T: serde::Serialize>(
    id: Value,
    action: impl std::future::Future<Output = Result<T, vanyline_lib::VnyError>>,
) -> String {
    let result = action.await;
    serde_json::to_string(&config_error_response(id, result)).expect("serialize config response")
}

/// `initialize` : valide `protocol_version == PROTOCOL_VERSION`. Si mismatch,
/// NE MET PAS `state.initialized = true` et retourne l'erreur
/// `VNL-RPC-003` (code JSON-RPC `SERVER_ERROR`) — le message d'erreur inclut
/// la version reçue et la version attendue. Si `params` ne désérialise pas
/// en `InitializeParams` (ex. `protocolVersion` absent ou mauvais type),
/// traiter comme `VNL-RPC-000` (requête malformée), PAS `VNL-RPC-003`.
///
/// Succès : `state.initialized = true`, store branché, résultat
/// `InitializeResult` avec `server_version` = `env!("CARGO_PKG_VERSION")`,
/// `workspace_root` = `layers.workspace_dir.display().to_string()`
/// (ou `None` si aucun marqueur `.vanyline`/`.git`), `default_agent`
/// = `store.default_agent()` (`Ok(Some(name))` -> `Some(name)`,
/// `Ok(None)` ou `Err` -> `None`).
async fn handle_initialize(state: &mut ServerState, id: Value, params: serde_json::Value) -> JsonRpcResponse {
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

    // Build the store from workspace
    let layers = resolve_layers(initialize_params.workspace.as_deref());
    let store = crate::fs_store::FsConfigStore::new(layers);

    // Resolve default_agent from store — an error here does NOT fail
    // initialize itself; the real error will surface on first config/* call.
    let default_agent = store.default_agent().await.ok().flatten();

    // Resolve workspace_root from layers
    let workspace_root = store.layers().workspace_dir.as_ref().map(|p| p.display().to_string());

    state.initialized = true;
    state.shutdown_requested = false;
    state.store = Some(Arc::new(store));
    state.busy.lock().unwrap().clear();
    state.seq.lock().unwrap().clear();

    JsonRpcResponse::success(
        id,
        serde_json::to_value(InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            workspace_root,
            default_agent,
        })
        .expect("serialize InitializeResult"),
    )
}

/// `shutdown` renvoie `result: null`. Ne désérialise pas `params`
/// (ignoré, cf. table du design : `shutdown` n'a pas de params).
fn shutdown_response(id: Value) -> JsonRpcResponse {
    JsonRpcResponse::success(id, Value::Null)
}

/// `conversations/list` : `store::list_conversations()` -> erreur io ->
/// `VNL-RPC-007` (message = `format!("{e}")`) ; succès -> tableau de
/// `ConversationSummary` (PAS la `Conversation` complète — pas de
/// `messages` dans la liste, cf. design : `Conversation` complète
/// seulement pour `conversations/get`).
fn handle_conversations_list(id: Value) -> String {
    let result = store::list_conversations();
    match result {
        Ok(convs) => {
            let summaries: Vec<ConversationSummary> = convs.iter().map(ConversationSummary::from).collect();
            serde_json::to_string(&JsonRpcResponse::success(id, Value::Array(
                summaries.into_iter().map(|s| serde_json::to_value(s).expect("serialize summary")).collect()
            ))).expect("serialize list response")
        }
        Err(e) => {
            serde_json::to_string(&JsonRpcResponse::error(
                id,
                jsonrpc_code::SERVER_ERROR,
                format!("{e}"),
                vnl_code::CONVERSATION_STORAGE_ERROR,
            )).expect("serialize list error response")
        }
    }
}

/// `conversations/get` : params -> `ConversationIdParams`. Si
/// désérialisation échoue OU si `id` n'est pas un UUID valide
/// (`uuid::Uuid::parse_str`) -> `VNL-RPC-000` (requête malformée). Sinon
/// `store::get_conversation(&uuid)` : `Err` avec
/// `e.kind() == std::io::ErrorKind::NotFound` -> `VNL-RPC-005` ; toute
/// autre erreur io -> `VNL-RPC-007`. Succès -> la `Conversation` COMPLÈTE
/// (`vanyline_lib::Conversation`, déjà `Serialize`), PAS un
/// `ConversationSummary`.
fn handle_conversations_get(id: Value, params: serde_json::Value) -> String {
    let params: ConversationIdParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::PARSE_ERROR,
            "Malformed request: params could not be deserialized as ConversationIdParams",
            vnl_code::MALFORMED_REQUEST,
        )).expect("serialize get error response"),
    };
    let uuid = match Uuid::parse_str(&params.id) {
        Ok(u) => u,
        Err(_) => return serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::PARSE_ERROR,
            format!("Invalid UUID in id: {}", params.id),
            vnl_code::MALFORMED_REQUEST,
        )).expect("serialize get error response"),
    };
    match store::get_conversation(&uuid) {
        Ok(conv) => serde_json::to_string(&JsonRpcResponse::success(id, serde_json::to_value(&conv).expect("serialize conversation"))).expect("serialize get response"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            serde_json::to_string(&JsonRpcResponse::error(
                id,
                jsonrpc_code::SERVER_ERROR,
                format!("Conversation not found: {}", params.id),
                vnl_code::CONVERSATION_NOT_FOUND,
            )).expect("serialize get not found error response")
        }
        Err(e) => serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::SERVER_ERROR,
            format!("{e}"),
            vnl_code::CONVERSATION_STORAGE_ERROR,
        )).expect("serialize get error response"),
    }
}

/// `conversations/create` : params -> `ConversationCreateParams`
/// (désérialisation échoue seulement sur un type incorrect, PAS sur
/// l'absence de champs — les deux sont `#[serde(default)]` -> `VNL-RPC-000`
/// si malformé quand même, ex. `agent` envoyé comme nombre). Construit une
/// nouvelle `vanyline_lib::Conversation` : `id: Uuid::new_v4()`, `agent`,
/// `title` depuis les params, `messages: Vec::new()`. AUCUNE validation
/// que `agent` référence un agent existant du `FsConfigStore` (même
/// comportement que la commande CLI `conversations new`, cf.
/// `main.rs::run_conversation::New` déjà lu — ne pas introduire une
/// vérification que la CLI elle-même ne fait pas). `store::save_conversation(&conv)` :
/// `Err` -> `VNL-RPC-007` ; succès -> `ConversationSummary::from(&conv)`.
fn handle_conversations_create(id: Value, params: serde_json::Value) -> String {
    let params: ConversationCreateParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::PARSE_ERROR,
            "Malformed request: params could not be deserialized as ConversationCreateParams",
            vnl_code::MALFORMED_REQUEST,
        )).expect("serialize create error response"),
    };
    let conv = vanyline_lib::Conversation {
        id: Uuid::new_v4(),
        agent: params.agent,
        title: params.title,
        messages: Vec::new(),
    };
    match store::save_conversation(&conv) {
        Ok(()) => {
            let summary = ConversationSummary::from(&conv);
            serde_json::to_string(&JsonRpcResponse::success(id, serde_json::to_value(summary).expect("serialize summary"))).expect("serialize create response")
        }
        Err(e) => serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::SERVER_ERROR,
            format!("{e}"),
            vnl_code::CONVERSATION_STORAGE_ERROR,
        )).expect("serialize create error response"),
    }
}

/// `conversations/delete` : params -> `ConversationIdParams`, mêmes règles
/// de validation d'UUID que `conversations/get` (`VNL-RPC-000` si
/// malformé). `store::delete_conversation(&uuid)` est déjà IDEMPOTENT côté
/// store (ne retourne pas d'erreur si le fichier n'existe pas déjà, cf.
/// `cli/src/store.rs::delete_conversation` déjà lu) — NE PAS ajouter de
/// vérification d'existence supplémentaire ici, ni retourner
/// `VNL-RPC-005` pour un id absent : `delete` d'un id inconnu réussit
/// silencieusement, exactement comme la commande CLI. Seule une vraie
/// erreur io (permissions, disque...) -> `VNL-RPC-007`. Succès -> `result:
/// null` (même pattern que `shutdown`, tâche 01 : `JsonRpcResponse::success(id, Value::Null)`).
fn handle_conversations_delete(id: Value, params: serde_json::Value) -> String {
    let params: ConversationIdParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::PARSE_ERROR,
            "Malformed request: params could not be deserialized as ConversationIdParams",
            vnl_code::MALFORMED_REQUEST,
        )).expect("serialize delete error response"),
    };
    let uuid = match Uuid::parse_str(&params.id) {
        Ok(u) => u,
        Err(_) => return serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::PARSE_ERROR,
            format!("Invalid UUID in id: {}", params.id),
            vnl_code::MALFORMED_REQUEST,
        )).expect("serialize delete error response"),
    };
    match store::delete_conversation(&uuid) {
        Ok(()) => serde_json::to_string(&JsonRpcResponse::success(id, Value::Null)).expect("serialize delete response"),
        Err(e) => serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::SERVER_ERROR,
            format!("{e}"),
            vnl_code::CONVERSATION_STORAGE_ERROR,
        )).expect("serialize delete error response"),
    }
}

/// `chat/cancel` — M3, no-op en v1 (design : "accepté et no-op en v1,
/// documenté"). Valide seulement que `conversationId` est un UUID bien
/// formé (`VNL-RPC-000` sinon) ; n'exige PAS que la conversation existe ni
/// qu'un tour soit effectivement en cours — pur no-op accepté, cohérent
/// avec le fait que l'annulation réelle n'existe pas encore côté lib.
fn handle_chat_cancel(id: Value, params: serde_json::Value) -> String {
    let params: ChatCancelParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(_) => return serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::PARSE_ERROR,
            "Malformed request: params could not be deserialized as ChatCancelParams",
            vnl_code::MALFORMED_REQUEST,
        )).expect("serialize cancel error response"),
    };
    if Uuid::parse_str(&params.conversation_id).is_err() {
        return serde_json::to_string(&JsonRpcResponse::error(
            id,
            jsonrpc_code::PARSE_ERROR,
            format!("Invalid UUID in conversationId: {}", params.conversation_id),
            vnl_code::MALFORMED_REQUEST,
        )).expect("serialize cancel error response");
    }
    serde_json::to_string(&JsonRpcResponse::success(id, Value::Null)).expect("serialize cancel response")
}

/// Adapteur qui traduit un `ChatEvent` émis par le tour en notification
/// `chat/event` écrite sur `tx`. Utilisé par `run_agent_turn` via
/// `SessionContext.sink` (tâche 03b).
#[allow(dead_code)] // construit par la tâche 03b (chat/send) ; déjà testé directement dans cette tâche
pub struct RpcEventSink {
    pub conversation_id: Uuid,
    pub seq: Arc<Mutex<HashMap<Uuid, u64>>>,
    pub tx: mpsc::UnboundedSender<String>,
}

#[async_trait::async_trait]
impl EventSink for RpcEventSink {
    /// Incrémente le compteur `seq` DE CETTE conversation (verrou tenu le
    /// temps du lire-incrémenter-écrire uniquement, jamais à travers un
    /// `.await`), construit la notification, l'envoie sur `tx`. Best-effort :
    /// une erreur de `send` (le lecteur stdin a fermé le canal) est
    /// silencieusement ignorée.
    async fn emit(&self, event: ChatEvent) {
        let seq = {
            let mut map = self.seq.lock().unwrap();
            let counter = map.entry(self.conversation_id).or_insert(0);
            let current = *counter;
            *counter += 1;
            current
        };
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "chat/event",
            params: ChatEventNotificationParams {
                conversation_id: self.conversation_id.to_string(),
                seq,
                event,
            },
        };
        if let Ok(line) = serde_json::to_string(&notif) {
            let _ = self.tx.send(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;
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

    // -- Existing tests from task 01 --

    #[test]
    async fn initialize_success() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
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
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
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
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
        let line = make_request_json(3, "initialize", Some(json!({})));
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-000");
    }

    #[test]
    async fn method_before_initialize_rejected() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
        let line = make_request_json(4, "shutdown", None);
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-001");
    }

    #[test]
    async fn unknown_method_after_initialize() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
        // First initialize
        let init_line = make_request_json(5, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // Then try unknown method
        let unknown_line = make_request_json(6, "chat/history", None);
        let result = handle_line(&mut state, &unknown_line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-004");
    }

    #[test]
    async fn shutdown_after_initialize() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
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
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
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
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
        let init_line = make_request_json("abc", "initialize", Some(json!({"protocolVersion": 1})));
        let result = handle_line(&mut state, &init_line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        assert_eq!(resp.id, Value::String("abc".into()));

        // Test with string id for error
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state2 = ServerState::new(tx);
        let error_line = make_request_json("xyz", "chat/history", None);
        let result2 = handle_line(&mut state2, &error_line).await;
        assert!(result2.is_some());
        let resp2: JsonRpcResponse = serde_json::from_str(&result2.unwrap()).expect("parse response");
        assert!(resp2.error.is_some());
        assert_eq!(resp2.id, Value::String("xyz".into()));
    }

    // -- New tests for task 02a --

    /// initialize_resolves_workspace_root — `workspace` pointant vers un
    /// tempdir contenant `.vanyline/` (créer
    /// `<tempdir>/.vanyline/agents/build.md` avec un frontmatter minimal, cf.
    /// `cli/src/chat.rs::tests::workspace_agent_reported` pour le format) ->
    /// `result.workspaceRoot` non-`None` et contient le chemin du tempdir.
    #[test]
    async fn initialize_resolves_workspace_root() {
        let tmp = tempdir().unwrap();
        let vanyline = tmp.path().join(".vanyline");
        let agents_dir = vanyline.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("build.md"),
            "---\nmodel: test-model\n---\nbuild agent\n",
        )
        .unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let tmp_path = tmp.path().to_str().unwrap();
        let line = make_request_json(10, "initialize", Some(json!({
            "protocolVersion": 1,
            "workspace": tmp_path,
        })));
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_none(), "initialize should succeed");
        let r = resp.result.as_ref().expect("result should be Some");
        assert!(
            r["workspaceRoot"].is_string(),
            "workspaceRoot should be a string, got: {:?}", r["workspaceRoot"]
        );
        let ws_root = r["workspaceRoot"].as_str().unwrap();
        assert!(
            ws_root.contains(tmp.path().to_str().unwrap()),
            "workspaceRoot should contain tempdir path, got: {}", ws_root
        );
    }

    /// initialize_no_workspace_marker_yields_none_root — `workspace` pointant
    /// vers un tempdir SANS `.vanyline/` ni `.git/` -> `result.workspaceRoot ==
    /// null`.
    #[test]
    async fn initialize_no_workspace_marker_yields_none() {
        // Use a temporary directory that won't be discovered as a workspace.
        // Create a unique dir that doesn't contain .vanyline/ or .git/
        // anywhere inside or in parent dirs down to the test's repo root.
        let tmp = tempdir().unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let tmp_path = tmp.path().to_str().unwrap();
        let line = make_request_json(11, "initialize", Some(json!({
            "protocolVersion": 1,
            "workspace": tmp_path,
        })));
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        
        assert!(resp.error.is_none(), "initialize should succeed");
        // `workspace_root: Option<String>` has `skip_serializing_if =
        // Option::is_none` (task 01, consistent with the design's `?`
        // optional-field notation) — when no marker is found, the field is
        // ABSENT from the JSON entirely, not present-as-null.
        let r = resp.result.as_ref().expect("result should be Some");
        assert!(
            r.get("workspaceRoot").is_none(),
            "workspaceRoot should be absent when no .vanyline/.git marker is found, got: {:?}",
            r.get("workspaceRoot")
        );
    }

    /// config_agents_before_and_after_initialize — sans `initialize` d'abord,
    /// `config/agents` -> `VNL-RPC-001` (non initialisé). Avec `initialize`,
    /// retourne un tableau valide.
    #[test]
    async fn config_agents_before_and_after_initialize() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);

        // Before initialize: should get NOT_INITIALIZATED (VNL-RPC-001)
        let line = make_request_json(12, "config/agents", None);
        let result = handle_line(&mut state, &line).await;
        assert!(result.is_some());
        let resp: JsonRpcResponse = serde_json::from_str(&result.unwrap()).expect("parse response");
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-001");
        assert!(!state.initialized);
    }

    /// config_agents_lists_workspace_agent — `initialize` avec `workspace` =
    /// tempdir contenant `.vanyline/agents/build.md` -> `config/agents` ->
    /// `result` est un tableau JSON contenant un objet avec `"name":"build"`.
    #[test]
    async fn config_agents_lists_workspace_agent() {
        let tmp = tempdir().unwrap();
        let vanyline = tmp.path().join(".vanyline");
        let agents_dir = vanyline.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("build.md"),
            "---\nmodel: test-model\n---\nbuild agent system prompt\n",
        )
        .unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let tmp_path = tmp.path().to_str().unwrap();

        // Initialize with workspace
        let init_line = make_request_json(20, "initialize", Some(json!({
            "protocolVersion": 1,
            "workspace": tmp_path,
        })));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // List agents
        let agents_line = make_request_json(21, "config/agents", None);
        let result = handle_line(&mut state, &agents_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_none(), "config/agents should succeed, got error: {:?}", resp.error);
        let agents = &resp.result.as_ref().expect("result should be Some");
        assert!(agents.is_array(), "result should be a JSON array");
        let arr = agents.as_array().unwrap();
        assert!(!arr.is_empty(), "agents list should contain at least the build agent");
        assert_eq!(arr[0]["name"], "build");
    }

    /// config_models_empty_list — `initialize` avec un tempdir vide ->
    /// `config/models` -> `result == []` (pas d'erreur).
    #[test]
    async fn config_models_empty_list() {
        // Create an empty tempdir with no .vanyline/ config
        let tmp = tempdir().unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let tmp_path = tmp.path().to_str().unwrap();

        // Initialize with empty tempdir
        let init_line = make_request_json(30, "initialize", Some(json!({
            "protocolVersion": 1,
            "workspace": tmp_path,
        })));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // List models — should return empty array, not an error
        let models_line = make_request_json(31, "config/models", None);
        let result = handle_line(&mut state, &models_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_none(), "config/models should succeed with empty list, got error: {:?}", resp.error);
        let models = &resp.result.as_ref().expect("result should be Some");
        assert!(models.is_array());
        assert_eq!(models.as_array().unwrap().len(), 0);
    }

    /// config_toolsets_and_skills_dispatch — vérifie que `config/toolsets` et
    /// `config/skills` sont bien dispatchées (pas `VNL-RPC-004` méthode inconnue).
    #[test]
    async fn config_toolsets_and_skills_dispatch() {
        let tmp = tempdir().unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let tmp_path = tmp.path().to_str().unwrap();

        // Initialize
        let init_line = make_request_json(40, "initialize", Some(json!({
            "protocolVersion": 1,
            "workspace": tmp_path,
        })));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // config/toolsets — should NOT be VNL-RPC-004 (method not found)
        let ts_line = make_request_json(41, "config/toolsets", None);
        let result = handle_line(&mut state, &ts_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");
        assert!(
            resp.error.is_none() || resp.error.as_ref().map(|e| e.data.code.as_str()) != Some("VNL-RPC-004"),
            "config/toolsets should be a known method, got error code: {:?}",
            resp.error.as_ref().map(|e| e.data.code.as_str())
        );
        // Result should be a JSON array
        assert!(resp.result.as_ref().map(|v| v.is_array()).unwrap_or_default(), "result should be an array");

        // config/skills — same pattern
        let skills_line = make_request_json(42, "config/skills", None);
        let result = handle_line(&mut state, &skills_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");
        assert!(
            resp.error.is_none() || resp.error.as_ref().map(|e| e.data.code.as_str()) != Some("VNL-RPC-004"),
            "config/skills should be a known method, got error code: {:?}",
            resp.error.as_ref().map(|e| e.data.code.as_str())
        );
        assert!(resp.result.as_ref().map(|v| v.is_array()).unwrap_or_default(), "result should be an array");
    }

    /// Isolation lock and helper for tests that touch `crate::store` (reads
    /// `XDG_DATA_HOME` via `dirs` crate without injection parameter).
    static DATA_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Pointe `XDG_DATA_HOME` vers un tempdir frais et retourne (tempdir,
    /// guard) — les DEUX doivent être gardés en vie (bindés, pas `let _ =`)
    /// jusqu'à la fin du test : dropper le tempdir supprime le répertoire,
    /// dropper le guard laisse un autre test muter la variable pendant que
    /// celui-ci tourne encore.
    fn isolated_data_dir() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let guard = DATA_DIR_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_DATA_HOME", tmp.path());
        (tmp, guard)
    }

    #[test]
    async fn conversations_list_empty() {
        let (_tmp, _guard) = isolated_data_dir();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let init_line = make_request_json(50, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        let line = make_request_json(51, "conversations/list", None);
        let result = handle_line(&mut state, &line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_none(), "list should succeed with empty result, got: {:?}", resp.error);
        let items = &resp.result.as_ref().expect("result should be Some");
        assert!(items.is_array());
        assert_eq!(items.as_array().unwrap().len(), 0);
    }

    #[test]
    async fn conversations_create_then_list() {
        let (_tmp, _guard) = isolated_data_dir();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let init_line = make_request_json(55, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // Create
        let create_line = make_request_json(56, "conversations/create", Some(json!({"agent":"build","title":"Test"})));
        let result = handle_line(&mut state, &create_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_none(), "create should succeed, got: {:?}", resp.error);
        let created = &resp.result.as_ref().expect("result should be Some");
        assert!(created["id"].is_string());
        let created_id = created["id"].as_str().unwrap();
        assert!(!created_id.is_empty());
        assert_eq!(created["agent"], "build");
        assert_eq!(created["title"], "Test");
        assert_eq!(created["messageCount"], 0);

        // List
        let line = make_request_json(57, "conversations/list", None);
        let result = handle_line(&mut state, &line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_none(), "list should succeed, got: {:?}", resp.error);
        let items = &resp.result.as_ref().expect("result should be Some");
        assert!(items.is_array());
        assert_eq!(items.as_array().unwrap().len(), 1);
        assert_eq!(items.as_array().unwrap()[0]["id"], created_id);
    }

    #[test]
    async fn conversations_get_found() {
        let (_tmp, _guard) = isolated_data_dir();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let init_line = make_request_json(60, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // Create
        let create_line = make_request_json(61, "conversations/create", Some(json!({"agent":"build","title":"Test"})));
        let result = handle_line(&mut state, &create_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");
        let created_id = resp.result.as_ref().unwrap()["id"].as_str().unwrap();

        // Get by id
        let get_line = make_request_json(62, "conversations/get", Some(json!({"id": created_id})));
        let result = handle_line(&mut state, &get_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_none(), "get should succeed, got: {:?}", resp.error);
        let got = &resp.result.as_ref().expect("result should be Some");
        assert_eq!(got["id"], created_id);
        assert!(got["messages"].is_array());
        assert_eq!(got["messages"].as_array().unwrap().len(), 0);
    }

    #[test]
    async fn conversations_get_not_found() {
        let (_tmp, _guard) = isolated_data_dir();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let init_line = make_request_json(65, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // Get with a valid UUID that was never created
        let fake_uuid = "aaaaaaaa-0000-0000-0000-000000000001";
        let get_line = make_request_json(66, "conversations/get", Some(json!({"id": fake_uuid})));
        let result = handle_line(&mut state, &get_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-005");
    }

    #[test]
    async fn conversations_get_malformed_id() {
        let (_tmp, _guard) = isolated_data_dir();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let init_line = make_request_json(68, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // Get with non-UUID string
        let get_line = make_request_json(69, "conversations/get", Some(json!({"id":"not-a-uuid"})));
        let result = handle_line(&mut state, &get_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-000");
    }

    #[test]
    async fn conversations_delete_then_list_empty() {
        let (_tmp, _guard) = isolated_data_dir();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let init_line = make_request_json(70, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // Create
        let create_line = make_request_json(71, "conversations/create", Some(json!({"agent":"build","title":"Test"})));
        let result = handle_line(&mut state, &create_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");
        let created_id = resp.result.as_ref().unwrap()["id"].as_str().unwrap();

        // Delete
        let delete_line = make_request_json(72, "conversations/delete", Some(json!({"id": created_id})));
        let result = handle_line(&mut state, &delete_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_none(), "delete should succeed, got: {:?}", resp.error);
        assert!(result.contains("\"result\":null"));

        // List should be empty
        let line = make_request_json(73, "conversations/list", None);
        let result = handle_line(&mut state, &line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_none(), "list should succeed, got: {:?}", resp.error);
        let items = &resp.result.as_ref().expect("result should be Some");
        assert_eq!(items.as_array().unwrap().len(), 0);
    }

    #[test]
    async fn conversations_delete_unknown_id_succeeds() {
        let (_tmp, _guard) = isolated_data_dir();

        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = ServerState::new(tx);
        let init_line = make_request_json(75, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        // Delete with a valid UUID that was never created
        let fake_uuid = "bbbbbbbb-0000-0000-0000-000000000001";
        let delete_line = make_request_json(76, "conversations/delete", Some(json!({"id": fake_uuid})));
        let result = handle_line(&mut state, &delete_line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        // Should succeed silently (idempotent, like CLI)
        assert!(resp.error.is_none(), "delete of unknown id should succeed, got: {:?}", resp.error);
        assert!(result.contains("\"result\":null"));
    }

    #[test]
    async fn conversations_methods_require_initialize() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);

        // Any of the 4 methods should return NOT_INITIALIZED before initialize
        let line = make_request_json(80, "conversations/list", None);
        let result = handle_line(&mut state, &line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-001");
        assert!(!state.initialized);
    }

    // -- New tests for task 03a --

    #[test]
    async fn chat_cancel_valid_uuid_returns_null() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
        let init_line = make_request_json(90, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        let fake_uuid = uuid::Uuid::new_v4().to_string();
        let line = make_request_json(91, "chat/cancel", Some(json!({"conversationId": fake_uuid})));
        let result = handle_line(&mut state, &line).await.unwrap();
        // Wire format check on the raw string first — `Option<Value>`'s
        // Deserialize collapses a JSON `null` into `None` (cf.
        // shutdown_after_initialize), so a round-tripped struct can't
        // distinguish "result: null" from "no result field" — the raw
        // string is the only way to assert the wire behavior directly.
        assert!(result.contains("\"result\":null"));

        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");
        assert!(resp.error.is_none(), "chat/cancel should succeed, got: {:?}", resp.error);
        assert_eq!(resp.result, None);
    }

    #[test]
    async fn chat_cancel_malformed_uuid() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
        let init_line = make_request_json(92, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        let line = make_request_json(93, "chat/cancel", Some(json!({"conversationId": "not-a-uuid"})));
        let result = handle_line(&mut state, &line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-000");
    }

    #[test]
    async fn chat_cancel_requires_initialize() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);

        let fake_uuid = uuid::Uuid::new_v4().to_string();
        let line = make_request_json(94, "chat/cancel", Some(json!({"conversationId": fake_uuid})));
        let result = handle_line(&mut state, &line).await.unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&result).expect("parse response");

        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().data.code, "VNL-RPC-001");
        assert!(!state.initialized);
    }

    #[test]
    async fn reinitialize_clears_busy_and_seq() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = ServerState::new(tx);
        let init_line = make_request_json(95, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &init_line).await;
        assert!(state.initialized);

        state.busy.lock().unwrap().insert(uuid::Uuid::new_v4());
        state.seq.lock().unwrap().insert(uuid::Uuid::new_v4(), 5);
        assert!(!state.busy.lock().unwrap().is_empty());
        assert!(!state.seq.lock().unwrap().is_empty());

        let reinit_line = make_request_json(96, "initialize", Some(json!({"protocolVersion": 1})));
        handle_line(&mut state, &reinit_line).await;

        assert!(state.busy.lock().unwrap().is_empty());
        assert!(state.seq.lock().unwrap().is_empty());
    }

    #[test]
    async fn rpc_event_sink_emits_notification_with_incrementing_seq() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let seq = Arc::new(Mutex::new(HashMap::new()));
        let conversation_id = uuid::Uuid::new_v4();
        let sink = RpcEventSink { conversation_id, seq, tx };

        sink.emit(ChatEvent::Token { content: "a".into() }).await;
        sink.emit(ChatEvent::Token { content: "b".into() }).await;

        let line1 = rx.recv().await.expect("first notification");
        let line2 = rx.recv().await.expect("second notification");

        let v1: serde_json::Value = serde_json::from_str(&line1).expect("parse notification 1");
        let v2: serde_json::Value = serde_json::from_str(&line2).expect("parse notification 2");

        assert_eq!(v1["method"], "chat/event");
        assert!(v1.get("id").is_none(), "notification must not have an id field");
        assert_eq!(v1["params"]["conversationId"], conversation_id.to_string());
        assert_eq!(v1["params"]["seq"], 0);
        assert_eq!(v1["params"]["event"]["type"], "token");
        assert_eq!(v1["params"]["event"]["content"], "a");

        assert_eq!(v2["params"]["seq"], 1);
        assert_eq!(v2["params"]["event"]["content"], "b");
    }
}