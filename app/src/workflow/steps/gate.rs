//! Kind `gate` — contrat : `gate.sdd`. Gate de validation fondée sur la santé du code de la
//! sandbox : recalcul frais demandé, attendu, puis évaluation du rapport contre des seuils
//! configurables — désactivables un par un. Les pourcentages et le `status` viennent de
//! `vanyline-health` (`CoverageTotals::percent`, `HealthReport::status`), jamais recalculés ici.

use miryad_core::workflow::MiryadWorkflowStep;
use miryad_core::workflow::RunInfo as EngineRunInfo;
use miryad_core::workflow::StepError;
use serde_json::Value;
use vanyline_health::report::{CheckStatus, HealthReport};

use super::{RunInfo, SandboxAccess, StepEnv};

/// Kind de step `gate`.
pub struct GateStep {
    env: StepEnv,
}

impl GateStep {
    /// Le nom de kind contractuel (`gate.sdd` `Exposes`).
    pub(crate) const KIND: &'static str = "gate";

    /// Construit le kind à partir de l'environnement d'exécution.
    pub(crate) fn new(env: StepEnv) -> Self {
        Self { env }
    }
}

/// La `config` du kind `gate` et ses valeurs par défaut contractuelles (`gate.sdd` `Exposes`).
#[derive(Debug, serde::Deserialize)]
pub(crate) struct GateConfig {
    /// Seuil de couverture globale — `null`/absent ⇒ désactivé, défaut absent.
    #[serde(default)]
    pub min_coverage: Option<f64>,
    /// Seuil de couverture du code neuf — désactivé par défaut, exige `base`.
    #[serde(default)]
    pub min_new_code_coverage: Option<f64>,
    /// Ref git de comparaison pour la vue « code neuf ».
    #[serde(default)]
    pub base: Option<String>,
    /// Plafond d'issues `error` — défaut `0`, `null` désactive.
    #[serde(default = "default_max_errors")]
    pub max_errors: Option<i64>,
    /// Vérifications exclues du `status` évalué.
    #[serde(default)]
    pub ignore_checks: Vec<String>,
    /// Refuser tout langage non vérifié.
    #[serde(default)]
    pub fail_on_unchecked: bool,
    /// `false` : la sortie est rendue avec `passed: false` mais le step réussit.
    #[serde(default = "default_enforce")]
    pub enforce: bool,
    /// Délai maximal d'attente de l'état frais.
    #[serde(default = "default_wait")]
    pub wait_timeout_secs: u64,
}

// `max_errors` a un défaut Some(0) ET peut être `null` (désactivé) — `gate.sdd` ; la signature
// `Option` est imposée par serde (le type du champ est `Option<i64>`).
#[allow(clippy::unnecessary_wraps)] // défaut Some(0) désactivable par null — gate.sdd
fn default_max_errors() -> Option<i64> {
    Some(0)
}
fn default_enforce() -> bool {
    true
}
fn default_wait() -> u64 {
    900
}

/// Parse de la config (`_run` et clés inconnues ignorés).
pub(crate) fn parse_config(config: &Value) -> Result<GateConfig, StepError> {
    serde_json::from_value(config.clone()).map_err(|e| StepError {
        message: format!("config du step `gate` invalide : {e}"),
        retryable: false,
    })
}

/// Validation des incompatibilités de config (`min_new_code_coverage` sans `base`).
pub(crate) fn validate_config(cfg: &GateConfig) -> Result<(), StepError> {
    if cfg.min_new_code_coverage.is_some() && cfg.base.is_none() {
        return Err(StepError {
            message: "config du step `gate` invalide : `min_new_code_coverage` exige `base` (la ref git de comparaison)".to_string(),
            retryable: false,
        });
    }
    Ok(())
}

/// Rendu JSON d'une mesure de pourcentage (`null` si absente).
fn pct(v: Option<f64>) -> Value {
    v.and_then(|p| serde_json::Number::from_f64(p).map(Value::Number))
        .unwrap_or(Value::Null)
}

