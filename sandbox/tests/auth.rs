//! Scenarios de `sandbox/src/auth.sdd` — un test par Scenario, taggé par son titre.
//!
//! Il n'existe AUCUN mode de contournement : chaque test présente un vrai jeton
//! signé par une clé de test (`auth::test_support`, feature `test-support`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Extension, Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use jsonwebtoken::jwk::JwkSet;
use tower::ServiceExt;
use vanyline_sandbox::auth::{
    AuthError, AuthInfo, AuthState,
    test_support::{self, TokenSpec},
};
use vanyline_sandbox::config::Config;
use vanyline_sandbox::ws::ticket::TicketStore;
use vanyline_sandbox::{AppState, LspManager, build_app, fs_push_channels};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Route-sonde : renvoie le `subject` de l'`AuthInfo` injecté par `require_auth`.
async fn probe(Extension(info): Extension<AuthInfo>) -> String {
    info.subject
}

/// `require_auth` seul (sans le routeur complet) pour tester le middleware.
fn probe_app(state: AppState) -> Router {
    Router::new()
        .route("/probe", get(probe))
        .route("/probe-post", axum::routing::post(probe))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            vanyline_sandbox::auth::require_auth,
        ))
        .with_state(state)
}

fn app_state_with(auth: AuthState, root: &std::path::Path) -> AppState {
    let config = Arc::new(test_support::test_config(root));
    let (fs_events, fs_flush) = fs_push_channels();
    AppState {
        health: vanyline_sandbox::health::CodeHealth::new(config.clone()),
        jobs: Arc::new(vanyline_sandbox::jobs::Jobs::new(Arc::clone(&config))),
        config,
        auth: Arc::new(auth),
        tickets: TicketStore::new(),
        lsp: Arc::new(LspManager::default()),
        fs_events,
        fs_flush,
    }
}

