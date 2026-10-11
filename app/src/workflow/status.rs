//! Statut et annulation des runs, lus dans Restate — contrat : `status.sdd`.
//!
//! miryad-core ne fournit pas de lecture de statut ; ce module la fournit côté app via l'admin
//! API de Restate : introspection SQL (`POST {admin_url}/query`, tables `sys_invocation` et
//! `sys_journal`) pour les statuts, `PATCH {admin_url}/invocations/{id}/cancel` pour
//! l'annulation. Le statut n'est **jamais** copié en base. Une invocation que Restate ne connaît
//! plus (purgée) rend `RunState::Unknown`, jamais une erreur.
//!
//! L'interface attendue du runtime W2 est portée localement par le trait [`RestateAdmin`] :
//! `@WorkflowRuntime` (runtime.sdd, à venir) n'aura qu'à l'implémenter — URL d'admin et client
//! HTTP partagé.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use miryad_core::workflow::WorkflowError;
use serde::Deserialize;

/// Budget de chaque appel à l'admin Restate (`status.sdd` `Must`) : au-delà, erreur de
/// disponibilité (`MRD-WORKFLOW-001`).
const ADMIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Plafond d'identifiants par requête d'admin (`status.sdd` `Exposes` : au plus 50 par appel) ;
/// au-delà, la liste est découpée en requêtes groupées, jamais une requête par run.
const MAX_IDS_PER_QUERY: usize = 50;

/// Ce que `status.rs` attend du runtime (interface minimale définie localement en W1 ;
/// `@WorkflowRuntime` de runtime.sdd l'implémentera en W2) : l'URL d'admin et le client HTTP
/// partagé de l'app.
pub trait RestateAdmin: Send + Sync {
    /// URL de base de l'admin API Restate (sans `/` final imposé).
    fn admin_url(&self) -> &str;
    /// Client HTTP partagé de l'app (pool de connexions réutilisé).
    fn http(&self) -> &reqwest::Client;
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
    /// Entre deux tentatives après échec transitoire (`backing-off`).
    BackingOff,
    /// Terminé avec succès (`completed` + `completion_result = success`).
    Succeeded,
    /// Terminé en échec (`completed` + `completion_result = failure`, ou tué).
    Failed,
    /// Annulé (`cancelled`).
    Cancelled,
    /// Restate ne connaît plus l'invocation (purgée), ou statut méconnu.
    Unknown,
}

/// Statut d'un step connu du journal de l'invocation (`status.sdd` : quand Restate expose le
/// journal). La marche réelle des steps par miryad-core peut ne pas nommer chaque step dans le
/// journal de l'invocation : la liste est alors vide, jamais inventée.
#[derive(Clone, Debug, PartialEq)]
pub struct StepStatus {
    /// Identifiant du step, lu dans le `name` d'une entrée de journal.
    pub step_id: String,
    /// `Succeeded` si le journal marque l'entrée complétée, `Running` sinon.
    pub state: RunState,
    /// Horodatage de la première entrée du step, si le journal le porte.
    pub started_at: Option<DateTime<Utc>>,
    /// Horodatage de la dernière entrée si le step est complété, `None` sinon.
    pub ended_at: Option<DateTime<Utc>>,
    /// Message d'échec porté par l'invocation quand elle a échoué sur ce step, sinon `None`.
    pub error: Option<String>,
}

/// Statut d'un run, lu dans Restate (jamais en base).
#[derive(Clone, Debug, PartialEq)]
pub struct RunStatus {
    /// Identifiant d'invocation Restate demandé.
    pub invocation_id: String,
    /// État projeté (`RunState`) — `Unknown` si Restate ne connaît plus l'invocation.
    pub state: RunState,
    /// `created_at` de l'invocation, si connu.
    pub started_at: Option<DateTime<Utc>>,
    /// `completed_at` de l'invocation, si connu.
    pub ended_at: Option<DateTime<Utc>>,
    /// Message d'échec (`completion_failure`, à défaut `last_failure`) quand l'exécution a échoué.
    pub error: Option<String>,
    /// Steps connus du journal — vide si le journal ne les nomme pas.
    pub steps: Vec<StepStatus>,
}