/// Évaluation des cinq règles dans l'ordre du contrat, sur le rapport du crate.
/// Rend `(reasons, sortie)` ; la gate est passante quand `reasons` est vide.
pub(crate) fn evaluate(report: &HealthReport, cfg: &GateConfig) -> (Vec<String>, Value) {
    let mut reasons: Vec<String> = Vec::new();

    // Règle 1 — le `status` évalué, celui du crate : `ignore_checks` exclut les vérifications
    // désignées du seul status évalué (le filtre des checks puis `HealthReport::status()`).
    let evaluated = if cfg.ignore_checks.is_empty() {
        report.clone()
    } else {
        let mut filtered = report.clone();
        filtered
            .checks
            .retain(|c| !cfg.ignore_checks.iter().any(|ig| ig == &c.id));
        filtered
    };
    let status = evaluated.status();
    if status != CheckStatus::Passed {
        reasons.push(format!(
            "vérifications en échec : statut évalué « {} »",
            status_str(&status)
        ));
    }

    // Règle 2 — couverture globale : une mesure est exigée quand le seuil est posé.
    let coverage_percent = report.coverage.as_ref().map(|c| c.totals().percent());
    if let Some(threshold) = cfg.min_coverage {
        match coverage_percent {
            Some(p) if p < threshold => {
                reasons.push(format!("couverture {p} < seuil {threshold}"));
            }
            Some(_) => {}
            None => {
                reasons.push("couverture non mesurée alors que le seuil est défini".to_string());
            }
        }
    }

    // Règle 3 — couverture du code neuf (cohérence `base` déjà vérifiée par `validate_config`).
    let new_code_percent = report
        .new_code
        .as_ref()
        .map(|n| n.coverage.totals().percent());
    if let Some(threshold) = cfg.min_new_code_coverage {
        match new_code_percent {
            Some(p) if p < threshold => {
                reasons.push(format!("couverture du code neuf {p} < seuil {threshold}"));
            }
            Some(_) => {}
            None => reasons.push(
                "couverture du code neuf non mesurée alors que le seuil est défini".to_string(),
            ),
        }
    }

    // Règle 4 — plafond d'issues `error` (`null` désactive).
    let errors = report
        .issues
        .iter()
        .filter(|i| i.severity == vanyline_health::report::Severity::Error)
        .count();
    let warnings = report.issues.len().saturating_sub(errors);
    if let Some(max) = cfg.max_errors {
        let errors_i64 = i64::try_from(errors).unwrap_or(i64::MAX);
        if errors_i64 > max {
            reasons.push(format!(
                "{errors} issues de gravité « error » au-delà du plafond {max}"
            ));
        }
    }

    // Règle 5 — langages non vérifiés (informatifs par défaut, jamais masqués).
    if cfg.fail_on_unchecked && !report.unchecked_languages.is_empty() {
        reasons.push(format!(
            "langages non vérifiés : {}",
            report.unchecked_languages.join(", ")
        ));
    }

    let out = serde_json::json!({
        "passed": reasons.is_empty(),
        "status": status_str(&status),
        "coverage_percent": pct(coverage_percent),
        "new_code_percent": pct(new_code_percent),
        "issues": { "error": errors, "warning": warnings },
        "checks": serde_json::to_value(&report.checks).unwrap_or(Value::Null),
        "unchecked_languages": report.unchecked_languages,
        "reasons": reasons,
    });
    (reasons, out)
}

/// L'état `/code-health` tel que rendu par la sandbox (`/sandbox/src/health.sdd`).
#[derive(Debug, serde::Deserialize)]
struct HealthState {
    phase: String,
    report: Option<HealthReport>,
    computed_at: Option<u64>,
}

