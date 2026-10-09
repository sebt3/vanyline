//! Tests d'intégration de `sandbox/src/health.rs` (spec `./health.sdd`, lot H1) :
//! machine d'état `CodeHealth` (démarrage, délai de calme/debounce, empreinte du
//! worktree, sondage périodique, un-seul-calcul, refresh forcé, base sticky,
//! échec de calcul — fraîcheur honnête et retry sur la même empreinte),
//! routes `GET /code-health` et `POST /code-health/refresh` sur le routeur
//! protégé (jeton d'Owner) et séparation d'avec la sonde `/health` du port
//! d'exploitation.
//!
//! Hors périmètre (Tasks de `./health.sdd`, lot H2 et câblage LSP) : l'outil MCP
//! `code_health` et ses Scenarios `outil_mcp_*`, le câblage de `notify_modified`
//! dans les dispatchs `tools_impl`/`ws/fs`/git (les appels directs en test sont
//! le moyen de vérifier le debounce), les diagnostics LSP transmis comme
//! `preliminary_issues`, et la vue `new_code` du worktree non commité (mécanique
//! du runner, couverte par `health/src/runner.rs` — cf. le test `code_neuf`).
//!
//! Horloges : l'horloge simulée de tokio est INCOMPATIBLE avec les faux
//! subprocesses du runner (auto-advance : avancer le temps déclenche le
//! `check_timeout` avant la fin des commandes, et rend les observations
//! intermédiaires du type « stale à 29 s » inatteignables — vérifié
//! expérimentalement). Les Scenarios `modification_declenche_*`, `rafale_*`,
//! `edition_au_terminal_*` et `cooldown_zero_*` utilisent donc une horloge
//! réelle à l'échelle 100:1 via `CodeHealth::with_timing` (30 s → 300 ms,
//! 60 s → 300 ms), les marges d'observation étant elles-mêmes bornées.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use vanyline_health::checks::{Check, OutputKind};
use vanyline_health::report::{CheckKind, CheckStatus};
use vanyline_sandbox::auth::test_support;
use vanyline_sandbox::health::{CheckSource, CodeHealth};
use vanyline_sandbox::ws::ticket::TicketStore;
use vanyline_sandbox::{AppState, LspManager, build_app, build_ops_app, fs_push_channels};

// ── Helpers fixtures git ──────────────────────────────────────────────────────

/// Lance `git` dans `cwd` avec un environnement de test déterministe (aucune
/// config globale/système, auteurs posés) — motif des tests de
/// `health/src/runner.rs`.
fn git(cwd: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git disponible");
    assert!(
        output.status.success(),
        "git {args:?} : {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Worktree de test : `git init` sur `main`, un `Cargo.toml` commité (marqueur
/// de langage `rust` pour la détection) ; retourne la racine.
fn init_worktree(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"t\"\n").expect("Cargo.toml");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "initial"]);
}

/// Même chose, plus une origine distante `tmp/origin.git` poussée sur `main`
/// (la ref `origin/main` doit exister pour `git rev-parse --verify`).
fn init_worktree_with_origin(root: &Path) {
    init_worktree(root);
    let bare = root.parent().expect("root sous tmp").join("origin.git");
    std::fs::create_dir_all(&bare).expect("origin.git");
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    git(root, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(root, &["push", "-q", "origin", "main"]);
}

// ── Helpers presets factices (aucune toolchain requise) ───────────────────────

fn probe(id: &str, argv: &[&str]) -> Check {
    Check {
        id: id.to_string(),
        kind: CheckKind::Tests,
        argv: argv.iter().map(|a| (*a).to_string()).collect(),
        cwd: String::new(),
        output: OutputKind::None,
        expand: None,
        serial_group: Some(id.to_string()),
        after: None,
    }
}

/// Preset factice « echo » : chaque appel de la source (donc chaque calcul)
/// incrémente le compteur et rend une vérification `/bin/echo` qui passe.
fn echo_source(counter: &Arc<AtomicUsize>) -> CheckSource {
    let counter = Arc::clone(counter);
    Arc::new(move |_languages: &[String]| {
        counter.fetch_add(1, Ordering::SeqCst);
        vec![probe("echo", &["echo", "ok"])]
    })
}

/// Script de calcul lent : tourne jusqu'à disparition du fichier `lock`,
/// journalise `start`/`end` dans `log` (preuve de non-concurrence et de
/// non-annulation). Aucun timer tokio : contrôlé par le filesystem.
fn slow_script(dir: &Path, lock: &Path, log: &Path) -> PathBuf {
    let script = dir.join("slow-check.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho start >> {}\nwhile [ -e {} ]; do sleep 0.05; done\necho end >> {}\n",
            log.display(),
            lock.display(),
            log.display()
        ),
    )
    .expect("script écrit");
    let mut perms = std::fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).expect("chmod");
    script
}

