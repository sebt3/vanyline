#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod auth;
pub mod config;
pub mod git;
pub mod lsp;
pub mod lsp_client;
pub mod maint;
pub mod mcp;
pub mod telemetry;
pub mod tools_impl;
pub mod ws;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::{Extension, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use tokio::sync::{broadcast, oneshot};
use tower_http::trace::TraceLayer;

use auth::AuthState;
use config::Config;
pub use lsp::LspManager;
use ws::ticket::TicketStore;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub auth: Arc<AuthState>,
    pub tickets: TicketStore,
    pub lsp: Arc<LspManager>,
    /// Canal de push des sessions `/ws/fs` (tâche 08b) : frames JSON toutes
    /// prêtes (ex: `{"event":"file-changed","path":…}`) diffusées à chaque
    /// client WS abonné. L'émetteur d'événements est `edit_and_check` (08d) ;
    /// ici seul le canal existe.
    pub fs_events: tokio::sync::broadcast::Sender<String>,
    /// Requêtes « flush cet URI » aller-retour (tâche 08c, arbitrage R1 sq3,
    /// cas B de `edit_and_check`) : broadcast `flush-request` sur le canal
    /// ci-dessus, puis attente bornée de l'`flush-ack` qui revient par la WS.
    /// Consommatrice : `edit_and_check` (08d), via
    /// [`FsFlushRequests::request_flush`].
    pub fs_flush: Arc<FsFlushRequests>,
}

/// Capacité du canal `fs_events` (frames de push /ws/fs, tâche 08b).
pub const FS_EVENTS_CAPACITY: usize = 64;

/// Crée le canal `fs_events` d'un `AppState` : le `Sender` seul, capacité
/// [`FS_EVENTS_CAPACITY`]. Helper unique pour que la capacité ne se
/// retrouve pas dupliquée dans les vingt constructeurs d'`AppState`
/// (main + tests http + intégrations). Les sessions `/ws/fs` s'abonnent
/// via `subscribe` après leur upgrade WS.
pub fn fs_events_channel() -> tokio::sync::broadcast::Sender<String> {
    tokio::sync::broadcast::channel::<String>(FS_EVENTS_CAPACITY).0
}

/// Timeout de référence de l'aller-retour flush/ack (tâche 08c) : la WS est
/// locale (ingress sandbox aller-retour en ms, canal 08b), 2 s est déjà une
/// marge énorme ; le repli sur la fenêtre de debounce en cas de timeout est
/// sain, inutile d'attendre plus longtemps avant d'écrire.
pub const FS_FLUSH_TIMEOUT_SECS: u64 = 2;

/// Atelier du duo push `/ws/fs` d'un `AppState` : le canal `fs_events` et le
/// [`FsFlushRequests`] branché dessus (tâche 08c). Les frames `flush-request`
/// voyagent sur le MÊME canal que celui que les sessions `/ws/fs` relayent —
/// d'où ce helper unique plutôt que deux constructions séparées : les deux
/// champs doivent partager le canal, et le motif reste unique dans les vingt
/// constructeurs d'`AppState` (même esprit que [`fs_events_channel`]).
pub fn fs_push_channels() -> (broadcast::Sender<String>, Arc<FsFlushRequests>) {
    let tx = fs_events_channel();
    (tx.clone(), Arc::new(FsFlushRequests::new(tx)))
}

/// Requêtes « flush cet URI » émises vers le(s) frontend(s) (design R1 sq3,
/// cas B de edit_and_check — tâche 08d consommatrice).
pub struct FsFlushRequests {
    next_id: AtomicU64,
    tx: broadcast::Sender<String>, // clone de fs_events
    waiters: Mutex<HashMap<u64, oneshot::Sender<()>>>,
}