/// Séquence `refresh` → attente de l'état FRAIS postérieur à la demande → évaluation →
/// `enforce` (contrat : `gate.sdd` `Handles`).
pub(crate) async fn execute(access: &SandboxAccess, cfg: &GateConfig) -> Result<Value, StepError> {
    let started = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(cfg.wait_timeout_secs);
    // Horloge du run en millisecondes unix, dans le même référentiel que `computed_at`.
    let t0 = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);

    // Recalcul demandé (202 = accepté ; 4xx = refus non retryable, ex. base invalide).
    let mut refresh = serde_json::Map::new();
    if let Some(base) = &cfg.base {
        refresh.insert("base".to_string(), Value::String(base.clone()));
    }
    access
        .post_json("/code-health/refresh", &Value::Object(refresh))
        .await
        .map_err(|e| StepError {
            message: format!("demande de recalcul refusée : {}", e.message),
            retryable: e.retryable,
        })?;

    // Attendre un état `fresh` dont `computed_at` est postérieur à la demande (un fresh
    // antérieur est la mesure d'un autre run). Sonde à 2 s fixes (`gate.sdd`).
    let state = loop {
        let raw = access
            .get_json("/code-health")
            .await
            .map_err(|e| StepError {
                message: format!("lecture de l'état de santé impossible : {}", e.message),
                retryable: e.retryable,
            })?;
        let state: HealthState = serde_json::from_value(raw).map_err(|e| StepError {
            message: format!("état de santé de la sandbox non désérialisable : {e}"),
            retryable: false,
        })?;
        if state.phase == "fresh"
            && state.report.is_some()
            && state.computed_at.is_some_and(|at| at > t0)
        {
            break state;
        }
        if started.elapsed() > timeout {
            return Err(StepError {
                message: format!(
                    "délai dépassé : la santé du code n'est pas revenue fraîche (phase « {} ») en {} s",
                    state.phase, cfg.wait_timeout_secs
                ),
                retryable: false,
            });
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    };
    let report = state.report.ok_or_else(|| StepError {
        message: "état frais sans rapport — jamais un verdict par défaut".to_string(),
        retryable: false,
    })?;

    let (reasons, out) = evaluate(&report, cfg);
    if !reasons.is_empty() && cfg.enforce {
        return Err(StepError {
            message: format!("gate non passante : {}", reasons.join(" ; ")),
            retryable: false,
        });
    }
    Ok(out)
}

/// Affichage contractuel d'un `CheckStatus` (`snake_case`, mêmes mots que le wire).
fn status_str(status: &CheckStatus) -> &'static str {
    match status {
        CheckStatus::Passed => "passed",
        CheckStatus::Failed => "failed",
        CheckStatus::Errored => "errored",
        CheckStatus::Skipped => "skipped",
    }
}