/// Preset de calcul lent (tient le calcul ouvert jusqu'à retrait du lock),
/// avec compteur d'appels de la source.
fn slow_source(dir: &Path, counter: &Arc<AtomicUsize>) -> (CheckSource, PathBuf, PathBuf) {
    let lock = dir.join("hold.lock");
    let log = dir.join("runs.log");
    std::fs::write(&lock, "x").expect("lock créé");
    let script = slow_script(dir, &lock, &log);
    let counter = Arc::clone(counter);
    let source: CheckSource = Arc::new(move |_languages: &[String]| {
        counter.fetch_add(1, Ordering::SeqCst);
        vec![probe("slow", &[&script.to_string_lossy()])]
    });
    (source, lock, log)
}

fn read_log_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

// ── Helpers état & HTTP (motif de sandbox/tests/http.rs) ─────────────────────

/// `CodeHealth` de test sur la racine du fixture, horloges réelles à l'échelle
/// réduite, et l'`AppState` correspondant (jeton d'Owner de test, cf. auth.sdd).
fn health_on(
    root: &Path,
    source: CheckSource,
    cooldown: u64,
    poll: u64,
) -> (AppState, Arc<CodeHealth>) {
    let config = Arc::new(test_support::test_config(root));
    let health = CodeHealth::with_timing(
        Arc::clone(&config),
        source,
        Duration::from_millis(cooldown),
        Duration::from_millis(poll),
    );
    let auth = Arc::new(test_support::auth_state(Arc::clone(&config)));
    let (fs_events, fs_flush) = fs_push_channels();
    let state = AppState {
        config,
        auth,
        tickets: TicketStore::new(),
        lsp: Arc::new(LspManager::default()),
        fs_events,
        fs_flush,
        health: Arc::clone(&health),
    };
    (state, health)
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

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    auth: Option<String>,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    if auth.is_some() {
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

/// Attend (horloge réelle, borné) une condition sur un snapshot ou un compteur.
async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now()
        .checked_add(Duration::from_secs(8))
        .expect("deadline de wait_until dans l'espace d'horloge");
    while !cond() {
        assert!(
            std::time::Instant::now() < deadline,
            "timeout en attente de : {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn snapshot_phase_reported(health: &Arc<CodeHealth>) -> (String, bool) {
    let snap = health.snapshot();
    (
        serde_json::to_value(&snap).unwrap()["phase"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        snap.report.is_some(),
    )
}

// ── Scenarios ─────────────────────────────────────────────────────────────────

/// Scenario `etat_toujours_disponible_des_le_demarrage` : pendant le tout
/// premier calcul, `GET /code-health` répond 200 + `phase: "computing"`,
/// `report: null`, jamais 404.
#[tokio::test]
async fn etat_toujours_disponible_des_le_demarrage() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (source, lock, log) = slow_source(tmp.path(), &counter);
    let (state, health) = health_on(&root, source, 10_000, 0);
    health.spawn_background();

    // Le premier calcul est bien parti (la source a été appelée) : la phase est
    // « computing » et le lock tient encore le calcul ouvert.
    wait_until("premier calcul démarré", || {
        counter.load(Ordering::SeqCst) == 1
    })
    .await;
    let app = build_app(state);
    let (status, json) = request(&app, Method::GET, "/code-health", Some(bearer()), "").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "jamais 404 pendant le premier calcul"
    );
    assert_eq!(json["phase"], "computing");
    assert!(
        json["report"].is_null(),
        "report null pendant le tout premier"
    );

    std::fs::remove_file(&lock).unwrap();
    wait_until("premier calcul terminé", || {
        counter.load(Ordering::SeqCst) == 1 && health.snapshot().report.is_some()
    })
    .await;
    // Un seul calcul au total malgré build_app (spawn idempotent).
    let lines = read_log_lines(&log);
    assert_eq!(lines.iter().filter(|l| l.as_str() == "start").count(), 1);
}

/// Scenario `calcul_au_demarrage_puis_fresh` : au terme du premier calcul,
/// `phase: "fresh"`, un `report` et un `computed_at`.
#[tokio::test]
async fn calcul_au_demarrage_puis_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (state, health) = health_on(&root, echo_source(&counter), 0, 0);
    health.spawn_background();

    wait_until("premier calcul terminé en fresh", || {
        let snap = health.snapshot();
        snap.phase == vanyline_sandbox::health::Phase::Fresh && snap.report.is_some()
    })
    .await;
    let snap = health.snapshot();
    assert!(snap.computed_at.is_some(), "computed_at posé");
    let report = snap.report.expect("report rendu");
    assert_eq!(report.status(), CheckStatus::Passed, "echo passe");

    let app = build_app(state);
    let (status, json) = request(&app, Method::GET, "/code-health", Some(bearer()), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["phase"], "fresh");
    assert!(!json["report"].is_null());
    assert!(!json["computed_at"].is_null());
}

/// Scenario `modification_declenche_apres_le_delai_de_calme` : une notification
/// rend la phase `stale` sans démarrer de calcul pendant le délai, le calcul
/// démarre une fois le délai écoulé (échelle 30 s → 300 ms).
#[tokio::test]
async fn modification_declenche_apres_le_delai_de_calme() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (state, health) = health_on(&root, echo_source(&counter), 300, 0);
    let _ = state;
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"modif\"\n").unwrap();
    health.notify_modified();

    // À mi-délai (≈ 29 s à l'échelle) : stale, aucun calcul démarré.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let (phase, _has_report) = snapshot_phase_reported(&health);
    assert_eq!(phase, "stale", "la notification passe la phase à stale");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "aucun calcul pendant le délai"
    );

    // Après écoulement du délai : le calcul démarre, puis l'état redevient fresh.
    wait_until("calcul démarré après le délai", || {
        counter.load(Ordering::SeqCst) == 2
    })
    .await;
    wait_until("retour à fresh", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;
}