impl FsFlushRequests {
    pub fn new(tx: broadcast::Sender<String>) -> Self {
        Self {
            // Ids à partir de 1 : l'id 0 est falsy en JS — un abonné un peu
            // lâche (`if (event.id)`) le jetterait ; le client symétrique
            // (`SandboxFsClient.nextId`) démarre aussi à 1.
            next_id: AtomicU64::new(1),
            tx,
            waiters: Mutex::new(HashMap::new()),
        }
    }

    /// Garde d'accès aux waiters : une lock empoisonnée (panic survenu dans
    /// une section qui ne fait qu'insérer/retirer des entrées) garde une map
    /// intacte — on récupère l'intérieur plutôt que de propager (le crate
    /// interdit `unwrap` hors tests).
    fn waiters(&self) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<()>>> {
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Broadcast `{"event":"flush-request","id":N,"path":<relatif>}` puis
    /// attend l'ack, borné à `timeout`. `true` = ack reçu (le frontend a flush
    /// et acquitté APRÈS son write — ordre FIFO de la queue client). `false` =
    /// timeout (frontend absent/déconnecté/lent) : l'appelant (08d) retombe
    /// sur la fenêtre de debounce et le mentionne dans le rapport.
    /// Le waiter est retiré de la map à l'acquit ET au timeout (pas de fuite).
    pub async fn request_flush(&self, path: &str, timeout: Duration) -> bool {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = serde_json::json!({
            "event": "flush-request",
            "id": id,
            "path": path,
        })
        .to_string();

        let (ack_tx, ack_rx) = oneshot::channel::<()>();
        // Le waiter est inséré AVANT le broadcast (course classique : un
        // client rapide peut acquitter avant que ce code atteigne l'await
        // ci-dessous — si le waiter n'est pas déjà en place, `handle_ack` ne
        // trouve personne et le flush attendrait un ack déjà consommé). La
        // lock est rendue immédiatement : jamais tenue à travers l'await.
        self.waiters().insert(id, ack_tx);
        // Zéro abonné (aucune session `/ws/fs` ouverte) : la frame ne va
        // nulle part et aucun ack ne viendra jamais — cas traité par le
        // chemin timeout ci-dessous, rien de spécial à en faire.
        let _ = self.tx.send(frame);

        match tokio::time::timeout(timeout, ack_rx).await {
            // Ack reçu : `handle_ack` a déjà retiré le waiter de la map.
            Ok(Ok(())) => true,
            // Timeout, ou sender déposé sans valeur (cas défensif — le
            // retireur est `handle_ack`) : nettoyage ici, pas de fuite.
            Ok(Err(_)) | Err(_) => {
                self.waiters().remove(&id);
                false
            }
        }
    }

    /// Appelé par la branche `flush-ack` : résout le waiter (premier ack
    /// gagne ; id inconnu ou déjà résolu → rien).
    pub fn handle_ack(&self, id: u64) {
        // Multi-onglets : le broadcast touche toutes les `fs_session`,
        // chacune acquitte ; le `remove` rend le premier ack gagnant, les
        // suivants trouvent une map sans l'id → no-op.
        if let Some(ack_tx) = self.waiters().remove(&id) {
            // Le récepteur peut être parti (timeout passé) : échec de send
            // silencieux, le repli est déjà pris par l'appelant.
            let _ = ack_tx.send(());
        }
    }

    #[cfg(test)]
    pub(crate) fn waiter_count(&self) -> usize {
        self.waiters().len()
    }
}

/// Build the main MCP application router (MCP + public routes).
///
/// The metrics server runs on a separate port — see [`spawn_metrics_server`].
pub fn build_app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/mcp", post(mcp::handle))
        .route("/git/status", get(git::handle_status))
        .route("/git/unpushed", get(git::handle_unpushed))
        .route("/git/diff", get(git::handle_diff))
        .route("/git/stage", post(git::handle_stage))
        .route("/git/unstage", post(git::handle_unstage))
        .route("/git/commit", post(git::handle_commit))
        .route("/git/branches", get(git::handle_branches))
        .route("/git/branches", post(git::handle_create_branch))
        .route("/git/checkout", post(git::handle_checkout))
        .route("/git/branches/{name}", delete(git::handle_delete_branch))
        .route("/git/push", post(git::handle_push))
        .route("/git/log", get(git::handle_log))
        .route("/git/ssh-key", get(git::handle_ssh_key_status))
        .route("/git/ssh-key", post(git::handle_ssh_key_create))
        .route("/git/merge", post(git::handle_merge))
        .route("/git/merge/abort", post(git::handle_merge_abort))
        .route("/ws/ticket", post(handle_ws_ticket))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let public = Router::new()
        .route("/health", get(health))
        .route(
            "/.well-known/oauth-protected-resource",
            get(mcp::oauth_metadata),
        )
        .route(
            "/ws/fs",
            get(crate::ws::fs::handle_ws_fs).layer(middleware::from_fn_with_state(
                state.clone(),
                crate::ws::ticket::ws_auth_middleware,
            )),
        )
        .route(
            "/ws/terminal",
            get(crate::ws::terminal::handle_ws_terminal).layer(middleware::from_fn_with_state(
                state.clone(),
                crate::ws::ticket::ws_auth_middleware,
            )),
        )
        .route(
            "/ws/lsp/{toolchain}",
            get(crate::ws::lsp::handle_ws_lsp).layer(middleware::from_fn_with_state(
                state.clone(),
                crate::ws::ticket::ws_auth_middleware,
            )),
        );

