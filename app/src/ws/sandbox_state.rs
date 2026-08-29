use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::ws::WebSocket,
    extract::{State, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc;
use vanyline_crds::Sandbox;

use vanyline_lib::k8s::{VnlK8sClient, WatchEvent};

use miryad_core::auth::AuthUser;
use miryad_core::users::resolve_user;

use crate::{AppState, api::owners, error::AppError, k8s};

/// Event pushé sur le WebSocket quand le `status.phase` d'une sandbox change.
/// `phase` vaut `None` en suppression (ou avant la première phase connue).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxStateEvent {
    pub sandbox: String,
    pub phase: Option<String>,
}

/// Subscriber : sender (Arc pour comparaison par `ptr_eq`) + owner cible.
struct Subscriber {
    tx: Arc<mpsc::UnboundedSender<SandboxStateEvent>>,
    owner: String,
}

/// Cache `project-name` → owner. `None` mémorise un miss (projet introuvable —
/// sandbox orpheline) pour ne pas rappeler l'API K8s ni re-logger à chaque
/// événement de watch. Un namespace peut contenir plusieurs owners
/// (multi-tenant, cf. `sandboxes.rs`) : on ne peut pas dispatcher un événement
/// sans résoudre `spec.project` → `project.spec.owner`.
type OwnerCache = Arc<Mutex<std::collections::HashMap<String, Option<String>>>>;

