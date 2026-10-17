//! WS ticket store: short-lived, single-use tickets for WebSocket authentication.
//!
//! The browser WebSocket API does not support custom headers on the handshake.
//! This module solves that by having the frontend call `POST /ws/ticket` (behind
//! `require_auth`), receive a short-lived opaque ticket, and then present it as
//! a query-string parameter on the `GET /ws/*` upgrade request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Middleware d'auth WS partagé par `/ws/fs` et `/ws/terminal`. Lit le ticket en
/// query string (`?ticket=`), le consomme via `redeem_from_query`, renvoie 401 en
/// cas d'erreur (MissingTicket/InvalidTicket), sinon laisse passer vers le handler.
pub async fn ws_auth_middleware(
    State(state): State<crate::AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // Extract ticket from query string
    let ticket = req.uri().query().and_then(|qs| {
        for pair in qs.split('&') {
            if let Some((key, value)) = pair.split_once('=')
                && key == "ticket"
            {
                return Some(value.to_string());
            }
        }
        None
    });

    match crate::ws::ticket::redeem_from_query(&state.tickets, ticket) {
        Ok(_) => next.run(req).await,
        Err(e) => e.into_response(),
    }
}

/// Convert a `WsAuthError` into an HTTP response (401 for both variants,
/// matching the token-auth error path).
impl IntoResponse for WsAuthError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            WsAuthError::MissingTicket | WsAuthError::InvalidTicket => StatusCode::UNAUTHORIZED,
        };
        (
            status,
            Json(serde_json::json!({
                "error": self.to_string(),
            })),
        )
            .into_response()
    }
}

/// Duration of a ticket — a few seconds, long enough for the browser to
/// chain `POST /ws/ticket` then the WS handshake. Not a full session.
pub const TICKET_TTL_SECS: u64 = 30;

/// What a ticket authenticates — same shape as `AuthInfo` (auth.rs): the
/// owner's `subject` only. This is the data that the ticket path must carry,
/// not a shortcut less rich than the JWT path. Aucun niveau d'accès n'existe
/// (auth.sdd, décision 2026-10-04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketClaims {
    /// `sub` du jeton d'Owner, copié de l'`AuthInfo` à l'émission.
    pub subject: String,
}

/// Error returned by the WS extractor when the ticket is missing/consumed.
/// Reused by `ws-fs` and `ws-terminal`.
#[derive(Debug, thiserror::Error)]
pub enum WsAuthError {
    /// `VNL-SBX-WS-001` — requête `GET /ws/*` sans `?ticket=` (ou query sans clé
    /// `ticket`).
    #[error("VNL-SBX-WS-001: missing or malformed ticket")]
    MissingTicket,
    /// `VNL-SBX-WS-002` — ticket inconnu, expiré ou déjà consommé.
    #[error("VNL-SBX-WS-002: invalid or expired ticket")]
    InvalidTicket,
}

/// In-memory store, single-use: each ticket is removed on the first `GET /ws/*`
/// that presents it, whether the request succeeds or not. Never reusable.
#[derive(Debug, Clone)]
pub struct TicketStore {
    tickets: Arc<Mutex<HashMap<String, (TicketClaims, Instant)>>>,
}

impl Default for TicketStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TicketStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tickets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Issues an opaque ticket for `claims`, dated now, with TTL `TICKET_TTL_SECS`.
    ///
    /// # Panics
    ///
    /// Uniquement si le mutex de la map est empoisonné (panic préexistant dans une
    /// section critique du store — bug interne, jamais un acteur externe ; l'`expect`
    /// est exempté en ce sens).
    // tooling.sdd D3c — ticket.sdd : TTL figé 30 s sur Instant monotone ; le dépassement
    // de `now + ttl` exigerait un uptime de ~584 milliards d'années, hors contrat.
    #[allow(clippy::arithmetic_side_effects)]
    #[allow(clippy::expect_used)] // poison means a bug in *our* code, not an external actor
    #[must_use]
    pub fn issue(&self, claims: TicketClaims) -> String {
        use rand::{Rng, distributions::Alphanumeric};

        let ticket: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(32)
            .map(|c| c as char)
            .collect();

        let now = Instant::now();
        let ttl = Duration::from_secs(TICKET_TTL_SECS);

        let mut map = self
            .tickets
            .lock()
            .expect("ticket store mutex should not be poisoned");
        map.insert(ticket.clone(), (claims, now + ttl));
        ticket
    }

    /// Consumes the ticket. Removes the entry **no matter what** (valid or expired).
    /// Returns `Some(TicketClaims)` if the ticket is present and not expired;
    /// `None` otherwise.
    ///
    /// # Panics
    ///
    /// Uniquement si le mutex de la map est empoisonné (bug interne du store —
    /// l'`expect` est exempté en ce sens, jamais un acteur externe).
    #[allow(clippy::expect_used)] // poison means a bug in *our* code, not an external actor
    #[must_use]
    pub fn redeem(&self, ticket: &str) -> Option<TicketClaims> {
        let mut map = self
            .tickets
            .lock()
            .expect("ticket store mutex should not be poisoned");
        if let Some((claims, expires_at)) = map.remove(ticket) {
            if Instant::now() < expires_at {
                Some(claims)
            } else {
                // Expired — already removed, return None
                None
            }
        } else {
            // Ticket already consumed or never existed
            None
        }
    }
}

