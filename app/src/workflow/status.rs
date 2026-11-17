//! Statut et annulation des runs, lus dans Restate — contrat : `status.sdd`.
//!
//! Adaptateur du client d'administration Restate de miryad-core
//! (`miryad_core::workflow::client`, depuis 0.4.0) : les lectures (`invocation_statuses`,
//! `invocation_journal`) et l'annulation (`cancel_invocation`) sont celles de la crate ; ce
//! module les projette sur [`RunState`]/[`RunStatus`] et borne chaque appel à 5 s. Le statut
//! n'est **jamais** copié en base. Une invocation que Restate ne connaît plus (purgée) rend
//! `RunState::Unknown`, jamais une erreur.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use miryad_core::workflow::WorkflowConfig;
use miryad_core::workflow::WorkflowError;
use miryad_core::workflow::client::{self, InvocationStatus};

/// Budget de chaque appel à l'admin Restate (`status.sdd` `Must`) : au-delà, erreur de
/// disponibilité (`MRD-WORKFLOW-001`).
const ADMIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Ce que `status.rs` attend du runtime : la config miryad-core (URL d'admin comprise).
pub trait RestateAdmin: Send + Sync {
    /// Config Restate de l'application, au format de miryad-core.
    fn restate_config(&self) -> WorkflowConfig;
}

/// Client HTTP des appels d'admin, créé une fois, avec le délai de 5 s.
fn admin_http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(ADMIN_TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

/// État d'un run, projeté depuis le statut d'invocation de Restate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Acceptée, pas encore démarrée (`pending`, `scheduled`, `ready`).
    Pending,
    /// En cours d'exécution (`running`).
    Running,
    /// En pause ou en attente (`suspended`, `paused`).
    Suspended,
    /// Entre deux tentatives après échec transitoire (`backoff`).
    BackingOff,
    /// Terminé avec succès (`completed` sans échec).
    Succeeded,
    /// Terminé en échec (`completed` avec un échec).
    Failed,
    /// Annulé (`completed` avec un échec d'annulation).
    Cancelled,
    /// Restate ne connaît plus l'invocation (purgée), ou statut méconnu.
    Unknown,
}

/// Statut d'un step connu du journal de l'invocation : son `ctx.run` nommé `step:<id>` est
/// journalisé, donc terminé. Le journal ne porte ni dates ni message : ces champs restent `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct StepStatus {
    /// Identifiant du step (`<id>` de `step:<id>`).
    pub step_id: String,
    /// Toujours `Succeeded` (entrée journalisée = step terminé).
    pub state: RunState,
    /// Non porté par le journal.
    pub started_at: Option<DateTime<Utc>>,
    /// Non porté par le journal.
    pub ended_at: Option<DateTime<Utc>>,
    /// Non porté par le journal.
    pub error: Option<String>,
}

/// Statut d'un run, lu dans Restate (jamais en base).
#[derive(Clone, Debug, PartialEq)]
pub struct RunStatus {
    /// Identifiant d'invocation Restate demandé.
    pub invocation_id: String,
    /// État projeté (`RunState`) — `Unknown` si Restate ne connaît plus l'invocation.
    pub state: RunState,
    /// `created_at` de l'invocation, si lisible en RFC 3339.
    pub started_at: Option<DateTime<Utc>>,
    /// `completed_at` de l'invocation, si lisible en RFC 3339.
    pub ended_at: Option<DateTime<Utc>>,
    /// Échec terminal de l'invocation.
    pub error: Option<String>,
    /// Steps connus du journal — vide si le journal n'en nomme pas.
    pub steps: Vec<StepStatus>,
}

/// Lit le statut de plusieurs runs : une requête groupée par tranche de 50 identifiants. Un
/// identifiant absent de Restate (purgé) rend `Unknown`, sans erreur ; une indisponibilité de
/// l'admin est une erreur, jamais une liste vide.
///
/// # Errors
///
/// Erreurs de miryad-core : `MRD-WORKFLOW-001` (admin injoignable ou délai de 5 s),
/// `MRD-WORKFLOW-002` (réponse hors `2xx`), `MRD-WORKFLOW-007` (identifiant hors grammaire).
pub async fn fetch_statuses(
    runtime: &dyn RestateAdmin,
    invocation_ids: &[String],
) -> Result<HashMap<String, RunStatus>, WorkflowError> {
    let config = runtime.restate_config();
    let lues = client::invocation_statuses(&config, admin_http(), invocation_ids).await?;
    let mut statuts: HashMap<String, RunStatus> = invocation_ids
        .iter()
        .map(|id| (id.clone(), unknown_status(id)))
        .collect();
    for ligne in lues {
        statuts.insert(ligne.id.clone(), run_status_from(ligne, Vec::new()));
    }
    Ok(statuts)
}