#[async_trait::async_trait]
impl MiryadWorkflowStep for GateStep {
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    async fn run(
        &self,
        _run: &EngineRunInfo,
        config: Value,
        _inputs: std::collections::HashMap<String, Value>,
    ) -> Result<Value, StepError> {
        let cfg = parse_config(&config)?;
        validate_config(&cfg)?;
        let info = RunInfo::from_config(&config)?;
        let access = SandboxAccess::open(&self.env, &info).await?;
        execute(&access, &cfg).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::steps::tests::{config_with_run, run_ctx};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::response::IntoResponse;

    type Reply = (u16, Value);

    #[derive(Clone)]
    struct HealthFake {
        get_seq: Arc<Mutex<VecDeque<Reply>>>,
        get_calls: Arc<AtomicUsize>,
        refresh_bodies: Arc<Mutex<Vec<Value>>>,
        refresh_status: u16,
    }

    /// Faux serveur `/code-health` (`/sandbox/src/health.sdd`) : POST /code-health/refresh,
    /// GET /code-health sur séquence de réponses (la dernière se répète).
    async fn spawn_health_fake(get_seq: Vec<Reply>, refresh_status: u16) -> (String, HealthFake) {
        let fake = HealthFake {
            get_seq: Arc::new(Mutex::new(get_seq.into())),
            get_calls: Arc::new(AtomicUsize::new(0)),
            refresh_bodies: Arc::new(Mutex::new(Vec::new())),
            refresh_status,
        };
        let app = axum::Router::new()
            .route(
                "/code-health/refresh",
                axum::routing::post({
                    let fake = fake.clone();
                    move |body: String| {
                        let fake = fake.clone();
                        let parsed: Value =
                            serde_json::from_str(&body).unwrap_or(Value::String(body));
                        fake.refresh_bodies.lock().unwrap().push(parsed);
                        let status = fake.refresh_status;
                        async move {
                            (axum::http::StatusCode::from_u16(status).unwrap(), "").into_response()
                        }
                    }
                }),
            )
            .route(
                "/code-health",
                axum::routing::get({
                    let fake = fake.clone();
                    move || {
                        let fake = fake.clone();
                        async move {
                            fake.get_calls.fetch_add(1, Ordering::SeqCst);
                            let (status, reply) = {
                                let mut q = fake.get_seq.lock().unwrap();
                                if q.iter().nth(1).is_some() {
                                    q.pop_front().unwrap()
                                } else {
                                    q.front().cloned().unwrap_or((500, json!({})))
                                }
                            };
                            (
                                axum::http::StatusCode::from_u16(status).unwrap(),
                                axum::Json(reply),
                            )
                                .into_response()
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), fake)
    }

    // ── Fixtures rapport, forme réelle du wire `/sandbox/src/health.sdd` +
    //    `/health/src/report.sdd` (enums snake_case, millisecondes unix) ──────

    fn check(id: &str, kind: &str, status: &str) -> Value {
        json!({
            "id": id, "kind": kind, "status": status,
            "exit_code": Some(i64::from(status != "passed")),
            "duration_ms": 50, "output_tail": "",
        })
    }

    fn coverage(found: u32, hit: u32) -> Value {
        json!({"files": [{
            "path": "src/main.rs", "lines_found": found, "lines_hit": hit,
            "uncovered": [{ "start": hit.saturating_add(1), "end": found }]
        }]})
    }

    fn health_state(phase: &str, computed_at: Option<u64>, report: Option<Value>) -> Value {
        json!({
            "phase": phase,
            "report": report.unwrap_or(Value::Null),
            "computed_at": computed_at,
            "stale_since": null,
            "base": null,
        })
    }

    fn report(
        checks: Vec<Value>,
        cov: Option<Value>,
        issues: Vec<Value>,
        unchecked: Vec<&str>,
    ) -> Value {
        // Construction par Map : les paramètres sont consommés (json! les emprunterait).
        let mut map = serde_json::Map::new();
        map.insert("revision".to_string(), json!("abc123"));
        map.insert("checks".to_string(), Value::Array(checks));
        map.insert("coverage".to_string(), cov.unwrap_or(Value::Null));
        map.insert("issues".to_string(), Value::Array(issues));
        map.insert("new_code".to_string(), Value::Null);
        map.insert(
            "unchecked_languages".to_string(),
            Value::Array(unchecked.into_iter().map(Value::from).collect()),
        );
        Value::Object(map)
    }

    /// `computed_at` franchement postérieur à toute demande de ce test (millisecondes futuristes).
    fn now_ms() -> u64 {
        chrono::Utc::now().timestamp_millis().cast_positive()
    }
    trait CastPositive {
        fn cast_positive(self) -> u64;
    }
    impl CastPositive for i64 {
        fn cast_positive(self) -> u64 {
            u64::try_from(self).unwrap_or(0)
        }
    }

    fn passed_report_85() -> Value {
        report(
            vec![check("cargo-test", "tests", "passed")],
            Some(coverage(20, 17)),
            vec![],
            vec![],
        )
    }

    fn fresh_85() -> Reply {
        (
            200,
            health_state(
                "fresh",
                Some(now_ms().saturating_add(60_000)),
                Some(passed_report_85()),
            ),
        )
    }

    async fn access_to(base: &str) -> SandboxAccess {
        let cfg = config_with_run(&run_ctx(Some("sbx-1")), serde_json::Map::new());
        let run = RunInfo::from_config(&cfg).unwrap();
        let locator = crate::workflow::steps::tests::FakeLocator::running("tok-1", base);
        SandboxAccess::open_with(locator, reqwest::Client::new(), &run)
            .await
            .unwrap()
    }

    fn cfg_of(map: serde_json::Map<String, Value>) -> GateConfig {
        let cfg = parse_config(&Value::Object(map)).expect("config de test valide");
        validate_config(&cfg).expect("config de test cohérente");
        cfg
    }

    // ── Scenario: gate_passante ─────────────────────────────────────────────
    // Rapport `passed`, couverture 85 %, `min_coverage = 80` ⇒ `passed == true`. Vérifie aussi
    // la forme complète de la sortie et le `base` transmis au refresh.
    #[tokio::test]
    async fn gate_passante() {
        let (base, fake) = spawn_health_fake(vec![fresh_85()], 202).await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::map::Map::from_iter([
            ("min_coverage".to_string(), json!(80)),
            ("base".to_string(), json!("origin/main")),
        ]));

        let out = execute(&access, &cfg).await.expect("gate passante");
        assert_eq!(out["passed"], true);
        assert_eq!(out["status"], "passed");
        assert_eq!(out["coverage_percent"], 85.0);
        assert_eq!(out["new_code_percent"], Value::Null);
        assert_eq!(out["issues"]["error"], 0);
        assert_eq!(out["checks"][0]["id"], "cargo-test");
        assert_eq!(out["checks"][0]["status"], "passed");
        // `Handles` : le `base` fourni part dans la demande de refresh.
        assert_eq!(
            fake.refresh_bodies.lock().unwrap()[0]["base"],
            "origin/main"
        );
    }

    // ── Scenario: couverture_insuffisante ───────────────────────────────────
    // 70 % contre un seuil de 80 ⇒ erreur non retryable dont les `reasons` mentionnent 70/80.
    #[tokio::test]
    async fn couverture_insuffisante() {
        let stale_report = report(
            vec![check("cargo-test", "tests", "passed")],
            Some(coverage(10, 7)),
            vec![],
            vec![],
        );
        let (base, _fake) = spawn_health_fake(
            vec![(
                200,
                health_state(
                    "fresh",
                    Some(now_ms().saturating_add(60_000)),
                    Some(stale_report),
                ),
            )],
            202,
        )
        .await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::map::Map::from_iter([(
            "min_coverage".to_string(),
            json!(80),
        )]));

        let err = execute(&access, &cfg)
            .await
            .expect_err("70 < 80 échoue la gate");
        assert!(
            !err.retryable,
            "échec de gate enforce ⇒ non retryable : {err}"
        );
        assert!(err.message.contains("70"), "la mesure : {err}");
        assert!(err.message.contains("80"), "le seuil : {err}");
    }

    // ── Scenario: seuil_desactive ───────────────────────────────────────────
    // `min_coverage` absent, couverture 10 % : la couverture n'est pas évaluée.
    #[tokio::test]
    async fn seuil_desactive() {
        let thin = report(
            vec![check("cargo-test", "tests", "passed")],
            Some(coverage(10, 1)),
            vec![],
            vec![],
        );
        let (base, _fake) = spawn_health_fake(
            vec![(
                200,
                health_state("fresh", Some(now_ms().saturating_add(60_000)), Some(thin)),
            )],
            202,
        )
        .await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::Map::new());
        let out = execute(&access, &cfg)
            .await
            .expect("aucun seuil de couverture ⇒ 10 % ignore");
        assert_eq!(out["passed"], true);
        assert_eq!(out["coverage_percent"], 10.0, "mesurée mais non évaluée");
    }

    // ── Scenario: mesure_absente_avec_seuil ─────────────────────────────────
    // Seuil 50 et rapport SANS couverture ⇒ échec « couverture non mesurée » (jamais 0, jamais ok).
    #[tokio::test]
    async fn mesure_absente_avec_seuil() {
        let nocov = report(
            vec![check("cargo-test", "tests", "passed")],
            None,
            vec![],
            vec![],
        );
        let (base, _fake) = spawn_health_fake(
            vec![(
                200,
                health_state("fresh", Some(now_ms().saturating_add(60_000)), Some(nocov)),
            )],
            202,
        )
        .await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::map::Map::from_iter([(
            "min_coverage".to_string(),
            json!(50),
        )]));

        let err = execute(&access, &cfg)
            .await
            .expect_err("aucune mesure avec seuil défini = échec");
        assert!(!err.retryable);
        assert!(
            err.message.contains("couverture non mesurée"),
            "reason contractuelle : {err}"
        );
    }