/// Consomme `ticket` (retiré quoi qu'il arrive) pour produire `TicketClaims`.
/// `None` de query → `MissingTicket` ; ticket absent/expiré → `InvalidTicket`.
///
/// # Errors
///
/// `VNL-SBX-WS-001` (`MissingTicket`) si la query ne portait aucun ticket ;
/// `VNL-SBX-WS-002` (`InvalidTicket`) si le ticket est absent, expiré ou consommé.
pub fn redeem_from_query(
    store: &TicketStore,
    ticket: Option<String>,
) -> Result<TicketClaims, WsAuthError> {
    match ticket {
        Some(t) => store.redeem(&t).ok_or(WsAuthError::InvalidTicket),
        None => Err(WsAuthError::MissingTicket),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::auth::test_support;
    use crate::{AppState, build_app};
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header::AUTHORIZATION};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// `AuthState` de test dont la config vise l'`test_support::TEST_OWNER` ;
    /// l'`AppState` et le jeton d'Owner à présenter sont cohérents (pas de
    /// contournement : un vrai jeton signé par la clé de test).
    fn state_with_test_owner() -> (AppState, String) {
        let config = Arc::new(test_support::test_config(std::path::Path::new(
            "/tmp/vanyline-ticket-test",
        )));
        let auth = test_support::auth_state(config.clone());
        let (fs_events, fs_flush) = crate::fs_push_channels();
        let state = AppState {
            health: crate::health::CodeHealth::new(config.clone()),
            jobs: Arc::new(crate::jobs::Jobs::new(Arc::clone(&config))),
            config,
            auth: Arc::new(auth),
            tickets: TicketStore::new(),
            lsp: Arc::new(crate::lsp::LspManager::default()),
            fs_events,
            fs_flush,
        };
        let token = test_support::mint_owner_token(
            test_support::TEST_OWNER,
            test_support::TEST_AUDIENCE,
            test_support::TEST_ISSUER,
            600,
        );
        (state, token)
    }

    // Test 1: ticket_redeem_returns_claims
    #[test]
    fn ticket_redeem_returns_claims() {
        let store = TicketStore::new();
        let claims = TicketClaims {
            subject: "user-u".into(),
        };
        let ticket = store.issue(claims.clone());
        let redeemed = store.redeem(&ticket);
        assert!(redeemed.is_some());
        let r = redeemed.unwrap();
        assert_eq!(r.subject, "user-u");
    }

    // Test 2: ticket_is_single_use
    #[test]
    fn ticket_is_single_use() {
        let store = TicketStore::new();
        let claims = TicketClaims {
            subject: "u".into(),
        };
        let ticket = store.issue(claims);
        // First redeem succeeds
        assert!(store.redeem(&ticket).is_some());
        // Second redeem fails (ticket consumed)
        assert!(store.redeem(&ticket).is_none());
    }

    // Test 3: expired_ticket_redeem_none_and_removed
    #[test]
    fn expired_ticket_redeem_none_and_removed() {
        let store = TicketStore::new();
        // Inject an already-expired ticket directly into the map.
        // mod tests is inside the same module, so it has access to the private field.
        let claims = TicketClaims {
            subject: "u".into(),
        };
        // Overwrite the Arc with a new map containing an expired entry
        // checked_sub : le test peut tourner moins de 60 s après le démarrage du
        // process (Instant monotone) ; sur None, l'entrée posée à `now` est déjà
        // expirée à la lecture (comparaison strictement inférieure de redeem).
        let expired_instant = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap_or_else(Instant::now);
        store
            .tickets
            .lock()
            .unwrap()
            .insert("expired-ticket".to_string(), (claims, expired_instant));

        // Redeem returns None (expired)
        assert!(store.redeem("expired-ticket").is_none());
        // Second redeem also returns None (entry already removed)
        assert!(store.redeem("expired-ticket").is_none());
    }

    // Test 4: issuance_without_auth_returns_401
    // Route-level test: POST /ws/ticket without Authorization → 401
    #[tokio::test]
    async fn issuance_without_auth_returns_401() {
        let (state, _token) = state_with_test_owner();
        let app = build_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/ws/ticket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // Test 5: issuance_with_owner_token_returns_ticket
    // Route-level test: POST /ws/ticket with a signed test owner token →
    // 200 + ticket, and the issued ticket redeems to the same subject it was
    // minted for.
    #[tokio::test]
    async fn issuance_with_owner_token_returns_ticket() {
        let (mut state, token) = state_with_test_owner();
        // Keep a clone of the store outside the app. Since TicketStore internally
        // uses Arc<Mutex<...>>, the clone shares the same underlying map — we can
        // verify redemptions after the request completes.
        let external_store = TicketStore::new();
        state.tickets = external_store.clone();
        let app = build_app(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/ws/ticket")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        // Read the body to recover the issued opaque ticket.
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ticket = json["ticket"]
            .as_str()
            .expect("response must carry a ticket");

        // Redeem on the (shared) external store clone and verify the claims.
        let claims = external_store
            .redeem(ticket)
            .expect("the ticket issued by the handler must be redeemable");
        assert_eq!(claims.subject, test_support::TEST_OWNER);
        // The ticket is single-use: a second redeem must fail.
        assert!(external_store.redeem(ticket).is_none());
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    // ── Tests par Scenario (titres de `ticket.sdd`) ───────────────────────────

    fn claims_alice() -> TicketClaims {
        TicketClaims {
            subject: "system:serviceaccount:ns:owner-alice".into(),
        }
    }

    /// Requête `GET` sans corps, pour la garde `ws_auth_middleware`.
    fn get_req(uri: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    // Scenario: ticket_rachete_rend_les_claims_emis
    #[test]
    fn ticket_rachete_rend_les_claims_emis() {
        let store = TicketStore::new();
        let claims = claims_alice();
        let ticket = store.issue(claims.clone());
        assert_eq!(store.redeem(&ticket), Some(claims));
    }

    // Scenario: second_rachat_du_meme_ticket_echoue
    #[test]
    fn second_rachat_du_meme_ticket_echoue() {
        let store = TicketStore::new();
        let ticket = store.issue(claims_alice());
        assert!(store.redeem(&ticket).is_some());
        assert!(store.redeem(&ticket).is_none());
    }

    // Scenario: ticket_expire_renvoie_aucun_et_disparait_de_la_map
    #[test]
    fn ticket_expire_renvoie_aucun_et_disparait_de_la_map() {
        let store = TicketStore::new();
        let expired = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap_or_else(Instant::now);
        store
            .tickets
            .lock()
            .unwrap()
            .insert("expire".to_string(), (claims_alice(), expired));
        assert!(store.redeem("expire").is_none());
        assert!(store.tickets.lock().unwrap().get("expire").is_none());
        assert!(store.redeem("expire").is_none());
    }

    // Scenario: format_ticket_alphanumerique_de_trente_deux_caracteres
    #[test]
    fn format_ticket_alphanumerique_de_trente_deux_caracteres() {
        let store = TicketStore::new();
        for _ in 0..20 {
            let t = store.issue(claims_alice());
            assert_eq!(t.len(), 32);
            assert!(t.chars().all(|c| c.is_ascii_alphanumeric()), "{t}");
        }
    }

    // Scenario: emissions_successives_ne_repetent_jamais_le_meme_ticket
    #[test]
    fn emissions_successives_ne_repetent_jamais_le_meme_ticket() {
        let store = TicketStore::new();
        let tickets: Vec<String> = (0..100).map(|_| store.issue(claims_alice())).collect();
        let distinct: std::collections::HashSet<&String> = tickets.iter().collect();
        assert_eq!(distinct.len(), 100);
        for t in &tickets {
            assert!(store.redeem(t).is_some());
        }
    }

    // Scenario: rachats_paralleles_du_meme_ticket_un_seul_gagnant
    #[test]
    fn rachats_paralleles_du_meme_ticket_un_seul_gagnant() {
        const N: usize = 8;
        let store = TicketStore::new();
        let ticket = store.issue(claims_alice());
        let barrier = Arc::new(std::sync::Barrier::new(N));
        let handles: Vec<_> = (0..N)
            .map(|_| {
                let store = store.clone();
                let ticket = ticket.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.redeem(&ticket).is_some()
                })
            })
            .collect();
        let winners = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 1);
    }

    // Scenario: ttl_et_default_sont_verrouilles
    #[test]
    fn ttl_et_default_sont_verrouilles() {
        assert_eq!(TICKET_TTL_SECS, 30);
        let store = TicketStore::default();
        assert!(store.redeem("nimporte-quelle-valeur").is_none());
        assert!(store.tickets.lock().unwrap().is_empty());
    }

    // Scenario: store_clone_voit_la_meme_map_de_tickets
    #[test]
    fn store_clone_voit_la_meme_map_de_tickets() {
        let store = TicketStore::new();
        let clone = store.clone();
        let ticket = store.issue(claims_alice());
        assert_eq!(clone.redeem(&ticket), Some(claims_alice()));
        assert!(store.redeem(&ticket).is_none());
    }

    // Scenario: requete_sans_ticket_rejetee_en_001
    #[tokio::test]
    async fn requete_sans_ticket_rejetee_en_001() {
        let (state, _token) = state_with_test_owner();
        for uri in [
            "/ws/fs",
            "/ws/fs?other=1",
            "/ws/fs?ticket",
            "/ws/fs?Ticket=ab12",
        ] {
            let app = build_app(state.clone());
            let resp = app.oneshot(get_req(uri)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{uri}");
            assert_eq!(
                body_bytes(resp).await,
                br#"{"error":"VNL-SBX-WS-001: missing or malformed ticket"}"#,
                "{uri}"
            );
        }
    }

    // Scenario: ticket_inconnu_rejete_en_002
    #[tokio::test]
    async fn ticket_inconnu_rejete_en_002() {
        let (state, _token) = state_with_test_owner();
        let app = build_app(state);
        let resp = app
            .oneshot(get_req(
                "/ws/terminal?ticket=jamais-emis-00000000000000000000",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            body_bytes(resp).await,
            br#"{"error":"VNL-SBX-WS-002: invalid or expired ticket"}"#
        );
    }

    // Scenario: parametre_ticket_vide_rejete_en_002
    #[tokio::test]
    async fn parametre_ticket_vide_rejete_en_002() {
        let (state, _token) = state_with_test_owner();
        let app = build_app(state);
        let resp = app.oneshot(get_req("/ws/fs?ticket=")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            body_bytes(resp).await,
            br#"{"error":"VNL-SBX-WS-002: invalid or expired ticket"}"#
        );
    }

    // Scenario: parametre_ticket_duplique_premier_gagnant
    #[tokio::test]
    async fn parametre_ticket_duplique_premier_gagnant() {
        let (state, _token) = state_with_test_owner();
        let store = state.tickets.clone();
        let t_valide = store.issue(claims_alice());
        let app = build_app(state.clone());
        let uri = format!("/ws/fs?ticket={t_valide}&ticket=intrus");
        let resp = app.oneshot(get_req(&uri)).await.unwrap();
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            store.redeem(&t_valide).is_none(),
            "t_valide doit être consommé"
        );

        let t_valide2 = store.issue(claims_alice());
        let app = build_app(state);
        let uri = format!("/ws/fs?ticket=intrus&ticket={t_valide2}");
        let resp = app.oneshot(get_req(&uri)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(store.redeem(&t_valide2).is_some(), "t_valide2 non consommé");
    }

    // Scenario: corps_401_verrouille_le_format_des_deux_identifiants
    #[tokio::test]
    async fn corps_401_verrouille_le_format_des_deux_identifiants() {
        for (err, expected) in [
            (
                WsAuthError::MissingTicket,
                r#"{"error":"VNL-SBX-WS-001: missing or malformed ticket"}"#,
            ),
            (
                WsAuthError::InvalidTicket,
                r#"{"error":"VNL-SBX-WS-002: invalid or expired ticket"}"#,
            ),
        ] {
            let resp = err.into_response();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            assert!(resp.headers().get("www-authenticate").is_none());
            assert_eq!(body_bytes(resp).await, expected.as_bytes());
        }
    }

    // Scenario: rejets_inconnu_expire_et_consomme_sont_indistinguables
    #[tokio::test]
    async fn rejets_inconnu_expire_et_consomme_sont_indistinguables() {
        let (state, _token) = state_with_test_owner();
        let store = state.tickets.clone();
        let expired = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap_or_else(Instant::now);
        store
            .tickets
            .lock()
            .unwrap()
            .insert("expire".to_string(), (claims_alice(), expired));
        let consomme = store.issue(claims_alice());
        assert!(store.redeem(&consomme).is_some());

        let mut bodies = Vec::new();
        for ticket in ["expire", consomme.as_str(), "jamais-emis"] {
            let app = build_app(state.clone());
            let resp = app
                .oneshot(get_req(&format!("/ws/fs?ticket={ticket}")))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{ticket}");
            bodies.push(body_bytes(resp).await);
        }
        assert_eq!(bodies[0], bodies[1]);
        assert_eq!(bodies[1], bodies[2]);
    }

    // Scenario: ticket_valide_consomme_malgre_upgrade_absent
    #[tokio::test]
    async fn ticket_valide_consomme_malgre_upgrade_absent() {
        let (state, _token) = state_with_test_owner();
        let store = state.tickets.clone();
        let ticket = store.issue(claims_alice());
        let uri = format!("/ws/fs?ticket={ticket}");

        let app = build_app(state.clone());
        let resp = app.oneshot(get_req(&uri)).await.unwrap();
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);

        let app = build_app(state);
        let resp = app.oneshot(get_req(&uri)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            body_bytes(resp).await,
            br#"{"error":"VNL-SBX-WS-002: invalid or expired ticket"}"#
        );
    }

    // Scenario: claims_valides_ne_sont_pas_injectes_en_aval
    #[tokio::test]
    async fn claims_valides_ne_sont_pas_injectes_en_aval() {
        use axum::extract::Request as AxRequest;
        use axum::routing::get as axget;

        async fn probe(req: AxRequest) -> StatusCode {
            let exts = req.extensions();
            if exts.get::<TicketClaims>().is_none() && exts.get::<crate::auth::AuthInfo>().is_none()
            {
                StatusCode::OK
            } else {
                StatusCode::IM_A_TEAPOT
            }
        }

        let (state, _token) = state_with_test_owner();
        let ticket = state.tickets.issue(claims_alice());
        let app = axum::Router::new()
            .route(
                "/ws/probe",
                axget(probe).layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    ws_auth_middleware,
                )),
            )
            .with_state(state);
        let resp = app
            .oneshot(get_req(&format!("/ws/probe?ticket={ticket}")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Scenario: aucun_log_du_module_ne_fuit_la_valeur_d_un_ticket
    #[tokio::test]
    async fn aucun_log_du_module_ne_fuit_la_valeur_d_un_ticket() {
        use std::sync::Mutex;

        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Buf::default();
        let sub = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer({
                let buf = buf.clone();
                move || buf.clone()
            })
            .finish();
        let _guard = tracing::subscriber::set_default(sub);

        let (state, _token) = state_with_test_owner();
        let store = state.tickets.clone();
        let ticket = store.issue(claims_alice());
        assert!(store.redeem("valeur-inconnue").is_none());
        let app = build_app(state);
        let resp = app
            .oneshot(get_req(&format!("/ws/fs?ticket={ticket}")))
            .await
            .unwrap();
        // Ticket valide : la garde laisse passer (puis l'extracteur WS rejette sans upgrade).
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);

        let logs = String::from_utf8_lossy(&buf.0.lock().unwrap()).into_owned();
        assert!(
            !logs.contains(&ticket),
            "la valeur du ticket apparaît dans les logs"
        );
    }
}
