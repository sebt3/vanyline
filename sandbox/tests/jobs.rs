//! Tests d'intégration de `sandbox/src/jobs.rs` (spec `./jobs.sdd`) : API de jobs
//! shell durables du port principal protégé (jeton d'Owner) — `POST /jobs`
//! (démarrage idempotent sur `job_id`, 201/200, 400 validation, 429 plafond de
//! jobs simultanés), `GET /jobs/{id}` (état, 404 si inconnu),
//! `GET /jobs/{id}/logs?offset&limit` (lecture incrémentale bornée 64 KiB,
//! en-tête `X-Log-Size`), `DELETE /jobs/{id}` (SIGTERM→5 s→SIGKILL du GROUPE de
//! processus, idempotent) et `DELETE /jobs?prefix=` (annulation par préfixe,
//! préfixe vide 400).
//!
//! Périmètre réel : un test taggé par titre de chacun des 14 `Scenario` de la
//! spec (le `Tasks` « Convertir chaque Scenario en test taggé » — la spec en
//! compte 14) + tests d'appoint hors Scenario verrouillant les clauses
//! `Handles`/`Must` que les Scenario ne nomment pas : éviction des 20 terminés
//! conservés (avec fichier de log), `script` vide refusé, `job_id` invalide
//! refusé, `env` additif appliqué et jamais exposé.
//!
//! Preuves réelles (pas de symbole) :
//! - kill du GROUPE (régression R8) : le job lance un descendant `sleep` en
//!   arrière-plan dont le PID est écrit dans un pidfile ; l'assertion est la
//!   DISPARITION de `/proc/<pid>` après timeout/annulation — un descendant ne
//!   survivant pas, le groupe entier est tué, pas seulement le `sh` ;
//! - idempotence sans second processus : le log du job relu après le second
//!   POST contient exactement une exécution (aucune réécriture, aucun doublon) ;
//! - plafond : quatre `POST /jobs` `sleep` passent, le cinquième rend 429 ;
//! - backoff du callback : faux Restate (`tokio::net::TcpListener`, motif du
//!   faux JWKS de `tests/auth.rs`) horodatant chaque tentative ; le backoff est
//!   injectable par `Jobs::with_callback_timing` (base/max/fenêtre en
//!   `Duration`) pour observer 3 tentatives en millisecondes ; le test asserting
//!   des gaps mesurés ≥ aux délais de backoff demandés (marge d'horloge près).
//!
//! Contraintes : aucune toolchain requise (`sh` + `echo`/`sleep`/`yes`/`head`
//! uniquement) ; horloges réelles courtes (`timeout_secs = 1`, backoff
//! injecté). Les `job_id` sont préfixés par le nom du test : les fichiers de
//! log vivent sous `<tmp>/vanyline-jobs/` partagé par les binaires de test
//! parallèles.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use vanyline_sandbox::auth::test_support;
use vanyline_sandbox::jobs::Jobs;
use vanyline_sandbox::{AppState, LspManager, build_app, fs_push_channels};

// ── Helpers état & HTTP (motif de sandbox/tests/health.rs) ───────────────────

/// `AppState` de test sur `root` : jeton d'Owner de test, `restate_ingress_url`
/// optionnelle et backoff de callback injecté (`Some((base_ms, max_ms,
/// fenêtre_ms))`) pour les Scenario de callback.
fn state_on(
    root: &Path,
    restate_url: Option<String>,
    backoff: Option<(u64, u64, u64)>,
) -> AppState {
    let mut config = test_support::test_config(root);
    config.restate_ingress_url = restate_url;
    let config = Arc::new(config);
    let jobs = match backoff {
        Some((base, max, window)) => Jobs::with_callback_timing(
            Arc::clone(&config),
            Duration::from_millis(base),
            Duration::from_millis(max),
            Duration::from_millis(window),
        ),
        None => Jobs::new(Arc::clone(&config)),
    };
    let health = vanyline_sandbox::health::CodeHealth::new(Arc::clone(&config));
    let auth = Arc::new(test_support::auth_state(Arc::clone(&config)));
    let (fs_events, fs_flush) = fs_push_channels();
    AppState {
        config,
        auth,
        tickets: vanyline_sandbox::ws::ticket::TicketStore::new(),
        lsp: Arc::new(LspManager::default()),
        fs_events,
        fs_flush,
        health,
        jobs: Arc::new(jobs),
    }
}

