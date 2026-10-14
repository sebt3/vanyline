//! Routes REST des workflows de projet (CRUD des définitions) — contrat : `./workflows.sdd`.
//!
//! LOT W4a : `GET/POST /projects/{project}/workflows`, `GET/PUT/DELETE …/{id}`, 503 moteur,
//! propriété (404 identique inexistant/autre). Les routes de RUNS et la fonction de service
//! `launch_workflow` (lot W4b) vivront dans les sections « runs » en bas de ce fichier ;
//! les helpers de propriété ([`owner_context`], [`load_owned`]) et la garde moteur
//! ([`engine`]) sont déjà factorisés pour elles.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use miryad_core::auth::{AuthPrincipal, AuthUser, PrincipalSource};
use miryad_core::resource::MiryadResource;
use miryad_core::users::resolve_user;
use miryad_core::workflow::{
    DagSteps, StepDefinition, WorkflowDefinition, WorkflowDefinitionActiveModel,
    WorkflowDefinitionColumn, WorkflowError,
};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ActiveValue::Set, ColumnTrait, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use vanyline_lib::k8s::VnlK8sClient;

use crate::db::entities::project_workflows::{
    Column as ProjectWorkflowColumn, Entity as ProjectWorkflow, Model as ProjectWorkflowModel,
};
use crate::db::entities::workflow_runs::{
    Column as WorkflowRunColumn, Entity as WorkflowRun, Model as WorkflowRunModel,
};
use crate::workflow::WorkflowRuntime;
use crate::workflow::run_context::{RunContext, RunScope, allowed_kinds, materialize};
use crate::workflow::status::{self, RestateAdmin, RunState, RunStatus};
use crate::workflow::steps::{K8sLocator, RunInfo, SandboxAccess};
use crate::{AppState, api::owners, error::AppError};

// ---------------------------------------------------------------------------
// Erreurs — variantes dédiées AppError (famille VNL-WFA-* de ../error.sdd, amendée
// post-validation W4a : le type d'erreur du binaire est UNIQUE et ./api/workflows.rs
// n'implante jamais @IntoResponse localement). Les erreurs internes du module
// réutilisent @AppError::InternalError (VNL-INT-001, pas de VNL-WFA-009).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Corps et vues JSON
// ---------------------------------------------------------------------------

/// Corps de `POST /projects/{project}/workflows` (spec `Exposes`).
#[derive(Deserialize)]
pub struct CreateWorkflowBody {
    pub name: String,
    pub scope: String,
    #[serde(default)]
    pub description: Option<String>,
    pub steps: Vec<StepDefinition>,
}

/// Corps de `PUT /projects/{project}/workflows/{id}` : `{ description?, steps? }`.
/// La clé `scope` est CAPTÉE (et refusée 400) : la portée n'est pas modifiable.
#[derive(Deserialize)]
pub struct UpdateWorkflowBody {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub steps: Option<Vec<StepDefinition>>,
    #[serde(default)]
    pub scope: Option<serde_json::Value>,
}

/// Détail d'un workflow (`GET …/{id}`, réponse 201 du `POST`) : `steps` inclus.
#[derive(Debug, Serialize)]
pub struct WorkflowDetail {
    pub id: i32,
    pub name: String,
    pub scope: String,
    pub description: Option<String>,
    pub steps: Vec<StepDefinition>,
}

/// Élément de la liste `GET /projects/{project}/workflows`.
#[derive(Debug, Serialize)]
pub struct WorkflowListItem {
    pub id: i32,
    pub name: String,
    pub scope: String,
    pub description: Option<String>,
    pub kinds: Vec<String>,
    pub steps_count: usize,
}

// ---------------------------------------------------------------------------
// Helpers de propriété et de moteur — factorisés pour le lot W4b (runs)
// ---------------------------------------------------------------------------

/// Garde moteur commune aux routes de CE fichier (`workflows.sdd` `Must not` : « Exécuter
/// quoi que ce soit si le moteur n'est pas configuré » — y compris le CRUD qui ne parle pas
/// à Restate). Premier appel de chaque handler, AVANT toute lecture de base. Le lot W4b
/// (`runs`) réutilise ce garde et consomme le handle retourné.
fn engine(state: &AppState) -> Result<&WorkflowRuntime, AppError> {
    state
        .workflow
        .as_ref()
        .ok_or(AppError::WorkflowEngineNotConfigured)
}

/// Contexte de propriété de l'utilisateur authentifié : (clé primaire miryad de
/// l'utilisateur, nom de son Owner K8s). `None` = aucun Owner lié (l'appelant décide :
/// liste vide pour `GET liste`, 404 pour les routes d'objet — tranché, cf. rapport).
///
/// Même scoping que `./sandboxes.rs` : résolution miryad-core de l'utilisateur puis
/// `vanyline_owner_links`. `resolve_user` CRÉE la ligne utilisateur à la volée (contrat
/// miryad-core), identique aux autres routes de l'app.
async fn owner_context(
    state: &AppState,
    user: &AuthUser,
) -> Result<Option<(i32, String)>, AppError> {
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref()).await?;
    let owner = owners::resolve_owner_name(state, principal_user.id).await?;
    Ok(owner.map(|name| (principal_user.id, name)))
}

/// Charge la paire (rattachement, définition) du workflow `id` du projet `project`,
/// POSÉDÉE par `user_id`. Rattachement absent, hors projet, ou appartenant à un autre
/// utilisateur ⇒ [`AppError::WorkflowNotFound`] — 404 identique, aucune fuite d'existence
/// (`workflows.sdd` `Must`). Une définition référencée absente (incohérence que la FK
/// `CASCADE` du schéma produit ne laisse pas exister) est une erreur interne.
async fn load_owned(
    db: &sea_orm::DatabaseConnection,
    user_id: i32,
    project: &str,
    id: i32,
) -> Result<
    (
        ProjectWorkflowModel,
        miryad_core::workflow::WorkflowDefinitionModel,
    ),
    AppError,
> {
    let attach = ProjectWorkflow::find()
        .filter(ProjectWorkflowColumn::Id.eq(id))
        .filter(ProjectWorkflowColumn::OwnerId.eq(user_id))
        .filter(ProjectWorkflowColumn::ProjectName.eq(project))
        .one(db)
        .await?
        .ok_or(AppError::WorkflowNotFound)?;
    let definition = WorkflowDefinition::find_by_id(attach.definition_id)
        .one(db)
        .await?
        .ok_or_else(|| {
            AppError::InternalError(format!(
                "VNL-WFA-010: rattachement {id} référence la définition absente {}",
                attach.definition_id
            ))
        })?;
    Ok((attach, definition))
}

/// Portée textuelle → [`RunScope`] ; portée inconnue ⇒ 400 (`Raises` : « portée inconnue »).
fn parse_scope(scope: &str) -> Result<RunScope, AppError> {
    match scope {
        "project" => Ok(RunScope::Project),
        "sandbox" => Ok(RunScope::Sandbox),
        other => Err(AppError::WorkflowUnknownScope(other.to_string())),
    }
}

/// Validation structurelle du DAG par le hook `before_create` de la ressource
/// miryad-core (seule porte publique vers `validate_dag`, `pub(crate)` dans la crate —
/// vérifié 0.1.5) : `WorkflowError::InvalidDag` → `HookError` sans code dont le message
/// est rendu 400 sous `MRD-WORKFLOW-004` (`Raises`). Le principal construit ici n'est
/// jamais lu par le hook (signature d'interface).
fn validate_dag(steps: &[StepDefinition], user: &AuthUser) -> Result<(), AppError> {
    let active = WorkflowDefinitionActiveModel {
        id: NotSet,
        name: Set(String::new()),
        steps: Set(DagSteps(steps.to_vec())),
        owner_id: NotSet,
        created_at: NotSet,
    };
    let principal = AuthPrincipal {
        subject: user.subject.clone(),
        email: user.email.clone(),
        preferred_username: None,
        source: PrincipalSource::ApiToken { token_id: 0 },
    };
    WorkflowDefinition::before_create(active, &principal)
        .map_err(|hook| AppError::WorkflowDagInvalid(hook.message))?;
    Ok(())
}

/// Kinds autorisés pour la portée (`run_context.sdd` `allowed_kinds`) : un step hors liste
/// ⇒ 422 nommant le step et le kind. À la création ET à chaque écriture de steps (le `Must`
/// de `workflows.sdd` est refait au lancement en W4b).
fn validate_kinds(steps: &[StepDefinition], scope: RunScope) -> Result<(), AppError> {
    let autorises = allowed_kinds(scope);
    for etape in steps {
        if !autorises.contains(&etape.kind.as_str()) {
            return Err(AppError::WorkflowKindForbidden(format!(
                "step {} : kind « {} » non autorisé pour la portée {} (autorisés : {})",
                etape.id,
                etape.kind,
                etape_kind_portee(scope),
                autorises.join(", ")
            )));
        }
    }
    Ok(())
}

fn etape_kind_portee(scope: RunScope) -> &'static str {
    match scope {
        RunScope::Project => "projet",
        RunScope::Sandbox => "sandbox",
    }
}

/// Nom complet de la définition : `{owner}/{project}/{name}` (`workflows.sdd` `Handles` —
/// l'unicité `name` de miryad-core est globale par propriétaire, le préfixe la garantit
/// entre utilisateurs et projets). L'API ne rend jamais que `{name}`.
fn full_name(owner: &str, project: &str, name: &str) -> String {
    format!("{owner}/{project}/{name}")
}

/// Nom nu rendu par l'API : préfixe `{owner}/{project}/` retiré. Une définition créée hors
/// de cette API (hors préfixe) est rendue sous son nom complet, sans inventer de nom.
fn bare_name(stored: &str, owner: &str, project: &str) -> String {
    stored
        .strip_prefix(&format!("{owner}/{project}/"))
        .unwrap_or(stored)
        .to_string()
}

/// Kinds distincts du DAG, dans l'ordre de première apparition (vue de liste).
fn distinct_kinds(steps: &[StepDefinition]) -> Vec<String> {
    let mut kinds: Vec<String> = Vec::new();
    for etape in steps {
        if !kinds.contains(&etape.kind) {
            kinds.push(etape.kind.clone());
        }
    }
    kinds
}

fn detail(
    attach: &ProjectWorkflowModel,
    definition: &miryad_core::workflow::WorkflowDefinitionModel,
    owner: &str,
    project: &str,
) -> WorkflowDetail {
    WorkflowDetail {
        id: attach.id,
        name: bare_name(&definition.name, owner, project),
        scope: attach.scope.clone(),
        description: attach.description.clone(),
        steps: definition.steps.0.clone(),
    }
}

// ---------------------------------------------------------------------------
// Handlers CRUD (lot W4a)
// ---------------------------------------------------------------------------

/// `GET /projects/{project}/workflows` — liste des rattachements du projet avec les kinds
/// et le nombre de steps lus dans la définition stockée. Aucun Owner ⇒ liste vide (200),
/// même motif que `projects::list_projects`.
pub async fn list_workflows(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project): Path<String>,
) -> Result<Json<Vec<WorkflowListItem>>, AppError> {
    engine(&state)?;
    let Some((user_id, owner)) = owner_context(&state, &user).await? else {
        return Ok(Json(Vec::new()));
    };
    let db = &state.auth.db;
    let attachs = ProjectWorkflow::find()
        .filter(ProjectWorkflowColumn::OwnerId.eq(user_id))
        .filter(ProjectWorkflowColumn::ProjectName.eq(&project))
        .order_by(ProjectWorkflowColumn::Id, sea_orm::Order::Asc)
        .all(db)
        .await?;
    let definitions = WorkflowDefinition::find()
        .filter(WorkflowDefinitionColumn::Id.is_in(attachs.iter().map(|a| a.definition_id)))
        .all(db)
        .await?;
    let mut items = Vec::with_capacity(attachs.len());
    for attach in &attachs {
        let definition = definitions
            .iter()
            .find(|d| d.id == attach.definition_id)
            .ok_or_else(|| {
                AppError::InternalError(format!(
                    "VNL-WFA-010: rattachement {} référence la définition absente {}",
                    attach.id, attach.definition_id
                ))
            })?;
        items.push(WorkflowListItem {
            id: attach.id,
            name: bare_name(&definition.name, &owner, &project),
            scope: attach.scope.clone(),
            description: attach.description.clone(),
            kinds: distinct_kinds(&definition.steps.0),
            steps_count: definition.steps.0.len(),
        });
    }
    Ok(Json(items))
}