/// Lit le statut détaillé d'un run, steps compris (journal). Une invocation absente de Restate
/// rend `Unknown`, sans erreur ; un journal introuvable (`404`) rend une liste de steps vide.
///
/// # Errors
///
/// Comme [`fetch_statuses`].
pub async fn fetch_status(
    runtime: &dyn RestateAdmin,
    invocation_id: &str,
) -> Result<RunStatus, WorkflowError> {
    let config = runtime.restate_config();
    let ids = [invocation_id.to_string()];
    let lues = client::invocation_statuses(&config, admin_http(), &ids).await?;
    let Some(ligne) = lues.into_iter().next() else {
        return Ok(unknown_status(invocation_id));
    };
    let etapes = match client::invocation_journal(&config, admin_http(), invocation_id).await {
        Ok(entrees) => derive_steps(&entrees),
        Err(WorkflowError::RestateRejected { status: 404, .. }) => Vec::new(),
        Err(erreur) => return Err(erreur),
    };
    Ok(run_status_from(ligne, etapes))
}

/// Annule une invocation dans Restate (idempotent côté miryad-core : inconnue ou déjà terminée
/// vaut succès). L'appelant complète l'annulation côté sandbox (jobs shell) en best-effort.
///
/// # Errors
///
/// Comme [`fetch_statuses`] (`001`, `002`, `007`).
pub async fn cancel(runtime: &dyn RestateAdmin, invocation_id: &str) -> Result<(), WorkflowError> {
    client::cancel_invocation(&runtime.restate_config(), admin_http(), invocation_id).await
}

/// Projection du statut Restate (`status` + texte d'échec) vers `RunState` — tableau
/// contractuel de `status.sdd`. Toute valeur méconnue rend `Unknown`, jamais une erreur.
fn state_from(status: &str, failure: Option<&str>) -> RunState {
    match status {
        "pending" | "scheduled" | "ready" => RunState::Pending,
        "running" => RunState::Running,
        "suspended" | "paused" => RunState::Suspended,
        "backoff" | "backing-off" => RunState::BackingOff,
        "completed" => match failure {
            None => RunState::Succeeded,
            Some(texte) if texte.to_lowercase().contains("cancel") => RunState::Cancelled,
            Some(_) => RunState::Failed,
        },
        _ => RunState::Unknown,
    }
}

fn unknown_status(invocation_id: &str) -> RunStatus {
    RunStatus {
        invocation_id: invocation_id.to_string(),
        state: RunState::Unknown,
        started_at: None,
        ended_at: None,
        error: None,
        steps: Vec::new(),
    }
}