async fn body_text(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn body_json(resp: Response) -> serde_json::Value {
    serde_json::from_str(&body_text(resp).await).unwrap()
}

// ── Serveur JWKS simulé ───────────────────────────────────────────────────────

#[derive(Clone)]
enum FakeMode {
    /// répond 200 avec ce corps JWKS
    Json(String),
    /// répond l'erreur HTTP donnée
    Status(u16),
    /// accepte la connexion, lit la requête, ne répond jamais
    Hang,
}

/// Serveur HTTP minimaliste qui compte chaque requête reçue.
struct FakeJwks {
    addr: std::net::SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl FakeJwks {
    async fn spawn(mode: FakeMode) -> FakeJwks {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = hits.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            // Sockets « hang » maintenues vivantes jusqu'à la fin du runtime.
            let mut held = Vec::new();
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                hits2.fetch_add(1, Ordering::SeqCst);
                // Lire (au moins une fois) la requête avant de répondre.
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                match &mode {
                    FakeMode::Json(body) => {
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                    }
                    FakeMode::Status(code) => {
                        let reason = if *code == 500 {
                            "Internal Server Error"
                        } else {
                            "Service Unavailable"
                        };
                        let resp = format!(
                            "HTTP/1.1 {code} {reason}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                    }
                    FakeMode::Hang => {
                        held.push(sock);
                    }
                }
            }
        });
        FakeJwks { addr, hits }
    }

    fn url(&self) -> String {
        format!("http://{}/openid/v1/jwks", self.addr)
    }

    fn count(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

/// Config pointée sur un serveur simulé (pas de CA, pas de jeton de SA).
fn live_config(url: &str, root: &std::path::Path) -> Config {
    let mut c = test_support::test_config(root);
    c.jwks_url = Some(url.to_string());
    c.jwks_file = None;
    c.jwks_ca_file = None;
    c.jwks_token_file = None;
    c
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

fn render_missing_token() -> u64 {
    vanyline_sandbox::telemetry::init_prometheus().unwrap();
    let text = vanyline_sandbox::telemetry::render_metrics().expect("recorder installé ci-dessus");
    // ligne `mcp_auth_errors_total{reason="missing_token"} <n>`
    text.lines()
        .find(|l| l.starts_with("mcp_auth_errors_total{") && l.contains("reason=\"missing_token\""))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

// ── Scenarios ─────────────────────────────────────────────────────────────────

/// Scénario `jeton_owner_valide_atteint_le_handler` (`auth.sdd`).
#[tokio::test]
async fn scenario_jeton_owner_valide_atteint_le_handler() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = test_support::auth_state(Arc::new(test_support::test_config(tmp.path())));
    let app = probe_app(app_state_with(auth, tmp.path()));

    let token = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/probe")
                .header("authorization", bearer(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_text(resp).await, test_support::TEST_OWNER);
}

/// Scénario `jeton_d_un_autre_owner_est_403` (`auth.sdd`).
#[tokio::test]
async fn scenario_jeton_d_un_autre_owner_est_403() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = test_support::auth_state(Arc::new(test_support::test_config(tmp.path())));
    let app = probe_app(app_state_with(auth, tmp.path()));

    let token = test_support::mint_owner_token(
        "system:serviceaccount:ns:owner-bob",
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/probe")
                .header("authorization", bearer(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        !resp
            .headers()
            .contains_key(axum::http::header::WWW_AUTHENTICATE)
    );
    let json = body_json(resp).await;
    assert_eq!(json["error"], "forbidden");
    assert_eq!(
        json["error_description"],
        "VNL-SBX-AUTH-006: token does not belong to this sandbox's owner"
    );
}

/// Scénario `jeton_du_pod_audience_apiserver_refuse` (`auth.sdd`).
#[tokio::test]
async fn scenario_jeton_du_pod_audience_apiserver_refuse() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = test_support::auth_state(Arc::new(test_support::test_config(tmp.path())));
    let app = probe_app(app_state_with(auth, tmp.path()));

    // Jeton du bon `sub`, signé par la clé de test, mais audience apiserver.
    let token = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        "https://kubernetes.default.svc",
        test_support::TEST_ISSUER,
        600,
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/probe")
                .header("authorization", bearer(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_token");
    assert_eq!(
        json["error_description"],
        "VNL-SBX-AUTH-002: invalid or expired token"
    );
}

/// Scénario `en_tete_absent_ou_mal_forme` (`auth.sdd`).
#[tokio::test]
async fn scenario_en_tete_absent_ou_mal_forme() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = test_support::auth_state(Arc::new(test_support::test_config(tmp.path())));
    let app = probe_app(app_state_with(auth, tmp.path()));

    let before = render_missing_token();

    let cases: Vec<Request<Body>> = vec![
        // aucun en-tête
        Request::builder()
            .uri("/probe")
            .body(Body::empty())
            .unwrap(),
        // Basic
        Request::builder()
            .uri("/probe")
            .header("authorization", "Basic xxx")
            .body(Body::empty())
            .unwrap(),
        // préfixe « bearer » en minuscules (préfixe « Bearer » sensible à la casse)
        Request::builder()
            .uri("/probe")
            .header("authorization", "bearer x")
            .body(Body::empty())
            .unwrap(),
        // valeur non UTF-8
        Request::builder()
            .uri("/probe")
            .header(
                "authorization",
                HeaderValue::from_bytes(&[0xff_u8, 0xfe, 0xfd]).unwrap(),
            )
            .body(Body::empty())
            .unwrap(),
    ];

    for req in cases {
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let json = body_json(resp).await;
        assert_eq!(json["error"], "invalid_token");
        assert_eq!(
            json["error_description"],
            "VNL-SBX-AUTH-001: missing or malformed Authorization header"
        );
    }

    // compteur `missing_token` incrémenté de 4 — et aucun de ces cas n'atteint
    // le réseau (valide_token n'est jamais appelé ici).
    let after = render_missing_token();
    assert_eq!(after.saturating_sub(before), 4);
}

/// Scénario `jeton_non_jwt_ou_sans_kid_sans_reseau` (`auth.sdd`).
#[tokio::test]
async fn scenario_jeton_non_jwt_ou_sans_kid_sans_reseau() {
    let tmp = tempfile::tempdir().unwrap();
    let server = FakeJwks::spawn(FakeMode::Json(r#"{"keys":[]}"#.into())).await;
    let auth =
        test_support::auth_state_without_cache(Arc::new(live_config(&server.url(), tmp.path())));

    let err = auth.validate_token("pas-un-jwt").await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidToken), "got {err:?}");

    let no_kid = test_support::mint_token(&TokenSpec {
        kid: None,
        ..test_support::owner_spec(600)
    });
    let err = auth.validate_token(&no_kid).await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidToken), "got {err:?}");

    assert_eq!(
        server.count(),
        0,
        "aucun appel réseau avant un header décodable avec kid"
    );
}

/// Scénario `audience_emetteur_expiration_non_conformes` (`auth.sdd`).
#[tokio::test]
async fn scenario_audience_emetteur_expiration_non_conformes() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = test_support::auth_state(Arc::new(test_support::test_config(tmp.path())));

    let bad_aud = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        "another-audience",
        test_support::TEST_ISSUER,
        600,
    );
    let bad_iss = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        "https://evil.example",
        600,
    );
    let slash_iss = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        &format!("{}/", test_support::TEST_ISSUER),
        600,
    );
    let expired = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        -3600, // exp dépassé au-delà de la tolérance de 30 s
    );
    let future_nbf = test_support::mint_token(&TokenSpec {
        nbf: Some(600),
        ..test_support::owner_spec(3600)
    });

    for token in [bad_aud, bad_iss, slash_iss, expired, future_nbf] {
        let err = auth.validate_token(&token).await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken), "got {err:?}");
    }
}