    // ── Scenario: nouveau_code_sans_base ────────────────────────────────────
    // `min_new_code_coverage` défini sans `base` ⇒ erreur de configuration non retryable, avant
    // tout appel.
    #[test]
    fn nouveau_code_sans_base() {
        let cfg = parse_config(&json!({"min_new_code_coverage": 80})).unwrap();
        let err =
            validate_config(&cfg).expect_err("seuil de code neuf sans base = config invalide");
        assert!(!err.retryable, "erreur de configuration : {err}");
        assert!(
            err.message.contains("base"),
            "le message nomme `base` : {err}"
        );
        // et la base rend la config cohérente (garde bidirectionnelle).
        let ok =
            parse_config(&json!({"min_new_code_coverage": 80, "base": "origin/main"})).unwrap();
        validate_config(&ok).expect("avec base, la config est valide");
    }

    // ── Scenario: etat_frais_exige ──────────────────────────────────────────
    // Un état `fresh` calculé AVANT la demande n'est pas accepté : le step attend un état dont
    // `computed_at` est postérieur, et évalue le rapport de CET état (marqueur : la couverture
    // du second rapport, 85, est celle évaluée, pas le 40 du premier).
    #[tokio::test]
    async fn etat_frais_exige() {
        let stale_but_fresh = (
            200,
            health_state(
                "fresh",
                Some(now_ms().saturating_sub(100_000)),
                Some(report(
                    vec![check("t", "tests", "passed")],
                    Some(coverage(10, 4)),
                    vec![],
                    vec![],
                )),
            ),
        );
        let (base, fake) = spawn_health_fake(vec![stale_but_fresh, fresh_85()], 202).await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::map::Map::from_iter([(
            "min_coverage".to_string(),
            json!(80),
        )]));