/// Routeur complet sur racine temporaire existante (racine de confinement).
fn app_on(root: &Path, restate_url: Option<String>, backoff: Option<(u64, u64, u64)>) -> Router {
    build_app(state_on(root, restate_url, backoff))
}

fn owner_token() -> String {
    test_support::mint_owner_token(
        test_support::TEST_OWNER,
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        3600,
    )
}

fn foreign_token() -> String {
    test_support::mint_owner_token(
        "system:serviceaccount:ns:owner-bob",
        test_support::TEST_AUDIENCE,
        test_support::TEST_ISSUER,
        600,
    )
}

fn bearer() -> String {
    format!("Bearer {}", owner_token())
}

async fn request_json(
    app: &Router,
    method: Method,
    uri: &str,
    auth: Option<String>,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    if !body.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(token) = auth {
        builder = builder.header("authorization", token);
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::String(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    };
    (status, json)
}

/// Requête « brute » (logs) : renvoie en-têtes et corps octets tels quels.
async fn request_raw(app: &Router, uri: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, bytes.to_vec())
}

/// POST `/jobs` authentifié.
async fn post_job(app: &Router, body: &str) -> (StatusCode, serde_json::Value) {
    request_json(app, Method::POST, "/jobs", Some(bearer()), body).await
}

/// `{"job_id": …, "script": …}` minimal, échappé par serde.
fn job_body(job_id: &str, script: &str) -> String {
    serde_json::to_string(&serde_json::json!({"job_id": job_id, "script": script})).unwrap()
}