/// Scénario `algorithmes_interdits` (`auth.sdd`).
#[tokio::test]
async fn scenario_algorithmes_interdits() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = test_support::auth_state(Arc::new(test_support::test_config(tmp.path())));

    // alg: none — jeton non signé fabriqué à la main
    let none_token = test_support::mint_none_token(&test_support::owner_spec(600));
    let err = auth.validate_token(&none_token).await.unwrap_err();
    assert!(
        matches!(
            err,
            AuthError::InvalidToken | AuthError::UnsupportedAlgorithm
        ),
        "got {err:?}"
    );

    // HS256 « signé » avec la clé publique (clé fournie par le jeton : jamais accepté)
    let hs256 = test_support::mint_token(&TokenSpec {
        alg_override: Some("HS256"),
        ..test_support::owner_spec(600)
    });
    let err = auth.validate_token(&hs256).await.unwrap_err();
    assert!(
        matches!(
            err,
            AuthError::InvalidToken | AuthError::UnsupportedAlgorithm
        ),
        "got {err:?}"
    );

    // clé JWKS de type `oct` sous le kid attendu
    let oct_jwks: JwkSet = serde_json::from_str(&format!(
        r#"{{"keys":[{{"kty":"oct","kid":"{}","k":"c2VjcmV0LXRlc3Q"}}]}}"#,
        test_support::TEST_KID
    ))
    .unwrap();
    let auth_oct = test_support::auth_state_with_jwks(
        Arc::new(test_support::test_config(tmp.path())),
        oct_jwks,
        None,
    );
    let good_kid = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );
    let err = auth_oct.validate_token(&good_kid).await.unwrap_err();
    assert!(
        matches!(
            err,
            AuthError::InvalidToken | AuthError::UnsupportedAlgorithm
        ),
        "got {err:?}"
    );
}

/// Scénario `kid_inconnu_un_refresh_puis_key_not_found` (`auth.sdd`).
#[tokio::test]
async fn scenario_kid_inconnu_un_refresh_puis_key_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let server = FakeJwks::spawn(FakeMode::Json(r#"{"keys":[]}"#.into())).await;
    // cache injecté sans le kid du jeton, le serveur simulé ne l'a pas non plus
    let auth = test_support::auth_state_with_jwks(
        Arc::new(live_config(&server.url(), tmp.path())),
        test_support::test_jwks_with_kid("autre-cle"),
        None,
    );

    let token = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );

    let err = auth.validate_token(&token).await.unwrap_err();
    assert!(matches!(err, AuthError::KeyNotFound), "got {err:?}");
    assert_eq!(
        server.count(),
        1,
        "exactement un refresh forcé sur le premier rejet"
    );

    // second appel à moins de 30 s : aucun refresh supplémentaire (throttle)
    let err = auth.validate_token(&token).await.unwrap_err();
    assert!(matches!(err, AuthError::KeyNotFound), "got {err:?}");
    assert_eq!(
        server.count(),
        1,
        "le throttle de 30 s interdit le second refresh"
    );
}

