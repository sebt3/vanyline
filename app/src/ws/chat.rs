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
fn try_acquire_busy(busy: &Mutex<HashSet<i32>>, conversation_id: i32) -> bool {
    let mut guard = busy
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    fn drop(&mut self) {
        self.busy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.conversation_id);
    }
}

/// Garde d'appartenance de `run_socket` (`chat.sdd`, DÉCIDÉ 2026-10-18, oracle d'existence
/// fermé) : rend l'id de l'appelant. La conversation d'un autre owner rend EXACTEMENT l'erreur
/// d'une conversation inexistante (`ConversationNotFound`, `VNL-CNV-001`), jamais
/// `ConversationAccessDenied` : un utilisateur ne peut pas sonder l'existence d'un id d'autrui.
async fn verifier_proprietaire(
    state: &AppState,
    user: &AuthUser,
    conversation_id: i32,
) -> Result<(i32, crate::db::entities::conversations::Model), AppError> {
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
        return Err(AppError::ConversationNotFound);
    }
    Ok((user_id, conv))
}

async fn run_socket(
    socket: WebSocket,
    state: AppState,
    user: AuthUser,
    conversation_id: i32,
) -> Result<(), AppError> {
    let (user_id, conv) = verifier_proprietaire(&state, &user, conversation_id).await?;

    // `AuthUser` n'est pas `Clone` (miryad-core) : un `Arc` par socket, cloné vers
    // chaque tâche de tour.
    let user = Arc::new(user);
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

        let Some(agent_id) = conv.agent_id else {
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
        let spawn_user = Arc::clone(&user);
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
                &spawn_user,
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

/// Résout `extra_mcp` à partir du contexte de la conversation (ws/chat.sdd `Must`,
/// 2026-10-04 : « outils des agents système »). Pour un contexte `settings`, `home`,
/// `project` ou `sandbox` — quel que soit l'agent, système ou ordinaire — la résolution
/// passe par `system_agents::context_tools` : serveurs MCP de l'app elle-même avec un
/// jeton d'API jetable de l'utilisateur, + serveur de la sandbox du contexte pour
/// `sandbox` (scoping par propriétaire conservé, garde IDOR incluse). Le `ContextTools`
/// rendu porte la capacité de révoquer le jeton en fin de tour ; l'appelant DOIT appeler
/// `revoke`, succès du tour comme échec. Les autres kinds de contexte (ex. `chat`)
/// restent sans serveurs forcés, comme avant 2026-10-04.
///
/// L'amorçage idempotent des quatre lignes d'agents système (`ensure_system_agents`) est
/// appelé ici pour les contextes système : `main.sdd` le contracte « à la première
/// connexion d'un utilisateur », mais miryad-core ne prête aucun hook de login — le tour
/// de chat est le point atteignable le plus proche (QUESTION remontée à la spec-dd).
async fn resolve_extra_mcp(
    state: &AppState,
    context_id: i32,
    user: &AuthUser,
    user_id: i32,
    agent_name: &str,
) -> Result<crate::system_agents::ContextTools, AppError> {
    let db = &state.auth.db;
    let context = crate::db::entities::chat_contexts::Entity::find_by_id(context_id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::InternalError("VNL-DB-007: context not found".into()))?;

    if crate::system_agents::system_agent_for(&context.kind).is_none() {
        return Ok(crate::system_agents::ContextTools::default());
    }

    crate::system_agents::ensure_system_agents(db, user_id).await?;
    crate::system_agents::context_tools(state, user, &context.kind, &context, agent_name).await
}

/// Persistance (R9) : le message user est enregistré AVANT l'appel à
/// `run_agent_turn` (il a bien été envoyé, qu'importe l'issue du tour) ;
/// le message assistant seulement APRÈS un tour réussi (le `?` sur
/// `run_agent_turn` empêche d'atteindre le `persist_message` final en cas
/// d'échec). C'est cette sémantique qui fait référence — le RPC
/// (`cli/src/rpc/handlers.rs`) est aligné dessus séparément.
#[allow(clippy::too_many_arguments)] // ws/chat.sdd handle_message : les 8 parametres sont le contrat du tour (2026-10-04)
async fn handle_message(
    tx: &mpsc::UnboundedSender<ChatEvent>,
    state: &AppState,
    conversation_id: i32,
    context_id: i32,
    agent_id: i32,
    user_id: i32,
    user: &AuthUser,
    user_msg: &str,
) -> Result<(), AppError> {
    let db = &state.auth.db;
    // Filtre owner (ws/chat.sdd Handles, étape 1) : un agent d'un autre owner est
    // introuvable, comme un agent absent.
    let agent_name: String = crate::db::entities::agents::Entity::find_by_id(agent_id)
        .filter(crate::db::entities::agents::Column::OwnerId.eq(user_id))
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

    let mut tools = resolve_extra_mcp(state, context_id, user, user_id, &agent_name).await?;
    for (server, reason) in &tools.unavailable {
        send_tool_unavailable(tx, server, reason);
    }

    let sink = Arc::new(ChannelSink { tx: tx.clone() });
    let ctx = SessionContext {
        store: Arc::new(PgConfigStore::new(state.auth.db.clone(), user_id)),
        sink,
        local_tools: HashMap::new(),
        subagent_depth_max: 1,
        extra_mcp: tools.take_servers(),
        model_override: None,
        todo_state: Arc::new(std::sync::Mutex::new(todo_initial.clone())),
    };

    let turn =
        vanyline_lib::session::run_agent_turn(&ctx, &agent_name, history, user_msg, None).await;
    // Jeton jetable du tour : révoqué QUE LE TOUR RÉUSSISSE OU ÉCHEUE (system_agents.sdd
    // `Must`), avant toute propagation d'erreur ; l'échec de révocation est loggé, le
    // jeton meurt de toute façon à 1 h.
    if let Err(e) = tools.revoke().await {
        tracing::warn!("ws chat: token revocation failed: {e}");
    }
    let result = turn?;

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

    let current_todo = ctx
        .todo_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(todo) = todo_to_persist(current_todo, todo_initial.as_ref()) {
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
fn todo_to_persist(current: Option<String>, initial: Option<&String>) -> Option<String> {
    match current {
        Some(_) if current.as_ref() != initial => current,
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

    /// Scenario ws/chat.sdd `mutex_empoisonne_reutilise_la_garde` (contrat
    /// tooling.sdd decide 2026-10-14 : empoisonnement NON erreur sur busy —
    /// `.unwrap_or_else(PoisonError::into_inner)`, aucun allow de prod).
    #[test]
    fn mutex_empoisonne_reutilise_la_garde() {
        let busy = Arc::new(Mutex::new(HashSet::new()));
        // Poison volontaire : un thread panique en detenant la garde (le
        // seul chemin std qui empoisonne un Mutex).
        let poison = Arc::clone(&busy);
        let handle = std::thread::spawn(move || {
            let _guard = poison.lock().unwrap();
            panic!("poison volontaire (test)");
        });
        let _ = handle.join();
        assert!(busy.is_poisoned(), "precondition : garde empoisonnee");
        // try_acquire_busy insere sans paniquer sur la garde recuperee.
        assert!(try_acquire_busy(&busy, 7));
        assert!(!try_acquire_busy(&busy, 7));
        // Le drop du BusyGuard retire sans paniquer lui non plus.
        {
            let _guard = BusyGuard {
                busy: Arc::clone(&busy),
                conversation_id: 7,
            };
        }
        assert!(
            !busy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&7),
            "le retrait a opere sur la garde recuperee"
        );
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
            todo_to_persist(Some("x".to_string()), Some(&"x".to_string())),
            None
        );
    }

    #[test]
    fn todo_to_persist_changed_to_none_does_not_null_previous() {
        assert_eq!(todo_to_persist(None, Some(&"old".to_string())), None);
    }

    #[test]
    fn todo_to_persist_unchanged_none_is_none() {
        assert_eq!(todo_to_persist(None, None), None);
    }

    // ── Outils des agents système (ws/chat.sdd `Must`, 2026-10-04 ; system_agents.sdd) ──

    use crate::db::entities::{chat_contexts, model_profiles};
    use sea_orm::{ActiveValue::NotSet, ConnectionTrait, EntityTrait, Set, Statement};

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
            oidc_connect_timeout: std::time::Duration::from_secs(5),
            oidc_timeout: std::time::Duration::from_secs(15),
            cookie_secret: "0".repeat(64),
            database_url: "sqlite::memory:".to_string(),
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
            k8s_namespace: None,
            application_name: None,
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
            restate_admin_url: None,
            restate_ingress_url: None,
            workflow_listen: "0.0.0.0:9080".to_string(),
            workflow_deployment_url: None,
        }
    }

    fn test_state_with_db(db: sea_orm::DatabaseConnection) -> AppState {
        AppState {
            config: test_config(),
            cookie_key: cookie::Key::from(&[0u8; 64]),
            busy: Arc::new(Mutex::new(HashSet::new())),
            k8s: Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    fn test_user() -> AuthUser {
        AuthUser {
            subject: "alice".to_string(),
            email: None,
            id_token: "test-id-token".to_string(),
        }
    }

    async fn seed_context(state: &AppState, kind: &str, data: serde_json::Value) -> i32 {
        chat_contexts::Entity::insert(chat_contexts::ActiveModel {
            id: NotSet,
            kind: Set(kind.to_string()),
            data: Set(data),
            created_at: Set(chrono::Utc::now()),
        })
        .exec(&state.auth.db)
        .await
        .expect("context inserts")
        .last_insert_id
    }

    async fn seed_profile(state: &AppState, owner: i32) {
        let provider = crate::db::entities::llm_providers::ActiveModel {
            id: NotSet,
            name: Set("p".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://x".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(true),
        };
        let provider = crate::db::entities::llm_providers::Entity::insert(provider)
            .exec(&state.auth.db)
            .await
            .expect("provider inserts")
            .last_insert_id;
        model_profiles::Entity::insert(model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set("qwen".to_string()),
            provider_id: Set(provider),
            model: Set("qwen".to_string()),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(serde_json::json!({})),
        })
        .exec(&state.auth.db)
        .await
        .expect("profile inserts");
    }

    async fn token_count(state: &AppState) -> i64 {
        let res = state
            .auth
            .db
            .query_one_raw(Statement::from_sql_and_values(
                state.auth.db.get_database_backend(),
                "SELECT COUNT(*) FROM miryad_api_tokens",
                [],
            ))
            .await
            .expect("count succeeds")
            .expect("count row");
        res.try_get_by_index::<i64>(0)
            .or_else(|_| res.try_get_by_index::<i32>(0).map(i64::from))
            .expect("count decodes")
    }

    fn noop_tx() -> mpsc::UnboundedSender<ChatEvent> {
        let (tx, _rx) = mpsc::unbounded_channel();
        tx
    }

    /// IDOR (ws/chat.sdd `Tasks`, « BUG DE SÉCURITÉ CONFIRMÉ » ; Handles étape 1) : un
    /// `agent_id` appartenant à un autre owner est traité comme introuvable —
    /// `AppError::AgentNotFound`, AUCUN message persisté, aucun tour lancé.
    #[tokio::test]
    async fn handle_message_refuse_un_agent_dun_autre_owner() {
        use crate::db::entities::{agents, conversations, messages};
        use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};

        let state = test_state_with_db(crate::db::test_support::real_db().await);
        let db = state.auth.db.clone();
        let alice = miryad_core::users::resolve_user(&db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        let bob = miryad_core::users::resolve_user(&db, "bob", None)
            .await
            .expect("bob resolves")
            .id;
        let provider = crate::db::entities::llm_providers::Entity::insert(
            crate::db::entities::llm_providers::ActiveModel {
                id: NotSet,
                name: Set("prov-bob".to_string()),
                provider_type: Set("ollama".to_string()),
                endpoint: Set("http://127.0.0.1:9".to_string()),
                api_key: Set(None),
                available_models: Set(serde_json::json!([])),
                is_default: Set(false),
            },
        )
        .exec(&db)
        .await
        .expect("provider inserts")
        .last_insert_id;
        let profile_bob = model_profiles::Entity::insert(model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(bob),
            name: Set("qwen-bob".to_string()),
            provider_id: Set(provider),
            model: Set("qwen".to_string()),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(serde_json::json!({})),
        })
        .exec(&db)
        .await
        .expect("profile inserts")
        .last_insert_id;
        let agent_bob = agents::Entity::insert(agents::ActiveModel {
            id: NotSet,
            owner_id: Set(bob),
            name: Set("agent-de-bob".to_string()),
            description: Set(None),
            mode: Set("primary".to_string()),
            model_profile_id: Set(profile_bob),
            toolsets: Set(serde_json::json!([])),
            skills: Set(serde_json::json!("auto")),
            system_prompt: Set(String::new()),
            system_key: Set(None),
            system_version: Set(None),
        })
        .exec(&db)
        .await
        .expect("agent inserts")
        .last_insert_id;
        let context_id = seed_context(&state, "chat", serde_json::json!({})).await;
        let conv = conversations::Entity::insert(conversations::ActiveModel {
            id: NotSet,
            owner_id: Set(alice),
            agent_id: Set(None),
            context_id: Set(context_id),
            title: Set(None),
            todo: Set(None),
            created_at: Set(chrono::Utc::now()),
            updated_at: Set(chrono::Utc::now()),
        })
        .exec(&db)
        .await
        .expect("conversation inserts")
        .last_insert_id;

        let (tx, _rx) = mpsc::unbounded_channel();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            handle_message(
                &tx,
                &state,
                conv,
                context_id,
                agent_bob,
                alice,
                &test_user(),
                "bonjour",
            ),
        )
        .await;
        assert!(
            matches!(result, Ok(Err(AppError::AgentNotFound))),
            "agent d'un autre owner doit être AgentNotFound, obtenu : {result:?}"
        );
        let persisted = messages::Entity::find()
            .filter(messages::Column::ConversationId.eq(conv))
            .count(&db)
            .await
            .expect("count");
        assert_eq!(persisted, 0, "aucun message ne doit être persisté");
    }

    /// ws/chat.sdd `Must` : une conversation HORS contexte système (ex. `chat`) ne reçoit
    /// aucun serveur forcé et ne fait émettre AUCUN jeton.
    #[tokio::test]
    async fn hors_contexte_systeme_aucun_outil_aucun_jeton() {
        let state = test_state_with_db(crate::db::test_support::real_db().await);
        let user_id = miryad_core::users::resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        let context_id = seed_context(&state, "chat", serde_json::json!({})).await;

        let tools = resolve_extra_mcp(&state, context_id, &test_user(), user_id, "mon-agent")
            .await
            .expect("hors contexte: aucune panne");
        assert!(tools.servers.is_empty());
        assert_eq!(
            token_count(&state).await,
            0,
            "aucun jeton émis hors contexte"
        );
        let _ = noop_tx();
    }

    /// ws/chat.sdd `Must` : un contexte `settings` résout `extra_mcp` par `context_tools`
    /// (serveur `vanyline-config` + jeton émis) ET amorce les quatre agents système.
    #[tokio::test]
    async fn contexte_settings_resout_config_et_amorce_les_agents() {
        let state = test_state_with_db(crate::db::test_support::real_db().await);
        let user_id = miryad_core::users::resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        seed_profile(&state, user_id).await;
        let context_id = seed_context(&state, "settings", serde_json::json!({})).await;

        let tools = resolve_extra_mcp(&state, context_id, &test_user(), user_id, "config")
            .await
            .expect("settings résout");
        let names: Vec<&str> = tools.servers.iter().map(|(s, _)| s.name.as_str()).collect();
        assert_eq!(names, ["vanyline-config"]);
        assert_eq!(token_count(&state).await, 1, "un jeton par tour");

        let agents = crate::db::entities::agents::Entity::find()
            .all(&state.auth.db)
            .await
            .expect("find succeeds");
        assert_eq!(
            agents.len(),
            4,
            "l\'amorçage des agents système est branché ici"
        );
    }

    /// Pannes de résolution sandbox NON bloquantes : sans cluster joignable, le serveur
    /// `vanyline-domain` du contexte `sandbox` reste et le serveur sandbox est rendu
    /// indisponible (signalé en `ToolUnavailable` par l\'appelant).
    #[tokio::test]
    async fn sandbox_hors_cluster_non_bloquante() {
        let state = test_state_with_db(crate::db::test_support::real_db().await);
        let user_id = miryad_core::users::resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        seed_profile(&state, user_id).await;
        let context_id = seed_context(
            &state,
            "sandbox",
            serde_json::json!({"sandbox_name": "sb-1"}),
        )
        .await;

        let tools = resolve_extra_mcp(&state, context_id, &test_user(), user_id, "sandbox")
            .await
            .expect("la panne sandbox ne doit pas faire échouer le tour");
        let names: Vec<&str> = tools.servers.iter().map(|(s, _)| s.name.as_str()).collect();
        assert_eq!(names, ["vanyline-domain"]);
        assert!(
            tools.unavailable.iter().any(|(s, _)| s == "sandbox"),
            "got: {:?}",
            tools.unavailable
        );
    }

    /// Scenario ws/chat.sdd « contexte sandbox sans `sandbox_name` signale `ToolUnavailable`
    /// server sandbox » : un contexte `sandbox` dont `data` ne porte pas la clé
    /// `sandbox_name` (ou la porte non-chaîne) → résolution NON bloquante (`Ok`), signal
    /// `ToolUnavailable { server: "sandbox", reason exact }` (porté par
    /// `ContextTools::unavailable`, converti en `@ChatEvent::ToolUnavailable` par
    /// `handle_message`), et AUCUNE url MCP sandbox rendue. Sans cluster : la branche
    /// échoue avant tout appel kube. Écart tracé pour la spec-dd : le « `Ok(vec![])` »
    /// littéral du Scenario (rédigé pré-2026-10-04) contredit le §Must « Outils des
    /// agents système » de la même spec (table `sandbox` → domain + sandbox) ; le
    /// serveur `vanyline-domain` du contexte est donc rendu, seul le serveur `sandbox`
    /// est absent.
    #[tokio::test]
    async fn contexte_sandbox_sans_sandbox_name_signale_tool_unavailable_server_sandbox() {
        let state = test_state_with_db(crate::db::test_support::real_db().await);
        let user_id = miryad_core::users::resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        seed_profile(&state, user_id).await;

        for data in [
            serde_json::json!({}),
            serde_json::json!({"sandbox_name": 42}),
            serde_json::json!({"sandbox_name": null}),
        ] {
            let context_id = seed_context(&state, "sandbox", data).await;
            let tools = resolve_extra_mcp(&state, context_id, &test_user(), user_id, "sandbox")
                .await
                .expect("sandbox_name absent ou non-chaîne : panne non bloquante (Ok)");
            assert!(
                tools.unavailable.iter().any(|(s, r)| s == "sandbox"
                    && r == "contexte sandbox invalide (sandbox_name absent)"),
                "ToolUnavailable exact attendu, got: {:?}",
                tools.unavailable
            );
            let names: Vec<&str> = tools.servers.iter().map(|(s, _)| s.name.as_str()).collect();
            assert!(
                !names.contains(&"sandbox"),
                "aucune url MCP sandbox rendue, got: {names:?}"
            );
            assert_eq!(
                names,
                ["vanyline-domain"],
                "le contexte sandbox conserve les outils §Must du contexte (domain)"
            );
        }
    }

    /// ws/chat.sdd `Must` : le contexte inexistant reste la seule panne bloquante
    /// (comportement historique `VNL-DB-007`).
    #[tokio::test]
    async fn contexte_absent_reste_bloquant() {
        let state = test_state_with_db(crate::db::test_support::real_db().await);
        let user_id = miryad_core::users::resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        let err = resolve_extra_mcp(&state, 999, &test_user(), user_id, "x")
            .await
            .expect_err("contexte absent: bloquant");
        assert!(err.to_string().contains("VNL-DB-007"));
    }

    /// ws/chat.sdd §Must (S8) : le jeton jetable du tour est révoqué en fin de tour,
    /// que le tour RÉUSSISSE OU ÉCHOUE. Ici l'échec est naturel et post-émission : le
    /// fournisseur vise un endpoint surement injoignable (port 9 « discard », fermé →
    /// connection refused immédiate), le jeton est donc bien émis par `context_tools`
    /// avant que `run_agent_turn` n'échoue.
    #[tokio::test]
    async fn jeton_revoque_malgre_tour_en_erreur() {
        let state = test_state_with_db(crate::db::test_support::real_db().await);
        let user_id = miryad_core::users::resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        // Fournisseur injoignable garanti : le tour échoue APRÈS l'émission du jeton.
        let provider = crate::db::entities::llm_providers::ActiveModel {
            id: NotSet,
            name: Set("injoignable".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://127.0.0.1:9".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(true),
        };
        let provider = crate::db::entities::llm_providers::Entity::insert(provider)
            .exec(&state.auth.db)
            .await
            .expect("provider inserts")
            .last_insert_id;
        model_profiles::Entity::insert(model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(user_id),
            name: Set("qwen".to_string()),
            provider_id: Set(provider),
            model: Set("qwen".to_string()),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(serde_json::json!({})),
        })
        .exec(&state.auth.db)
        .await
        .expect("profile inserts");

        let rows = crate::system_agents::ensure_system_agents(&state.auth.db, user_id)
            .await
            .expect("system agents ensure");
        let agent_id = rows
            .iter()
            .find(|r| r.system_key.as_deref() == Some("config"))
            .expect("config agent")
            .id;
        let context_id = seed_context(&state, "settings", serde_json::json!({})).await;
        let conversation_id = crate::db::entities::conversations::Entity::insert(
            crate::db::entities::conversations::ActiveModel {
                id: NotSet,
                owner_id: Set(user_id),
                agent_id: Set(Some(agent_id)),
                context_id: Set(context_id),
                title: Set(None),
                todo: Set(None),
                created_at: Set(chrono::Utc::now()),
                updated_at: Set(chrono::Utc::now()),
            },
        )
        .exec(&state.auth.db)
        .await
        .expect("conversation inserts")
        .last_insert_id;

        assert_eq!(
            token_count(&state).await,
            0,
            "aucun jeton avant le tour (le compte viendra de context_tools)"
        );

        let err = handle_message(
            &noop_tx(),
            &state,
            conversation_id,
            context_id,
            agent_id,
            user_id,
            &test_user(),
            "salut",
        )
        .await
        .expect_err("le tour doit échouer sur l'endpoint injoignable");
        assert!(
            !err.to_string().contains("VNL-DB-007"),
            "l'échec attendu est celui du tour, pas du contexte: {err}"
        );

        assert_eq!(
            token_count(&state).await,
            0,
            "le jeton émis pour le tour doit être révoqué même quand le tour échoue"
        );
    }

    /// Oracle d'existence fermé (`chat.sdd`, DÉCIDÉ 2026-10-18) : pour alice, l'id d'une
    /// conversation de bob rend EXACTEMENT l'erreur de l'id inexistant (`VNL-CNV-001`) ; la
    /// conversation d'alice elle-même passe la garde.
    #[tokio::test]
    async fn garde_conversation_d_autrui_indiscernable_d_une_absente() {
        use crate::db::entities::conversations;
        use sea_orm::EntityTrait;

        let state = test_state_with_db(crate::db::test_support::real_db().await);
        let db = state.auth.db.clone();
        let alice = miryad_core::users::resolve_user(&db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        let bob = miryad_core::users::resolve_user(&db, "bob", None)
            .await
            .expect("bob resolves")
            .id;
        let context_id = seed_context(&state, "chat", serde_json::json!({})).await;
        let insert_conv = |owner: i32| {
            let db = db.clone();
            async move {
                conversations::Entity::insert(conversations::ActiveModel {
                    id: NotSet,
                    owner_id: Set(owner),
                    agent_id: Set(None),
                    context_id: Set(context_id),
                    title: Set(None),
                    todo: Set(None),
                    created_at: Set(chrono::Utc::now()),
                    updated_at: Set(chrono::Utc::now()),
                })
                .exec(&db)
                .await
                .expect("conversation inserts")
                .last_insert_id
            }
        };
        let bob_conv = insert_conv(bob).await;
        let alice_conv = insert_conv(alice).await;
        let absent = bob_conv + 1_000;

        let alice_user = test_user();
        let err_absent = verifier_proprietaire(&state, &alice_user, absent)
            .await
            .expect_err("id inexistant refusé");
        let err_autrui = verifier_proprietaire(&state, &alice_user, bob_conv)
            .await
            .expect_err("id d'autrui refusé");
        assert_eq!(
            err_autrui.to_string(),
            err_absent.to_string(),
            "un id d'autrui doit être indiscernable d'un id absent"
        );
        assert!(
            err_autrui.to_string().starts_with("VNL-CNV-001"),
            "code attendu VNL-CNV-001, reçu : {err_autrui}"
        );
        let (id_garde, conv_garde) = verifier_proprietaire(&state, &alice_user, alice_conv)
            .await
            .expect("la conversation d'alice passe la garde");
        assert_eq!(id_garde, alice);
        assert_eq!(conv_garde.id, alice_conv);
    }
}
