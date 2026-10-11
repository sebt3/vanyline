//! Routes REST des workflows de projet (CRUD des définitions) — contrat : `./workflows.sdd`.
//!
//! LOT W4a : `GET/POST /projects/{project}/workflows`, `GET/PUT/DELETE …/{id}`, 503 moteur,
//! propriété (404 identique inexistant/autre). Les routes de RUNS et la fonction de service
//! `launch_workflow` (lot W4b) vivront dans les sections « runs » en bas de ce fichier ;
//! les helpers de propriété ([`owner_context`], [`load_owned`]) et la garde moteur
//! ([`engine`]) sont déjà factorisés pour elles.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};

use miryad_core::auth::{AuthPrincipal, AuthUser, PrincipalSource};
use miryad_core::resource::MiryadResource;
use miryad_core::users::resolve_user;
use miryad_core::workflow::{
    DagSteps, StepDefinition, WorkflowDefinition, WorkflowDefinitionActiveModel,
    WorkflowDefinitionColumn,
};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ActiveValue::Set, ColumnTrait, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, TransactionTrait,
};

use crate::db::entities::project_workflows::{
    Column as ProjectWorkflowColumn, Entity as ProjectWorkflow, Model as ProjectWorkflowModel,
};
use crate::db::entities::workflow_runs::{Column as WorkflowRunColumn, Entity as WorkflowRun};
use crate::workflow::WorkflowRuntime;
use crate::workflow::run_context::{RunScope, allowed_kinds};
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
}