/// Attend (horloge réelle, borné) une condition.
async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now()
        .checked_add(Duration::from_secs(10))
        .expect("deadline de wait_until dans l'espace d'horloge");
    while !cond() {
        assert!(
            std::time::Instant::now() < deadline,
            "timeout en attente de : {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// `GET /jobs/{id}` jusqu'à l'état attendu, et rend le `@JobStatus` obtenu.
async fn wait_state(app: &Router, job_id: &str, want: &str) -> serde_json::Value {
    let uri = format!("/jobs/{job_id}");
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .expect("deadline de wait_state dans l'espace d'horloge");
    loop {
        let (status, json) = request_json(app, Method::GET, &uri, Some(bearer()), "").await;
        if json["state"] == want {
            assert_eq!(status, StatusCode::OK, "GET {uri} après {want}");
            return json;
        }
        assert!(
            Instant::now() < deadline,
            "timeout en attente de l'état {want} de {job_id} : {json}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Lit un pidfile (jusqu'à `timeout`) : le descendant que le job a lancé.
async fn read_pidfile(path: &Path) -> u32 {
    wait_until(&format!("pidfile {}", path.display()), || {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .is_some()
    })
    .await;
    std::fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Le processus `pid` existe-t-il encore (présence de `/proc/<pid>`) ?
fn proc_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Script : descendant `sleep 300` en arrière-plan, PID au pidfile, `sh` en
/// attente — le descendant est dans le MÊME groupe que le `sh` (régression R8).
fn background_job_script(pidfile: &Path) -> String {
    format!("sleep 300 & echo $! > {}; wait", pidfile.display())
}

// ── Faux Restate (motif du faux JWKS de tests/auth.rs) ───────────────────────

#[derive(Clone)]
struct Attempt {
    method: String,
    path: String,
    body: String,
    at: Instant,
}

/// Serveur HTTP minimaliste horodatant chaque requête et répondant une séquence
/// de codes (le dernier code est répété au-delà de la séquence).
struct FakeRestate {
    addr: std::net::SocketAddr,
    attempts: Arc<Mutex<Vec<Attempt>>>,
}

impl FakeRestate {
    async fn spawn(codes: Vec<u16>) -> FakeRestate {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts2 = Arc::clone(&attempts);
        let idx = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                // Lire la requête complète (entêtes + corps de content-length).
                let mut buf: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 8192];
                let mut header_end = None;
                let mut content_len = 0usize;
                // while let : la fin de lecture (Err) termine la boucle ; la
                // lecture 0 octets (fermeture) et la requête complète cassent.
                while let Ok(n) = sock.read(&mut tmp).await {
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if header_end.is_none()
                        && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
                    {
                        header_end = Some(pos);
                        let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                        content_len = head
                            .lines()
                            .find_map(|l| {
                                l.strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                    }
                    if let Some(pos) = header_end
                        && buf.len() >= pos.wrapping_add(4).wrapping_add(content_len)
                    {
                        break;
                    }
                }
                let Some(pos) = header_end else { continue };
                let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
                let mut lines = head.lines();
                let Some(first) = lines.next() else { continue };
                let mut words = first.split_whitespace();
                let method = words.next().unwrap_or_default().to_string();
                let path = words.next().unwrap_or_default().to_string();
                let body = String::from_utf8_lossy(&buf[pos.wrapping_add(4)..]).into_owned();
                attempts2.lock().unwrap().push(Attempt {
                    method,
                    path,
                    body,
                    at: Instant::now(),
                });
                let n = idx.fetch_add(1, Ordering::SeqCst);
                let code = codes[n.min(codes.len().saturating_sub(1))];
                let reason = if code == 200 {
                    "OK"
                } else {
                    "Service Unavailable"
                };
                let resp = format!(
                    "HTTP/1.1 {code} {reason}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        FakeRestate { addr, attempts }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn attempts(&self) -> Vec<Attempt> {
        self.attempts.lock().unwrap().clone()
    }
}

// ── Scenarios ────────────────────────────────────────────────────────────────

/// Scenario `demarrage_puis_lecture_de_l_etat` : `POST /jobs` (`echo hi; exit 0`)
/// répond 201 ; au terme du job, `GET /jobs/j1` rend `state: "succeeded"`,
/// `exit_code: 0`, un `log_tail` contenant `hi` et un `started_at`.
#[tokio::test]
async fn demarrage_puis_lecture_de_l_etat() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    let body = job_body("demarrage_j1", "echo hi; exit 0");
    let (status, json) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED, "201 au démarrage");
    assert_eq!(json["job_id"], "demarrage_j1");
    assert_eq!(json["state"], "running");

    let final_json = wait_state(&app, "demarrage_j1", "succeeded").await;
    assert_eq!(final_json["exit_code"], 0);
    assert!(
        final_json["log_tail"]
            .as_str()
            .is_some_and(|t| t.contains("hi")),
        "log_tail contient la sortie : {final_json}"
    );
    assert!(final_json["started_at"].is_number(), "started_at horodaté");
    assert!(final_json["ended_at"].is_number(), "ended_at horodaté");
    assert_eq!(
        final_json["log_size"].as_u64(),
        Some(3),
        "taille de « hi\\n »"
    );
}

/// Scenario `code_de_sortie_non_nul_est_failed` : un job `exit 3` se termine
/// `state: "failed"` avec `exit_code: 3`.
#[tokio::test]
async fn code_de_sortie_non_nul_est_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    let body = job_body("failed_j1", "exit 3");
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);

    let json = wait_state(&app, "failed_j1", "failed").await;
    assert_eq!(json["exit_code"], 3, "code de sortie non nul rapporté");
}

/// Scenario `idempotence_sur_job_id` : un second `POST /jobs` avec le même
/// `job_id` (job terminé ici) rend 200 avec le même `@JobStatus` et AUCUN second
/// processus n'est lancé — preuve réelle : le fichier de log relu contient
/// exactement une exécution du script (aucune réécriture, aucun doublon).
#[tokio::test]
async fn idempotence_sur_job_id() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    let body = job_body("idempotent_j1", "echo run");
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let first = wait_state(&app, "idempotent_j1", "succeeded").await;

    // Second POST, même job_id : 200 + le même `@JobStatus`.
    let (status, second) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "200 sur job_id déjà connu");
    assert_eq!(second["state"], first["state"]);
    assert_eq!(
        second["started_at"], first["started_at"],
        "même job, pas un nouveau"
    );
    assert_eq!(second["ended_at"], first["ended_at"]);

    // Aucun second processus : le log est resté strictement identique.
    let (_, headers, bytes) = request_raw(&app, "/jobs/idempotent_j1/logs").await;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert_eq!(
        text, "run\n",
        "aucune trace d'une seconde exécution dans le log"
    );
    assert_eq!(
        headers["x-log-size"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap(),
        4,
        "taille totale inchangée"
    );
}

/// Scenario `timeout_tue_le_groupe` : un job qui lance un descendant
/// d'arrière-plan et `timeout_secs = 1` passe à `state: "timed_out"` et plus
/// AUCUN processus du groupe ne survit — preuve : disparition de
/// `/proc/<pid>` du descendant `sleep` (régression R8 de `tools/src/command.rs` :
/// tuer le seul `sh` laisserait ce descendant vivant).
#[tokio::test]
async fn timeout_tue_le_groupe() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("timeout_child.pid");
    let app = app_on(tmp.path(), None, None);

    let body = serde_json::to_string(&serde_json::json!({
        "job_id": "timeout_j1",
        "script": background_job_script(&pidfile),
        "timeout_secs": 1,
    }))
    .unwrap();
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let child = read_pidfile(&pidfile).await;
    assert!(
        proc_alive(child),
        "le descendant est vivant avant l'échéance"
    );

    let json = wait_state(&app, "timeout_j1", "timed_out").await;
    assert!(json["ended_at"].is_number(), " ended_at posé sur timed_out");

    // Le descendant du groupe a été tué aussi (SIGTERM du groupe, SIGKILL de
    // grâce au plus) : /proc/<pid> disparaît sous ~8 s.
    wait_until("descendant tué par le kill de groupe", || {
        !proc_alive(child)
    })
    .await;
}

/// Scenario `annulation` : `DELETE /jobs/j1` sur un job long rend 200 +
/// `state: "cancelled"` et le groupe est tué (pidfile du descendant disparu de
/// `/proc`) ; un second `DELETE` est sans effet (200, idempotent).
#[tokio::test]
async fn annulation() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("annulation_child.pid");
    let app = app_on(tmp.path(), None, None);

    let body = job_body("annulation_j1", &background_job_script(&pidfile));
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let child = read_pidfile(&pidfile).await;

    let (status, json) = request_json(
        &app,
        Method::DELETE,
        "/jobs/annulation_j1",
        Some(bearer()),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["state"], "cancelled");

    // Preuve réelle du kill du groupe : le descendant ne survit pas.
    wait_until("descendant tué par le SIGTERM/SIGKILL du groupe", || {
        !proc_alive(child)
    })
    .await;

    // Second DELETE : idempotent, sans effet.
    let (status, json2) = request_json(
        &app,
        Method::DELETE,
        "/jobs/annulation_j1",
        Some(bearer()),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "idempotent sur job déjà terminé");
    assert_eq!(json2["state"], "cancelled");
}

/// Scenario `annulation_par_prefixe` : trois jobs `r1:a`, `r1:b`, `r2:a` en
/// cours ; `DELETE /jobs?prefix=r1:` annule `r1:a` et `r1:b` (liste rendue),
/// laisse `r2:a` courir ; un préfixe vide (ou absent) rend 400.
#[tokio::test]
async fn annulation_par_prefixe() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    for id in ["r1:a", "r1:b", "r2:a"] {
        let body = job_body(id, "sleep 120");
        let (status, json) = post_job(&app, &body).await;
        assert_eq!(status, StatusCode::CREATED, "démarrage {id}");
        assert_eq!(json["state"], "running");
    }

    let (status, json) = request_json(
        &app,
        Method::DELETE,
        "/jobs?prefix=r1%3A",
        Some(bearer()),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list = json.as_array().expect("liste des `@JobStatus` annulés");
    assert_eq!(list.len(), 2, "les deux r1: sont annulés");
    for item in list {
        assert_eq!(item["state"], "cancelled");
        assert!(item["job_id"].as_str().unwrap().starts_with("r1:"));
    }

    // r2:a continue.
    let (status, json) = request_json(&app, Method::GET, "/jobs/r2:a", Some(bearer()), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["state"], "running", "r2:a n'est pas touché");

    // Nettoyage : annuler r2:a pour ne rien laisser derrière.
    let (status, json) = request_json(&app, Method::DELETE, "/jobs/r2:a", Some(bearer()), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["state"], "cancelled");

    // Préfixe vide et préfixe absent : 400.
    for uri in ["/jobs?prefix=", "/jobs"] {
        let (status, _) = request_json(&app, Method::DELETE, uri, Some(bearer()), "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "préfixe refusé : {uri}");
    }
}

/// Scenario `plafond_de_jobs_simultanes` : quatre jobs `running` passent, le
/// cinquième `POST /jobs` rend 429 (les jobs dormants sont annulés après).
#[tokio::test]
async fn plafond_de_jobs_simultanes() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    for i in 1..=4 {
        let body = job_body(&format!("plafond_p{i}"), "sleep 60");
        let (status, json) = post_job(&app, &body).await;
        assert_eq!(status, StatusCode::CREATED, "le {i}e job passe");
        assert_eq!(json["state"], "running");
    }
    let body = job_body("plafond_p5", "sleep 60");
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "le cinquième est refusé"
    );

    // Un job_id déjà connu passe même au plafond (l'idempotence prime).
    let body = job_body("plafond_p1", "sleep 60");
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::OK, "idempotence avant le plafond");

    // Nettoyage.
    let (status, _) = request_json(
        &app,
        Method::DELETE,
        "/jobs?prefix=plafond_",
        Some(bearer()),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Scenario `logs_incrementaux_bornes` : un job dont la sortie dépasse 100 KiB ;
/// `GET /jobs/j1/logs?offset=0&limit=1000000` rend au plus 64 KiB, `X-Log-Size`
/// donne la taille totale ; la concaténation des lectures à `offset` croissant
/// redonne exactement les logs.
#[tokio::test]
async fn logs_incrementaux_bornes() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    // 1100 lignes de 100 « x » + \n = 111 100 octets (> 100 KiB).
    let body = job_body(
        "logs_j1",
        "yes xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx | head -n 1100",
    );
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    wait_state(&app, "logs_j1", "succeeded").await;

    let expected = vec!["x".repeat(100); 1100].join("\n") + "\n";
    assert_eq!(expected.len(), 111_100);

    let mut offset = 0usize;
    let mut total_size = None;
    let mut concat = String::new();
    loop {
        let (status, headers, bytes) = request_raw(
            &app,
            &format!("/jobs/logs_j1/logs?offset={offset}&limit=1000000"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            bytes.len() <= 64 * 1024,
            "chaque réponse bornée à 64 KiB, reçu {}",
            bytes.len()
        );
        let size: u64 = headers["x-log-size"].to_str().unwrap().parse().unwrap();
        match total_size {
            None => total_size = Some(size),
            Some(prev) => assert_eq!(prev, size, "X-Log-Size stable"),
        }
        concat.push_str(&String::from_utf8_lossy(&bytes));
        offset = offset.wrapping_add(bytes.len());
        if usize::try_from(size).unwrap() <= offset || bytes.is_empty() {
            break;
        }
    }
    assert_eq!(total_size.expect("X-Log-Size posé"), 111_100);
    assert_eq!(concat, expected, "la concaténation redonne les logs");
}

/// Scenario `callback_vers_restate_configure` : `restate_ingress_url` pointant
/// un faux serveur et `callback.awakeable_id = "a1"` ; à la fin du job le faux
/// serveur reçoit `POST /restate/awakeables/a1/resolve` avec le `@JobStatus`, et
/// rien n'est envoyé ailleurs (une seule tentative, aucune autre).
#[tokio::test]
async fn callback_vers_restate_configure() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeRestate::spawn(vec![200]).await;
    let app = app_on(tmp.path(), Some(fake.url()), None);

    let body = serde_json::to_string(&serde_json::json!({
        "job_id": "callback_j1",
        "script": "echo done",
        "callback": {"awakeable_id": "a1"},
    }))
    .unwrap();
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);

    wait_until("resolve reçu par le faux Restate", || {
        fake.attempts().len() == 1
    })
    .await;
    let attempt = fake.attempts().remove(0);
    assert_eq!(attempt.method, "POST");
    assert_eq!(attempt.path, "/restate/awakeables/a1/resolve");
    let payload: serde_json::Value = serde_json::from_str(&attempt.body).unwrap();
    assert_eq!(payload["job_id"], "callback_j1");
    assert_eq!(payload["state"], "succeeded");
    assert_eq!(payload["exit_code"], 0);

    // Rien d'autre n'est envoyé : le faux serveur (seule destination connue)
    // n'a reçu qu'une tentative, et rien de plus après un court délai.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fake.attempts().len(),
        1,
        "exactement une requête, nulle part ailleurs"
    );
}