/// Lit le statut de plusieurs runs en requêtes groupées : une requête `sys_invocation` par
/// tranche de 50 identifiants, jamais une par run. Un identifiant absent de Restate (purgé)
/// rend `Unknown`, sans erreur ; une indisponibilité de l'admin est une erreur, jamais une
/// liste vide.
///
/// # Errors
///
/// - `MRD-WORKFLOW-001` (`RestateUnreachable`) : admin injoignable ou délai de 5 s dépassé.
/// - `MRD-WORKFLOW-002` (`RestateRejected`) : l'admin répond hors `2xx` (par ex. `503`).
/// - `MRD-WORKFLOW-003` (`Serialization`) : réponse qui ne se décode pas.
pub async fn fetch_statuses(
    runtime: &dyn RestateAdmin,
    invocation_ids: &[String],
) -> Result<HashMap<String, RunStatus>, WorkflowError> {
    // Tout identifiant absent des lignes rendues (invocation purgée) reste `Unknown`.
    let mut statuts: HashMap<String, RunStatus> = invocation_ids
        .iter()
        .map(|id| (id.clone(), unknown_status(id)))
        .collect();
    for groupe in invocation_ids.chunks(MAX_IDS_PER_QUERY) {
        let in_liste = groupe
            .iter()
            .map(|id| quote_sql(id))
            .collect::<Vec<_>>()
            .join(", ");
        let requete = format!(
            "SELECT id, status, created_at, completed_at, completion_result, completion_failure, \
             last_failure FROM sys_invocation WHERE id IN ({in_liste})"
        );
        for ligne in query_rows::<InvocationRow>(runtime, &requete).await? {
            statuts.insert(ligne.id.clone(), run_status_from(ligne, Vec::new()));
        }
    }
    Ok(statuts)
}

/// Lit le statut détaillé d'un run, steps compris (`sys_journal`). Une invocation absente de
/// Restate — réponse sans ligne ou `404` de l'admin — rend `Unknown`, sans erreur.
///
/// # Errors
///
/// - `MRD-WORKFLOW-001` (`RestateUnreachable`) : admin injoignable ou délai de 5 s dépassé.
/// - `MRD-WORKFLOW-002` (`RestateRejected`) : l'admin répond hors `2xx` sauf `404`.
/// - `MRD-WORKFLOW-003` (`Serialization`) : réponse qui ne se décode pas.
pub async fn fetch_status(
    runtime: &dyn RestateAdmin,
    invocation_id: &str,
) -> Result<RunStatus, WorkflowError> {
    let requete = format!(
        "SELECT id, status, created_at, completed_at, completion_result, completion_failure, \
         last_failure FROM sys_invocation WHERE id = {}",
        quote_sql(invocation_id)
    );
    let lignes = match query_rows::<InvocationRow>(runtime, &requete).await {
        Ok(lignes) => lignes,
        // Un `404` de l'admin sur une lecture de statut est une invocation inconnue, pas une panne.
        Err(WorkflowError::RestateRejected { status: 404, .. }) => Vec::new(),
        Err(erreur) => return Err(erreur),
    };
    let Some(ligne) = lignes.into_iter().next() else {
        return Ok(unknown_status(invocation_id));
    };
    // Le détail des steps est best-effort : « quand Restate expose le journal ». Un journal
    // injoignable ou absent (`404`) rend une liste vide, jamais une erreur du statut.
    let requete_journal = format!(
        "SELECT name, completed, appended_at FROM sys_journal WHERE id = {}",
        quote_sql(invocation_id)
    );
    let etapes = match query_rows::<JournalRow>(runtime, &requete_journal).await {
        Ok(lignes) => derive_steps(lignes),
        Err(WorkflowError::RestateRejected { status: 404, .. }) => Vec::new(),
        Err(erreur) => return Err(erreur),
    };
    Ok(run_status_from(ligne, etapes))
}

