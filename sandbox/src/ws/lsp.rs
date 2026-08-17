//! LSP WebSocket endpoint: `GET /ws/lsp/:toolchain`.
//!
//! Authentication is performed by a middleware layer (`ws_auth_middleware`)
//! in [`crate::ws::ticket`], which runs before the `WebSocketUpgrade`
//! extractor (même pattern que `/ws/fs` et `/ws/terminal`).
//!
//! Transport : un message JSON-RPC par frame texte. Le framing `Content-Length`
//! ne concerne que le stdio du process LSP, côté [`crate::lsp`].

use crate::AppState;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::response::IntoResponse;

/// Close code WS quand la toolchain n'a pas de LSP configuré (mode dégradé).
pub const CLOSE_NO_LSP: u16 = 4004;
/// Close code WS quand le spawn du process LSP échoue.
pub const CLOSE_SPAWN_FAILED: u16 = 4005;

/// Handler de `GET /ws/lsp/:toolchain`. Le middleware
/// [`crate::ws::ticket::ws_auth_middleware`] a déjà validé et consommé le ticket.
pub async fn handle_ws_lsp(
    State(state): State<AppState>,
    Path(toolchain): Path<String>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |ws| lsp_session(state, toolchain, ws))
}

/// Session WS LSP : lookup du process via `state.lsp.get_or_spawn` ; si
/// `Ok(None)` → close frame `CLOSE_NO_LSP` ; si `Err` → close frame
/// `CLOSE_SPAWN_FAILED` (log `tracing::error`) ; sinon abonne un client et boucle :
/// - réponses/notifications serveur (canal du client) → frame texte ;
/// - frame texte entrante → `session.send(client_id, bytes)` ; en cas d'erreur,
///   envoyer une frame texte JSON-RPC d'erreur
///   `{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"invalid JSON-RPC request"}}`
///   et continuer la boucle ;
/// - frame binaire → ignorée ; Ping/Pong → ignorés (axum répond aux Ping) ;
/// - Close/erreur/None → fin ; `unsubscribe(client_id)` avant de rendre.
async fn lsp_session(state: AppState, toolchain: String, mut ws: WebSocket) {
    // Lookup / spawn le process LSP
    let session = match state.lsp.get_or_spawn(&toolchain).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            let _ = ws
                .send(Message::Close(Some(CloseFrame {
                    code: CLOSE_NO_LSP,
                    reason: "no lsp for toolchain".into(),
                })))
                .await;
            return;
        }
        Err(e) => {
            tracing::error!(toolchain, error = %e, "LSP spawn error");
            let _ = ws
                .send(Message::Close(Some(CloseFrame {
                    code: CLOSE_SPAWN_FAILED,
                    reason: "lsp spawn failed".into(),
                })))
                .await;
            return;
        }
    };

    // Abonne le client
    let (client_id, mut rx) = session.subscribe();

    // Boucle bidirectionnelle : WS ⇄ process LSP
    loop {
        tokio::select! {
            // Réponses / notifications serveur → WS
            server_payload = rx.recv() => {
                match server_payload {
                    Some(payload) => {
                        let text = match String::from_utf8(payload) {
                            Ok(t) => t,
                            Err(_) => {
                                tracing::error!("LSP: non-UTF8 server payload, breaking");
                                break;
                            }
                        };
                        if ws.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break, // Canal fermé → process mort ou session détruite
                }
            }

            // Frame entrante du client
            ws_frame = ws.recv() => {
                match ws_frame {
                    Some(Ok(Message::Text(text))) => {
                        let bytes = text.as_bytes().to_vec();
                        if session.send(client_id, bytes).await.is_err() {
                            // ER : envoyer une frame texte JSON-RPC d'erreur
                            let _ = ws
                                .send(Message::Text(
                                    serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": null,
                                        "error": { "code": -32600, "message": "invalid JSON-RPC request" }
                                    })
                                    .to_string()
                                    .into(),
                                ))
                                .await;
                        }
                    }
                    // Binaire, Ping, Pong → ignorés
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(Message::Binary(_))) | Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                }
            }

            else => break,
        }
    }

    session.unsubscribe(client_id);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use crate::{
        AppState, AuthState, LspManager, build_app, config::Config, ws::ticket::TicketStore,
    };
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Helper : construit un AppState avec `static_token: Some("s3cret")`
    /// pour émettre un ticket via `POST /ws/ticket`.
    fn make_state(test_name: &str) -> AppState {
        let tmpdir = std::env::temp_dir().join(format!("vanyline-sandbox-lsp-test/{}", test_name));
        let sandbox_root = tmpdir.join("sandbox");

        std::fs::create_dir_all(&sandbox_root).unwrap();

        let config = Arc::new(Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            oidc_issuer: None,
            oidc_audience: None,
            auth_groups_admin: "kubernetes-admin".into(),
            auth_groups_read: "kubernetes-view".into(),
            no_auth: false,
            static_token: Some("s3cret".into()),
            public_url: None,
            oidc_ca_cert: None,
            metrics_listen: "0.0.0.0:9090".into(),
            otel_endpoint: None,
            sandbox_root: sandbox_root.clone(),
        });
        let auth = AuthState::new(config.clone()).unwrap();

        // TicketStore externe partagé (pattern de ticket.rs)
        let external_store = TicketStore::new();
        AppState {
            config,
            auth: Arc::new(auth),
            tickets: external_store.clone(),
            lsp: Arc::new(LspManager::default()),
        }
    }

    /// Test 1: lsp_ticket_required — missing → 401, unknown → 401
    #[tokio::test]
    async fn lsp_ticket_required() {
        let state = make_state("ticket_lsp");
        let app = build_app(state);

        // No ticket → 401
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/ws/lsp/rust")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "missing ticket → 401"
        );

        // Unknown ticket → 401
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/ws/lsp/rust?ticket=unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "unknown ticket → 401"
        );
    }

    /// Test 2: lsp_ticket_required_with_valid_ticket — le route est câblée et le
    /// middleware ticket est traversé avec un ticket valide. La route
    /// `/ws/lsp/rust` est atteinte (ticket consommé, statut ≠ 401). Le
    /// comportement de repli (close 4004 : toolchain sans LSP configuré) est
    /// couvert par `manager_unknown_toolchain_returns_none` dans lsp.rs tests
    /// unitaires.
    #[tokio::test]
    async fn lsp_ticket_required_with_valid_ticket() {
        let state = make_state("ticket_lsp_upgrade");
        let app = build_app(state.clone());

        // Émettre un ticket
        let ticket_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/ws/ticket")
                    .header(header::AUTHORIZATION, "Bearer s3cret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ticket_resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(ticket_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ticket = json["ticket"].as_str().expect("ticket in response");

        // GET /ws/lsp/rust avec un ticket valide et une vraie poignée de main WS
        // (headers d'upgrade). En test `oneshot`, l'extension
        // `hyper::upgrade::OnUpgrade` est absente : l'extractor `WebSocketUpgrade`
        // rejette à 426 (`ConnectionNotUpgradable`), donc le statut 101 n'est pas
        // observable ici. On vérifie ce qui l'est :
        // - le middleware ticket a laissé passer (statut ≠ 401) ;
        // - la route est atteinte et le ticket a été consommé.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/ws/lsp/rust?ticket={ticket}"))
                    .header(header::CONNECTION, "upgrade")
                    .header(header::UPGRADE, "websocket")
                    .header(header::SEC_WEBSOCKET_VERSION, "13")
                    .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZQ==")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Le ticket valide est consommé (sinon on aurait 401)
        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "valid ticket should pass auth (got 401)"
        );

        // Vérifier que le ticket a été consommé par le middleware
        let claims = crate::ws::ticket::redeem_from_query(&state.tickets, Some(ticket.to_string()));
        assert!(claims.is_err(), "ticket should be consumed after request");
    }
}