/// Scenario `callback_relance_jusqu_au_succes` : un faux Restate répond 503
/// deux fois puis 200 ; trois tentatives espacées par le backoff (base injectée
/// à 40 ms, plafond 80 ms — `Jobs::with_callback_timing`), la dernière acquittée.
#[tokio::test]
async fn callback_relance_jusqu_au_succes() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeRestate::spawn(vec![503, 503, 200]).await;
    let app = app_on(tmp.path(), Some(fake.url()), Some((40, 80, 60_000)));

    let body = serde_json::to_string(&serde_json::json!({
        "job_id": "relance_j1",
        "script": "echo done",
        "callback": {"awakeable_id": "a1"},
    }))
    .unwrap();
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);

    wait_until("trois tentatives reçues", || fake.attempts().len() >= 3).await;
    // Marge : vérifier qu'aucune 4e tentative ne part après l'acquittement.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 3, "la dernière (200) arrête les relances");

    let gap1 = attempts[1].at.saturating_duration_since(attempts[0].at);
    let gap2 = attempts[2].at.saturating_duration_since(attempts[1].at);
    assert!(
        gap1 >= Duration::from_millis(30),
        "1re relance après le backoff de base (40 ms), mesuré {gap1:?}"
    );
    assert!(
        gap2 >= Duration::from_millis(60),
        "2e relance après backoff doublé (80 ms plafonné), mesuré {gap2:?}"
    );
}