/// Scénario `rotation_de_cle_acceptee_apres_refresh` (`auth.sdd`).
#[tokio::test]
async fn scenario_rotation_de_cle_acceptee_apres_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    // le serveur expose désormais la nouvelle clé
    let server = FakeJwks::spawn(FakeMode::Json(test_support::rotated_jwks_json())).await;
    // cache toujours périmé de l'ancienne clé
    let auth = test_support::auth_state_with_jwks(
        Arc::new(live_config(&server.url(), tmp.path())),
        test_support::test_jwks(),
        None,
    );

    // jeton signé par la nouvelle clé (nouveau kid)
    let token = test_support::mint_token(&TokenSpec {
        key_pem: test_support::TEST_KEY2_PEM,
        kid: Some(test_support::TEST_KID2.to_string()),
        ..test_support::owner_spec(600)
    });

    let info = auth
        .validate_token(&token)
        .await
        .expect("le jeton signé par la nouvelle clé doit être accepté après refresh");
    assert_eq!(info.subject, test_support::TEST_OWNER);
    assert_eq!(server.count(), 1, "un refresh forcé a eu lieu");
}

/// Scénario `panne_jwks_sans_cache_rend_503` (`auth.sdd`).
#[tokio::test]
async fn scenario_panne_jwks_sans_cache_rend_503() {
    let tmp = tempfile::tempdir().unwrap();
    let server = FakeJwks::spawn(FakeMode::Status(500)).await;
    let auth =
        test_support::auth_state_without_cache(Arc::new(live_config(&server.url(), tmp.path())));
    let app = probe_app(app_state_with(auth, tmp.path()));

    let token = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/probe")
                .header("authorization", bearer(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // 503 — pas de 401 : un jeton éventuellement valide ne doit pas être
    // signalé invalide à cause d'une panne amont
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !resp
            .headers()
            .contains_key(axum::http::header::WWW_AUTHENTICATE)
    );
    let json = body_json(resp).await;
    assert_eq!(json["error"], "temporarily_unavailable");
    assert_eq!(
        json["error_description"],
        "VNL-SBX-AUTH-003: cannot obtain token verification keys"
    );
}

/// Scénario `panne_jwks_conserve_l_ancien_cache` (`auth.sdd`).
#[tokio::test]
async fn scenario_panne_jwks_conserve_l_ancien_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let server = FakeJwks::spawn(FakeMode::Status(500)).await;
    // cache périmé contenant la bonne clé : la recharge échoue, l'ancien cache est conservé
    let auth = test_support::auth_state_with_jwks(
        Arc::new(live_config(&server.url(), tmp.path())),
        test_support::test_jwks(),
        Some(Duration::from_mins(61)),
    );

    let token = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );
    let info = auth
        .validate_token(&token)
        .await
        .expect("l'échec de recharge doit conserver l'ancien cache");
    assert_eq!(info.subject, test_support::TEST_OWNER);
    assert_eq!(server.count(), 1, "la recharge périmé a bien été tentée");
}

/// Scénario `timeout_jwks_de_5_secondes` (`auth.sdd`).
#[tokio::test]
async fn scenario_timeout_jwks_de_5_secondes() {
    let tmp = tempfile::tempdir().unwrap();
    let server = FakeJwks::spawn(FakeMode::Hang).await;
    let auth =
        test_support::auth_state_without_cache(Arc::new(live_config(&server.url(), tmp.path())));

    let token = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );

    let start = Instant::now();
    let err = auth.validate_token(&token).await.unwrap_err();
    let elapsed = start.elapsed();

    assert!(matches!(err, AuthError::KeysUnavailable), "got {err:?}");
    assert!(
        elapsed >= Duration::from_secs(4) && elapsed < Duration::from_secs(12),
        "le timeout de 5 s doit borner l'appel, mesuré : {elapsed:?}"
    );
    assert_eq!(
        server.count(),
        1,
        "pas de nouvelle tentative après le timeout"
    );
}