/// Scenario `rafale_de_modifications_un_seul_calcul` : dix modifications
/// espacées de moins que le délai (échelle 10 s/30 s) ne déclenchent aucun
/// calcul pendant la rafale ; un seul calcul démarre après la dernière.
#[tokio::test]
async fn rafale_de_modifications_un_seul_calcul() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (_state, health) = health_on(&root, echo_source(&counter), 300, 0);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;

    for i in 0..10 {
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"r{i}\"\n"),
        )
        .unwrap();
        health.notify_modified();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "aucun calcul pendant la rafale (délai jamais écoulé)"
    );

    wait_until("un seul calcul après la rafale", || {
        counter.load(Ordering::SeqCst) == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2, "exactement un calcul");
    wait_until("retour à fresh", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;
}

/// Scenario `cooldown_zero_recalcul_immediat` : `health_cooldown_secs = 0`
/// (ici `Duration::ZERO`) ⇒ le calcul démarre sans attente.
#[tokio::test]
async fn cooldown_zero_recalcul_immediat() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (_state, health) = health_on(&root, echo_source(&counter), 0, 0);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;

    let start = std::time::Instant::now();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"zero\"\n").unwrap();
    health.notify_modified();
    wait_until("calcul immédiat (cooldown 0)", || {
        counter.load(Ordering::SeqCst) == 2
    })
    .await;
    assert!(
        start.elapsed() < Duration::from_millis(1_000),
        "démarrage sans attente de délai, mesuré : {:?}",
        start.elapsed()
    );
}

/// Scenario `empreinte_identique_aucun_recalcul` : une notification qui n'a
/// rien changé au worktree ne lance aucun calcul et rend la phase `fresh`.
#[tokio::test]
async fn empreinte_identique_aucun_recalcul() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (_state, health) = health_on(&root, echo_source(&counter), 100, 0);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;

    health.notify_modified();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "aucun recalcul : l'empreinte est identique"
    );
    assert_eq!(
        health.snapshot().phase,
        vanyline_sandbox::health::Phase::Fresh,
        "la phase redevient fresh sans calcul"
    );
}