/// `POST /projects/{project}/workflows` — 201. Validation (portée, `validate_dag`,
/// kinds) AVANT toute écriture, puis définition + rattachement dans UNE transaction
/// (`Scenario: creation_atomique`). Le service effectif est [`create_workflow_db`].
pub async fn create_workflow(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project): Path<String>,
    Json(body): Json<CreateWorkflowBody>,
) -> Result<(StatusCode, Json<WorkflowDetail>), AppError> {
    engine(&state)?;
    let (user_id, owner) = owner_context(&state, &user)
        .await?
        .ok_or(AppError::WorkflowNotFound)?;
    let detail = create_workflow_db(&state.auth.db, &user, user_id, &owner, &project, body).await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

/// Service de création — commun à la route et aux appelants internes (MCP de domaine) :
/// `user` porte l'identité pour le hook de validation, `user_id`/`owner` sont les
/// composantes de propriété déjà résolues. Toute écriture est dans la transaction.
pub async fn create_workflow_db(
    db: &sea_orm::DatabaseConnection,
    user: &AuthUser,
    user_id: i32,
    owner: &str,
    project: &str,
    body: CreateWorkflowBody,
) -> Result<WorkflowDetail, AppError> {
    let scope = parse_scope(&body.scope)?;
    validate_dag(&body.steps, user)?;
    validate_kinds(&body.steps, scope)?;

    let stored_name = full_name(owner, project, &body.name);
    let txn = db.begin().await?;
    let taken = WorkflowDefinition::find()
        .filter(WorkflowDefinitionColumn::OwnerId.eq(user_id))
        .filter(WorkflowDefinitionColumn::Name.eq(&stored_name))
        .one(&txn)
        .await?
        .is_some();
    if taken {
        return Err(AppError::WorkflowNameTaken(body.name.clone()));
    }
    let definition = WorkflowDefinitionActiveModel {
        id: NotSet,
        name: Set(stored_name),
        steps: Set(DagSteps(body.steps.clone())),
        owner_id: Set(Some(user_id)),
        created_at: Set(chrono::Utc::now()),
    }
    .insert(&txn)
    .await?;
    let attach_id = ProjectWorkflow::insert(crate::db::entities::project_workflows::ActiveModel {
        id: NotSet,
        owner_id: Set(user_id),
        project_name: Set(project.to_string()),
        definition_id: Set(definition.id),
        scope: Set(body.scope.clone()),
        description: Set(body.description.clone()),
    })
    .exec(&txn)
    .await?
    .last_insert_id;
    txn.commit().await?;

    let attach = ProjectWorkflowModel {
        id: attach_id,
        owner_id: user_id,
        project_name: project.to_string(),
        definition_id: definition.id,
        scope: body.scope.clone(),
        description: body.description.clone(),
    };
    Ok(detail(&attach, &definition, owner, project))
}

/// `GET /projects/{project}/workflows/{id}` — détail avec `steps` ; 404 identique
/// inexistant / à un autre.
pub async fn get_workflow(
    State(state): State<AppState>,
    user: AuthUser,
    Path((project, id)): Path<(String, i32)>,
) -> Result<Json<WorkflowDetail>, AppError> {
    engine(&state)?;
    let (user_id, owner) = owner_context(&state, &user)
        .await?
        .ok_or(AppError::WorkflowNotFound)?;
    let (attach, definition) = load_owned(&state.auth.db, user_id, &project, id).await?;
    Ok(Json(detail(&attach, &definition, &owner, &project)))
}

/// `PUT /projects/{project}/workflows/{id}` — `{ description?, steps? }`. La portée n'est
/// PAS modifiable : toute clé `scope` dans le corps est rejetée 400 (tranche signalée —
/// rejet explicite plutôt qu'ignorée silencieusement). Les `steps` fournis sont
/// revalidés (`validate_dag` puis kinds contre la portée STOCKÉE) avant toute écriture.
pub async fn update_workflow(
    State(state): State<AppState>,
    user: AuthUser,
    Path((project, id)): Path<(String, i32)>,
    Json(body): Json<UpdateWorkflowBody>,
) -> Result<Json<WorkflowDetail>, AppError> {
    engine(&state)?;
    let (user_id, owner) = owner_context(&state, &user)
        .await?
        .ok_or(AppError::WorkflowNotFound)?;
    let (attach, definition) = load_owned(&state.auth.db, user_id, &project, id).await?;
    if body.scope.is_some() {
        return Err(AppError::WorkflowScopeNotEditable);
    }
    let scope = parse_scope(&attach.scope)?;
    if let Some(steps) = &body.steps {
        validate_dag(steps, &user)?;
        validate_kinds(steps, scope)?;
    }

    let db = &state.auth.db;
    let txn = db.begin().await?;
    let mut definition = definition.into_active_model();
    if let Some(steps) = body.steps {
        definition.steps = Set(DagSteps(steps));
    }
    let definition = definition.update(&txn).await?;
    let mut attach = attach.into_active_model();
    if let Some(description) = body.description {
        attach.description = Set(Some(description));
    }
    let attach = attach.update(&txn).await?;
    txn.commit().await?;
    Ok(Json(detail(&attach, &definition, &owner, &project)))
}

/// `DELETE /projects/{project}/workflows/{id}` — 204. Supprime rattachement et définition
/// dans UNE transaction ; les lignes d'index des runs déjà lancés survivent avec
/// `definition_id` remis à NULL — la FK `SET NULL` du schéma le fait déjà (`real_db`
/// l'applique, vérifié), la remise explicite à NULL est une double assurance plateforme,
/// jamais une suppression de run.
pub async fn delete_workflow(
    State(state): State<AppState>,
    user: AuthUser,
    Path((project, id)): Path<(String, i32)>,
) -> Result<StatusCode, AppError> {
    engine(&state)?;
    let (user_id, _owner) = owner_context(&state, &user)
        .await?
        .ok_or(AppError::WorkflowNotFound)?;
    let (attach, definition) = load_owned(&state.auth.db, user_id, &project, id).await?;
    let db = &state.auth.db;
    let txn = db.begin().await?;
    ProjectWorkflow::delete_by_id(attach.id).exec(&txn).await?;
    WorkflowDefinition::delete_by_id(definition.id)
        .exec(&txn)
        .await?;
    WorkflowRun::update_many()
        .col_expr(WorkflowRunColumn::DefinitionId, Expr::value(None::<i32>))
        .filter(WorkflowRunColumn::DefinitionId.eq(Some(definition.id)))
        .exec(&txn)
        .await?;
    txn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Runs — lot W4b : lancement, listes, détail, annulation (`./workflows.sdd`)
// ---------------------------------------------------------------------------

/// Corps de `POST /projects/{project}/workflows/{id}/runs` (`Exposes`) : `{ sandbox? }`.
#[derive(Deserialize, Default)]
pub struct CreateRunBody {
    #[serde(default)]
    pub sandbox: Option<String>,
}

/// Paramètres communs des deux listes de runs (`Exposes`) : `limit` (≤ 50, défaut 50) et
/// `since` (jours ; défaut 14 jours, au plus 14 jours). Tranche signalée : hors bornes, la
/// liste BORNE silencieusement (clamp) plutôt que de répondre 400 — verrouillé par test.
#[derive(Deserialize, Default)]
pub struct RunListQuery {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub since: Option<i64>,
}

/// `@RunLaunch` (`Exposes`) — réponse 202 du lancement, commune à la route REST et au MCP de
/// domaine.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RunLaunch {
    pub run_id: String,
    pub invocation_id: String,
}

/// Statut d'un run en vue de liste — SANS le détail des steps (`Exposes`). L'état est rendu
/// sous le nom de variante de `RunState` (les listes de `workflows.sdd` comparent
/// `status.state == "Unknown"`).
#[derive(Debug, Serialize)]
pub struct RunStatusView {
    pub state: String,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Élément des listes `GET /projects/{project}/runs` et `GET /sandboxes/{sandbox}/runs`.
#[derive(Debug, Serialize)]
pub struct RunListItem {
    pub run_id: String,
    /// Nom STOCKÉ de la définition (`definition_name` figé au lancement, survit à sa
    /// suppression — la vue de liste ne redécoupe pas le préfixe, instantané contractuel).
    pub workflow: String,
    pub scope: String,
    pub sandbox: Option<String>,
    pub triggered_by: String,
    pub started_at: DateTime<Utc>,
    pub status: RunStatusView,
}

/// Statut d'un step, lu du journal d'invocation Restate (`status.sdd`).
#[derive(Debug, Serialize)]
pub struct StepStatusView {
    pub step_id: String,
    pub state: String,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Statut détaillé (`GET /runs/{run_id}`) — AVEC les steps.
#[derive(Debug, Serialize)]
pub struct RunStatusDetail {
    pub state: String,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub steps: Vec<StepStatusView>,
}

/// Détail d'un run (`GET /runs/{run_id}`).
#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub run_id: String,
    pub workflow: String,
    pub scope: String,
    pub sandbox: Option<String>,
    pub triggered_by: String,
    pub started_at: DateTime<Utc>,
    pub status: RunStatusDetail,
}

/// Plafond des listes de runs (`Exposes` : « limit ≤ 50, défaut 50 »).
const RUNS_MAX_LIMIT: u32 = 50;

/// Fenêtre maximale des listes (`Exposes` : « since défaut 14 jours, au plus 14 jours »).
const RUNS_MAX_SINCE_DAYS: i64 = 14;

/// Bornage silencieux des paramètres de liste (tranche : clamp, jamais 400).
fn bounded_list(query: &RunListQuery) -> (u32, i64) {
    let limit = query
        .limit
        .unwrap_or(RUNS_MAX_LIMIT)
        .clamp(1, RUNS_MAX_LIMIT);
    let since = query
        .since
        .unwrap_or(RUNS_MAX_SINCE_DAYS)
        .clamp(1, RUNS_MAX_SINCE_DAYS);
    (limit, since)
}

fn state_str(state: RunState) -> &'static str {
    match state {
        RunState::Pending => "Pending",
        RunState::Running => "Running",
        RunState::Suspended => "Suspended",
        RunState::BackingOff => "BackingOff",
        RunState::Succeeded => "Succeeded",
        RunState::Failed => "Failed",
        RunState::Cancelled => "Cancelled",
        RunState::Unknown => "Unknown",
    }
}

/// Vue de liste d'un statut lu dans Restate ; une absence de la carte de lecture groupée
/// (invocation purgée que la pré-position `Unknown` couvre déjà) rend `Unknown`.
fn status_view(statut: Option<&RunStatus>) -> RunStatusView {
    match statut {
        Some(s) => RunStatusView {
            state: state_str(s.state).to_string(),
            started_at: s.started_at,
            ended_at: s.ended_at,
            error: s.error.clone(),
        },
        None => RunStatusView {
            state: "Unknown".to_string(),
            started_at: None,
            ended_at: None,
            error: None,
        },
    }
}

/// Vue détaillée d'un statut — steps du journal compris.
fn status_detail(statut: &RunStatus) -> RunStatusDetail {
    RunStatusDetail {
        state: state_str(statut.state).to_string(),
        started_at: statut.started_at,
        ended_at: statut.ended_at,
        error: statut.error.clone(),
        steps: statut
            .steps
            .iter()
            .map(|step| StepStatusView {
                step_id: step.step_id.clone(),
                state: state_str(step.state).to_string(),
                started_at: step.started_at,
                ended_at: step.ended_at,
                error: step.error.clone(),
            })
            .collect(),
    }
}

/// Erreurs du moteur (`WorkflowError` de miryad-core) levées APRÈS le garde `engine` :
/// Restate injoignable ou en refus ⇒ 503 (`Raises` : « moteur non configuré ou Restate
/// indisponible »). L'erreur interne porte le code `MRD-WORKFLOW-*` dans son message.
fn engine_unavailable(err: &WorkflowError) -> AppError {
    AppError::WorkflowEngineUnavailable(err.to_string())
}

// ── Seam sandbox du lot runs ───────────────────────────────────────────────
// Tranche signalée : le séam `SandboxLocator` de `../workflow/steps/mod.rs` n'expose que
// phase/jeton/base — jamais `spec.project` (la membership projet ⇒ 409) ni une suppression
// de jobs par préfixe, et son trait est hors périmètre d'`api/workflows.rs`. Le séam local
// [`SandboxLookup`] porte exactement les deux lectures qu' exigent lancement et annulation ;
// son implémentation de production [`ClusterSandbox`] branche l'annulation des jobs sur
// `SandboxAccess::open_with` (jeton redemandé, timeout 10 s — `steps/mod.sdd`).

/// Lecture de la CR Sandbox utile au lancement : projet de rattachement et phase.
#[derive(Clone, Debug)]
pub(crate) struct SandboxCrState {
    pub project: String,
    pub phase: Option<String>,
}

#[async_trait]
#[allow(clippy::double_must_use)] // async_trait pose `must_use` sur le BoxFuture déjà must_use — tooling.sdd (précédent `steps/mod.rs`)
pub(crate) trait SandboxLookup: Send + Sync {
    /// `spec.project` + `status.phase` de la CR Sandbox de l'Owner ; `Ok(None)` si la CR est
    /// absente (⇒ 404, aucune distinction d'existence). Erreur K8s transitoire propagée.
    async fn inspect(&self, owner: &str, sandbox: &str)
    -> Result<Option<SandboxCrState>, AppError>;

    /// Supprime les jobs shell du run : `DELETE /jobs?prefix={run_id}:` sur la sandbox
    /// (contrat `sandbox/src/jobs.sdd`, scénario `annulation_par_prefixe`). La politique
    /// best-effort (log + 202) appartient à l'appelant.
    async fn delete_run_jobs(
        &self,
        owner: &str,
        user: &str,
        run: &WorkflowRunModel,
    ) -> Result<(), AppError>;
}

/// Séam branché sur le client K8s de production (découverte lazy de l'`AppState`).
pub(crate) struct ClusterSandbox {
    k8s: VnlK8sClient,
    http: reqwest::Client,
}

#[async_trait]
impl SandboxLookup for ClusterSandbox {
    async fn inspect(
        &self,
        owner: &str,
        sandbox: &str,
    ) -> Result<Option<SandboxCrState>, AppError> {
        match self.k8s.get_sandbox(owner, sandbox).await {
            Ok(cr) => Ok(Some(SandboxCrState {
                project: cr.spec.project.clone(),
                phase: cr.status.and_then(|status| status.phase),
            })),
            // `lib/src/k8s.rs` aplatit `NotFound` dans `K8sApiError` (même aplatissement que
            // `K8sLocator`) : CR absente ⇒ `Ok(None)` ⇒ 404 sans fuite ; autre erreur K8s ⇒
            // propagée.
            Err(err @ vanyline_lib::VnyError::K8sApiError(_)) => {
                let msg = err.to_string();
                if msg.contains("404") || msg.to_lowercase().contains("not found") {
                    Ok(None)
                } else {
                    Err(AppError::from(err))
                }
            }
            Err(err) => Err(AppError::from(err)),
        }
    }

    async fn delete_run_jobs(
        &self,
        owner: &str,
        user: &str,
        run: &WorkflowRunModel,
    ) -> Result<(), AppError> {
        let sandbox = run.sandbox_name.as_deref().ok_or_else(|| {
            AppError::InternalError(format!(
                "VNL-WFA-020: suppression de jobs demandée pour le run {} sans sandbox",
                run.run_id
            ))
        })?;
        let ctx = RunContext {
            run_id: run.run_id.clone(),
            user: user.to_string(),
            user_id: run.owner_id,
            owner: owner.to_string(),
            project: run.project_name.clone(),
            sandbox: Some(sandbox.to_string()),
            triggered_by: run.triggered_by.clone(),
            definition: run.definition_name.clone(),
        };
        let value = serde_json::to_value(&ctx).map_err(|e| {
            AppError::InternalError(format!(
                "VNL-WFA-020: contexte de run non sérialisable: {e}"
            ))
        })?;
        let info = RunInfo::from_config(&serde_json::json!({ "_run": value }))
            .map_err(|e| AppError::InternalError(e.message))?;
        let access = SandboxAccess::open_with(
            Arc::new(K8sLocator::new(self.k8s.clone())),
            self.http.clone(),
            &info,
        )
        .await
        .map_err(|e| AppError::InternalError(format!("VNL-WFA-020: {}", e.message)))?;
        // Chemin exact du contrat `sandbox/src/jobs.sdd` : `DELETE /jobs?prefix={run_id}:`.
        access
            .delete(&format!(
                "/jobs?prefix={run_id}:",
                run_id = run.run_id.as_str()
            ))
            .await
            .map_err(|e| AppError::InternalError(format!("VNL-WFA-020: {}", e.message)))?;
        Ok(())
    }
}

/// Séam effectif : fourni par l'appelant de test, sinon branché sur le cluster.
async fn sandbox_lookup(
    state: &AppState,
    provided: Option<Arc<dyn SandboxLookup>>,
) -> Result<Arc<dyn SandboxLookup>, AppError> {
    if let Some(lookup) = provided {
        return Ok(lookup);
    }
    let runtime = engine(state)?;
    Ok(Arc::new(ClusterSandbox {
        k8s: crate::k8s::client(state).await?,
        http: runtime.http().clone(),
    }))
}

// ── Services communs (route REST + MCP de domaine, `Exposes`) ──────────────

/// `@launch_workflow` (`Exposes`) — service de lancement, `triggered_by` = `"ui"` pour la
/// route REST, valeur fournie par l'appelant pour le MCP de domaine. Séam sandbox résolu par
/// le cluster ; les tests branchent [`launch_inner`] sur un séam injecté.
pub async fn launch_workflow(
    state: &AppState,
    user: &AuthUser,
    project: &str,
    workflow_id: i32,
    sandbox: Option<String>,
    triggered_by: &str,
) -> Result<RunLaunch, AppError> {
    launch_inner(
        state,
        user,
        project,
        workflow_id,
        sandbox,
        triggered_by,
        None,
    )
    .await
}

/// Cœur du lancement (`Handles` de `workflows.sdd`, séquence contractuelle) sur le séam
/// sandbox en paramètre — jurisprudence `start_with`/`open_with` : hors cluster,
/// [`VnlK8sClient`] n'est pas constructible (convention anti-mock `lib/src/k8s.sdd`).
pub(crate) async fn launch_inner(
    state: &AppState,
    user: &AuthUser,
    project: &str,
    workflow_id: i32,
    sandbox: Option<String>,
    triggered_by: &str,
    lookup: Option<Arc<dyn SandboxLookup>>,
) -> Result<RunLaunch, AppError> {
    let runtime = engine(state)?;
    let (user_id, owner) = owner_context(state, user)
        .await?
        .ok_or(AppError::WorkflowNotFound)?;
    // (1) charger rattachement + définition (`Handles`).
    let (attach, definition) = load_owned(&state.auth.db, user_id, project, workflow_id).await?;
    // (2) portée et sandbox — cohérence du corps avec la portée stockée (400/409), puis
    // kind(s) REvalidés (`Must` : une définition éditée par un autre chemin ne contourne
    // pas la règle), puis CR sandbox (projet, phase `Running`).
    let scope = parse_scope(&attach.scope)?;
    let sandbox = match (scope, sandbox) {
        (RunScope::Project, Some(_)) => return Err(AppError::WorkflowSandboxNotAllowed),
        (RunScope::Project, None) => None,
        (RunScope::Sandbox, None) => return Err(AppError::WorkflowSandboxRequired),
        (RunScope::Sandbox, Some(name)) => Some(name),
    };
    validate_kinds(&definition.steps.0, scope)?;
    if let Some(name) = &sandbox {
        let lk = sandbox_lookup(state, lookup).await?;
        let cr = lk
            .inspect(&owner, name)
            .await?
            .ok_or(AppError::WorkflowSandboxNotFound)?;
        if cr.project != project {
            return Err(AppError::WorkflowSandboxForeignProject(name.clone()));
        }
        if cr.phase.as_deref() != Some("Running") {
            return Err(AppError::WorkflowSandboxNotRunning(format!(
                "{name} : phase {}",
                cr.phase.as_deref().unwrap_or("inconnue")
            )));
        }
    }
    // (3) run_id (UUID v4) — `materialize` ne génère rien, `RunContext` porte la clé.
    let run_id = uuid::Uuid::new_v4().to_string();
    let ctx = RunContext {
        run_id: run_id.clone(),
        user: user.subject.clone(),
        user_id,
        owner: owner.clone(),
        project: project.to_string(),
        sandbox: sandbox.clone(),
        triggered_by: triggered_by.to_string(),
        definition: definition.name.clone(),
    };
    // (4) matérialisation du DAG (injection `_run`, validation miryad-core).
    let dag = match materialize(&definition.steps, scope, &ctx) {
        Ok(dag) => dag,
        Err(WorkflowError::InvalidDag(msg)) => return Err(AppError::WorkflowDagInvalid(msg)),
        Err(other) => return Err(engine_unavailable(&other)),
    };
    // (5) trigger sur l'ingress Restate — jamais bloquant sur la complétion.
    let handle = runtime
        .trigger(&dag)
        .await
        .map_err(|e| engine_unavailable(&e))?;
    // (6) ligne d'index ; si elle échoue, JAMAIS un run sans index : l'invocation qui
    // vient d'être déclenchée est annulée (best-effort, l'échec d'annulation est loggé) et
    // la réponse est 500.
    let index = crate::db::entities::workflow_runs::ActiveModel {
        id: NotSet,
        owner_id: Set(user_id),
        project_name: Set(project.to_string()),
        sandbox_name: Set(sandbox),
        definition_id: Set(Some(attach.definition_id)),
        definition_name: Set(definition.name.clone()),
        run_id: Set(run_id.clone()),
        run_key: Set(handle.run_key.clone()),
        invocation_id: Set(handle.invocation_id.clone()),
        scope: Set(attach.scope.clone()),
        triggered_by: Set(triggered_by.to_string()),
        started_at: Set(Utc::now()),
    };
    if let Err(erreur) = index.insert(&state.auth.db).await {
        if let Err(annulation) = status::cancel(runtime, &handle.invocation_id).await {
            tracing::warn!(
                "VNL-WFA-018: échec d'indexation du run {run_id} ({erreur}) ET de l'annulation de l'invocation {}: {annulation}",
                handle.invocation_id
            );
        }
        return Err(AppError::InternalError(format!(
            "VNL-WFA-018: échec d'indexation du run {run_id}: {erreur}"
        )));
    }
    Ok(RunLaunch {
        run_id,
        invocation_id: handle.invocation_id,
    })
}

/// `@list_runs` (`Exposes`) — lecture bornée de l'index puis UNE lecture groupée des statuts
/// (`status.sdd`). `project`/`sandbox` : l'une des deux portées de liste.
pub async fn list_runs(
    state: &AppState,
    user_id: i32,
    project: Option<&str>,
    sandbox: Option<&str>,
    limit: u32,
    since_days: i64,
) -> Result<Vec<RunListItem>, AppError> {
    let runtime = engine(state)?;
    let delta = chrono::TimeDelta::try_days(since_days).ok_or_else(|| {
        AppError::InternalError(format!(
            "VNL-WFA-019: fenêtre de runs invalide: {since_days} j"
        ))
    })?;
    let cutoff = chrono::Utc::now()
        .checked_sub_signed(delta)
        .ok_or_else(|| {
            AppError::InternalError(format!(
                "VNL-WFA-019: coupure de fenêtre hors plage: {since_days} j"
            ))
        })?;
    let mut requete = WorkflowRun::find()
        .filter(WorkflowRunColumn::OwnerId.eq(user_id))
        .filter(WorkflowRunColumn::StartedAt.gte(cutoff));
    if let Some(project) = project {
        requete = requete.filter(WorkflowRunColumn::ProjectName.eq(project));
    }
    if let Some(sandbox) = sandbox {
        requete = requete.filter(WorkflowRunColumn::SandboxName.eq(sandbox));
    }
    let lignes = requete
        .order_by(WorkflowRunColumn::StartedAt, sea_orm::Order::Desc)
        .limit(u64::from(limit))
        .all(&state.auth.db)
        .await?;
    // UNE lecture groupée des statuts (`status.sdd`) : un run absent de Restate y est
    // pré-positionné `Unknown`, jamais une erreur.
    let ids: Vec<String> = lignes.iter().map(|l| l.invocation_id.clone()).collect();
    let statuts = status::fetch_statuses(runtime, &ids)
        .await
        .map_err(|e| engine_unavailable(&e))?;
    Ok(lignes
        .iter()
        .map(|ligne| RunListItem {
            run_id: ligne.run_id.clone(),
            workflow: ligne.definition_name.clone(),
            scope: ligne.scope.clone(),
            sandbox: ligne.sandbox_name.clone(),
            triggered_by: ligne.triggered_by.clone(),
            started_at: ligne.started_at,
            status: status_view(statuts.get(&ligne.invocation_id)),
        })
        .collect())
}

/// Ligne d'index d'un run POSÉDÉE par `user_id`, par `run_id` de l'app. Absente ou à un
/// autre ⇒ [`AppError::WorkflowRunNotFound`] — 404 identique, aucune fuite d'existence.
async fn find_run(
    db: &sea_orm::DatabaseConnection,
    user_id: i32,
    run_id: &str,
) -> Result<WorkflowRunModel, AppError> {
    WorkflowRun::find()
        .filter(WorkflowRunColumn::RunId.eq(run_id))
        .filter(WorkflowRunColumn::OwnerId.eq(user_id))
        .one(db)
        .await?
        .ok_or(AppError::WorkflowRunNotFound)
}

/// Cœur de l'annulation (`Handles`) sur le séam sandbox injectable : annulation de
/// l'invocation (l'échec est propagé — 503), puis jobs shell best-effort si le run porte
/// une sandbox (un échec est loggé, jamais un refus du 202).
pub(crate) async fn cancel_run_inner(
    state: &AppState,
    user: &AuthUser,
    run_id: &str,
    lookup: Option<Arc<dyn SandboxLookup>>,
) -> Result<StatusCode, AppError> {
    let runtime = engine(state)?;
    let (user_id, owner) = owner_context(state, user)
        .await?
        .ok_or(AppError::WorkflowRunNotFound)?;
    let run = find_run(&state.auth.db, user_id, run_id).await?;
    // L'annulation de l'invocation est l'action contractuelle : son échec (Restate
    // injoignable ou en refus) est propagé en 503, sans toucher aux jobs.
    status::cancel(runtime, &run.invocation_id)
        .await
        .map_err(|e| engine_unavailable(&e))?;
    // Puis les jobs shell du run, best-effort : un échec (sandbox absente, éteinte, 404…)
    // est loggé, jamais un refus du 202.
    if run.sandbox_name.is_some() {
        let lk = sandbox_lookup(state, lookup).await?;
        if let Err(erreur) = lk.delete_run_jobs(&owner, &user.subject, &run).await {
            tracing::warn!(
                "VNL-WFA-020: annulation best-effort des jobs shell du run {} échouée: {erreur}",
                run.run_id
            );
        }
    }
    Ok(StatusCode::ACCEPTED)
}

// ── Handlers runs (lot W4b) ────────────────────────────────────────────────

/// `POST /projects/{project}/workflows/{id}/runs` — corps `{ sandbox? }` ; 202
/// `{ run_id, invocation_id }` (`triggered_by` = `"ui"`).
pub async fn create_run(
    State(state): State<AppState>,
    user: AuthUser,
    Path((project, id)): Path<(String, i32)>,
    Json(body): Json<CreateRunBody>,
) -> Result<(StatusCode, Json<RunLaunch>), AppError> {
    let launch = launch_workflow(&state, &user, &project, id, body.sandbox, "ui").await?;
    Ok((StatusCode::ACCEPTED, Json(launch)))
}

/// `GET /projects/{project}/runs` — runs du projet (fenêtre bornée, plus récent d'abord).
/// Aucun Owner ⇒ liste vide (200), même motif que `list_workflows`.
pub async fn list_project_runs(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project): Path<String>,
    Query(query): Query<RunListQuery>,
) -> Result<Json<Vec<RunListItem>>, AppError> {
    engine(&state)?;
    let Some((user_id, _owner)) = owner_context(&state, &user).await? else {
        return Ok(Json(Vec::new()));
    };
    let (limit, since) = bounded_list(&query);
    Ok(Json(
        list_runs(&state, user_id, Some(&project), None, limit, since).await?,
    ))
}