/// Scénario `formats_de_reponse` (`auth.sdd`).
#[tokio::test]
async fn scenario_formats_de_reponse() {
    use axum::response::IntoResponse;

    let cases: Vec<(AuthError, u16, &str, &str, bool)> = vec![
        (
            AuthError::MissingToken,
            401,
            "invalid_token",
            "VNL-SBX-AUTH-001: missing or malformed Authorization header",
            true,
        ),
        (
            AuthError::InvalidToken,
            401,
            "invalid_token",
            "VNL-SBX-AUTH-002: invalid or expired token",
            true,
        ),
        (
            AuthError::KeysUnavailable,
            503,
            "temporarily_unavailable",
            "VNL-SBX-AUTH-003: cannot obtain token verification keys",
            false,
        ),
        (
            AuthError::KeyNotFound,
            401,
            "invalid_token",
            "VNL-SBX-AUTH-004: signing key not found",
            true,
        ),
        (
            AuthError::UnsupportedAlgorithm,
            401,
            "invalid_token",
            "VNL-SBX-AUTH-005: unsupported signing algorithm",
            true,
        ),
        (
            AuthError::NotOwner,
            403,
            "forbidden",
            "VNL-SBX-AUTH-006: token does not belong to this sandbox's owner",
            false,
        ),
    ];

    for (err, status, code, description, with_challenge) in cases {
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::from_u16(status).unwrap());
        let has_challenge = resp
            .headers()
            .contains_key(axum::http::header::WWW_AUTHENTICATE);
        assert_eq!(
            has_challenge, with_challenge,
            "WWW-Authenticate pour {code}"
        );
        let challenge = resp
            .headers()
            .get(axum::http::header::WWW_AUTHENTICATE)
            .map(|hv| hv.to_str().unwrap().to_string());
        let json = body_json(resp).await;
        assert_eq!(json["error"], code);
        assert_eq!(json["error_description"], description);
        if with_challenge {
            // sans resource_metadata : aucune découverte OAuth n'existe
            assert_eq!(
                challenge.as_deref(),
                Some(r#"Bearer error="invalid_token""#)
            );
        } else {
            assert_eq!(challenge, None);
        }
    }
}