/// Horodatage RFC 3339 de Restate ; toute autre forme rend `None`.
fn parse_ts(texte: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(texte)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Assemble un `RunStatus` depuis une invocation lue par miryad-core et les steps du journal.
fn run_status_from(ligne: InvocationStatus, etapes: Vec<StepStatus>) -> RunStatus {
    let etat = state_from(&ligne.status, ligne.failure.as_deref());
    let erreur = if matches!(etat, RunState::Failed | RunState::Cancelled) {
        ligne.failure
    } else {
        None
    };
    RunStatus {
        invocation_id: ligne.id,
        state: etat,
        started_at: parse_ts(&ligne.created_at),
        ended_at: ligne.completed_at.as_deref().and_then(parse_ts),
        error: erreur,
        steps: etapes,
    }
}

/// Steps du journal : les entrées nommées `step:<id>` (ordre du journal). Les entrées
/// `render:<id>`, sans nom ou d'un autre nom ne sont pas des steps.
fn derive_steps(entrees: &[client::JournalEntry]) -> Vec<StepStatus> {
    entrees
        .iter()
        .filter_map(|entree| entree.name.as_deref()?.strip_prefix("step:"))
        .map(|step_id| StepStatus {
            step_id: step_id.to_string(),
            state: RunState::Succeeded,
            started_at: None,
            ended_at: None,
            error: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Faux admin Restate aux formes de miryad-core 0.4.0 : `POST /query` capture le SQL et rend
    /// `{"rows":[…]}` selon la table interrogée ; `PATCH /invocations/{id}/cancel` capture l'id.
    /// `disponible=false` rend `503` sur toute requête.
    #[derive(Clone)]
    struct FakeAdmin {
        invocations_rows: Arc<String>,
        journal_rows: Arc<String>,
        disponible: bool,
        captures: Arc<Mutex<Captures>>,
    }

    #[derive(Default)]
    struct Captures {
        queries: Vec<String>,
        cancels: Vec<String>,
    }

    async fn spawn_fake_admin(
        invocations_rows: &str,
        journal_rows: &str,
        disponible: bool,
    ) -> (String, Arc<Mutex<Captures>>) {
        let captures = Arc::new(Mutex::new(Captures::default()));
        let fake = FakeAdmin {
            invocations_rows: Arc::new(invocations_rows.to_string()),
            journal_rows: Arc::new(journal_rows.to_string()),
            disponible,
            captures: captures.clone(),
        };
        let fake_cancel = fake.clone();
        let app = axum::Router::new()
            .route(
                "/query",
                axum::routing::post(move |body: String| {
                    let fake = fake.clone();
                    async move {
                        if !fake.disponible {
                            return (
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                [(axum::http::header::CONTENT_TYPE, "application/json")],
                                r#"{"message":"query service not available"}"#.to_string(),
                            );
                        }
                        let table = if body.contains("sys_journal") {
                            fake.journal_rows.as_str().to_string()
                        } else {
                            fake.invocations_rows.as_str().to_string()
                        };
                        if let Ok(mut vues) = fake.captures.lock() {
                            vues.queries.push(body);
                        }
                        (
                            axum::http::StatusCode::OK,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            table,
                        )
                    }
                }),
            )
            .route(
                "/invocations/{id}/cancel",
                axum::routing::patch(
                    move |axum::extract::Path(id): axum::extract::Path<String>| {
                        let fake = fake_cancel.clone();
                        async move {
                            if !fake.disponible {
                                return axum::http::StatusCode::SERVICE_UNAVAILABLE;
                            }
                            if let Ok(mut vues) = fake.captures.lock() {
                                vues.cancels.push(id);
                            }
                            axum::http::StatusCode::ACCEPTED
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), captures)
    }

    struct TestRuntime {
        base: String,
    }

    impl RestateAdmin for TestRuntime {
        fn restate_config(&self) -> WorkflowConfig {
            WorkflowConfig {
                admin_url: self.base.clone(),
                ingress_url: self.base.clone(),
                deployment_url: String::new(),
            }
        }
    }

    fn runtime(base: &str) -> TestRuntime {
        TestRuntime {
            base: base.to_string(),
        }
    }

    /// Ligne `sys_invocation` aux colonnes que lit miryad-core.
    fn invocation_row(id: &str, status: &str, failure: Option<&str>) -> String {
        let failure = failure.map_or_else(|| "null".to_string(), |f| format!("\"{f}\""));
        format!(
            r#"{{"id":"{id}","status":"{status}","target_service_name":"DagInterpreter","target_handler_name":"run","target_service_key":"k","created_at":"2026-10-10T12:00:00Z","completed_at":null,"retry_count":0,"completion_failure":{failure}}}"#
        )
    }

    fn rows(items: &[String]) -> String {
        format!(r#"{{"rows":[{}]}}"#, items.join(","))
    }

    fn nb_requetes(captures: &Arc<Mutex<Captures>>) -> usize {
        captures.lock().map_or(0, |c| c.queries.len())
    }

    // ── Scenario: liste_groupee ─────────────────────────────────────────────
    #[tokio::test]
    async fn liste_groupee() {
        let ids: Vec<String> = (0..50).map(|i| format!("inv_a{i}")).collect();
        let lignes: Vec<String> = ids
            .iter()
            .map(|id| invocation_row(id, "running", None))
            .collect();
        let (base, captures) = spawn_fake_admin(&rows(&lignes), r#"{"rows":[]}"#, true).await;

        let statuts = fetch_statuses(&runtime(&base), &ids).await.unwrap();

        assert_eq!(nb_requetes(&captures), 1, "une seule requête pour 50 ids");
        assert_eq!(statuts.len(), 50);
        for id in &ids {
            assert_eq!(statuts[id].state, RunState::Running, "statut de {id}");
        }
    }

    #[tokio::test]
    async fn decoupage_au_plus_cinquante_par_requete() {
        let ids: Vec<String> = (0..60).map(|i| format!("inv_b{i}")).collect();
        let lignes: Vec<String> = ids
            .iter()
            .map(|id| invocation_row(id, "running", None))
            .collect();
        let (base, captures) = spawn_fake_admin(&rows(&lignes), r#"{"rows":[]}"#, true).await;
        let statuts = fetch_statuses(&runtime(&base), &ids).await.unwrap();
        assert_eq!(nb_requetes(&captures), 2);
        assert_eq!(statuts.len(), 60);
    }

    #[tokio::test]
    async fn liste_vide_aucune_requete() {
        let (base, captures) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, true).await;
        let statuts = fetch_statuses(&runtime(&base), &[]).await.unwrap();
        assert!(statuts.is_empty());
        assert_eq!(nb_requetes(&captures), 0);
    }

    // ── Scenario: invocation_purgee ─────────────────────────────────────────
    #[tokio::test]
    async fn invocation_purgee() {
        let (base, _) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, true).await;
        let ids = vec!["inv_gone".to_string()];
        let statuts = fetch_statuses(&runtime(&base), &ids).await.unwrap();
        assert_eq!(statuts["inv_gone"].state, RunState::Unknown);
    }

    #[tokio::test]
    async fn invocation_purgee_en_detail() {
        let (base, _) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, true).await;
        let statut = fetch_status(&runtime(&base), "inv_gone").await.unwrap();
        assert_eq!(statut.state, RunState::Unknown);
        assert!(statut.steps.is_empty());
    }

    // ── Scenario: restate_indisponible ──────────────────────────────────────
    #[tokio::test]
    async fn restate_indisponible_sur_503() {
        let (base, _) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, false).await;
        let ids = vec!["inv_x".to_string()];
        let erreur = fetch_statuses(&runtime(&base), &ids).await.unwrap_err();
        assert!(matches!(
            erreur,
            WorkflowError::RestateRejected { status: 503, .. }
        ));
    }

    #[tokio::test]
    async fn restate_indisponible_sans_reponse() {
        let ids = vec!["inv_x".to_string()];
        let erreur = fetch_statuses(&runtime("http://127.0.0.1:1"), &ids)
            .await
            .unwrap_err();
        assert!(matches!(erreur, WorkflowError::RestateUnreachable(_)));
    }

    // ── Scenario: detail_avec_steps ─────────────────────────────────────────
    #[tokio::test]
    async fn detail_avec_steps() {
        let journal = r#"{"rows":[
            {"index":0,"entry_type":"InvocationStart","name":""},
            {"index":1,"entry_type":"Run","name":"step:a"},
            {"index":2,"entry_type":"Run","name":"render:a"},
            {"index":3,"entry_type":"Sleep","name":""}
        ]}"#;
        let (base, _) = spawn_fake_admin(
            &rows(&[invocation_row("inv_live", "running", None)]),
            journal,
            true,
        )
        .await;
        let statut = fetch_status(&runtime(&base), "inv_live").await.unwrap();
        assert_eq!(statut.state, RunState::Running);
        assert_eq!(
            statut.steps,
            vec![StepStatus {
                step_id: "a".to_string(),
                state: RunState::Succeeded,
                started_at: None,
                ended_at: None,
                error: None,
            }]
        );
        assert!(statut.started_at.is_some());
    }

    #[tokio::test]
    async fn statut_echoue_avec_message() {
        let (base, _) = spawn_fake_admin(
            &rows(&[invocation_row("inv_ko", "completed", Some("boom"))]),
            r#"{"rows":[]}"#,
            true,
        )
        .await;
        let statut = fetch_status(&runtime(&base), "inv_ko").await.unwrap();
        assert_eq!(statut.state, RunState::Failed);
        assert_eq!(statut.error.as_deref(), Some("boom"));
    }

    // ── Scenario: etats_projetes ────────────────────────────────────────────
    #[test]
    fn etats_projetes() {
        let cas = [
            ("pending", None, RunState::Pending),
            ("scheduled", None, RunState::Pending),
            ("ready", None, RunState::Pending),
            ("running", None, RunState::Running),
            ("suspended", None, RunState::Suspended),
            ("paused", None, RunState::Suspended),
            ("backoff", None, RunState::BackingOff),
            ("backing-off", None, RunState::BackingOff),
            ("completed", None, RunState::Succeeded),
            ("completed", Some("[409] Cancelled"), RunState::Cancelled),
            ("completed", Some("boom"), RunState::Failed),
            ("inconnu", None, RunState::Unknown),
        ];
        for (status, failure, attendu) in cas {
            assert_eq!(state_from(status, failure), attendu, "{status} {failure:?}");
        }
    }

    // ── Scenario: identifiant_hors_grammaire ────────────────────────────────
    #[tokio::test]
    async fn identifiant_hors_grammaire() {
        let (base, captures) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, true).await;
        let ids = vec!["inv_a_b".to_string()];
        let erreur = fetch_statuses(&runtime(&base), &ids).await.unwrap_err();
        assert!(matches!(erreur, WorkflowError::InvalidConfig(_)));
        let erreur = cancel(&runtime(&base), "inv_a_b").await.unwrap_err();
        assert!(matches!(erreur, WorkflowError::InvalidConfig(_)));
        assert_eq!(nb_requetes(&captures), 0);
    }

    // ── Scenario: annulation ────────────────────────────────────────────────
    #[tokio::test]
    async fn annulation() {
        let (base, captures) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, true).await;
        cancel(&runtime(&base), "inv_live").await.unwrap();
        let vues = captures.lock().unwrap();
        assert_eq!(vues.cancels, vec!["inv_live".to_string()]);
    }

    #[tokio::test]
    async fn annulation_refusee_propage_une_erreur() {
        let (base, _) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, false).await;
        let erreur = cancel(&runtime(&base), "inv_live").await.unwrap_err();
        assert!(matches!(
            erreur,
            WorkflowError::RestateRejected { status: 503, .. }
        ));
    }
}