        let out = execute(&access, &cfg)
            .await
            .expect("le second état (postérieur à la demande) est évalué");
        assert_eq!(out["passed"], true);
        assert_eq!(
            out["coverage_percent"], 85.0,
            "c'est le rapport postérieur qui est évalué"
        );
        assert!(
            fake.get_calls.load(Ordering::SeqCst) >= 2,
            "le premier fresh périmé a été attendu sans évaluation"
        );
    }

    // ── Scenario: delai_depasse ─────────────────────────────────────────────
    // L'état reste `computing` au-delà de `wait_timeout_secs` ⇒ erreur non retryable.
    #[tokio::test]
    async fn delai_depasse() {
        let computing = (200, health_state("computing", None, None));
        let (base, _fake) = spawn_health_fake(vec![computing], 202).await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::map::Map::from_iter([(
            "wait_timeout_secs".to_string(),
            json!(1),
        )]));

        let err = execute(&access, &cfg)
            .await
            .expect_err("jamais frais ⇒ erreur");
        assert!(
            !err.retryable,
            "délai dépassé ⇒ rejouer rendrait le même : {err}"
        );
    }

    // ── Scenario: verification_ignoree ──────────────────────────────────────
    // `ts-coverage` en échec + `ignore_checks = ["ts-coverage"]` ⇒ son échec n'entre pas dans
    // le `status` évalué. Sens inverse verrouillé : sans ignore, la gate échoue.
    #[tokio::test]
    async fn verification_ignoree() {
        let mixed = report(
            vec![
                check("ts-coverage", "coverage", "failed"),
                check("cargo-test", "tests", "passed"),
            ],
            None,
            vec![],
            vec![],
        );
        let state = (
            200,
            health_state("fresh", Some(now_ms().saturating_add(60_000)), Some(mixed)),
        );

        let (base, _fake) = spawn_health_fake(vec![state.clone()], 202).await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::map::Map::from_iter([(
            "ignore_checks".to_string(),
            json!(["ts-coverage"]),
        )]));
        let out = execute(&access, &cfg)
            .await
            .expect("la vérification ignorée ne compte pas dans le status");
        assert_eq!(out["passed"], true);
        // la sortie rend bien l'état propre de chaque vérification.
        assert_eq!(out["checks"][0]["status"], "failed");
        assert_eq!(out["status"], "passed", "status évalué sans l'ignore");

        let (base2, _fake2) = spawn_health_fake(vec![state], 202).await;
        let access2 = access_to(&base2).await;
        let cfg2 = cfg_of(serde_json::Map::new());
        let err = execute(&access2, &cfg2)
            .await
            .expect_err("sans ignore, le failed échoue la gate");
        assert!(!err.retryable);
    }

    // ── Scenario: observation_seulement ─────────────────────────────────────
    // Gate non passante et `enforce: false` ⇒ le step réussit avec `passed == false`.
    #[tokio::test]
    async fn observation_seulement() {
        let failing = report(
            vec![check("clippy", "lint", "failed")],
            None,
            vec![],
            vec![],
        );
        let (base, _fake) = spawn_health_fake(
            vec![(
                200,
                health_state(
                    "fresh",
                    Some(now_ms().saturating_add(60_000)),
                    Some(failing),
                ),
            )],
            202,
        )
        .await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::map::Map::from_iter([(
            "enforce".to_string(),
            json!(false),
        )]));

        let out = execute(&access, &cfg)
            .await
            .expect("enforce=false : la sortie est rendue sans erreur");
        assert_eq!(out["passed"], false);
        assert_ne!(
            out["reasons"].as_array().unwrap().len(),
            0,
            "des reasons sont rendus"
        );
        assert_eq!(out["status"], "failed");
    }

    // ── Scenario: langage_non_verifie ───────────────────────────────────────
    // `fail_on_unchecked = true` et `unchecked_languages = ["go"]` ⇒ la gate échoue.
    #[tokio::test]
    async fn langage_non_verifie() {
        let unchecked = report(
            vec![check("cargo-test", "tests", "passed")],
            None,
            vec![],
            vec!["go"],
        );
        let state = (
            200,
            health_state(
                "fresh",
                Some(now_ms().saturating_add(60_000)),
                Some(unchecked),
            ),
        );

        let (base, _fake) = spawn_health_fake(vec![state.clone()], 202).await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::map::Map::from_iter([(
            "fail_on_unchecked".to_string(),
            json!(true),
        )]));
        let err = execute(&access, &cfg)
            .await
            .expect_err("go non vérifié avec fail_on_unchecked = échec");
        assert!(!err.retryable);
        assert!(
            err.message.contains("go"),
            "le message nomme le langage : {err}"
        );

        // Sans fail_on_unchecked, le langage non vérifié ne fait pas échouer (`gate.sdd` :
        // info pour le relecteur, sans effet sur `status()`).
        let (base2, _fake2) = spawn_health_fake(vec![state], 202).await;
        let access2 = access_to(&base2).await;
        let cfg2 = cfg_of(serde_json::Map::new());
        let out = execute(&access2, &cfg2)
            .await
            .expect("défaut fail_on_unchecked=false");
        assert_eq!(out["passed"], true);
        assert_eq!(out["unchecked_languages"][0], "go");
    }

    // Appoint `Handles` : une vérification `errored` rend la gate non passante — un vert ne
    // repose jamais sur une mesure absente.
    #[tokio::test]
    async fn verification_errored_non_passante() {
        let errored = report(
            vec![check("hadolint", "lint", "errored")],
            None,
            vec![],
            vec![],
        );
        let (base, _fake) = spawn_health_fake(
            vec![(
                200,
                health_state(
                    "fresh",
                    Some(now_ms().saturating_add(60_000)),
                    Some(errored),
                ),
            )],
            202,
        )
        .await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::Map::new());
        let err = execute(&access, &cfg)
            .await
            .expect_err("errored jamais passable");
        assert!(!err.retryable);
        assert!(
            err.message.contains("errored"),
            "l'état errored est nommé : {err}"
        );
    }

    // Appoint `Returns`/`Handles` : règle 4 (issues error > max_errors ⇒ échec ; `null`
    // désactive) et règle 3 (code neuf mesuré/absent).
    #[tokio::test]
    async fn seuil_issues_et_code_neuf() {
        let issue = json!({
            "check_id": "clippy", "path": "src/a.rs", "line": 3, "column": 1,
            "severity": "error", "code": "clippy::unwrap_used", "message": "used unwrap",
        });
        let warned = json!({
            "check_id": "clippy", "path": "src/b.rs", "line": 9, "column": 1,
            "severity": "warning", "code": null, "message": "unused",
        });
        let two_errors = report(
            vec![check("clippy", "lint", "failed")],
            None,
            vec![issue.clone(), issue.clone(), warned.clone()],
            vec![],
        );
        let state_err = (
            200,
            health_state(
                "fresh",
                Some(now_ms().saturating_add(60_000)),
                Some(two_errors.clone()),
            ),
        );

        // max_errors défaut 0 : deux issues error ⇒ échec, et le compte rendu dans la sortie.
        let (base, _fake) = spawn_health_fake(vec![state_err.clone()], 202).await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::Map::new());
        let err = execute(&access, &cfg)
            .await
            .expect_err("2 issues error > 0");
        assert!(!err.retryable);

        // max_errors = null désactive la règle 4 (mais le status failed reste un échec séparé).
        let cfg_null = cfg_of(serde_json::map::Map::from_iter([(
            "max_errors".to_string(),
            Value::Null,
        )]));
        let out_err = evaluate(
            &serde_json::from_value(two_errors.clone()).unwrap(),
            &cfg_null,
        );
        assert!(
            !out_err.0.iter().any(|r| r.contains("issue")),
            "règle 4 désactivée par null : {:?}",
            out_err.0
        );

        // règle 3 : code neuf mesuré sous le seuil ⇒ échec nommant la mesure.
        let with_new = {
            let mut r = two_errors.clone();
            r["new_code"] = json!({
                "base": "origin/main",
                "coverage": coverage(10, 3),
                "issues": [issue, warned],
            });
            r
        };
        let rep: HealthReport = serde_json::from_value(with_new).unwrap();
        let cfg_new = cfg_of(serde_json::map::Map::from_iter([
            ("min_new_code_coverage".to_string(), json!(80)),
            ("base".to_string(), json!("origin/main")),
            ("max_errors".to_string(), Value::Null),
        ]));
        let (reasons, out) = evaluate(&rep, &cfg_new);
        assert!(!reasons.is_empty(), "30 < 80 sur le code neuf ⇒ échec");
        assert_eq!(out["new_code_percent"], 30.0);
        assert_eq!(out["issues"]["error"], 2);
        assert_eq!(out["issues"]["warning"], 1);
    }

    // Appoint `Must` : le rapport lu est celui du crate — un wire non désérialisable est une
    // erreur non retryable (jamais un verdict par défaut).
    #[tokio::test]
    async fn refresh_refuse_est_non_retryable() {
        let (base, _fake) = spawn_health_fake(vec![fresh_85()], 400).await;
        let access = access_to(&base).await;
        let cfg = cfg_of(serde_json::Map::new());
        let err = execute(&access, &cfg)
            .await
            .expect_err("refresh 400 (base invalide) ⇒ refus non retryable");
        assert!(!err.retryable);
    }

    // ── Scenario: refuse_sans_sandbox ───────────────────────────────────────
    // Garde dans `SandboxAccess::open` (partagée avec `shell.sdd`).
    #[tokio::test]
    async fn refuse_sans_sandbox() {
        let cfg = config_with_run(&run_ctx(None), serde_json::Map::new());
        let info = RunInfo::from_config(&cfg).unwrap();
        let locator =
            crate::workflow::steps::tests::FakeLocator::running("tok", "http://127.0.0.1:1");
        let err = SandboxAccess::open_with(locator, reqwest::Client::new(), &info)
            .await
            .expect_err("la gate exige une sandbox");
        assert!(!err.retryable);
    }
}