/// `GET /sandboxes/{sandbox}/runs` — runs liés à cette sandbox (index, pas de lecture de la
/// CR : un run survit à sa sandbox).
pub async fn list_sandbox_runs(
    State(state): State<AppState>,
    user: AuthUser,
    Path(sandbox): Path<String>,
    Query(query): Query<RunListQuery>,
) -> Result<Json<Vec<RunListItem>>, AppError> {
    engine(&state)?;
    let Some((user_id, _owner)) = owner_context(&state, &user).await? else {
        return Ok(Json(Vec::new()));
    };
    let (limit, since) = bounded_list(&query);
    Ok(Json(
        list_runs(&state, user_id, None, Some(&sandbox), limit, since).await?,
    ))
}

/// `GET /runs/{run_id}` — détail avec le statut des steps lu dans Restate.
pub async fn get_run(
    State(state): State<AppState>,
    user: AuthUser,
    Path(run_id): Path<String>,
) -> Result<Json<RunDetail>, AppError> {
    let runtime = engine(&state)?;
    let (user_id, _owner) = owner_context(&state, &user)
        .await?
        .ok_or(AppError::WorkflowRunNotFound)?;
    let run = find_run(&state.auth.db, user_id, &run_id).await?;
    let statut = status::fetch_status(runtime, &run.invocation_id)
        .await
        .map_err(|e| engine_unavailable(&e))?;
    Ok(Json(RunDetail {
        run_id: run.run_id,
        workflow: run.definition_name,
        scope: run.scope,
        sandbox: run.sandbox_name,
        triggered_by: run.triggered_by,
        started_at: run.started_at,
        status: status_detail(&statut),
    }))
}