/// Annule une invocation dans Restate (`PATCH /invocations/{id}/cancel`, non bloquant côté
/// Restate). L'appelant complète l'annulation côté sandbox (jobs shell) en best-effort.
///
/// # Errors
///
/// - `MRD-WORKFLOW-001` (`RestateUnreachable`) : admin injoignable ou délai de 5 s dépassé.
/// - `MRD-WORKFLOW-002` (`RestateRejected`) : l'admin répond hors `2xx`.
pub async fn cancel(runtime: &dyn RestateAdmin, invocation_id: &str) -> Result<(), WorkflowError> {
    let url = format!(
        "{}/invocations/{}/cancel",
        runtime.admin_url().trim_end_matches('/'),
        invocation_id
    );
    let reponse = runtime
        .http()
        .patch(url)
        .header("accept", "application/json")
        .timeout(ADMIN_TIMEOUT)
        .send()
        .await
        .map_err(classify_transport_error)?;
    let statut = reponse.status();
    if statut.is_success() {
        return Ok(());
    }
    let corps = reponse.text().await.map_err(classify_transport_error)?;
    Err(WorkflowError::RestateRejected {
        status: statut.as_u16(),
        body: corps,
    })
}

/// Projection du statut d'invocation Restate (`sys_invocation.status` + `completion_result`)
/// vers `RunState` — tableau contractuel de `status.sdd`, valeurs documentées de l'API
/// d'introspection (Restate 1.x : `pending`, `scheduled`, `ready`, `running`, `paused`,
/// `backing-off`, `suspended`, `completed` ; `cancelled`/`killed` rendus tels quels quand
/// l'admin les expose). Toute valeur méconnue rend `Unknown`, jamais une erreur.
fn state_from(status: &str, completion_result: Option<&str>) -> RunState {
    match (status, completion_result) {
        ("pending" | "scheduled" | "ready", _) => RunState::Pending,
        ("running", _) => RunState::Running,
        ("suspended" | "paused", _) => RunState::Suspended,
        ("backing-off", _) => RunState::BackingOff,
        ("completed", Some("failure")) | ("killed", _) => RunState::Failed,
        ("completed", _) => RunState::Succeeded,
        ("cancelled", _) => RunState::Cancelled,
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

/// Échappe un identifiant pour la prédicat `IN`/`=` SQL de l'introspection (simple
/// doubling des quotes — les identifiants Restate sont alphanumériques, le doublage est
/// une ceinture, pas un mécanisme d'échappement complet).
fn quote_sql(id: &str) -> String {
    format!("'{}'", id.replace('\'', "''"))
}

/// Range un `reqwest::Error` nu dans la taxonomie miryad-core : décodage/corps →
/// `Serialization`, transport → `RestateUnreachable` (même règle que
/// `classify_transport_error` de la crate, `pub(crate)` et donc inaccessible ici).
fn classify_transport_error(err: reqwest::Error) -> WorkflowError {
    if err.is_decode() || err.is_body() {
        WorkflowError::Serialization(err)
    } else {
        WorkflowError::RestateUnreachable(err)
    }
}

/// Exécute une requête d'introspection SQL (`POST {admin_url}/query`,
/// `accept: application/json`) et rend ses lignes. Le délai de 5 s borne chaque appel
/// (`status.sdd` `Must`) ; un hors-`2xx` devient `RestateRejected`, un échec de transport
/// ou de décodage est classé par `classify_transport_error`.
async fn query_rows<T>(runtime: &dyn RestateAdmin, requete: &str) -> Result<Vec<T>, WorkflowError>
where
    T: serde::de::DeserializeOwned,
{
    let url = format!("{}/query", runtime.admin_url().trim_end_matches('/'));
    let reponse = runtime
        .http()
        .post(url)
        .header("accept", "application/json")
        .timeout(ADMIN_TIMEOUT)
        .json(&serde_json::json!({ "query": requete }))
        .send()
        .await
        .map_err(classify_transport_error)?;
    let statut = reponse.status();
    if !statut.is_success() {
        let corps = reponse.text().await.map_err(classify_transport_error)?;
        return Err(WorkflowError::RestateRejected {
            status: statut.as_u16(),
            body: corps,
        });
    }
    let corpus: QueryResponse<T> = reponse.json().await.map_err(classify_transport_error)?;
    Ok(corpus.rows)
}

/// Enveloppe `{"rows":[…]}` observée de l'admin Restate sur `POST /query` en
/// `application/json` (schéma `openapi-admin` 1.7 : exemple `{"rows": []}`).
#[derive(Deserialize)]
struct QueryResponse<T> {
    rows: Vec<T>,
}

/// Une ligne de `sys_invocation` : les seules colonnes consommées par `RunStatus`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct InvocationRow {
    id: String,
    status: String,
    #[serde(deserialize_with = "de_opt_ts")]
    created_at: Option<DateTime<Utc>>,
    #[serde(deserialize_with = "de_opt_ts")]
    completed_at: Option<DateTime<Utc>>,
    completion_result: Option<String>,
    completion_failure: Option<String>,
    last_failure: Option<String>,
}

/// Une ligne de `sys_journal` limitée aux colonnes dont un step est déductible : le `name`
/// fourni par l'SDK, l'indicateur de complétion, l'horodatage d'entrée.
#[derive(Deserialize, Default)]
#[serde(default)]
struct JournalRow {
    name: Option<String>,
    completed: Option<bool>,
    #[serde(deserialize_with = "de_opt_ts")]
    appended_at: Option<DateTime<Utc>>,
}

/// Représentation brute d'un horodatage de l'admin : les colonnes
/// `TimestampMillisecond` de l'introspection sont rendues tantôt en chaîne RFC 3339,
/// tantôt en millisecondes depuis l'époque selon la version ; toute autre forme rend
/// `None` (jamais une erreur de décodage).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawTimestamp {
    Text(String),
    Millis(i64),
}