/// Scenario `callback_sans_url_configuree` : aucune `restate_ingress_url` ;
/// `POST /jobs` avec un `callback` rend 400 et AUCUN job n'est créé.
#[tokio::test]
async fn callback_sans_url_configuree() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    let body = serde_json::to_string(&serde_json::json!({
        "job_id": "nocallback_j1",
        "script": "echo done",
        "callback": {"awakeable_id": "a1"},
    }))
    .unwrap();
    let (status, json) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(json["error"].is_string(), "message d'erreur unique");

    let (status, _) =
        request_json(&app, Method::GET, "/jobs/nocallback_j1", Some(bearer()), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "aucun job créé");
}

/// Scenario `cwd_hors_confinement` : `cwd = "../etc"` puis un chemin absolu
/// (`/etc`) — 400 dans les deux cas (`confine` de `./tools_impl.rs`).
#[tokio::test]
async fn cwd_hors_confinement() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    for cwd in ["../etc", "/etc"] {
        let body = serde_json::to_string(&serde_json::json!({
            "job_id": "cwd_j1",
            "script": "echo hi",
            "cwd": cwd,
        }))
        .unwrap();
        let (status, json) = post_job(&app, &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "cwd refusé : {cwd}");
        assert!(json["error"].is_string());
    }
    let (status, _) = request_json(&app, Method::GET, "/jobs/cwd_j1", Some(bearer()), "").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "aucun job créé sur cwd refusé"
    );
}