/// `POST /runs/{run_id}/cancel` — 202.
pub async fn cancel_run(
    State(state): State<AppState>,
    user: AuthUser,
    Path(run_id): Path<String>,
) -> Result<StatusCode, AppError> {
    cancel_run_inner(&state, &user, &run_id, None).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use super::*;
    use crate::db::entities::owner_links;
    use crate::db::entities::project_workflows::Entity as ProjectWorkflow;
    use crate::db::entities::workflow_runs;
    use crate::db::test_support::real_db;
    use axum::response::IntoResponse;
    use sea_orm::ActiveModelTrait;
    use sea_orm::ConnectionTrait;

    // ── harnais (mêmes conventions que api/sandboxes.rs) ──

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
            oidc_connect_timeout: std::time::Duration::from_secs(5),
            oidc_timeout: std::time::Duration::from_secs(15),
            cookie_secret: "0".repeat(64),
            database_url: "postgres://localhost/test".to_string(),
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
            k8s_namespace: None,
            application_name: None,
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
            restate_admin_url: None,
            restate_ingress_url: None,
            workflow_listen: "0.0.0.0:9080".to_string(),
            workflow_deployment_url: None,
        }
    }

    fn test_state_with_db(db: sea_orm::DatabaseConnection) -> AppState {
        AppState {
            config: test_config(),
            cookie_key: cookie::Key::from(&[0u8; 64]),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    /// État sur base simulée SANS résultat programmé : toute interrogation de la base
    /// échoue — le test de garde moteur en déduit que le handler n'a rien lu.
    fn test_state_mock_db() -> AppState {
        AppState {
            config: test_config(),
            cookie_key: cookie::Key::from(&[0u8; 64]),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    fn user(subject: &str) -> AuthUser {
        AuthUser {
            subject: subject.to_string(),
            email: Some(format!("{subject}@example.com")),
            id_token: format!("id-token-{subject}"),
        }
    }

    /// Faux Restate minimal : `POST /deployments` rend `200 {"deploymentId":"dp_fake"}`
    /// (pattern `workflow/runtime.rs` tests) — l'enregistrement du moteur réussit du premier
    /// coup, aucune tâche de réessai en arrière-plan.
    async fn spawn_fake_restate_ok() -> String {
        let app = axum::Router::new().route(
            "/deployments",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::OK,
                    r#"{"deploymentId":"dp_fake"}"#.to_string(),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    /// Handle moteur réel (seam `pub(crate) start_with`, endpoint sur port éphémère,
    /// backoff de test) — seul le champ `workflow: Some(..)` importe au lot W4a.
    async fn spawn_engine() -> WorkflowRuntime {
        let base = spawn_fake_restate_ok().await;
        let cfg = crate::workflow::WorkflowConfigApp {
            admin_url: base.clone(),
            ingress_url: base,
            deployment_url: "http://vanyline.test:9410".to_string(),
            listen: "127.0.0.1:0".to_string(),
        };
        crate::workflow::runtime::start_with(
            cfg,
            miryad_core::workflow::StepRegistry::new(),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(50),
        )
        .await
        .expect("moteur de test démarré")
    }

    /// Base réelle + utilisateurs `alice`/`bob` résolus et liés à leur Owner K8s + moteur
    /// configuré (la garde 503 passe ; le comportement CRUD testé est derrière elle).
    async fn seeded_state() -> (AppState, i32, i32) {
        let db = real_db().await;
        let alice = miryad_core::users::resolve_user(&db, "alice", Some("alice@example.com"))
            .await
            .expect("alice resolves");
        let bob = miryad_core::users::resolve_user(&db, "bob", Some("bob@example.com"))
            .await
            .expect("bob resolves");
        for (uid, name) in [(alice.id, "alice"), (bob.id, "bob")] {
            owner_links::ActiveModel {
                id: NotSet,
                user_id: Set(uid),
                k8s_owner_name: Set(Some(name.to_string())),
            }
            .insert(&db)
            .await
            .expect("owner link seeded");
        }
        let mut state = test_state_with_db(db);
        state.workflow = Some(spawn_engine().await);
        (state, alice.id, bob.id)
    }

    fn db_of(state: &AppState) -> &sea_orm::DatabaseConnection {
        &state.auth.db
    }

    fn body_json(name: &str, scope: &str, steps: serde_json::Value) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), serde_json::Value::from(name));
        map.insert("scope".to_string(), serde_json::Value::from(scope));
        map.insert("steps".to_string(), steps);
        serde_json::Value::Object(map)
    }

    fn agent_step(id: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "depends_on": [], "kind": "agent", "config": {}})
    }

    fn shell_step(id: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "depends_on": [], "kind": "shell", "config": {"cmd": "make"}})
    }

    async fn post_workflow(
        state: &AppState,
        who: &str,
        project: &str,
        body: serde_json::Value,
    ) -> Result<(StatusCode, WorkflowDetail), AppError> {
        let body: CreateWorkflowBody =
            serde_json::from_value(body.clone()).expect("fixture body deserializes");
        create_workflow(
            State(state.clone()),
            user(who),
            Path(project.to_string()),
            Json(body),
        )
        .await
        .map(|(status, Json(detail))| (status, detail))
    }

    async fn render(err: AppError) -> (StatusCode, Vec<u8>) {
        let (parts, body) = err.into_response().into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (parts.status, bytes)
    }

    async fn status_of(err: AppError) -> StatusCode {
        render(err).await.0
    }

    async fn body_of(err: AppError) -> Vec<u8> {
        render(err).await.1
    }

    async fn definition_count(state: &AppState) -> usize {
        WorkflowDefinition::find()
            .all(db_of(state))
            .await
            .unwrap()
            .len()
    }

    async fn attach_count(state: &AppState) -> usize {
        ProjectWorkflow::find()
            .all(db_of(state))
            .await
            .unwrap()
            .len()
    }

    // ── Scenario: creation_atomique ─────────────────────────────────────────
    // POST valide (portée sandbox) ⇒ 201, définition `{owner}/{project}/{name}` ET
    // rattachement créés ; l'API rend le nom nu.
    #[tokio::test]
    async fn creation_atomique() {
        let (state, alice_id, _bob_id) = seeded_state().await;
        let (status, detail) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "sandbox", serde_json::json!([shell_step("build")])),
        )
        .await
        .expect("POST valide doit rendre 201");
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(detail.name, "ci", "l'API rend le nom nu, sans préfixe");
        assert_eq!(detail.scope, "sandbox");

        let defs = WorkflowDefinition::find().all(db_of(&state)).await.unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "alice/demo/ci", "nom stocké préfixé");
        assert_eq!(defs[0].owner_id, Some(alice_id));

        let attachs = ProjectWorkflow::find().all(db_of(&state)).await.unwrap();
        assert_eq!(attachs.len(), 1);
        assert_eq!(attachs[0].project_name, "demo");
        assert_eq!(attachs[0].scope, "sandbox");
        assert_eq!(attachs[0].definition_id, defs[0].id);
    }

    // Volet atomicité du Scenario : « si le rattachement échoue, la définition n'existe
    // pas ». L'échec est FORCÉ par un trigger sqlite `RAISE(ABORT)` sur l'insertion du
    // rattachement (vérifié : `real_db` APPLIQUE les FK — une insertion pendante est
    // impossible, le trigger est le seul levier déterministe). Après rollback, AUCUNE
    // définition ne doit exister.
    #[tokio::test]
    async fn creation_atomique_rattachement_en_echec_aucune_definition() {
        let (state, _, _) = seeded_state().await;
        db_of(&state)
            .execute_unprepared(
                "CREATE TRIGGER force_attach_echec BEFORE INSERT ON vanyline_project_workflows
                 BEGIN SELECT RAISE(ABORT, 'échec forcé (test atomicité W4a)'); END;",
            )
            .await
            .expect("trigger d'échec créé");

        let err = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "sandbox", serde_json::json!([shell_step("build")])),
        )
        .await
        .expect_err("l'échec du rattachement doit faire échouer le POST");
        assert_eq!(status_of(err).await, StatusCode::INTERNAL_SERVER_ERROR);

        assert_eq!(
            definition_count(&state).await,
            0,
            "le rollback doit avoir effacé la définition insérée dans la transaction"
        );
        assert_eq!(attach_count(&state).await, 0);
    }

    // ── Scenario: kind_interdit_pour_la_portee ──────────────────────────────
    // step `shell` + scope `project` ⇒ 422 et RIEN n'est créé.
    #[tokio::test]
    async fn kind_interdit_pour_la_portee() {
        let (state, _, _) = seeded_state().await;
        let err = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([shell_step("build")])),
        )
        .await
        .expect_err("shell est interdit en portée projet");
        assert_eq!(status_of(err).await, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(definition_count(&state).await, 0);
        assert_eq!(attach_count(&state).await, 0);
    }

    // ── Scenario: workflow_d_un_autre_utilisateur (volet CRUD) ──────────────
    // Le workflow de bob : alice ne peut ni le lire ni le supprimer ⇒ 404, aucune action.
    #[tokio::test]
    async fn workflow_d_un_autre_utilisateur_en_lecture_et_suppression() {
        let (state, _, _) = seeded_state().await;
        let (_, created) = post_workflow(
            &state,
            "bob",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("bob crée son workflow");

        let read_err = get_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
        )
        .await
        .expect_err("alice ne doit pas lire le workflow de bob");
        assert_eq!(status_of(read_err).await, StatusCode::NOT_FOUND);

        let del_err = delete_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
        )
        .await
        .expect_err("alice ne doit pas supprimer le workflow de bob");
        assert_eq!(status_of(del_err).await, StatusCode::NOT_FOUND);

        // aucune action : le workflow de bob est intact
        assert_eq!(definition_count(&state).await, 1);
        assert_eq!(attach_count(&state).await, 1);
    }

    // ── Must: 404 identique « inexistant » vs « à un autre » (aucune fuite) ──
    // Statut ET corps octet à octet identiques, en GET comme en DELETE.
    #[tokio::test]
    async fn quarante_quatre_identique_inexistant_contre_autre_utilisateur() {
        let (state, _, _) = seeded_state().await;
        let (_, created) = post_workflow(
            &state,
            "bob",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("bob crée son workflow");

        let missing_get = get_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), 99_999)),
        )
        .await
        .expect_err("inexistant ⇒ erreur");
        let other_get = get_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
        )
        .await
        .expect_err("à un autre ⇒ erreur");
        let (missing_status, missing_body) = render(missing_get).await;
        let (other_status, other_body) = render(other_get).await;
        assert_eq!(
            missing_status,
            StatusCode::NOT_FOUND,
            "inexistant ⇒ 404 (GET)"
        );
        assert_eq!(other_status, missing_status, "GET : statut identique");
        assert_eq!(
            other_body, missing_body,
            "GET : corps identique octet à octet"
        );

        let missing_del = delete_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), 99_999)),
        )
        .await
        .expect_err("inexistant ⇒ erreur");
        let other_del = delete_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
        )
        .await
        .expect_err("à un autre ⇒ erreur");
        let (missing_status, missing_body) = render(missing_del).await;
        let (other_status, other_body) = render(other_del).await;
        assert_eq!(
            missing_status,
            StatusCode::NOT_FOUND,
            "inexistant ⇒ 404 (DELETE)"
        );
        assert_eq!(other_status, missing_status, "DELETE : statut identique");
        assert_eq!(
            other_body, missing_body,
            "DELETE : corps identique octet à octet"
        );
    }

    // ── Scenario: moteur_non_configure (routes CRUD du lot W4a) ─────────────
    // workflow: None ⇒ 503 sur les 5 routes, AVANT toute lecture de base (base simulée
    // sans résultats programmés : si le handler interrogeait la base, l'erreur serait un
    // 500, pas un 503).
    #[tokio::test]
    async fn moteur_non_configure_rend_503_sur_les_cinq_routes() {
        let state = test_state_mock_db();
        let cases: Vec<(&str, Result<StatusCode, AppError>)> = vec![
            (
                "GET /workflows",
                list_workflows(State(state.clone()), user("alice"), Path("demo".into()))
                    .await
                    .map(|_| StatusCode::OK),
            ),
            (
                "POST /workflows",
                create_workflow(
                    State(state.clone()),
                    user("alice"),
                    Path("demo".into()),
                    Json(
                        serde_json::from_value(body_json(
                            "ci",
                            "project",
                            serde_json::json!([agent_step("a")]),
                        ))
                        .unwrap(),
                    ),
                )
                .await
                .map(|_| StatusCode::OK),
            ),
            (
                "GET /workflows/{id}",
                get_workflow(
                    State(state.clone()),
                    user("alice"),
                    Path(("demo".into(), 1)),
                )
                .await
                .map(|_| StatusCode::OK),
            ),
            (
                "PUT /workflows/{id}",
                update_workflow(
                    State(state.clone()),
                    user("alice"),
                    Path(("demo".into(), 1)),
                    Json(serde_json::from_value(serde_json::json!({})).unwrap()),
                )
                .await
                .map(|_| StatusCode::OK),
            ),
            (
                "DELETE /workflows/{id}",
                delete_workflow(
                    State(state.clone()),
                    user("alice"),
                    Path(("demo".into(), 1)),
                )
                .await
                .map(|_| StatusCode::OK),
            ),
        ];
        for (label, res) in cases {
            let err = res.expect_err("{label} sans moteur doit rendre une erreur");
            assert_eq!(
                status_of(err).await,
                StatusCode::SERVICE_UNAVAILABLE,
                "{label} doit rendre 503"
            );
        }
    }

    // ── Scenario: suppression_garde_l_historique ────────────────────────────
    #[tokio::test]
    async fn suppression_garde_l_historique() {
        let (state, alice_id, _) = seeded_state().await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow créé");
        let def = WorkflowDefinition::find()
            .one(db_of(&state))
            .await
            .unwrap()
            .unwrap();
        workflow_runs::ActiveModel {
            id: NotSet,
            owner_id: Set(alice_id),
            project_name: Set("demo".to_string()),
            sandbox_name: Set(None),
            definition_id: Set(Some(def.id)),
            definition_name: Set(def.name.clone()),
            run_id: Set("run-1".to_string()),
            run_key: Set("key-1".to_string()),
            invocation_id: Set("inv-1".to_string()),
            scope: Set("project".to_string()),
            triggered_by: Set("ui".to_string()),
            started_at: Set(chrono::Utc::now()),
        }
        .insert(db_of(&state))
        .await
        .expect("ligne d'index insérée");

        let status = delete_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
        )
        .await
        .expect("DELETE doit réussir");
        assert_eq!(status, StatusCode::NO_CONTENT);

        assert_eq!(attach_count(&state).await, 0, "rattachement supprimé");
        assert_eq!(definition_count(&state).await, 0, "définition supprimée");
        let runs = workflow_runs::Entity::find()
            .all(db_of(&state))
            .await
            .unwrap();
        assert_eq!(runs.len(), 1, "la ligne d'index survit");
        assert_eq!(runs[0].definition_id, None, "definition_id passé à NULL");
        assert_eq!(
            runs[0].definition_name, "alice/demo/ci",
            "definition_name intact"
        );
    }

    // ── Must: la portée n'est pas modifiable (PUT) — rejet explicite 400 ────
    #[tokio::test]
    async fn put_rejette_le_changement_de_portee() {
        let (state, _, _) = seeded_state().await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow projet créé");

        let err = update_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
            Json(
                serde_json::from_value(serde_json::json!({"scope": "sandbox"}))
                    .expect("PUT avec scope dans le corps"),
            ),
        )
        .await
        .expect_err("la portée n'est pas modifiable");
        assert_eq!(status_of(err).await, StatusCode::BAD_REQUEST);

        let attach = ProjectWorkflow::find()
            .one(db_of(&state))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(attach.scope, "project", "la portée stockée est intacte");
    }

    // ── Revalidation au PUT : un step shell ajouté sur un projet ⇒ 422, RIEN modifié ──
    #[tokio::test]
    async fn put_revalide_les_kinds_avant_ecriture() {
        let (state, _, _) = seeded_state().await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow projet créé");

        let err = update_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
            Json(
                serde_json::from_value(serde_json::json!({"steps": [shell_step("s")]}))
                    .expect("PUT corps valide"),
            ),
        )
        .await
        .expect_err("shell sur portée projet ⇒ 422");
        assert_eq!(status_of(err).await, StatusCode::UNPROCESSABLE_ENTITY);

        let def = WorkflowDefinition::find()
            .one(db_of(&state))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(def.steps.0.len(), 1, "les steps stockés sont intacts");
        assert_eq!(def.steps.0[0].kind, "agent");
    }

    // Appoint `PUT` nominal : description (rattachement) et steps (définition) modifiés.
    #[tokio::test]
    async fn put_modifie_description_et_steps() {
        let (state, _, _) = seeded_state().await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow créé");

        let Json(detail) = update_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
            Json(
                serde_json::from_value(serde_json::json!({
                    "description": "mis à jour",
                    "steps": [agent_step("a"), agent_step("b")],
                }))
                .unwrap(),
            ),
        )
        .await
        .expect("PUT nominal");
        assert_eq!(detail.description.as_deref(), Some("mis à jour"));
        assert_eq!(detail.steps.len(), 2);
    }

    // ── Raises 409 : nom déjà pris dans le projet ───────────────────────────
    #[tokio::test]
    async fn nom_de_workflow_deja_pris_rend_409() {
        let (state, _, _) = seeded_state().await;
        post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("première création");

        let err = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect_err("le doublon de nom doit être refusé");
        assert_eq!(status_of(err).await, StatusCode::CONFLICT);
        assert_eq!(definition_count(&state).await, 1);
        assert_eq!(attach_count(&state).await, 1);
    }

    // Appoint Raises 400 : DAG invalide (`validate_dag`, MRD-WORKFLOW-004), rien créé.
    #[tokio::test]
    async fn dag_invalide_rend_400_mrd_workflow_004_sans_ecriture() {
        let (state, _, _) = seeded_state().await;
        let cyclic = serde_json::json!([
            {"id": "A", "depends_on": ["B"], "kind": "agent", "config": {}},
            {"id": "B", "depends_on": ["A"], "kind": "agent", "config": {}},
        ]);
        let err = post_workflow(&state, "alice", "demo", body_json("ci", "project", cyclic))
            .await
            .expect_err("un DAG cyclique doit être refusé");
        let resp_body = body_of(err).await;
        let text = String::from_utf8(resp_body).unwrap();
        assert!(
            text.contains("MRD-WORKFLOW-004"),
            "le code MRD-WORKFLOW-004 doit traverser : {text}"
        );
        assert_eq!(definition_count(&state).await, 0);
        assert_eq!(attach_count(&state).await, 0);
    }

    // Appoint Raises 400 : portée inconnue, rien créé.
    #[tokio::test]
    async fn portee_inconnue_rend_400_sans_ecriture() {
        let (state, _, _) = seeded_state().await;
        let err = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "global", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect_err("la portée « global » n'existe pas");
        assert_eq!(status_of(err).await, StatusCode::BAD_REQUEST);
        assert_eq!(definition_count(&state).await, 0);
        assert_eq!(attach_count(&state).await, 0);
    }

    // ── Appoint Exposes : GET liste — kinds + steps_count, nom nu, cloisonnement ──
    #[tokio::test]
    async fn liste_rend_kinds_steps_count_nom_nu_et_cloisonne_les_owners() {
        let (state, _, _) = seeded_state().await;
        post_workflow(
            &state,
            "alice",
            "demo",
            serde_json::json!({
                "name": "ci", "scope": "project", "description": "la CI",
                "steps": [agent_step("a"), agent_step("a2")],
            }),
        )
        .await
        .expect("workflow projet d'alice");
        post_workflow(
            &state,
            "alice",
            "demo",
            body_json("build", "sandbox", serde_json::json!([shell_step("s")])),
        )
        .await
        .expect("workflow sandbox d'alice");
        post_workflow(
            &state,
            "bob",
            "demo",
            body_json("autre", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow de bob");

        let Json(list) = list_workflows(State(state.clone()), user("alice"), Path("demo".into()))
            .await
            .expect("liste alice");
        assert_eq!(list.len(), 2, "les workflows de bob sont invisibles");
        let ci = list.iter().find(|w| w.name == "ci").expect("nom nu « ci »");
        assert_eq!(ci.scope, "project");
        assert_eq!(ci.description.as_deref(), Some("la CI"));
        assert_eq!(
            ci.kinds,
            vec!["agent".to_string()],
            "kinds distincts du DAG"
        );
        assert_eq!(ci.steps_count, 2);
        let build = list
            .iter()
            .find(|w| w.name == "build")
            .expect("nom nu « build »");
        assert_eq!(build.kinds, vec!["shell".to_string()]);
        assert_eq!(build.steps_count, 1);
    }

    // Appoint Exposes : la liste d'un utilisateur SANS Owner est vide (200, pas d'erreur).
    #[tokio::test]
    async fn liste_sans_owner_est_vide() {
        let db = real_db().await;
        let mut state = test_state_with_db(db);
        state.workflow = Some(spawn_engine().await);
        let Json(list) = list_workflows(State(state.clone()), user("alice"), Path("demo".into()))
            .await
            .expect("liste sans Owner ⇒ 200");
        assert!(list.is_empty());
    }

    // Appoint Exposes : détail avec steps.
    #[tokio::test]
    async fn detail_rend_steps_et_nom_nu() {
        let (state, _, _) = seeded_state().await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow créé");
        let Json(detail) = get_workflow(
            State(state.clone()),
            user("alice"),
            Path(("demo".to_string(), created.id)),
        )
        .await
        .expect("détail");
        assert_eq!(detail.name, "ci");
        assert_eq!(detail.steps.len(), 1);
        assert_eq!(detail.steps[0].kind, "agent");
    }

    // =========================================================================
    // Lot W4b — runs : harnais
    // =========================================================================

    use crate::workflow::steps::{RunInfo, SandboxAccess, SandboxLocator};
    use miryad_core::workflow::StepError;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Faux Restate complet pour les runs (motif combiné de `status.rs` et `runtime.rs`) :
    /// admin (`POST /query` avec compteur, `PATCH /invocations/{id}/cancel`) + ingress
    /// (`POST /DagInterpreter/{key}/run/send`, corps capturés, `invocationId` dans le corps,
    /// fixe si fourni — levier d'echec d'indexation) + `POST /deployments` (enregistrement
    /// du moteur). Le `log` partagé est séquentiel avec le faux serveur sandbox (ordre
    /// annulation → jobs).
    #[derive(Clone)]
    struct FakeEngine {
        sends: Arc<Mutex<Vec<serde_json::Value>>>,
        queries: Arc<Mutex<Vec<String>>>,
        cancels: Arc<Mutex<Vec<String>>>,
        log: Arc<Mutex<Vec<String>>>,
    }

    async fn spawn_fake_engine(
        inv_rows: &str,
        journal_rows: &str,
        fixed_invocation: Option<&str>,
        log: Arc<Mutex<Vec<String>>>,
    ) -> (String, FakeEngine) {
        let fake = FakeEngine {
            sends: Arc::new(Mutex::new(Vec::new())),
            queries: Arc::new(Mutex::new(Vec::new())),
            cancels: Arc::new(Mutex::new(Vec::new())),
            log: log.clone(),
        };
        let (sends, queries, cancels, log_cancel) = (
            fake.sends.clone(),
            fake.queries.clone(),
            fake.cancels.clone(),
            log.clone(),
        );
        let inv_rows = Arc::new(inv_rows.to_string());
        let journal_rows = Arc::new(journal_rows.to_string());
        let fixed = Arc::new(fixed_invocation.map(str::to_string));
        let counter = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new()
            .route(
                "/deployments",
                axum::routing::post(|| async {
                    (
                        axum::http::StatusCode::OK,
                        r#"{"deploymentId":"dp_fake"}"#.to_string(),
                    )
                }),
            )
            .route(
                "/DagInterpreter/{key}/run/send",
                axum::routing::post(move |body: String| {
                    let sends = sends.clone();
                    let fixed = fixed.clone();
                    let counter = counter.clone();
                    async move {
                        let valeur: serde_json::Value =
                            serde_json::from_str(&body).unwrap_or(serde_json::Value::String(body));
                        sends.lock().unwrap().push(valeur);
                        let id = match fixed.as_ref() {
                            Some(id) => id.clone(),
                            None => format!("inv_{}", counter.fetch_add(1, Ordering::SeqCst)),
                        };
                        (
                            axum::http::StatusCode::ACCEPTED,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            format!(r#"{{"invocationId":"{id}","status":"Accepted"}}"#),
                        )
                    }
                }),
            )
            .route(
                "/query",
                axum::routing::post(move |body: String| {
                    let (queries, inv_rows, journal_rows) =
                        (queries.clone(), inv_rows.clone(), journal_rows.clone());
                    async move {
                        queries.lock().unwrap().push(body.clone());
                        let table = if body.contains("sys_journal") {
                            journal_rows.as_str()
                        } else {
                            inv_rows.as_str()
                        };
                        (
                            axum::http::StatusCode::OK,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            table.to_string(),
                        )
                    }
                }),
            )
            .route(
                "/invocations/{id}/cancel",
                axum::routing::patch(
                    move |axum::extract::Path(id): axum::extract::Path<String>| {
                        let (cancels, log_cancel) = (cancels.clone(), log_cancel.clone());
                        async move {
                            cancels.lock().unwrap().push(id.clone());
                            log_cancel.lock().unwrap().push(format!("cancel:{id}"));
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
        (format!("http://{addr}"), fake)
    }

    /// Handle moteur branché sur le faux Restate (admin et ingress sur la même base,
    /// endpoint sur port éphémère, backoff de test).
    async fn spawn_engine_on(base: &str) -> WorkflowRuntime {
        let cfg = crate::workflow::WorkflowConfigApp {
            admin_url: base.to_string(),
            ingress_url: base.to_string(),
            deployment_url: "http://vanyline.test:9410".to_string(),
            listen: "127.0.0.1:0".to_string(),
        };
        crate::workflow::runtime::start_with(
            cfg,
            miryad_core::workflow::StepRegistry::new(),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(50),
        )
        .await
        .expect("moteur de test démarré sur le faux Restate")
    }

    /// Base réelle + alice/bob liés à leur Owner + moteur sur le faux Restate W4b.
    async fn seeded_state_runs(
        inv_rows: &str,
        journal_rows: &str,
        fixed_invocation: Option<&str>,
    ) -> (AppState, i32, i32, FakeEngine) {
        let db = real_db().await;
        let alice = miryad_core::users::resolve_user(&db, "alice", Some("alice@example.com"))
            .await
            .expect("alice resolves");
        let bob = miryad_core::users::resolve_user(&db, "bob", Some("bob@example.com"))
            .await
            .expect("bob resolves");
        for (uid, name) in [(alice.id, "alice"), (bob.id, "bob")] {
            owner_links::ActiveModel {
                id: NotSet,
                user_id: Set(uid),
                k8s_owner_name: Set(Some(name.to_string())),
            }
            .insert(&db)
            .await
            .expect("owner link seeded");
        }
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let (base, fake) = spawn_fake_engine(inv_rows, journal_rows, fixed_invocation, log).await;
        let mut state = test_state_with_db(db);
        state.workflow = Some(spawn_engine_on(&base).await);
        (state, alice.id, bob.id, fake)
    }

    const NO_ROWS: &str = r#"{"rows":[]}"#;

    fn invocation_row(id: &str, status: &str) -> String {
        format!(r#"{{"id":"{id}","status":"{status}","created_at":"2026-10-10T12:00:00Z"}}"#)
    }

    fn rows(items: &[String]) -> String {
        format!(r#"{{"rows":[{}]}}"#, items.join(","))
    }

    fn ago_hours(h: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
            .checked_sub_signed(chrono::TimeDelta::try_hours(h).unwrap())
            .unwrap()
    }

    fn ago_days(d: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
            .checked_sub_signed(chrono::TimeDelta::try_days(d).unwrap())
            .unwrap()
    }

    async fn seed_run(
        state: &AppState,
        owner_id: i32,
        project: &str,
        sandbox: Option<&str>,
        run_id: &str,
        invocation: &str,
        started_at: chrono::DateTime<chrono::Utc>,
    ) {
        crate::db::entities::workflow_runs::ActiveModel {
            id: NotSet,
            owner_id: Set(owner_id),
            project_name: Set(project.to_string()),
            sandbox_name: Set(sandbox.map(str::to_string)),
            definition_id: Set(None),
            definition_name: Set("alice/demo/ci".to_string()),
            run_id: Set(run_id.to_string()),
            run_key: Set(format!("key-{run_id}")),
            invocation_id: Set(invocation.to_string()),
            scope: Set(if sandbox.is_some() {
                "sandbox"
            } else {
                "project"
            }
            .to_string()),
            triggered_by: Set("ui".to_string()),
            started_at: Set(started_at),
        }
        .insert(db_of(state))
        .await
        .expect("ligne d'index seedée");
    }

    async fn run_row_count(state: &AppState) -> usize {
        workflow_runs::Entity::find()
            .all(db_of(state))
            .await
            .unwrap()
            .len()
    }

    /// Faux séam cluster des runs (motif `FakeLocator` de `steps/mod.rs`) : phase `Running`,
    /// base du faux serveur sandbox ; sert l'ouverture de `SandboxAccess` dans
    /// `delete_run_jobs`.
    #[derive(Clone)]
    struct FakeLocator {
        base: String,
    }

    #[async_trait]
    impl SandboxLocator for FakeLocator {
        async fn phase(&self, _owner: &str, _sandbox: &str) -> Result<Option<String>, StepError> {
            Ok(Some("Running".to_string()))
        }
        async fn token(&self, _owner: &str) -> Result<String, StepError> {
            Ok("tok-fake".to_string())
        }
        async fn base_url(&self, _owner: &str, _sandbox: &str) -> Result<String, StepError> {
            Ok(self.base.clone())
        }
    }

    /// Séam `SandboxLookup` injecté : CR contrôlée (`inspect`) ; `delete_run_jobs` passe
    /// réellement par `SandboxAccess::open_with` + DELETE HTTP sur le faux serveur (le même
    /// chemin que la production, séam cluster seul factice).
    struct FakeSandboxLookup {
        cr: Option<SandboxCrState>,
        base: Option<String>,
    }

    #[async_trait]
    impl SandboxLookup for FakeSandboxLookup {
        async fn inspect(
            &self,
            _owner: &str,
            _sandbox: &str,
        ) -> Result<Option<SandboxCrState>, AppError> {
            Ok(self.cr.clone())
        }
        async fn delete_run_jobs(
            &self,
            owner: &str,
            user: &str,
            run: &crate::db::entities::workflow_runs::Model,
        ) -> Result<(), AppError> {
            let base = self.base.clone().ok_or_else(|| {
                AppError::InternalError("test : aucune base sandbox configurée".to_string())
            })?;
            let ctx = RunContext {
                run_id: run.run_id.clone(),
                user: user.to_string(),
                user_id: run.owner_id,
                owner: owner.to_string(),
                project: run.project_name.clone(),
                sandbox: run.sandbox_name.clone(),
                triggered_by: run.triggered_by.clone(),
                definition: run.definition_name.clone(),
            };
            let cfg = serde_json::json!({ "_run": serde_json::to_value(&ctx).unwrap() });
            let info = RunInfo::from_config(&cfg).unwrap();
            let access = SandboxAccess::open_with(
                Arc::new(FakeLocator { base }),
                reqwest::Client::new(),
                &info,
            )
            .await
            .map_err(|e| AppError::InternalError(e.message))?;
            access
                .delete(&format!(
                    "/jobs?prefix={run_id}:",
                    run_id = run.run_id.as_str()
                ))
                .await
                .map_err(|e| AppError::InternalError(e.message))?;
            Ok(())
        }
    }

    fn running_cr() -> SandboxCrState {
        SandboxCrState {
            project: "demo".to_string(),
            phase: Some("Running".to_string()),
        }
    }

    /// Faux serveur sandbox (motif `spawn_fake_sandbox` de `steps/mod.rs`) : répond `status`
    /// à toute requête, empile « MÉTHODE uri » dans le log partagé (ordre avec l'admin).
    async fn spawn_fake_sandbox_logged(
        status: u16,
        body: &'static str,
        log: Arc<Mutex<Vec<String>>>,
    ) -> String {
        let app = axum::Router::new().route(
            "/{*rest}",
            axum::routing::any(move |req: axum::extract::Request| {
                let log = log.clone();
                async move {
                    log.lock()
                        .unwrap()
                        .push(format!("{} {}", req.method(), req.uri()));
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        axum::body::Body::from(body),
                    )
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    async fn post_run(
        state: &AppState,
        who: &str,
        project: &str,
        id: i32,
        body: serde_json::Value,
    ) -> Result<(StatusCode, RunLaunch), AppError> {
        let body: CreateRunBody = serde_json::from_value(body).expect("corps de run valide");
        create_run(
            State(state.clone()),
            user(who),
            Path((project.to_string(), id)),
            Json(body),
        )
        .await
        .map(|(status, Json(launch))| (status, launch))
    }

    async fn project_runs(
        state: &AppState,
        who: &str,
        project: &str,
        query: serde_json::Value,
    ) -> Vec<RunListItem> {
        let q: RunListQuery = serde_json::from_value(query).expect("query de liste valide");
        list_project_runs(
            State(state.clone()),
            user(who),
            Path(project.to_string()),
            Query(q),
        )
        .await
        .expect("liste des runs")
        .0
    }

    // =========================================================================
    // Lot W4b — runs : Scenario
    // =========================================================================

    // ── Scenario: lancement_de_projet ───────────────────────────────────────
    // 202, ligne d'index sans `sandbox_name`, run Restate lancé avec `_run.sandbox` absent
    // (le corps capturé de l'ingress est le DAG matérialisé).
    #[tokio::test]
    async fn lancement_de_projet() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow projet créé");

        let (status, launch) = post_run(&state, "alice", "demo", created.id, serde_json::json!({}))
            .await
            .expect("lancement de projet ⇒ 202");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(
            uuid::Uuid::parse_str(&launch.run_id).is_ok(),
            "run_id est un UUID v4 généré par le service : {}",
            launch.run_id
        );
        assert_eq!(
            launch.invocation_id, "inv_0",
            "invocationId lu dans le corps de la réponse de l'ingress"
        );

        let rows = workflow_runs::Entity::find()
            .all(db_of(&state))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sandbox_name, None, "index sans sandbox_name");
        assert_eq!(rows[0].run_id, launch.run_id);
        assert_eq!(rows[0].invocation_id, launch.invocation_id);
        assert!(!rows[0].run_key.is_empty(), "run_key de l'ingress indexé");
        assert_eq!(rows[0].definition_name, "alice/demo/ci", "nom figé");
        assert_eq!(rows[0].scope, "project");
        assert_eq!(rows[0].triggered_by, "ui", "route REST ⇒ triggered_by ui");

        let sends = fake.sends.lock().unwrap();
        assert_eq!(sends.len(), 1, "un seul envoi à l'ingress");
        let etapes = sends[0].as_array().expect("le corps est le DAG sérialisé");
        assert_eq!(etapes.len(), 1);
        let run = &etapes[0]["config"]["_run"];
        assert_eq!(run["run_id"], serde_json::json!(launch.run_id));
        assert!(
            run["sandbox"].is_null(),
            "_run.sandbox absent (null) pour un run de projet : {run}"
        );
        assert_eq!(run["owner"], serde_json::json!("alice"));
        assert_eq!(run["triggered_by"], serde_json::json!("ui"));
    }

    // ── Scenario: lancement_de_sandbox ──────────────────────────────────────
    // Portée sandbox + sandbox `Running` du projet ⇒ 202 et `_run.sandbox == "s1"` dans la
    // config de chaque step ; l'index porte `sandbox_name`.
    #[tokio::test]
    async fn lancement_de_sandbox() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("build", "sandbox", serde_json::json!([shell_step("s")])),
        )
        .await
        .expect("workflow sandbox créé");
        let lookup = Arc::new(FakeSandboxLookup {
            cr: Some(running_cr()),
            base: None,
        });

        let launch = launch_inner(
            &state,
            &user("alice"),
            "demo",
            created.id,
            Some("s1".to_string()),
            "ui",
            Some(lookup),
        )
        .await
        .expect("sandbox Running ⇒ 202");
        assert_eq!(launch.invocation_id, "inv_0");

        {
            let sends = fake.sends.lock().unwrap();
            assert_eq!(sends.len(), 1);
            for etape in sends[0].as_array().expect("DAG matérialisé") {
                assert_eq!(
                    etape["config"]["_run"]["sandbox"],
                    serde_json::json!("s1"),
                    "_run.sandbox == s1 dans chaque step"
                );
            }
        }
        let row = workflow_runs::Entity::find()
            .one(db_of(&state))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.sandbox_name.as_deref(), Some("s1"));
        assert_eq!(row.scope, "sandbox");
    }

    // ── Scenario: sandbox_non_demarree ──────────────────────────────────────
    // Phase `Provisioning` ⇒ 409, AUCUN run (rien déclenché, aucune ligne d'index).
    #[tokio::test]
    async fn sandbox_non_demarree() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("build", "sandbox", serde_json::json!([shell_step("s")])),
        )
        .await
        .expect("workflow sandbox créé");
        let lookup = Arc::new(FakeSandboxLookup {
            cr: Some(SandboxCrState {
                project: "demo".to_string(),
                phase: Some("Provisioning".to_string()),
            }),
            base: None,
        });

        let err = launch_inner(
            &state,
            &user("alice"),
            "demo",
            created.id,
            Some("s1".to_string()),
            "ui",
            Some(lookup),
        )
        .await
        .expect_err("Provisioning n'est pas Running ⇒ refus");
        assert_eq!(status_of(err).await, StatusCode::CONFLICT);
        assert!(fake.sends.lock().unwrap().is_empty(), "aucun run déclenché");
        assert_eq!(run_row_count(&state).await, 0, "aucune ligne d'index");
    }

    // Raises 409 : sandbox d'un autre projet. Raises 404 : CR absente (404 identique,
    // aucune fuite — même variante que le test `sandbox_absente_rend_404_identique`).
    #[tokio::test]
    async fn sandbox_hors_projet_rend_409() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("build", "sandbox", serde_json::json!([shell_step("s")])),
        )
        .await
        .expect("workflow sandbox créé");
        let lookup = Arc::new(FakeSandboxLookup {
            cr: Some(SandboxCrState {
                project: "autre-projet".to_string(),
                phase: Some("Running".to_string()),
            }),
            base: None,
        });

        let err = launch_inner(
            &state,
            &user("alice"),
            "demo",
            created.id,
            Some("s1".to_string()),
            "ui",
            Some(lookup),
        )
        .await
        .expect_err("la sandbox doit être du projet visé");
        assert_eq!(status_of(err).await, StatusCode::CONFLICT);
        assert!(fake.sends.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sandbox_absente_rend_404_identique() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("build", "sandbox", serde_json::json!([shell_step("s")])),
        )
        .await
        .expect("workflow sandbox créé");
        let lookup = Arc::new(FakeSandboxLookup {
            cr: None,
            base: None,
        });

        let err = launch_inner(
            &state,
            &user("alice"),
            "demo",
            created.id,
            Some("fantome".to_string()),
            "ui",
            Some(lookup.clone()),
        )
        .await
        .expect_err("CR absente ⇒ 404");
        let (statut, corps) = render(err).await;
        assert_eq!(statut, StatusCode::NOT_FOUND);

        let err = launch_inner(
            &state,
            &user("alice"),
            "demo",
            created.id,
            Some("inexistante-aussi".to_string()),
            "ui",
            Some(lookup),
        )
        .await
        .expect_err("toute absence rend le même 404");
        let (statut2, corps2) = render(err).await;
        assert_eq!(statut2, statut, "404 identique, aucune fuite d'existence");
        assert_eq!(corps2, corps, "corps identique octet à octet");
        assert!(fake.sends.lock().unwrap().is_empty());
    }

    // ── Scenario: sandbox_pour_un_workflow_de_projet ────────────────────────
    // 400 — et RIEN déclenché (le séam sandbox n'est même pas consulté).
    #[tokio::test]
    async fn sandbox_pour_un_workflow_de_projet() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow projet créé");

        let err = post_run(
            &state,
            "alice",
            "demo",
            created.id,
            serde_json::json!({"sandbox": "s1"}),
        )
        .await
        .expect_err("une sandbox est refusée en portée projet");
        assert_eq!(status_of(err).await, StatusCode::BAD_REQUEST);
        assert!(fake.sends.lock().unwrap().is_empty());
        assert_eq!(run_row_count(&state).await, 0);
    }

    // Raises 409 symétrique : workflow de portée sandbox lancé sans `sandbox`.
    #[tokio::test]
    async fn workflow_de_sandbox_sans_sandbox_rend_409() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("build", "sandbox", serde_json::json!([shell_step("s")])),
        )
        .await
        .expect("workflow sandbox créé");

        let err = post_run(&state, "alice", "demo", created.id, serde_json::json!({}))
            .await
            .expect_err("la portée sandbox exige une sandbox");
        assert_eq!(status_of(err).await, StatusCode::CONFLICT);
        assert!(fake.sends.lock().unwrap().is_empty());
    }

    // ── Scenario: definition_editee_par_un_autre_chemin ─────────────────────
    // Un step `shell` injecté dans la définition hors de cette API (UPDATE SQL direct) ⇒
    // 422 AU LANCEMENT (la validation de portée est refaite), rien déclenché.
    #[tokio::test]
    async fn definition_editee_par_un_autre_chemin() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow projet créé");
        db_of(&state)
            .execute_unprepared(
                "UPDATE miryad_workflow_definitions SET steps =
                 '[{\"id\":\"s\",\"depends_on\":[],\"kind\":\"shell\",\"config\":{\"cmd\":\"make\"}}]'",
            )
            .await
            .expect("step shell injecté hors API");

        let err = post_run(&state, "alice", "demo", created.id, serde_json::json!({}))
            .await
            .expect_err("la revalidation au lancement refuse le step shell");
        assert_eq!(status_of(err).await, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            fake.sends.lock().unwrap().is_empty(),
            "rien n'est déclenché"
        );
        assert_eq!(run_row_count(&state).await, 0);
    }

    // « Ne jamais lancer sans index » : l'insertion échoue (UNIQUE sur `invocation_id` —
    // le faux ingress rend un `invocationId` fixe déjà présent en base) ⇒ 500 et le faux
    // admin reçoit le `PATCH …/cancel` de l'invocation qui vient d'être déclenchée.
    #[tokio::test]
    async fn echec_index_annule_invocation() {
        let (state, alice_id, _bob_id, fake) =
            seeded_state_runs(NO_ROWS, NO_ROWS, Some("inv_fixed")).await;
        seed_run(
            &state,
            alice_id,
            "demo",
            None,
            "run-occupee",
            "inv_fixed",
            chrono::Utc::now(),
        )
        .await;
        let (_, created) = post_workflow(
            &state,
            "alice",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("workflow projet créé");

        let err = post_run(&state, "alice", "demo", created.id, serde_json::json!({}))
            .await
            .expect_err("l'indexation en échec fait échouer le lancement");
        assert_eq!(
            status_of(err).await,
            StatusCode::INTERNAL_SERVER_ERROR,
            "jamais un run sans index ⇒ 500"
        );
        assert_eq!(
            fake.cancels.lock().unwrap().as_slice(),
            ["inv_fixed".to_string()],
            "l'invocation déclenchée est annulée après l'échec d'indexation"
        );
        assert_eq!(
            run_row_count(&state).await,
            1,
            "seule la ligne préexistante reste"
        );
    }

    // ── Scenario: liste_des_runs_fenetree ───────────────────────────────────
    // 60 runs dont 8 vieux de 20 jours ⇒ au plus 50, aucun de plus de 14 jours, du plus
    // récent au plus ancien, statuts lus en UNE seule lecture groupée.
    #[tokio::test]
    async fn liste_des_runs_fenetree() {
        let recents: Vec<String> = (0..52)
            .map(|i| invocation_row(&format!("inv_f_{i}"), "running"))
            .collect();
        let vieux: Vec<String> = (0..8)
            .map(|j| invocation_row(&format!("inv_o_{j}"), "completed"))
            .collect();
        let mut lignes = recents;
        lignes.extend(vieux);
        let (state, alice_id, _bob_id, fake) =
            seeded_state_runs(&rows(&lignes), NO_ROWS, None).await;
        for i in 0..52u32 {
            seed_run(
                &state,
                alice_id,
                "demo",
                None,
                &format!("r-f-{i}"),
                &format!("inv_f_{i}"),
                ago_hours(i64::from(i)),
            )
            .await;
        }
        for j in 0..8u32 {
            seed_run(
                &state,
                alice_id,
                "demo",
                None,
                &format!("r-o-{j}"),
                &format!("inv_o_{j}"),
                ago_days(20),
            )
            .await;
        }

        let items = project_runs(&state, "alice", "demo", serde_json::json!({})).await;
        assert_eq!(items.len(), 50, "au plus 50 runs rendus");
        let plancher = ago_days(14);
        for item in &items {
            assert!(
                item.started_at >= plancher,
                "aucun run de plus de 14 jours : {} à {}",
                item.run_id,
                item.started_at
            );
        }
        for paire in items.windows(2) {
            assert!(
                paire[0].started_at >= paire[1].started_at,
                "ordre décroissant : {} avant {}",
                paire[0].run_id,
                paire[1].run_id
            );
        }
        assert_eq!(items[0].run_id, "r-f-0", "le plus récent en premier");
        assert_eq!(
            fake.queries.lock().unwrap().len(),
            1,
            "UNE seule requête /query pour les 50 statuts"
        );
    }

    // ── Scenario: run_purge_par_restate ─────────────────────────────────────
    // Une ligne d'index dont Restate ne connaît plus l'invocation rend
    // `status.state == "Unknown"` — en liste comme en détail.
    #[tokio::test]
    async fn run_purge_par_restate() {
        let (state, alice_id, _bob_id, _fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        seed_run(
            &state,
            alice_id,
            "demo",
            None,
            "r-purge",
            "inv_ghost",
            chrono::Utc::now(),
        )
        .await;

        let items = project_runs(&state, "alice", "demo", serde_json::json!({})).await;
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].status.state, "Unknown",
            "invocation purgée ⇒ Unknown"
        );

        let Json(detail) = get_run(
            State(state.clone()),
            user("alice"),
            Path("r-purge".to_string()),
        )
        .await
        .expect("le détail d'un run purgé rend un statut, pas une erreur");
        assert_eq!(detail.status.state, "Unknown");
        assert!(detail.status.steps.is_empty());
    }

    // Appoint `Exposes` : `GET /runs/{run_id}` rend le détail AVEC les steps du journal.
    #[tokio::test]
    async fn detail_de_run_avec_steps() {
        let (state, alice_id, _bob_id, _fake) = seeded_state_runs(
            &rows(&[invocation_row("inv_live", "running")]),
            &rows(&[
                r#"{"entry_type":"StartSideEffect","name":"a","completed":true,"appended_at":"2026-10-10T12:01:00Z"}"#.to_string(),
                r#"{"entry_type":"StartSideEffect","name":"b","completed":false,"appended_at":"2026-10-10T12:02:00Z"}"#.to_string(),
            ]),
            None,
        )
        .await;
        seed_run(
            &state,
            alice_id,
            "demo",
            None,
            "r-live",
            "inv_live",
            chrono::Utc::now(),
        )
        .await;

        let Json(detail) = get_run(
            State(state.clone()),
            user("alice"),
            Path("r-live".to_string()),
        )
        .await
        .expect("détail");
        assert_eq!(detail.run_id, "r-live");
        assert_eq!(detail.workflow, "alice/demo/ci");
        assert_eq!(detail.status.state, "Running");
        assert_eq!(
            detail.status.steps.len(),
            2,
            "un StepStatus par step du journal"
        );
        assert_eq!(detail.status.steps[0].step_id, "a");
        assert_eq!(detail.status.steps[0].state, "Succeeded");
        assert_eq!(detail.status.steps[1].step_id, "b");
        assert_eq!(detail.status.steps[1].state, "Running");
    }

    // ── Scenario: runs_d_une_sandbox ────────────────────────────────────────
    // `GET /sandboxes/s1/runs` ⇒ exactement les runs de `s1` (ni `s2`, ni les runs de
    // portée projet).
    #[tokio::test]
    async fn runs_d_une_sandbox() {
        let (state, alice_id, _bob_id, _fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        seed_run(
            &state,
            alice_id,
            "demo",
            Some("s1"),
            "r-a",
            "inv_a",
            ago_hours(1),
        )
        .await;
        seed_run(
            &state,
            alice_id,
            "demo",
            Some("s1"),
            "r-b",
            "inv_b",
            ago_hours(2),
        )
        .await;
        seed_run(
            &state,
            alice_id,
            "demo",
            Some("s2"),
            "r-c",
            "inv_c",
            ago_hours(3),
        )
        .await;
        seed_run(&state, alice_id, "demo", None, "r-d", "inv_d", ago_hours(4)).await;

        let items = list_sandbox_runs(
            State(state.clone()),
            user("alice"),
            Path("s1".to_string()),
            Query(RunListQuery::default()),
        )
        .await
        .expect("liste des runs de s1")
        .0;
        assert_eq!(items.len(), 2, "seuls les runs de s1");
        for item in &items {
            assert_eq!(item.sandbox.as_deref(), Some("s1"));
        }
        assert_eq!(items[0].run_id, "r-a", "ordre décroissant conservé");
    }

    // ── Scenario: annulation ────────────────────────────────────────────────
    // Run de sandbox : le `PATCH …/cancel` de l'admin est émis PUIS
    // `DELETE /jobs?prefix={run_id}:` sur la sandbox (ordre et préfixe exacts, contrat
    // `sandbox/src/jobs.sdd`). Le 404 du sandbox sur le DELETE est best-effort : le 202 est
    // rendu quand même. Volet appoint : un run de projet annule sans toucher aucune sandbox.
    #[tokio::test]
    async fn annulation() {
        let (state, alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        seed_run(
            &state,
            alice_id,
            "demo",
            Some("s1"),
            "run-annuler",
            "inv_live",
            chrono::Utc::now(),
        )
        .await;
        seed_run(
            &state,
            alice_id,
            "demo",
            None,
            "run-projet",
            "inv_proj",
            chrono::Utc::now(),
        )
        .await;
        // Le sandbox répond 404 au DELETE : best-effort, jamais un refus du 202.
        let base = spawn_fake_sandbox_logged(404, "not found", fake.log.clone()).await;
        let lookup = Arc::new(FakeSandboxLookup {
            cr: Some(running_cr()),
            base: Some(base),
        });

        let status = cancel_run_inner(&state, &user("alice"), "run-annuler", Some(lookup.clone()))
            .await
            .expect("le 404 du sandbox n'empêche pas le 202");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(
            fake.cancels.lock().unwrap().as_slice(),
            ["inv_live".to_string()],
            "l'invocation est annulée"
        );
        {
            let log = fake.log.lock().unwrap();
            assert_eq!(
                log.as_slice(),
                [
                    "cancel:inv_live".to_string(),
                    "DELETE /jobs?prefix=run-annuler:".to_string(),
                ],
                "invocation d'abord, jobs ensuite, préfixe exact `{{run_id}}:`"
            );
        }

        // Appoint : run de portée projet ⇒ aucune requête sandbox.
        let status = cancel_run_inner(&state, &user("alice"), "run-projet", Some(lookup))
            .await
            .expect("annulation d'un run de projet");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(
            fake.cancels.lock().unwrap().as_slice(),
            ["inv_live".to_string(), "inv_proj".to_string()]
        );
        assert_eq!(
            fake.log.lock().unwrap().as_slice(),
            [
                "cancel:inv_live".to_string(),
                "DELETE /jobs?prefix=run-annuler:".to_string(),
                "cancel:inv_proj".to_string(),
            ],
            "aucune requête sandbox de plus : le run de projet n'émet que le cancel"
        );
    }

    // ── Scenario: workflow_d_un_autre_utilisateur (volet lancer) ────────────
    // Alice lance le workflow de bob ⇒ 404 identique octet à byte à « inexistant », rien
    // déclenché.
    #[tokio::test]
    async fn workflow_d_un_autre_utilisateur_lancement() {
        let (state, _alice_id, _bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        let (_, created) = post_workflow(
            &state,
            "bob",
            "demo",
            body_json("ci", "project", serde_json::json!([agent_step("a")])),
        )
        .await
        .expect("bob crée son workflow");

        let autre = post_run(&state, "alice", "demo", created.id, serde_json::json!({}))
            .await
            .expect_err("alice ne lance pas le workflow de bob");
        let inexistant = post_run(&state, "alice", "demo", 99_999, serde_json::json!({}))
            .await
            .expect_err("id inexistant ⇒ erreur");
        let (statut_a, corps_a) = render(autre).await;
        let (statut_i, corps_i) = render(inexistant).await;
        assert_eq!(statut_a, StatusCode::NOT_FOUND);
        assert_eq!(statut_i, statut_a, "404 identique inexistant / à un autre");
        assert_eq!(corps_i, corps_a, "corps identique octet à octet");
        assert!(fake.sends.lock().unwrap().is_empty(), "aucune action");
    }

    // Must : le même 404 indistinguable s'applique aux RUNS (détail et annulation).
    #[tokio::test]
    async fn run_inexistant_et_d_un_autre_rendent_le_meme_404() {
        let (state, _alice_id, bob_id, fake) = seeded_state_runs(NO_ROWS, NO_ROWS, None).await;
        seed_run(
            &state,
            bob_id,
            "demo",
            None,
            "run-de-bob",
            "inv_bob",
            chrono::Utc::now(),
        )
        .await;

        let (statut_a, corps_a) = render(
            get_run(
                State(state.clone()),
                user("alice"),
                Path("run-de-bob".to_string()),
            )
            .await
            .expect_err("le run de bob est invisible"),
        )
        .await;
        let (statut_i, corps_i) = render(
            get_run(
                State(state.clone()),
                user("alice"),
                Path("run-jamais-vue".to_string()),
            )
            .await
            .expect_err("run inexistant"),
        )
        .await;
        assert_eq!(statut_a, StatusCode::NOT_FOUND);
        assert_eq!(statut_i, statut_a, "lecture : 404 identique");
        assert_eq!(corps_i, corps_a, "lecture : corps identique octet à octet");

        let (statut_a, corps_a) = render(
            cancel_run(
                State(state.clone()),
                user("alice"),
                Path("run-de-bob".to_string()),
            )
            .await
            .expect_err("annulation du run de bob refusée"),
        )
        .await;
        let (statut_i, corps_i) = render(
            cancel_run(
                State(state.clone()),
                user("alice"),
                Path("run-jamais-vue".to_string()),
            )
            .await
            .expect_err("annulation inexistante refusée"),
        )
        .await;
        assert_eq!(statut_a, StatusCode::NOT_FOUND);
        assert_eq!(statut_i, statut_a, "annulation : 404 identique");
        assert_eq!(
            corps_i, corps_a,
            "annulation : corps identique octet à octet"
        );
        assert!(
            fake.cancels.lock().unwrap().is_empty(),
            "aucune annulation émise"
        );
    }

    // ── Scenario: moteur_non_configure (volet runs) ─────────────────────────
    // Les 5 routes runs répondent 503 AVANT toute lecture de base (base simulée sans
    // résultats : une lecture produirait un 500).
    #[tokio::test]
    async fn moteur_non_configure_rend_503_sur_les_cinq_routes_runs() {
        let state = test_state_mock_db();
        let cas: Vec<(&str, Result<StatusCode, AppError>)> = vec![
            (
                "POST …/runs",
                create_run(
                    State(state.clone()),
                    user("alice"),
                    Path(("demo".to_string(), 1)),
                    Json(CreateRunBody::default()),
                )
                .await
                .map(|_| StatusCode::OK),
            ),
            (
                "GET /projects/{p}/runs",
                list_project_runs(
                    State(state.clone()),
                    user("alice"),
                    Path("demo".to_string()),
                    Query(RunListQuery::default()),
                )
                .await
                .map(|_| StatusCode::OK),
            ),
            (
                "GET /sandboxes/{s}/runs",
                list_sandbox_runs(
                    State(state.clone()),
                    user("alice"),
                    Path("s1".to_string()),
                    Query(RunListQuery::default()),
                )
                .await
                .map(|_| StatusCode::OK),
            ),
            (
                "GET /runs/{run_id}",
                get_run(State(state.clone()), user("alice"), Path("r".to_string()))
                    .await
                    .map(|_| StatusCode::OK),
            ),
            (
                "POST /runs/{run_id}/cancel",
                cancel_run(State(state.clone()), user("alice"), Path("r".to_string()))
                    .await
                    .map(|_| StatusCode::OK),
            ),
        ];
        for (libelle, res) in cas {
            let err = res.expect_err("{libelle} sans moteur doit rendre une erreur");
            assert_eq!(
                status_of(err).await,
                StatusCode::SERVICE_UNAVAILABLE,
                "{libelle} doit rendre 503"
            );
        }
    }

    // Bornes contractuelles (`Exposes` : limit ≤ 50, since au plus 14 jours) — tranche :
    // BORNÉES silencieusement (clamp), verrouillé ici : limit=51 rend 50, since=30 jours ne
    // sort jamais de la fenêtre de 14 jours.
    #[tokio::test]
    async fn bornes_des_listes_limit_et_since() {
        let lignes: Vec<String> = (0..53)
            .map(|i| invocation_row(&format!("inv_b_{i}"), "running"))
            .collect();
        let (state, alice_id, _bob_id, _fake) =
            seeded_state_runs(&rows(&lignes), NO_ROWS, None).await;
        for i in 0..52u32 {
            seed_run(
                &state,
                alice_id,
                "demo",
                None,
                &format!("r-b-{i}"),
                &format!("inv_b_{i}"),
                ago_hours(i64::from(i)),
            )
            .await;
        }
        seed_run(
            &state,
            alice_id,
            "demo",
            None,
            "r-vieux",
            "inv_b_52",
            ago_days(20),
        )
        .await;

        let items = project_runs(
            &state,
            "alice",
            "demo",
            serde_json::json!({"limit": 51, "since": 30}),
        )
        .await;
        assert_eq!(items.len(), 50, "limit=51 est borné à 50");
        assert!(
            items.iter().all(|i| i.started_at >= ago_days(14)),
            "since=30 est borné à 14 jours : la ligne de 20 jours ne sort pas"
        );
    }
}