fn de_opt_ts<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let brute = Option::<RawTimestamp>::deserialize(deserializer)?;
    Ok(brute.and_then(|brute| match brute {
        RawTimestamp::Text(texte) => DateTime::parse_from_rfc3339(&texte)
            .ok()
            .map(|horodatage| horodatage.with_timezone(&Utc)),
        RawTimestamp::Millis(epoch) => {
            <chrono::Utc as chrono::TimeZone>::timestamp_millis_opt(&chrono::Utc, epoch).single()
        }
    }))
}

/// Assemble un `RunStatus` depuis une ligne d'invocation et les steps dérivés du journal.
fn run_status_from(ligne: InvocationRow, etapes: Vec<StepStatus>) -> RunStatus {
    let etat = state_from(&ligne.status, ligne.completion_result.as_deref());
    let erreur = if etat == RunState::Failed {
        ligne.completion_failure.clone().or(ligne.last_failure)
    } else {
        None
    };
    RunStatus {
        invocation_id: ligne.id,
        state: etat,
        started_at: ligne.created_at,
        ended_at: ligne.completed_at,
        error: erreur,
        steps: etapes,
    }
}

/// Dérive les steps connus d'un run depuis les entrées nommées de son journal, groupées par
/// `name` (ordre déterministe par identifiant) : un step est `Succeeded` dès qu'une de ses
/// entrées est marquée complétée, `Running` sinon. Le journal de miryad-core ne nomme pas
/// forcément chaque step : la liste rendue est alors vide, jamais inventée.
fn derive_steps(lignes: Vec<JournalRow>) -> Vec<StepStatus> {
    let mut groupes: BTreeMap<String, Vec<JournalRow>> = BTreeMap::new();
    for ligne in lignes {
        if let Some(nom) = ligne.name.clone() {
            groupes.entry(nom).or_default().push(ligne);
        }
    }
    groupes
        .into_iter()
        .map(|(step_id, lignes)| {
            let commence = lignes.iter().filter_map(|l| l.appended_at).min();
            let termine = lignes.iter().any(|l| l.completed == Some(true));
            StepStatus {
                step_id,
                state: if termine {
                    RunState::Succeeded
                } else {
                    RunState::Running
                },
                started_at: commence,
                ended_at: if termine {
                    lignes.iter().filter_map(|l| l.appended_at).max()
                } else {
                    None
                },
                error: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Faux admin Restate (motif des faux serveurs du crate : `TcpListener` port 0 + axum en
    /// tâche, cf. `api/sandboxes.rs`) : `POST /query` capture le SQL émis et rend des lignes
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
                        fake.captures.lock().unwrap().queries.push(body);
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
                            fake.captures.lock().unwrap().cancels.push(id);
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
        http: reqwest::Client,
    }

    impl RestateAdmin for TestRuntime {
        fn admin_url(&self) -> &str {
            &self.base
        }
        fn http(&self) -> &reqwest::Client {
            &self.http
        }
    }

    fn runtime(base: &str) -> TestRuntime {
        TestRuntime {
            base: base.to_string(),
            http: reqwest::Client::new(),
        }
    }

    fn invocation_row(id: &str, status: &str) -> String {
        format!(r#"{{"id":"{id}","status":"{status}","created_at":"2026-10-10T12:00:00Z"}}"#)
    }

    fn rows(items: &[String]) -> String {
        format!(r#"{{"rows":[{}]}}"#, items.join(","))
    }

    // ── Scenario: liste_groupee ─────────────────────────────────────────────
    // 50 identifiants, un faux admin Restate : UNE seule requête émise, 50 statuts rendus.
    #[tokio::test]
    async fn liste_groupee() {
        let ids: Vec<String> = (0..50).map(|i| format!("inv_{i}")).collect();
        let lignes: Vec<String> = ids.iter().map(|id| invocation_row(id, "running")).collect();
        let (base, captures) = spawn_fake_admin(&rows(&lignes), r#"{"rows":[]}"#, true).await;

        let statuts = fetch_statuses(&runtime(&base), &ids).await.unwrap();

        let vues = captures.lock().unwrap();
        assert_eq!(vues.queries.len(), 1, "une seule requête pour 50 ids");
        assert_eq!(statuts.len(), 50, "50 statuts rendus");
        for id in &ids {
            assert_eq!(statuts[id].state, RunState::Running, "statut de {id}");
        }
        assert!(
            vues.queries[0].contains("sys_invocation"),
            "la requête interroge la table d'invocations"
        );
    }

    // Découpage contractuel « au plus 50 par appel » : 60 ids rendent DEUX requêtes groupées,
    // jamais une par run, et les 60 statuts.
    #[tokio::test]
    async fn decoupage_au_plus_cinquante_par_requete() {
        let ids: Vec<String> = (0..60).map(|i| format!("inv_{i}")).collect();
        let lignes: Vec<String> = ids.iter().map(|id| invocation_row(id, "running")).collect();
        let (base, captures) = spawn_fake_admin(&rows(&lignes), r#"{"rows":[]}"#, true).await;

        let statuts = fetch_statuses(&runtime(&base), &ids).await.unwrap();

        let vues = captures.lock().unwrap();
        assert_eq!(vues.queries.len(), 2, "deux requêtes groupées pour 60 ids");
        assert_eq!(statuts.len(), 60);
    }

    // Liste vide : aucune requête émise. Garde immédiate (le squelette la satisfait déjà en
    // rouge) — signalée au rapport, conservée comme verrou de non-régression.
    #[tokio::test]
    async fn liste_vide_aucune_requete() {
        let (base, captures) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, true).await;

        let statuts = fetch_statuses(&runtime(&base), &[]).await.unwrap();

        assert!(statuts.is_empty());
        assert_eq!(
            captures.lock().unwrap().queries,
            Vec::<String>::new(),
            "aucune requête pour une liste vide"
        );
    }

    // ── Scenario: invocation_purgee ─────────────────────────────────────────
    // Un identifiant que Restate ne connaît plus rend `Unknown`, sans erreur.
    #[tokio::test]
    async fn invocation_purgee() {
        let (base, _captures) = spawn_fake_admin(
            &rows(&[invocation_row("inv_vivante", "running")]),
            r#"{"rows":[]}"#,
            true,
        )
        .await;

        let statuts = fetch_statuses(
            &runtime(&base),
            &["inv_vivante".to_string(), "inv_purgée".to_string()],
        )
        .await
        .unwrap();

        assert_eq!(statuts["inv_vivante"].state, RunState::Running);
        assert_eq!(
            statuts["inv_purgée"].state,
            RunState::Unknown,
            "un id absent des lignes rend Unknown, sans erreur"
        );
        assert_eq!(statuts["inv_purgée"].invocation_id, "inv_purgée");
    }

    // Le même contrat côté détail : une lecture sans ligne rend `Unknown`.
    #[tokio::test]
    async fn invocation_purgee_en_detail() {
        let (base, _captures) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, true).await;

        let statut = fetch_status(&runtime(&base), "inv_purgée").await.unwrap();

        assert_eq!(statut.state, RunState::Unknown);
    }

    // Un `404` de l'admin sur la lecture de statut rend aussi `Unknown` (jamais un faux succès).
    #[tokio::test]
    async fn invocation_inconnue_en_404_rend_unknown() {
        // Routeur vide : toute route répond 404.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, axum::Router::new()).await;
        });

        let statut = fetch_status(&runtime(&format!("http://{addr}")), "inv_x")
            .await
            .unwrap();

        assert_eq!(statut.state, RunState::Unknown);
    }

    // ── Scenario: restate_indisponible ──────────────────────────────────────
    // Un admin qui répond 503 : une erreur de disponibilité, pas une liste vide.
    #[tokio::test]
    async fn restate_indisponible_sur_503() {
        let (base, _captures) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, false).await;

        let res =
            fetch_statuses(&runtime(&base), &["inv_1".to_string(), "inv_2".to_string()]).await;

        match res {
            Err(WorkflowError::RestateRejected { status, .. }) => {
                assert_eq!(status, 503, "le 503 de l'admin est propagé");
            }
            Err(other) => panic!("attendu RestateRejected(503), rendu {other:?}"),
            Ok(_) => panic!("une indisponibilité n'est jamais une liste vide"),
        }
    }

    // ── Scenario: restate_indisponible (volet « ou ne répond pas ») ────────
    // Port refermé : erreur de disponibilité (MRD-WORKFLOW-001), pas une liste vide.
    #[tokio::test]
    async fn restate_indisponible_sans_reponse() {
        // Se lier à un port libre puis le relâcher : toute connexion suivante est refusée
        // (motif des tests de miryad-core `error.rs`).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let res = fetch_statuses(&runtime(&format!("http://{addr}")), &["inv_1".to_string()]).await;

        match res {
            Err(WorkflowError::RestateUnreachable(_)) => {}
            Err(other) => panic!("attendu RestateUnreachable, rendu {other:?}"),
            Ok(_) => panic!("une indisponibilité n'est jamais une liste vide"),
        }
    }

    // ── Scenario: detail_avec_steps ─────────────────────────────────────────
    // Un run en cours dont le step `b` est en cours : `state: Running` et un `StepStatus` par
    // step connu du journal.
    #[tokio::test]
    async fn detail_avec_steps() {
        let (base, _captures) = spawn_fake_admin(
            &rows(&[invocation_row("inv_run", "running")]),
            &rows(&[
                r#"{"entry_type":"StartSideEffect","name":"a","completed":true,"appended_at":"2026-10-10T12:01:00Z"}"#.to_string(),
                r#"{"entry_type":"StartSideEffect","name":"b","completed":false,"appended_at":"2026-10-10T12:02:00Z"}"#.to_string(),
            ]),
            true,
        )
        .await;

        let statut = fetch_status(&runtime(&base), "inv_run").await.unwrap();

        assert_eq!(statut.state, RunState::Running);
        assert_eq!(statut.invocation_id, "inv_run");
        assert_eq!(
            statut.started_at,
            Some(
                DateTime::parse_from_rfc3339("2026-10-10T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            ),
            "created_at de l'invocation"
        );
        assert_eq!(statut.error, None);
        assert_eq!(statut.steps.len(), 2, "un StepStatus par step connu");
        assert_eq!(statut.steps[0].step_id, "a");
        assert_eq!(statut.steps[0].state, RunState::Succeeded);
        assert_eq!(statut.steps[0].ended_at, statut.steps[0].started_at);
        assert_eq!(statut.steps[1].step_id, "b");
        assert_eq!(statut.steps[1].state, RunState::Running);
        assert_eq!(
            statut.steps[1].ended_at, None,
            "un step en cours n'a pas de fin"
        );
    }

    // Un run terminé en échec : `Failed` et le message de l'échec dans `error`.
    #[tokio::test]
    async fn statut_echoue_avec_message() {
        let (base, _captures) = spawn_fake_admin(
            &rows(&[
                r#"{"id":"inv_run","status":"completed","completion_result":"failure","completion_failure":"le step a a échoué","completed_at":"2026-10-10T12:05:00Z"}"#.to_string()
            ]),
            r#"{"rows":[]}"#,
            true,
        )
        .await;

        let statut = fetch_status(&runtime(&base), "inv_run").await.unwrap();

        assert_eq!(statut.state, RunState::Failed);
        assert_eq!(statut.error.as_deref(), Some("le step a a échoué"));
        assert_eq!(
            statut.ended_at,
            Some(
                DateTime::parse_from_rfc3339("2026-10-10T12:05:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            ),
            "completed_at de l'invocation"
        );
    }

    // Mapping contractuel exhaustive des statuts d'invocation Restate (`status.sdd`
    // `@RunState`) — tableau des valeurs documentées de l'introspection 1.x.
    #[test]
    fn mapping_etats_exhaustif() {
        let cas: &[(&str, Option<&str>, RunState)] = &[
            ("pending", None, RunState::Pending),
            ("scheduled", None, RunState::Pending),
            ("ready", None, RunState::Pending),
            ("running", None, RunState::Running),
            ("suspended", None, RunState::Suspended),
            ("paused", None, RunState::Suspended),
            ("backing-off", None, RunState::BackingOff),
            ("completed", Some("success"), RunState::Succeeded),
            ("completed", Some("failure"), RunState::Failed),
            ("cancelled", None, RunState::Cancelled),
            ("killed", None, RunState::Failed),
            ("quelque-chose-d-inconnu", None, RunState::Unknown),
            ("", None, RunState::Unknown),
        ];
        for (status, result, attendu) in cas {
            assert_eq!(
                state_from(status, *result),
                *attendu,
                "statut Restate {status:?} completion_result {result:?}"
            );
        }
    }

    // ── Scenario: annulation ────────────────────────────────────────────────
    // L'appel d'annulation est émis sur l'invocation (PATCH admin, non bloquant côté Restate).
    #[tokio::test]
    async fn annulation() {
        let (base, captures) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, true).await;

        cancel(&runtime(&base), "inv_a_annuler").await.unwrap();

        let vues = captures.lock().unwrap();
        assert_eq!(vues.cancels, vec!["inv_a_annuler".to_string()]);
    }

    // Annulation refusée par l'admin : l'erreur HTTP est propagée (`MRD-WORKFLOW-002`).
    #[tokio::test]
    async fn annulation_refusee_propage_une_erreur() {
        // Le faux admin n'offre pas la route en mode indisponible : le 503 est propagé.
        let (base, _captures) = spawn_fake_admin(r#"{"rows":[]}"#, r#"{"rows":[]}"#, false).await;

        let res = cancel(&runtime(&base), "inv_x").await;

        assert!(
            matches!(res, Err(WorkflowError::RestateRejected { status: 503, .. })),
            "le refus de l'admin est propagé, rendu {res:?}"
        );
    }
}