/// Scenario `job_perdu_apres_redemarrage` : un `job_id` jamais connu de ce
/// processus (comme perdu à un redémarrage) — `GET /jobs/j1` rend 404.
#[tokio::test]
async fn job_perdu_apres_redemarrage() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    let (status, json) =
        request_json(&app, Method::GET, "/jobs/jamais_lance", Some(bearer()), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(json["error"].is_string());

    // Logs d'un job inconnu : 404 aussi.
    let (status, _) = request_json(
        &app,
        Method::GET,
        "/jobs/jamais_lance/logs",
        Some(bearer()),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Scenario `auth_exigee` : les routes `jobs` sont protégées par le jeton de
/// l'Owner — sans en-tête 401, avec le jeton valide d'un autre Owner 403.
#[tokio::test]
async fn auth_exigee() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    for (method, uri, body) in [
        (Method::POST, "/jobs", r#"{"job_id":"a","script":"true"}"#),
        (Method::GET, "/jobs/a", ""),
        (Method::GET, "/jobs/a/logs", ""),
        (Method::DELETE, "/jobs?prefix=a", ""),
        (Method::DELETE, "/jobs/a", ""),
    ] {
        let (status, _) = request_json(&app, method.clone(), uri, None, body).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} sans jeton"
        );
        let (status, _) = request_json(
            &app,
            method.clone(),
            uri,
            Some(format!("Bearer {}", foreign_token())),
            body,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} avec un autre Owner"
        );
    }
}

// ── Tests d'appoint (hors Scenario) — clauses Handles/Must non nommées ──────