/// Scénario `aucun_contournement` (`auth.sdd`).
#[test]
fn scenario_aucun_contournement() {
    // (a) aucune source de clés explicite hors cluster : sans matériel de
    //     vérification lisible (CA du cluster absent), AuthState::new échoue —
    //     jamais un serveur ouvert qui ne pourrait vérifier personne.
    let mut cfg = test_support::test_config(std::path::Path::new("/workspace"));
    cfg.jwks_ca_file = Some("/nonexistent-dir-vnl/ca.crt".into());
    assert!(
        AuthState::new(Arc::new(cfg)).is_err(),
        "sans clés vérifiables, AuthState::new doit refuser de démarrer"
    );

    // (a') CA présent mais non-PEM : refus également (Raises de auth.sdd).
    let tmp = tempfile::tempdir().unwrap();
    let junk = tmp.path().join("ca.not-pem");
    std::fs::write(&junk, b"pas du tout du PEM").unwrap();
    let mut cfg = test_support::test_config(std::path::Path::new("/workspace"));
    cfg.jwks_ca_file = Some(junk.to_string_lossy().into_owned());
    let err = AuthState::new(Arc::new(cfg)).err();
    assert!(
        err.is_some(),
        "un CA non-PEM doit faire échouer le démarrage"
    );

    // (b) owner_subject vide : Config::validate ET AuthState::new échouent.
    let mut cfg = test_support::test_config(std::path::Path::new("/workspace"));
    cfg.owner_subject = Some(String::new());
    assert!(cfg.validate().is_err());
    assert!(AuthState::new(Arc::new(cfg.clone())).is_err());
    let mut cfg2 = cfg.clone();
    cfg2.owner_subject = Some("   ".into());
    assert!(cfg2.validate().is_err());
    assert!(AuthState::new(Arc::new(cfg2)).is_err());

    // owner_subject non monté (None) : refus également.
    let mut cfg3 = test_support::test_config(std::path::Path::new("/workspace"));
    cfg3.owner_subject = None;
    assert!(cfg3.validate().is_err());

    // (c) source de clés vide jamais acceptée.
    let mut cfg4 = test_support::test_config(std::path::Path::new("/workspace"));
    cfg4.jwks_url = Some(String::new());
    assert!(cfg4.validate().is_err());

    // (d) `jwks_file` illisible en mode `File` : la pré-lecture au démarrage
    //     (fail-fast Sébastien 2026-10-08, Given réécrit du Scenario) doit faire
    //     échouer AuthState::new — jamais un serveur ouvert rendant 503 à chaque
    //     requête faute de clés.
    let mut cfg5 = test_support::test_config(std::path::Path::new("/workspace"));
    cfg5.jwks_url = None;
    cfg5.jwks_file = Some("/nonexistent-dir-vnl/jwks.json".into());
    assert!(
        AuthState::new(Arc::new(cfg5)).is_err(),
        "un jwks_file illisible doit faire échouer AuthState::new au démarrage (fail-fast)"
    );

    // (e) `jwks_file` lisible mais ne parsant pas comme JWKS : même refus.
    let junk_jwks = tmp.path().join("not-a-jwks.json");
    std::fs::write(&junk_jwks, b"pas du tout du JWKS").unwrap();
    let mut cfg6 = test_support::test_config(std::path::Path::new("/workspace"));
    cfg6.jwks_url = None;
    cfg6.jwks_file = Some(junk_jwks.to_string_lossy().into_owned());
    assert!(
        AuthState::new(Arc::new(cfg6)).is_err(),
        "un jwks_file non-JWKS doit faire échouer AuthState::new au démarrage (fail-fast)"
    );

    // (f) cas nominal `File` valide : la pré-lecture accepte un vrai JWKS — le
    //     fail-fast de (d)/(e) n'est pas un refus du mode `File` en général.
    let good_jwks = tmp.path().join("jwks.json");
    std::fs::write(
        &good_jwks,
        serde_json::to_vec(&test_support::test_jwks()).unwrap(),
    )
    .unwrap();
    let mut cfg7 = test_support::test_config(std::path::Path::new("/workspace"));
    cfg7.jwks_url = None;
    cfg7.jwks_file = Some(good_jwks.to_string_lossy().into_owned());
    assert!(
        AuthState::new(Arc::new(cfg7)).is_ok(),
        "un jwks_file valide doit démarrer (mode dev nominal)"
    );
}

// ── Passage réel par require_auth sur le routeur protégé (POST /ws/ticket) ────

/// Vérifie que `require_auth` est bien monté sur le routeur protégé de
/// `build_app` (contrat `Handles` de auth.sdd) — le handler d'émission de
/// ticket lit l'`AuthInfo` en extensions.
#[tokio::test]
async fn scenario_require_auth_protege_le_routeur_build_app() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = test_support::auth_state(Arc::new(test_support::test_config(tmp.path())));
    let app = build_app(app_state_with(auth, tmp.path()));

    // sans jeton → 401
    let resp = app
        .clone()
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

    // avec jeton d'Owner de test → 200 + ticket
    let token = test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/ws/ticket")
                .header("authorization", bearer(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert!(json["ticket"].is_string());
}

// garde de compilation : ces imports servent à verrouiller les signatures exposées
#[allow(dead_code)]
fn _api_lock(
    _name: &HeaderName,
    _next: Next,
    _state: State<AppState>,
    _req: Request,
    _json: Json<serde_json::Value>,
) {
}