/// Scenario `edition_au_terminal_detectee_par_sondage` : une modification hors
/// des outils (écriture directe, aucune notification) est détectée par le
/// sondage, qui passe l'état `stale` puis enchaîne délai de calme et calcul
/// (échelle 60 s → 300 ms de période).
#[tokio::test]
async fn edition_au_terminal_detectee_par_sondage() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (_state, health) = health_on(&root, echo_source(&counter), 200, 300);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;

    // Modification hors outils : aucune notification, écriture brute.
    std::fs::write(root.join("Lu_par_sondage.txt"), "terminal\n").unwrap();

    wait_until("stale par sondage", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Stale
    })
    .await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "le sondage seul ne calcule pas"
    );
    wait_until("calcul après délai de calme", || {
        counter.load(Ordering::SeqCst) == 2
    })
    .await;
    wait_until("retour à fresh", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;
}

/// Scenario `modification_pendant_un_calcul` : une notification pendant un
/// calcul n'annule pas le calcul en cours, passe la phase à `stale`, et un
/// nouveau cycle suit sa fin.
#[tokio::test]
async fn modification_pendant_un_calcul() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (source, lock, log) = slow_source(tmp.path(), &counter);
    let (_state, health) = health_on(&root, source, 200, 0);
    health.spawn_background();
    wait_until("calcul 1 démarré", || counter.load(Ordering::SeqCst) == 1).await;

    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"pendant\"\n").unwrap();
    health.notify_modified();

    let (phase, has_report) = snapshot_phase_reported(&health);
    assert_eq!(phase, "stale", "la modification passe la phase à stale");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "pas de second calcul lancé"
    );
    assert!(!has_report, "le calcul en cours n'a pas encore publié");

    std::fs::remove_file(&lock).unwrap();
    wait_until("calcul 1 terminé (non annulé)", || {
        read_log_lines(&log).iter().any(|l| l == "end")
    })
    .await;
    wait_until("nouveau cycle après la fin", || {
        counter.load(Ordering::SeqCst) == 2
    })
    .await;
    wait_until("retour à fresh", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;
    assert_eq!(
        read_log_lines(&log),
        vec![
            "start".to_string(),
            "end".to_string(),
            "start".to_string(),
            "end".to_string()
        ],
        "les deux calculs se terminent, sans annulation"
    );
}

/// Scenario `un_seul_calcul_a_la_fois` : `POST /code-health/refresh` pendant un
/// calcul répond 202 sans lancer de second calcul concurrent ; le recalcul est
/// replanifié derrière le premier.
#[tokio::test]
async fn un_seul_calcul_a_la_fois() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (source, lock, log) = slow_source(tmp.path(), &counter);
    let (state, health) = health_on(&root, source, 10_000, 0);
    health.spawn_background();
    wait_until("calcul 1 démarré", || counter.load(Ordering::SeqCst) == 1).await;

    let app = build_app(state);
    let (status, _json) = request(
        &app,
        Method::POST,
        "/code-health/refresh",
        Some(bearer()),
        "{}",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "replanifié derrière le premier"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        read_log_lines(&log),
        vec!["start".to_string()],
        "aucun second calcul concurrent"
    );

    std::fs::remove_file(&lock).unwrap();
    wait_until("recalcul replanifié exécuté", || {
        counter.load(Ordering::SeqCst) == 2
    })
    .await;
    wait_until("les deux calculs terminés", || {
        read_log_lines(&log).len() == 4
    })
    .await;
    assert_eq!(
        read_log_lines(&log),
        vec![
            "start".to_string(),
            "end".to_string(),
            "start".to_string(),
            "end".to_string()
        ],
        "exécution strictement séquentielle"
    );
}

/// Scenario `refresh_force_ignore_le_delai` : avec un délai de calme de 10 s
/// (échelle), `POST /code-health/refresh` (corps optionnel `{}`) démarre le
/// calcul immédiatement.
#[tokio::test]
async fn refresh_force_ignore_le_delai() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (state, health) = health_on(&root, echo_source(&counter), 10_000, 0);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;

    let app = build_app(state);
    let start = std::time::Instant::now();
    let (status, _json) = request(
        &app,
        Method::POST,
        "/code-health/refresh",
        Some(bearer()),
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    wait_until("calcul démarré sans attendre le délai", || {
        counter.load(Ordering::SeqCst) == 2
    })
    .await;
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "délai contractuel : 10 s, démarrage mesuré : {:?}",
        start.elapsed()
    );
    // Le refresh force même l'empreinte : aucun changement du worktree et pourtant recalcul.
    wait_until("retour à fresh", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;
}