/// Clause `Handles` « les 20 derniers jobs terminés sont conservés (le plus
/// ancien est évincé avec son fichier de log) » : 21 jobs séquentiels, le
/// premier disparaît du registre (404) et son fichier de log est supprimé ;
/// les 20 derniers restent consultables.
#[tokio::test]
async fn eviction_20_derniers_conserves() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    for i in 0..21 {
        let id = format!("eviction_j{i}");
        let body = job_body(&id, "echo done");
        let (status, _) = post_job(&app, &body).await;
        assert_eq!(status, StatusCode::CREATED, "POST {id}");
        wait_state(&app, &id, "succeeded").await;
    }

    let (status, _) =
        request_json(&app, Method::GET, "/jobs/eviction_j0", Some(bearer()), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "le plus ancien est évincé");

    let (status, json) =
        request_json(&app, Method::GET, "/jobs/eviction_j1", Some(bearer()), "").await;
    assert_eq!(status, StatusCode::OK, "les 20 derniers conservés");
    assert_eq!(json["state"], "succeeded");

    // Éviction « avec son fichier de log ».
    assert!(
        !Jobs::log_path("eviction_j0").exists(),
        "le fichier de log du job évincé est supprimé"
    );
    assert!(Jobs::log_path("eviction_j1").exists());

    // Les running ne sont jamais évincés : un job long reste, même au-delà de
    // 20 terminés.
    let body = job_body("eviction_long", "sleep 60");
    let (status, _) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = request_json(
        &app,
        Method::DELETE,
        "/jobs/eviction_long",
        Some(bearer()),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Clause `Exposes` : `script` vide → 400, aucun job créé.
#[tokio::test]
async fn script_vide_refuse() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    let body = job_body("scriptvide_j1", "");
    let (status, json) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(json["error"].is_string());
    let (status, _) =
        request_json(&app, Method::GET, "/jobs/scriptvide_j1", Some(bearer()), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Clause `Exposes` : `job_id` invalide (vide, chemin, `..`) → 400.
#[tokio::test]
async fn job_id_invalide_refuse() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(tmp.path(), None, None);

    for id in ["", "../evil", "a/b", "..", "x\ny", &"z".repeat(200)] {
        let body = job_body(id, "echo hi");
        let (status, _) = post_job(&app, &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "job_id refusé : {id:?}");
    }
}

/// Clauses `Handles`/`Must not` : `env` du corps additif appliqué au process
/// et JAMAIS exposé dans un `@JobStatus` ni une réponse ; dernière valeur
/// gagnante — démontrée ici sur la valeur de l'overlay `.venv` (le marqueur
/// `pyvenv.cfg` pose `VIRTUAL_ENV`, le corps l'écrase). `PATH` est écarté du
/// test : `Command` résout `sh` dans le `PATH` du corps, un `PATH` fictif
/// ferait échouer le spawn (sémantique POSIX réelle, pas un comportement
/// propre à `jobs`).
#[tokio::test]
async fn env_additif_applique_non_expose() {
    let tmp = tempfile::tempdir().unwrap();
    let venv = tmp.path().join(".venv");
    std::fs::create_dir_all(&venv).unwrap();
    std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    let app = app_on(tmp.path(), None, None);

    let body = serde_json::to_string(&serde_json::json!({
        "job_id": "env_j1",
        "script": "echo additif=$VNL_TEST_ADDITIF && echo venv=$VIRTUAL_ENV",
        "env": {"VNL_TEST_ADDITIF": "gagne", "VIRTUAL_ENV": "/corps"},
    }))
    .unwrap();
    let (status, created) = post_job(&app, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        !created.to_string().contains("VNL_TEST_ADDITIF"),
        "l'env n'est jamais exposé dans le `@JobStatus` : {created}"
    );
    let json = wait_state(&app, "env_j1", "succeeded").await;
    assert!(
        !json.to_string().contains("VNL_TEST_ADDITIF"),
        "`@JobStatus` final sans mention de l'env : {json}"
    );

    // Additif appliqué, et le corps gagne sur l'overlay `.venv` recalculé.
    let (_, _, bytes) = request_raw(&app, "/jobs/env_j1/logs").await;
    let log = String::from_utf8_lossy(&bytes).into_owned();
    assert!(
        log.contains("additif=gagne"),
        "env additif appliqué : {log}"
    );
    assert!(
        log.contains("venv=/corps"),
        "dernière valeur gagnante : {log}"
    );
}