    Router::new()
        .merge(protected)
        .merge(public)
        .layer(middleware::from_fn(track_metrics))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Build the internal metrics router (`GET /metrics`).
///
/// Bind on `config.metrics_listen` (default `0.0.0.0:9090`). Never expose this
/// port externally — protect it at the network/infra layer (NetworkPolicy, etc.).
pub fn build_metrics_app() -> Router {
    Router::new().route("/metrics", get(metrics_handler))
}

/// Handler for `POST /ws/ticket` — issues a short-lived, single-use ticket
/// for WebSocket upgrade authentication.
async fn handle_ws_ticket(
    State(state): State<AppState>,
    Extension(auth): Extension<auth::AuthInfo>,
) -> impl IntoResponse {
    use ws::ticket::{TICKET_TTL_SECS, TicketClaims};

    let store = &state.tickets;
    let ticket = store.issue(TicketClaims {
        subject: auth.subject,
        access: auth.access,
    });
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ticket": ticket,
            "expires_in_secs": TICKET_TTL_SECS,
        })),
    )
}

/// Spawn the metrics HTTP server as a background task.
///
/// A bind failure is logged as a warning but does **not** abort startup —
/// the main MCP server continues without an exposed `/metrics` endpoint.
pub async fn spawn_metrics_server(addr: std::net::SocketAddr) {
    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            tracing::info!("metrics server listening on {addr}");
            tokio::spawn(async move {
                if let Err(e) = axum::serve(listener, build_metrics_app()).await {
                    tracing::warn!("metrics server error: {e}");
                }
            });
        }
        Err(e) => {
            tracing::warn!("could not bind metrics server on {addr}: {e} — /metrics unavailable");
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

async fn metrics_handler() -> impl IntoResponse {
    match telemetry::render_metrics() {
        Some(body) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

// ── Metrics middleware ────────────────────────────────────────────────────────

async fn track_metrics(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    // Use the path as-is; routes are static in this template so cardinality is low.
    // When you add :param routes in a fork, normalise to the route template instead.
    let path = req.uri().path().to_string();
    let start = Instant::now();

    let response = next.run(req).await;

    let status = response.status().as_u16().to_string();
    let duration = start.elapsed().as_secs_f64();

    metrics::counter!("http_requests_total",
        "method" => method.clone(),
        "path"   => path.clone(),
        "status" => status
    )
    .increment(1);

    metrics::histogram!("http_request_duration_seconds",
        "method" => method,
        "path"   => path
    )
    .record(duration);

    response
}
