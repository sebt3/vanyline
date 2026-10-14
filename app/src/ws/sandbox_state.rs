// tooling.sdd D5 — ws/sandbox_state.sdd Exposes : @ws_sandbox_state_handler est contracté
// « `pub async` + #[axum::debug_handler] » ; retirer `async` contredirait la spec. L'allow est
// ici (portée module) car l'allow d'item n'atteint pas la seconde copie de signature émise par
// la macro debug_handler — vérifié expérimentalement : macro retirée, l'allow d'item éteint
// tout. Écart assumé : l'allow couvre le fichier entier (la seconde copie de signature par la
// macro debug_handler échappe à l'allow d'item) ; une future `async fn` sans `.await` y
// échapperait au lint — vérifié 2026-10-09 : les 6 autres `async fn` du fichier contiennent
// toutes au moins un `.await` ; seule la sienne est contractée `pub async` par ./sandbox_state.sdd.
#![allow(clippy::unused_async)] // ws/sandbox_state.sdd : signature `pub async` contractée (macro ci-dessus)

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

/// Cache `(owner, project)` → owner résolu (`spec.owner` du projet). `None`
/// mémorise un miss (projet introuvable — sandbox orpheline) pour ne pas
/// rappeler l'API K8s ni re-logger à chaque événement de watch. Depuis UN
/// OWNER = UN NAMESPACE (2026-10-04), les lectures sont scopées au namespace
/// de l'Owner ; la clé porte l'Owner pour distinguer deux namespaces.
type OwnerCache = Arc<Mutex<std::collections::HashMap<(String, String), Option<String>>>>;

/// State partagé entre toutes les connexions WS et les tâches `watch_loop`.
#[derive(Clone)]
pub struct SharedState {
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
    project_owner: OwnerCache,
    /// Handles des tâches `watch_loop`, une par Owner suivi (UN OWNER = UN
    /// NAMESPACE : un flux de watch par namespace d'Owner, jamais cluster-
    /// wide). `Some` par Owner dont un loop a été lancé (il tourne alors pour
    /// la vie du process). Sert de garde d'unicité par Owner.
    watch_handles: Arc<Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl SharedState {
    /// Nouvelle instance vide (aucun abonné, cache vide, aucun watch lancé).
    /// Posée dans `AppState::shared_sandbox_state` au démarrage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
            project_owner: Arc::new(Mutex::new(std::collections::HashMap::new())),
            watch_handles: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

/// Résout l'owner d'un Sandbox du namespace de l'Owner `owner_hint` via le
/// cache `(owner, project)` → `owner`, en le peuplant depuis l'API K8s en cas
/// de miss (hit ou miss mémorisés). `None` si le projet est introuvable
/// (sandbox orpheline / owner supprimé) — l'événement est ignoré.
async fn resolve_owner(
    client: &VnlK8sClient,
    cache: &OwnerCache,
    owner_hint: &str,
    project: &str,
) -> Option<String> {
    let key = (owner_hint.to_string(), project.to_string());
    if let Some(cached) = cache.lock().get(&key) {
        return cached.clone();
    }
    let resolved = match client.get_project(owner_hint, project).await {
        Ok(project_obj) => Some(project_obj.spec.owner),
        Err(e) => {
            tracing::warn!("sandbox-state: project '{project}' introuvable: {e}");
            None
        }
    };
    cache.lock().insert(key, resolved.clone());
    resolved
}

/// Dispatche un event aux subscribers dont l'owner correspond.
fn dispatch(owner: &str, event: &SandboxStateEvent, subs: &[Subscriber]) {
    for sub in subs.iter().filter(|sub| sub.owner == owner) {
        let _ = sub.tx.send(event.clone());
    }
}

/// Prépare + diffuse un event de sandbox pour l'owner de `spec.project`
/// (résolu dans le namespace de l'Owner `owner_hint` du flux de watch).
/// `phase` vaut `None` en suppression → le front retire l'entrée et refetch.
async fn dispatch_sandbox(
    shared: &SharedState,
    client: &VnlK8sClient,
    owner_hint: &str,
    s: &Sandbox,
    phase: Option<String>,
) {
    let Some(name) = s.metadata.name.as_deref() else {
        return;
    };
    if s.spec.project.is_empty() {
        return;
    }
    let Some(owner) =
        resolve_owner(client, &shared.project_owner, owner_hint, &s.spec.project).await
    else {
        return;
    };
    let event = SandboxStateEvent {
        sandbox: name.to_string(),
        phase,
    };
    let subs = shared.subscribers.lock();
    dispatch(&owner, &event, &subs);
}

/// Tâche tokio : lance un watch kube-runtime sur les Sandbox du namespace de
/// l'Owner `owner`, résout l'owner de chaque sandbox (cache `(owner, project)`
/// → `owner`) et diffuse les events de phase aux subscribers de cet owner. La
/// boucle se réinitialise quand le stream se termine (timeout serveur, erreur
/// kube, etc.).
async fn watch_loop(client: VnlK8sClient, shared: SharedState, owner: String) {
    loop {
        // Ne pas tenir de watch quand aucun subscriber n'est connecté.
        if shared
            .subscribers
            .lock()
            .iter()
            .all(|sub| sub.owner != owner)
        {
            tracing::trace!("sandbox-state: aucun subscriber, watch en pause");
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }

        tracing::info!("sandbox-state watch démarré");
        let stream = client.watch_sandboxes(&owner);
        futures::pin_mut!(stream);

        while let Some(event) = stream.next().await {
            match event {
                WatchEvent::Added(s) | WatchEvent::Modified(s) => {
                    let phase = s.status.as_ref().and_then(|st| st.phase.clone());
                    dispatch_sandbox(&shared, &client, &owner, &s, phase).await;
                }
                WatchEvent::Deleted(s) => {
                    // phase = None → le front retire de la map + refetch.
                    dispatch_sandbox(&shared, &client, &owner, &s, None).await;
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

// tooling.sdd D5 — signature `pub async` + debug_handler contractées par ws/sandbox_state.sdd
// (exemption portée en tête de fichier : la macro annule les allows d'item).
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

/// S'assure qu'une (et une seule) tâche `watch_loop` tourne pour l'Owner
/// `owner` (UN OWNER = UN NAMESPACE : un flux par Owner suivi). Le client K8s
/// est construit hors verrou (async), puis un second contrôle sous verrou
/// tranche la course entre deux connexions concurrentes du même Owner.
async fn ensure_watch_loop(
    state: &AppState,
    shared: &SharedState,
    owner: &str,
) -> Result<(), AppError> {
    if shared.watch_handles.lock().contains_key(owner) {
        return Ok(());
    }
    let client = k8s::client(state).await?;
    let mut handles = shared.watch_handles.lock();
    if !handles.contains_key(owner) {
        let handle = tokio::spawn(watch_loop(client, shared.clone(), owner.to_string()));
        handles.insert(owner.to_string(), handle);
    }
    Ok(())
}

async fn run_socket(socket: WebSocket, state: AppState, user: AuthUser) -> Result<(), AppError> {
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;

    // Pas d'owner K8s → close silencieusement (rien à watcher).
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Ok(());
    };

    let shared = state.shared_sandbox_state.clone();

    let (tx, rx) = mpsc::unbounded_channel::<SandboxStateEvent>();
    let tx = Arc::new(tx);

    // S'abonner.
    shared.subscribers.lock().push(Subscriber {
        tx: tx.clone(),
        owner: owner.clone(),
    });

    ensure_watch_loop(&state, &shared, &owner).await?;

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