/// State partagé entre toutes les connexions WS et la tâche `watch_loop`.
#[derive(Clone)]
pub struct SharedState {
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
    project_owner: OwnerCache,
    /// Handle de la tâche `watch_loop`. `Some` dès qu'un loop a été lancé (il
    /// tourne alors pour la vie du process). Sert de garde d'unicité.
    watch_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl SharedState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
            project_owner: Arc::new(Mutex::new(std::collections::HashMap::new())),
            watch_handle: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

/// Résout l'owner d'un Sandbox via le cache `project` → `owner`, en le peuplant
/// depuis l'API K8s en cas de miss (hit ou miss mémorisés). `None` si le projet
/// est introuvable (sandbox orpheline / owner supprimé) — l'événement est ignoré.
async fn resolve_owner(client: &VnlK8sClient, cache: &OwnerCache, project: &str) -> Option<String> {
    if let Some(cached) = cache.lock().get(project) {
        return cached.clone();
    }
    let resolved = match client.get_project(project).await {
        Ok(project_obj) => Some(project_obj.spec.owner),
        Err(e) => {
            tracing::warn!("sandbox-state: project '{project}' introuvable: {e}");
            None
        }
    };
    cache.lock().insert(project.to_string(), resolved.clone());
    resolved
}

/// Dispatche un event aux subscribers dont l'owner correspond.
fn dispatch(owner: &str, event: &SandboxStateEvent, subs: &[Subscriber]) {
    for sub in subs.iter().filter(|sub| sub.owner == owner) {
        let _ = sub.tx.send(event.clone());
    }
}

/// Prépare + diffuse un event de sandbox pour l'owner de `spec.project`.
/// `phase` vaut `None` en suppression → le front retire l'entrée et refetch.
async fn dispatch_sandbox(
    shared: &SharedState,
    client: &VnlK8sClient,
    s: &Sandbox,
    phase: Option<String>,
) {
    let Some(name) = s.metadata.name.as_deref() else {
        return;
    };
    if s.spec.project.is_empty() {
        return;
    }
    let Some(owner) = resolve_owner(client, &shared.project_owner, &s.spec.project).await else {
        return;
    };
    let event = SandboxStateEvent {
        sandbox: name.to_string(),
        phase,
    };
    let subs = shared.subscribers.lock();
    dispatch(&owner, &event, &subs);
}

/// Tâche tokio : lance un watch kube-runtime sur les Sandbox, résout l'owner de
/// chaque sandbox (cache `project` → `owner`) et diffuse les events de phase aux
/// subscribers de cet owner. La boucle se réinitialise quand le stream se
/// termine (timeout serveur, erreur kube, etc.).
async fn watch_loop(client: VnlK8sClient, shared: SharedState) {
    loop {
        // Ne pas tenir de watch quand aucun subscriber n'est connecté.
        if shared.subscribers.lock().is_empty() {
            tracing::trace!("sandbox-state: aucun subscriber, watch en pause");
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }

        tracing::info!("sandbox-state watch démarré");
        let stream = client.watch_sandboxes();
        futures::pin_mut!(stream);

        while let Some(event) = stream.next().await {
            match event {
                WatchEvent::Added(s) | WatchEvent::Modified(s) => {
                    let phase = s.status.as_ref().and_then(|st| st.phase.clone());
                    dispatch_sandbox(&shared, &client, &s, phase).await;
                }
                WatchEvent::Deleted(s) => {
                    // phase = None → le front retire de la map + refetch.
                    dispatch_sandbox(&shared, &client, &s, None).await;
                }
                WatchEvent::Error(msg) => {
                    tracing::warn!("sandbox-state watch error: {msg}, redémarrage…");
                    break;
                }
            }
        }

        tracing::info!("sandbox-state watch stream terminé, redémarrage…");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[axum::debug_handler]
pub async fn ws_sandbox_state_handler(
    State(state): State<AppState>,
    user: AuthUser,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, user))
}

async fn handle_socket(socket: WebSocket, state: AppState, user: AuthUser) {
    if let Err(e) = run_socket(socket, state, user).await {
        tracing::error!("ws sandbox-state error: {e}");
    }
}

/// S'assure qu'une (et une seule) tâche `watch_loop` tourne. Le client K8s est
/// construit hors verrou (async), puis un second contrôle sous verrou tranche
/// la course entre deux connexions concurrentes.
async fn ensure_watch_loop(state: &AppState, shared: &SharedState) -> Result<(), AppError> {
    if shared.watch_handle.lock().is_some() {
        return Ok(());
    }
    let client = k8s::client(state).await?;
    let mut handle = shared.watch_handle.lock();
    if handle.is_none() {
        *handle = Some(tokio::spawn(watch_loop(client, shared.clone())));
    }
    Ok(())
}

async fn run_socket(socket: WebSocket, state: AppState, user: AuthUser) -> Result<(), AppError> {
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;

    // Pas d'owner K8s → close silencieusement (rien à watcher).
    let owner = match owners::resolve_owner_name(&state, principal_user.id).await? {
        Some(o) => o,
        None => return Ok(()),
    };

    let shared = state.shared_sandbox_state.clone();

    let (tx, rx) = mpsc::unbounded_channel::<SandboxStateEvent>();
    let tx = Arc::new(tx);

    // S'abonner.
    shared.subscribers.lock().push(Subscriber {
        tx: tx.clone(),
        owner: owner.clone(),
    });

    ensure_watch_loop(&state, &shared).await?;

    let (ws_sink, mut ws_stream) = socket.split();

    // Tâche de transfert : channel → socket, possède `rx`.
    let forward_handle = tokio::spawn(async move {
        let mut sink = ws_sink;
        let mut rx = rx;
        while let Some(event) = rx.recv().await {
            let text = serde_json::to_string(&event).unwrap_or_default();
            if sink
                .send(axum::extract::ws::Message::Text(text.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Attendre la déconnexion du client.
    while let Some(Ok(_msg)) = ws_stream.next().await {}

    // Se désabonner (comparaison par pointeur Arc, pas `PartialEq`).
    shared
        .subscribers
        .lock()
        .retain(|sub| !Arc::ptr_eq(&sub.tx, &tx));

    // Fermer le channel (drop l'unique Arc<tx>) et attendre la tâche de transfert.
    drop(tx);
    let _ = forward_handle.await;

    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn sub(owner: &str) -> (Subscriber, mpsc::UnboundedReceiver<SandboxStateEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Subscriber {
                tx: Arc::new(tx),
                owner: owner.to_string(),
            },
            rx,
        )
    }

    #[test]
    fn event_serializes_camel_case_with_null_phase() {
        let json = serde_json::to_string(&SandboxStateEvent {
            sandbox: "sbx-1".to_string(),
            phase: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"sandbox":"sbx-1","phase":null}"#);
    }

    #[test]
    fn dispatch_only_reaches_matching_owner() {
        let (alice, mut alice_rx) = sub("alice");
        let (bob, mut bob_rx) = sub("bob");
        let subs = vec![alice, bob];

        let event = SandboxStateEvent {
            sandbox: "sbx-1".to_string(),
            phase: Some("Running".to_string()),
        };
        dispatch("alice", &event, &subs);

        assert_eq!(alice_rx.try_recv().unwrap().sandbox, "sbx-1");
        assert!(bob_rx.try_recv().is_err());
    }

    #[test]
    fn dispatch_fans_out_to_all_subscribers_of_owner() {
        let (a1, mut a1_rx) = sub("alice");
        let (a2, mut a2_rx) = sub("alice");
        let subs = vec![a1, a2];

        dispatch(
            "alice",
            &SandboxStateEvent {
                sandbox: "sbx-2".to_string(),
                phase: None,
            },
            &subs,
        );

        assert_eq!(a1_rx.try_recv().unwrap().sandbox, "sbx-2");
        assert_eq!(a2_rx.try_recv().unwrap().sandbox, "sbx-2");
    }
}