/// Scenario `base_invalide_refusee` : un corps `{"base": "--output=x"}` est
/// refusé 400 sans modifier la base sticky (ni syntaxiquement, ni par
/// `git rev-parse --verify` sur une ref inexistante).
#[tokio::test]
async fn base_invalide_refusee() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (state, health) = health_on(&root, echo_source(&counter), 0, 0);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;

    let app = build_app(state);
    let (status, json) = request(
        &app,
        Method::POST,
        "/code-health/refresh",
        Some(bearer()),
        r#"{"base": "--output=x"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        json["error"].is_string(),
        "message d'erreur porteur d'identifiant"
    );
    assert!(health.snapshot().base.is_none(), "base sticky inchangée");

    // Une ref syntaxiquement saine mais inexistante est refusée aussi ; la base
    // sticky en place (« main », posée juste avant) reste inchangée.
    let (status, _) = request(
        &app,
        Method::POST,
        "/code-health/refresh",
        Some(bearer()),
        r#"{"base": "main"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    wait_until("base sticky posée", || {
        health.snapshot().base.as_deref() == Some("main")
    })
    .await;

    let (status, json) = request(
        &app,
        Method::POST,
        "/code-health/refresh",
        Some(bearer()),
        r#"{"base": "refs/heads/nexiste-pas"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(json["error"].is_string());
    assert_eq!(
        health.snapshot().base.as_deref(),
        Some("main"),
        "la base sticky n'est pas modifiée par un refus"
    );
}

/// Scenario `base_sticky_pour_les_recalculs` : `POST /code-health/refresh
/// {"base": "origin/main"}` rend la base sticky ; le recalcul automatique qui
/// suit une modification produit une vue `new_code` relative à `origin/main`.
#[tokio::test]
async fn base_sticky_pour_les_recalculs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree_with_origin(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (state, health) = health_on(&root, echo_source(&counter), 150, 0);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;

    let app = build_app(state);
    let (status, _) = request(
        &app,
        Method::POST,
        "/code-health/refresh",
        Some(bearer()),
        r#"{"base": "origin/main"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    wait_until("new_code sur origin/main", || {
        let snap = health.snapshot();
        snap.phase == vanyline_sandbox::health::Phase::Fresh
            && snap
                .report
                .as_ref()
                .is_some_and(|r| r.new_code.as_ref().is_some_and(|n| n.base == "origin/main"))
    })
    .await;

    // Modification + recalcul automatique : la base sticky survit au refresh.
    // Le compteur de la source s'incrémente au DÉMARRAGE du calcul : attendre
    // la PUBLICATION (phase fresh et `computed_at` avancé au-delà de celui du
    // refresh) pour que l'assertion porte sur le rapport du recalcul
    // automatique, pas sur celui, encore en place, du refresh.
    let published = health
        .snapshot()
        .computed_at
        .expect("computed_at du refresh");
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"sticky\"\n").unwrap();
    health.notify_modified();
    wait_until("recalcul automatique publié", || {
        let snap = health.snapshot();
        snap.phase == vanyline_sandbox::health::Phase::Fresh
            && snap.computed_at.is_some_and(|at| at > published)
    })
    .await;
    assert!(
        counter.load(Ordering::SeqCst) >= 3,
        "le recalcul automatique a bien eu lieu"
    );
    let snap = health.snapshot();
    let new_code = snap
        .report
        .as_ref()
        .expect("report")
        .new_code
        .as_ref()
        .expect("vue new_code du recalcul automatique");
    assert_eq!(new_code.base, "origin/main", "base sticky conservée");
}

/// Scenario `langage_sans_preset_rapporte` : un langage sélectionné sans preset
/// (`go`) apparaît dans `report.unchecked_languages` (le détecteur de marqueurs
/// ne produit que rust/js-ts/vue/python/dockerfile ; la sélection explicite
/// par `/refresh` est le chemin contractuel observable).
#[tokio::test]
async fn langage_sans_preset_rapporte() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    // Source miroir de la production : les presets réels du crate (aucun preset
    // « go » ⇒ aucune commande, aucune toolchain requise).
    let source: CheckSource =
        Arc::new(|languages: &[String]| vanyline_health::checks::checks_for_languages(languages));
    let (state, health) = health_on(&root, source, 0, 0);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;

    let app = build_app(state);
    let (status, _) = request(
        &app,
        Method::POST,
        "/code-health/refresh",
        Some(bearer()),
        r#"{"languages": ["go"]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    wait_until("rapport avec go non vérifié", || {
        let snap = health.snapshot();
        snap.phase == vanyline_sandbox::health::Phase::Fresh
            && snap
                .report
                .as_ref()
                .is_some_and(|r| r.unchecked_languages.iter().any(|l| l == "go"))
    })
    .await;
}

/// Scenario `calcul_echoue_ne_ment_jamais` : un recalcul planifié par une
/// modification (empreinte différente) qui ÉCHOUE ne rend jamais l'état
/// `fresh` avec le rapport périmé du calcul précédent, ne pose pas
/// l'empreinte du calcul échoué, et le cycle suivant sur la MÊME empreinte
/// retente le calcul (jamais abandonné par empreinte « consommée »).
///
/// Déclencheur d'échec choisi d'après `compute()` : seule la branche `Err` du
/// HANDLE est visée (pas un check `errored` du rapport — la disparition de la
/// base sticky ne produit que cela, côté runner). `run_checks` ne rend
/// d'erreur propagée que `ReportIo` (création de `out_dir`) ; `out_dir` est
/// `<tmp>/vanyline-health/<n>` avec `n` = numéro de calcul de CETTE instance
/// (`out_seq`, zéro au départ). On consomme les numéros `0..FAIL_SEQ` par des
/// refresh forcés, puis un fichier posé sur le chemin `<tmp>/vanyline-health/
/// FAIL_SEQ` fait échouer `create_dir_all` (chemin existant non-répertoire)
/// — déterministe. `FAIL_SEQ` est hors de portée des autres tests de ce binaire
/// (aucun ne dépasse une demi-douzaine de calculs) : zéro interférence.
///
/// Le retry final est déclenché à la main (`notify_modified`, sondage
/// désactivé) sur une empreinte INCHANGÉE depuis l'échec : si l'empreinte
/// avait été posée par l'échec, le cycle la verrait identique à celle du
/// « dernier calcul terminé » et ne relancerait jamais rien — `wait_until`
/// expire. C'est l'assertion qui attrape spécifiquement l'empreinte posée.
#[tokio::test]
async fn calcul_echoue_ne_ment_jamais() {
    const FAIL_SEQ: usize = 20;
    // Le poison est retiré même si une assertion échoue avant l'étape retry.
    struct Unpoison(PathBuf);
    impl Drop for Unpoison {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let poison = std::env::temp_dir()
        .join("vanyline-health")
        .join(FAIL_SEQ.to_string());

    let counter = Arc::new(AtomicUsize::new(0));
    // cooldown 0 (cycle immédiat après notification), sondage désactivé : le
    // seul calcul spontané est celui du démarrage, les cycles suivants sont
    // déclenchés à la main — chronologie entièrement déterministe.
    let (_state, health) = health_on(&root, echo_source(&counter), 0, 0);
    health.spawn_background();
    wait_until("état fresh initial", || {
        health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "premier calcul sur l'empreinte d'origine"
    );

    for _ in 1..FAIL_SEQ {
        health.refresh(None, None).await.unwrap();
    }
    wait_until("refreshs de remplissage publiés", || {
        counter.load(Ordering::SeqCst) == FAIL_SEQ
            && health.snapshot().phase == vanyline_sandbox::health::Phase::Fresh
    })
    .await;
    let before = health.snapshot();
    let computed_before = before.computed_at.expect("computed_at posé");
    assert!(
        before.report.is_some(),
        "rapport du dernier calcul terminé présent"
    );

    // Given la modification (empreinte différente) + Given le recalcul qui
    // échoue : poison posé pile sur le numéro d'out du prochain calcul.
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"echec\"\n").unwrap();
    std::fs::write(&poison, "pas un repertoire").unwrap();
    let _unpoison = Unpoison(poison.clone());

    health.notify_modified();
    wait_until("recalcul en échec (source appelée)", || {
        counter.load(Ordering::SeqCst) == FAIL_SEQ + 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let after = health.snapshot();
    assert_eq!(
        after.phase,
        vanyline_sandbox::health::Phase::Stale,
        "un calcul échoué ne présente jamais le rapport périmé comme fresh"
    );
    assert_eq!(
        after.computed_at,
        Some(computed_before),
        "aucun rapport publié sur échec"
    );
    assert!(
        after.report.is_some(),
        "le rapport visible reste celui du dernier calcul terminé"
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        health.snapshot().phase,
        vanyline_sandbox::health::Phase::Stale,
        "rien ne fait repasser l'état fresh tout seul après l'échec"
    );

    // Then le cycle suivant sur la MÊME empreinte retente le calcul : le
    // poison retiré, une simple notification (aucune nouvelle modification du
    // worktree) doit relancer un calcul abouti, pas un refus « empreinte
    // identique ».
    std::fs::remove_file(&poison).unwrap();
    health.notify_modified();
    wait_until("retry publié sur la même empreinte", || {
        let snap = health.snapshot();
        counter.load(Ordering::SeqCst) == FAIL_SEQ + 2
            && snap.phase == vanyline_sandbox::health::Phase::Fresh
            && snap.computed_at.is_some_and(|at| at > computed_before)
    })
    .await;
}

/// Scenario `worktree_propre_apres_calcul` : un calcul qui écrit dans son
/// `out_dir` (`{out_dir}` étendu par le runner, hors worktree) laisse
/// `git status --porcelain` vide.
#[tokio::test]
async fn worktree_propre_apres_calcul() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let counter2 = Arc::clone(&counter);
    let source: CheckSource = Arc::new(move |_languages: &[String]| {
        counter2.fetch_add(1, Ordering::SeqCst);
        vec![probe("out-writer", &["touch", "{out_dir}/marker"])]
    });
    let (_state, health) = health_on(&root, source, 0, 0);
    health.spawn_background();
    wait_until("calcul terminé", || health.snapshot().report.is_some()).await;

    let porcelain = git(&root, &["status", "--porcelain"]);
    assert_eq!(porcelain, "", "un calcul ne doit jamais salir git status");
}

/// Scenario `ne_pas_confondre_avec_la_sonde` : `/health` est la sonde du port
/// d'exploitation uniquement (404 sur le port principal, 200 sur le port
/// d'exploitation) ; l'état du code est sous `/code-health`.
#[tokio::test]
async fn ne_pas_confondre_avec_la_sonde() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (state, _health) = health_on(&root, echo_source(&counter), 0, 0);
    let app = build_app(state);

    let (status, _) = request(&app, Method::GET, "/health", Some(bearer()), "").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "/health n'est pas du port principal"
    );
    let (status, _) = request(&app, Method::GET, "/health", None, "").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "chemin inconnu : 404 avant la garde d'auth (précédent lib.sdd \
         `sante_absente_du_port_principal`, interaction garde × 404 au Tasks `[?]`)"
    );
    let (status, json) = request(&app, Method::GET, "/code-health", Some(bearer()), "").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "l'état du code est sous /code-health"
    );
    assert!(!json["phase"].is_null());

    let ops = build_ops_app();
    let (status, json) = request(&ops, Method::GET, "/health", None, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({"status": "ok"}));
}

/// Scenario `auth_exigee` : les deux routes sont protégées par le jeton de
/// l'Owner — sans en-tête 401, avec le jeton valide d'un autre Owner 403.
#[tokio::test]
async fn auth_exigee() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (state, _health) = health_on(&root, echo_source(&counter), 0, 0);
    let app = build_app(state);

    for (method, uri, body) in [
        (Method::GET, "/code-health", ""),
        (Method::POST, "/code-health/refresh", "{}"),
    ] {
        let (status, _) = request(&app, method.clone(), uri, None, body).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} sans jeton"
        );
        let (status, _) = request(
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

// ── Test d'appoint (hors Scenario) : séparation sondes après câblage ─────────

/// Le compteur de calculs est la preuve d'observation des Scenarios ; ce test
/// verrouille que `build_app` ne lance pas un second cycle de fond sur un même
/// `CodeHealth` (spawn idempotent — décision « champ Arc dans `AppState` »,
/// health.sdd Tasks SOLDÉ).
#[tokio::test]
async fn build_app_ne_duplique_pas_la_tache_de_fond() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_worktree(&root);

    let counter = Arc::new(AtomicUsize::new(0));
    let (state, health) = health_on(&root, echo_source(&counter), 10_000, 0);
    health.spawn_background();
    wait_until("calcul démarré", || counter.load(Ordering::SeqCst) == 1).await;
    let _app = build_app(state.clone());
    let _app2 = build_app(state);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "build_app ne relance pas le cycle"
    );
}
