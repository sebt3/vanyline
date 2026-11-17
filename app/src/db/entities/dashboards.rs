//! Ressource `dashboards` (`DEC-3`) — contrat : `./dashboards.sdd`. Une ligne = une
//! définition PORTÉE par un utilisateur (personnalisation `custom` d'un écran, ou tableau
//! `complementary` d'un projet). CRUD par miryad-core (`resource_router`, MCP, module
//! lymaille `resources`) ; LA garde-d'écriture FR-3 vit dans les hooks : toute définition
//! invalide est refusée `VNL-APP-DSH-*`, jamais corrigée.

use miryad_core::auth::AuthPrincipal;
use miryad_core::resource::{AccessPolicy, HookError, MiryadResource};
use sea_orm::ActiveValue;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

use crate::db::entities::{projects, sandboxes};

/// Ligne de définition portée : clés d'écran (`scope`, `kind`, `project_id`), information
/// (`sandbox_id`), libellé, définition JSON `{layout, widgets}` (`DEC-3`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = Dashboard)]
#[sea_orm(table_name = "vanyline_dashboards")]
pub struct Model {
    // `#[serde(default)]` sur `id`/`owner_id` : les payloads de création du frontend
    // n'envoient ni l'un ni l'autre (écrasés de toute façon par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub owner_id: i32,
    /// Écran : `"home" | "project" | "sandbox"` (validé au hook — colonne texte, précédent
    /// `project_workflows.scope`).
    pub scope: String,
    /// Origine : `"custom" | "complementary"` à l'écriture (`"default"`/`"ephemeral"` refusés
    /// au hook — la base ne porte que ce que l'utilisateur écrit, `DEC-2`).
    pub kind: String,
    /// Projet de l'écran (FK `vanyline_projects.id`, supprimée avec le projet) ; NULL pour
    /// `home`.
    pub project_id: Option<i32>,
    /// Sandbox d'origine (FK `vanyline_sandboxes.id`, vidée à la suppression de la sandbox) ;
    /// information hors clé d'écran ; NULL ailleurs.
    pub sandbox_id: Option<i32>,
    /// Libellé (colonne, pas dupliquée dans `def`).
    pub title: String,
    /// Définition `{layout, widgets}` — les clés d'écran NE SONT PAS dans le JSON.
    pub def: serde_json::Value,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "dashboards"
    }
    fn read_policy() -> AccessPolicy {
        AccessPolicy::OwnerOnly
    }
    fn write_policy() -> AccessPolicy {
        AccessPolicy::OwnerOnly
    }
    fn owner_column() -> Option<Column> {
        Some(Column::OwnerId)
    }
    fn filterable_columns() -> Vec<Column> {
        vec![Column::Scope, Column::Kind, Column::ProjectId]
    }
    fn sortable_columns() -> Vec<Column> {
        vec![Column::Id, Column::Title, Column::Scope, Column::Kind]
    }

    /// Garde d'écriture FR-3 (règles a→h de `./dashboards.sdd`). Les règles a→f sont pures ;
    /// g (références) et h (unicité) interrogent la connexion fournie par miryad-core.
    async fn before_create<C>(
        db: &C,
        active: Self::ActiveModel,
        principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError>
    where
        C: sea_orm::ConnectionTrait,
        Self::ActiveModel: Send,
    {
        let ecrivain = ecrivain(db, principal).await?;
        valider_creation(db, ecrivain, &active).await?;
        Ok(active)
    }

    /// La paire effective est revalidée : les champs non posés viennent de la ligne relue
    /// `existing` (prêtée en lecture seule). La ligne elle-même est exclue de la règle h.
    async fn before_update<C>(
        db: &C,
        active: Self::ActiveModel,
        existing: &Self::Model,
        principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError>
    where
        C: sea_orm::ConnectionTrait,
        Self::ActiveModel: Send,
    {
        let ecrivain = ecrivain(db, principal).await?;
        valider_mise_a_jour(db, ecrivain, &active, existing).await?;
        Ok(active)
    }
}

/// Marqueur `VNL-APP-DSH-007` : incohérence d'écran ou de définition. Message fixe, jamais le
/// contenu d'un widget ni la valeur fautive.
fn incoherent(message: &str) -> HookError {
    HookError::with_code("VNL-APP-DSH-007", message)
}

/// Erreur de base pendant la validation : publiée sans contenu embarqué (`VNL-DB-006`, le
/// marqueur de base de l'app), détail journalisé.
fn erreur_base(erreur: &DbErr) -> HookError {
    tracing::error!("VNL-DB-006: validation de tableau de bord: {erreur}");
    HookError::with_code(
        "VNL-DB-006",
        "database error while validating the dashboard",
    )
}

/// Identifiant de l'ÉCRIVAIN (identité de la requête, jamais un `owner_id` posé par le
/// client). `resolve_user` ne crée une ligne que si l'utilisateur n'existe pas encore.
async fn ecrivain<C: sea_orm::ConnectionTrait>(
    db: &C,
    principal: &AuthPrincipal,
) -> Result<i32, HookError> {
    miryad_core::users::resolve_user(db, &principal.subject, principal.email.as_deref())
        .await
        .map(|user| user.id)
        .map_err(|erreur| erreur_base(&erreur))
}

/// Lecture d'une colonne `String` de l'`ActiveModel` : `NotSet` vaut « absent » (règle du
/// hook : une clé d'écran absente est un manquement 007, jamais une valeur implicite).
fn texte(valeur: &ActiveValue<String>) -> Option<&str> {
    match valeur {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => Some(v.as_str()),
        ActiveValue::NotSet => None,
    }
}

/// Valeur EFFECTIVE d'une colonne `Option<i32>` à la création : posée (`NULL` ou valeur) ou
/// non posée — non posé vaut `NULL` (une clé absente est tranchée par les règles b, jamais par
/// une valeur implicite).
fn cle(valeur: &ActiveValue<Option<i32>>) -> Option<i32> {
    match valeur {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => *v,
        ActiveValue::NotSet => None,
    }
}

/// Valeur EFFECTIVE à la mise à jour : posée vaut la valeur posée (`Some`/`NULL` explicite),
/// non posée est REPRISE de la ligne `existing`.
fn cle_reprise(valeur: &ActiveValue<Option<i32>>, existante: Option<i32>) -> Option<i32> {
    match valeur {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => *v,
        ActiveValue::NotSet => existante,
    }
}

/// Lecture de la colonne JSON `def`.
fn definition(valeur: &ActiveValue<serde_json::Value>) -> Option<&serde_json::Value> {
    match valeur {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => Some(v),
        ActiveValue::NotSet => None,
    }
}

/// Règles a→f de `./dashboards.sdd` sur les VALEURS EFFECTIVES (l'appelant a résolu les
/// champs non posés d'une mise à jour depuis la ligne `existing`). Premier manquement seul,
/// `validate_definition` (001..006) en dernier. Aucune lecture de base.
fn valider_ecran(
    kind: Option<&str>,
    scope: Option<&str>,
    project: Option<i32>,
    sandbox: Option<i32>,
    title: Option<&str>,
    def: Option<&serde_json::Value>,
) -> Result<(), HookError> {
    // a. kind écrivable uniquement.
    let ecrivable = matches!(kind, Some("custom" | "complementary"));
    if !ecrivable {
        return Err(incoherent(
            "kind must be 'custom' or 'complementary' (VNL-APP-DSH-007)",
        ));
    }
    // b. cohérence du couple (scope, project_id, sandbox_id).
    let coherent = match scope {
        Some("home") => project.is_none() && sandbox.is_none(),
        Some("project") => project.is_some() && sandbox.is_none(),
        Some("sandbox") => project.is_some(),
        _ => false,
    };
    if !coherent {
        return Err(incoherent(
            "invalid screen keys (scope, project_id, sandbox_id) (VNL-APP-DSH-007)",
        ));
    }
    // c. un complémentaire est un tableau de projet (`FR-5`).
    if kind == Some("complementary") && scope != Some("project") {
        return Err(incoherent(
            "complementary dashboards require scope 'project' (VNL-APP-DSH-007)",
        ));
    }
    // d. libellé non vide, borné (borne posée par la spec, `[?]` de ses Tasks).
    let titre_ok = title.is_some_and(|t| !t.is_empty() && t.chars().count() <= 200);
    if !titre_ok {
        return Err(incoherent(
            "title must be non-empty and at most 200 characters (VNL-APP-DSH-007)",
        ));
    }
    // e. définition désérialisable (tolérante aux clés inconnues : `DashboardDef` ignore
    // `id`/`title` par `#[serde(default)]`).
    let Some(brute) = def else {
        return Err(incoherent(
            "def must deserialize to a dashboard definition (VNL-APP-DSH-007)",
        ));
    };
    let reconstruite: crate::dashboards::model::DashboardDef =
        match serde_json::from_value(brute.clone()) {
            Ok(d) => d,
            Err(_) => {
                return Err(incoherent(
                    "def must deserialize to a dashboard definition (VNL-APP-DSH-007)",
                ));
            }
        };
    // f. garde-fou FR-3 PARTAGÉ (`../../dashboards/mod.sdd`) — marqueurs 001..006 rendus
    // tels quels.
    crate::dashboards::validate_definition(&reconstruite)
        .map_err(|(code, message)| HookError::with_code(code, message))
}

/// Règle g (`./dashboards.sdd`) : `project_id` désigne un projet de l'écrivain et, si
/// `sandbox_id` est renseigné, une sandbox de CE projet et de l'écrivain. Sinon 007.
async fn references_valides<C: sea_orm::ConnectionTrait>(
    db: &C,
    ecrivain: i32,
    project: Option<i32>,
    sandbox: Option<i32>,
) -> Result<(), HookError> {
    let Some(id_projet) = project else {
        // Hors écran projet/sandbox : la règle b a déjà refusé une sandbox sans projet.
        return if sandbox.is_none() {
            Ok(())
        } else {
            Err(incoherent(
                "sandbox_id without project_id (VNL-APP-DSH-007)",
            ))
        };
    };
    let trouve = projects::Entity::find_by_id(id_projet)
        .filter(projects::Column::OwnerId.eq(ecrivain))
        .one(db)
        .await
        .map_err(|erreur| erreur_base(&erreur))?;
    if trouve.is_none() {
        return Err(incoherent(
            "project_id must designate a project of the writer (VNL-APP-DSH-007)",
        ));
    }
    if let Some(sandbox_id) = sandbox {
        let trouvee = sandboxes::Entity::find_by_id(sandbox_id)
            .filter(sandboxes::Column::OwnerId.eq(ecrivain))
            .filter(sandboxes::Column::ProjectId.eq(id_projet))
            .one(db)
            .await
            .map_err(|erreur| erreur_base(&erreur))?;
        if trouvee.is_none() {
            return Err(incoherent(
                "sandbox_id must designate a sandbox of that project (VNL-APP-DSH-007)",
            ));
        }
    }
    Ok(())
}

/// Règle h (`./dashboards.sdd`) : pour `kind="custom"`, aucune AUTRE ligne `custom` du même
/// propriétaire sur le même écran `(scope, project_id)`. Marqueur 009 (422) sinon.
async fn unicite_personnalisation<C: sea_orm::ConnectionTrait>(
    db: &C,
    ecrivain: i32,
    scope: &str,
    project: Option<i32>,
    exclue: Option<i32>,
) -> Result<(), HookError> {
    let mut recherche = Entity::find()
        .filter(Column::OwnerId.eq(ecrivain))
        .filter(Column::Scope.eq(scope))
        .filter(Column::Kind.eq("custom"));
    recherche = match project {
        Some(p) => recherche.filter(Column::ProjectId.eq(p)),
        None => recherche.filter(Column::ProjectId.is_null()),
    };
    if let Some(id) = exclue {
        recherche = recherche.filter(Column::Id.ne(id));
    }
    let existante = recherche
        .one(db)
        .await
        .map_err(|erreur| erreur_base(&erreur))?;
    if existante.is_some() {
        return Err(HookError::with_code(
            "VNL-APP-DSH-009",
            "a custom dashboard already exists for this screen (VNL-APP-DSH-009)",
        ));
    }
    Ok(())
}

/// Hook de création : règles a→f (pures), puis g, puis h pour `custom`.
async fn valider_creation<C: sea_orm::ConnectionTrait>(
    db: &C,
    ecrivain: i32,
    active: &ActiveModel,
) -> Result<(), HookError> {
    let kind = texte(&active.kind);
    let scope = texte(&active.scope);
    let project = cle(&active.project_id);
    let sandbox = cle(&active.sandbox_id);
    valider_ecran(
        kind,
        scope,
        project,
        sandbox,
        texte(&active.title),
        definition(&active.def),
    )?;
    references_valides(db, ecrivain, project, sandbox).await?;
    if let (Some("custom"), Some(scope)) = (kind, scope) {
        unicite_personnalisation(db, ecrivain, scope, project, None).await?;
    }
    Ok(())
}

/// Hook de mise à jour : la paire effective (champs non posés repris de `existing`) subit les
/// mêmes règles ; la ligne elle-même est exclue de la règle h.
async fn valider_mise_a_jour<C: sea_orm::ConnectionTrait>(
    db: &C,
    ecrivain: i32,
    active: &ActiveModel,
    existing: &Model,
) -> Result<(), HookError> {
    let kind = texte(&active.kind).unwrap_or(existing.kind.as_str());
    let scope = texte(&active.scope).unwrap_or(existing.scope.as_str());
    let project = cle_reprise(&active.project_id, existing.project_id);
    let sandbox = cle_reprise(&active.sandbox_id, existing.sandbox_id);
    valider_ecran(
        Some(kind),
        Some(scope),
        project,
        sandbox,
        Some(texte(&active.title).unwrap_or(existing.title.as_str())),
        definition(&active.def).or(Some(&existing.def)),
    )?;
    references_valides(db, ecrivain, project, sandbox).await?;
    if kind == "custom" {
        unicite_personnalisation(db, ecrivain, scope, project, Some(existing.id)).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use miryad_core::rest::resource_router;
    use sea_orm::{ActiveValue::NotSet, ConnectionTrait, IntoActiveModel, Statement};
    use tower::ServiceExt;

    use crate::db::test_support::real_db;

    /// Routeur CRUD de la ressource, monté comme dans `../main.rs`, base réelle.
    async fn app_with_token() -> (axum::Router, sea_orm::DatabaseConnection) {
        let db = real_db().await;
        let state = crate::auth::test_support::test_auth_state_with_db(db.clone());
        let app = axum::Router::new()
            .merge(resource_router::<Entity, crate::AppState>())
            .with_state(crate::AppState {
                config: crate::config::Config {
                    oidc_issuer_url: "https://issuer.example.com".to_string(),
                    oidc_client_id: "client-id".to_string(),
                    oidc_client_secret: "client-secret".to_string(),
                    oidc_redirect_url: "https://app.example.com/callback".to_string(),
                    oidc_scopes: vec![],
                    oidc_ca_cert: None,
                    oidc_connect_timeout: std::time::Duration::from_secs(5),
                    oidc_timeout: std::time::Duration::from_secs(15),
                    cookie_secret: "0".repeat(64),
                    database_url: "sqlite::memory:".to_string(),
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
                    subagent_max_secs: 2700,
                },
                cookie_key: cookie::Key::from(&[0u8; 64]),
                k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
                auth: state,
                shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
                workflow: None,
            });
        (app, db)
    }

    /// Jeton API de `subject` (identité = `miryad_users.id` résolu par miryad-core).
    async fn token(db: &sea_orm::DatabaseConnection, subject: &str) -> String {
        miryad_core::auth::issue_token(
            db,
            subject,
            "test",
            None,
            crate::auth::test_support::TEST_TOKEN_PEPPER,
        )
        .await
        .expect("émission de jeton")
        .token
    }

    /// Identifiant miryad-core de `subject`.
    async fn user(db: &sea_orm::DatabaseConnection, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("l'utilisateur se résout")
            .id
    }

    /// Projet `name` de `owner` (via l'entité ; `created_at` par défaut de la base).
    async fn projet(db: &sea_orm::DatabaseConnection, owner: i32, name: &str) -> i32 {
        crate::db::entities::projects::Entity::insert(crate::db::entities::projects::ActiveModel {
            id: NotSet,
            owner_id: sea_orm::Set(owner),
            name: sea_orm::Set(name.to_string()),
            missing_since: NotSet,
            created_at: NotSet,
        })
        .exec(db)
        .await
        .expect("le projet s'insère")
        .last_insert_id
    }

    /// Sandbox `name` de `project` pour `owner` (via l'entité).
    async fn sandbox(
        db: &sea_orm::DatabaseConnection,
        owner: i32,
        project: i32,
        name: &str,
    ) -> i32 {
        crate::db::entities::sandboxes::Entity::insert(
            crate::db::entities::sandboxes::ActiveModel {
                id: NotSet,
                owner_id: sea_orm::Set(owner),
                project_id: sea_orm::Set(project),
                name: sea_orm::Set(name.to_string()),
                cr_name: sea_orm::Set(format!("cr-{name}")),
                missing_since: NotSet,
                created_at: NotSet,
            },
        )
        .exec(db)
        .await
        .expect("la sandbox s'insère")
        .last_insert_id
    }

    async fn ask(
        app: &axum::Router,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        body.map_or_else(|| "{}".to_string(), |v| v.to_string()),
                    ))
                    .expect("requête valide"),
            )
            .await
            .expect("le routeur répond");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("corps lisible");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn deux_widgets() -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"id": "w-note", "type": "markdown", "config": {"text": "a"}, "x": 0, "y": 0, "w": 6, "h": 4}),
            serde_json::json!({"id": "w-chat", "type": "chat", "config": {}, "x": 6, "y": 0, "w": 6, "h": 4}),
        ]
    }

    fn vingt_cinq_widgets() -> Vec<serde_json::Value> {
        (0..25)
            .map(|i| {
                serde_json::json!({"id": format!("w-{i}"), "type": "chat", "config": {}, "x": 0, "y": 0, "w": 1, "h": 1})
            })
            .collect()
    }

    /// Corps de création : clés d'écran `project_id`/`sandbox_id` (NULL par défaut).
    fn body(widgets: &[serde_json::Value], scope: &str, kind: &str) -> serde_json::Value {
        serde_json::json!({
            "scope": scope,
            "kind": kind,
            "project_id": null,
            "sandbox_id": null,
            "title": "Mon tableau",
            "def": { "layout": { "columns": 12 }, "widgets": widgets },
        })
    }

    /// Scénario d'accueil valide.
    fn body_home(widgets: &[serde_json::Value]) -> serde_json::Value {
        body(widgets, "home", "custom")
    }

    /// Scenario: `le CRUD de l'écran personnalisé est borné par les hooks` — création rendue
    /// (colonnes posées, `owner_id` écrasé), mise à jour de 25 widgets refusée `001` (422).
    #[tokio::test]
    async fn le_crud_de_lecran_personnalise_est_borne_par_les_hooks() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;
        let alice_id = user(&db, "alice").await;

        let mut creation = body_home(&deux_widgets());
        creation["owner_id"] = serde_json::json!(999);
        let (status, created) =
            ask(&app, "POST", "/api/v1/dashboards", &alice, Some(creation)).await;
        assert_eq!(status, StatusCode::CREATED, "création valide : {created}");
        assert_eq!(
            created["owner_id"],
            serde_json::json!(alice_id),
            "owner_id écrasé"
        );
        assert_eq!(created["scope"], serde_json::json!("home"));
        assert_eq!(created["kind"], serde_json::json!("custom"));
        assert_eq!(created["project_id"], serde_json::Value::Null);
        let id = created["id"].as_i64().expect("id rendue");

        let lignes = Entity::find().all(&db).await.expect("relisible");
        assert_eq!(lignes.len(), 1);
        assert_eq!(lignes[0].owner_id, alice_id);

        let mut maj = body_home(&vingt_cinq_widgets());
        maj["id"] = serde_json::json!(id);
        let (status, json) = ask(
            &app,
            "PUT",
            &format!("/api/v1/dashboards/{id}"),
            &alice,
            Some(maj),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "25 widgets : {json}"
        );
        assert_eq!(
            json["code"].as_str(),
            Some("VNL-APP-DSH-001"),
            "le hook doit rendre le marqueur 001"
        );
        let lignes = Entity::find().all(&db).await.expect("relisible");
        assert_eq!(
            lignes[0].def["widgets"].as_array().expect("widgets").len(),
            2,
            "la ligne est restée inchangée (refuser, jamais corriger)"
        );
    }

    /// Scenario: `les écrans incohérents sont refusés 007` — huit créations, aucune ligne
    /// écrite. Fixtures : `alice` a les projets `p1`, `p2` et la sandbox `s1` (de `p1`) et
    /// `s2` (de `p2`) ; `ben` a le projet `pb`.
    #[tokio::test]
    async fn les_ecrans_incoherents_sont_refuses_007() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;
        let alice_id = user(&db, "alice").await;
        let ben_id = user(&db, "ben").await;
        let p1 = projet(&db, alice_id, "p1").await;
        let p2 = projet(&db, alice_id, "p2").await;
        let s2 = sandbox(&db, alice_id, p2, "s2").await;
        let pb = projet(&db, ben_id, "pb").await;
        let widgets = deux_widgets();

        let huit: Vec<serde_json::Value> = vec![
            // (scope=home, project_id d'un projet de l'appelant) — règle b.
            {
                let mut b = body_home(&widgets);
                b["project_id"] = serde_json::json!(p1);
                b
            },
            // (scope=project, project_id=NULL) — règle b.
            body(&widgets, "project", "custom"),
            // (scope=sandbox, project_id=NULL) — règle b.
            body(&widgets, "sandbox", "custom"),
            // kind="default" — règle a.
            body(&widgets, "home", "default"),
            // kind="complementary" avec scope="home" — règle c.
            body(&widgets, "home", "complementary"),
            // title vide — règle d.
            {
                let mut b = body_home(&widgets);
                b["title"] = serde_json::json!("");
                b
            },
            // project_id d'un projet d'un AUTRE utilisateur — règle g.
            {
                let mut b = body(&widgets, "project", "custom");
                b["project_id"] = serde_json::json!(pb);
                b
            },
            // sandbox_id d'une sandbox d'un AUTRE projet — règle g.
            {
                let mut b = body(&widgets, "sandbox", "custom");
                b["project_id"] = serde_json::json!(p1);
                b["sandbox_id"] = serde_json::json!(s2);
                b
            },
        ];
        assert_eq!(huit.len(), 8, "huit cas du Scenario");
        for (index, b) in huit.into_iter().enumerate() {
            let (status, json) = ask(&app, "POST", "/api/v1/dashboards", &alice, Some(b)).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "création {index} : {json}"
            );
            assert_eq!(
                json["code"].as_str(),
                Some("VNL-APP-DSH-007"),
                "création {index} doit rendre 007"
            );
        }
        assert_eq!(
            Entity::find().count(&db).await.expect("compte"),
            0,
            "aucune ligne écrite"
        );
    }

    /// Scenario: `un type de widget inconnu refuse sans lire le registre du frontend` —
    /// `fantome` → 003, config scalaire → 006, `scripted` sans `script` → 011. Un `scripted`
    /// valide passe (type connu du registre).
    #[tokio::test]
    async fn un_type_de_widget_inconnu_refuse() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;

        let fantome = serde_json::json!({
            "scope": "home", "kind": "custom", "project_id": null, "sandbox_id": null,
            "title": "T", "def": { "layout": { "columns": 12 }, "widgets": [
                {"id": "w-x", "type": "fantome", "config": {}, "x": 0, "y": 0, "w": 6, "h": 4}
            ]}
        });
        let (status, json) = ask(&app, "POST", "/api/v1/dashboards", &alice, Some(fantome)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["code"].as_str(), Some("VNL-APP-DSH-003"));

        let config_scalaire = serde_json::json!({
            "scope": "home", "kind": "custom", "project_id": null, "sandbox_id": null,
            "title": "T", "def": { "layout": { "columns": 12 }, "widgets": [
                {"id": "w-x", "type": "markdown", "config": 3, "x": 0, "y": 0, "w": 6, "h": 4}
            ]}
        });
        let (status, json) = ask(
            &app,
            "POST",
            "/api/v1/dashboards",
            &alice,
            Some(config_scalaire),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["code"].as_str(), Some("VNL-APP-DSH-006"));

        let scripted_vide = serde_json::json!({
            "scope": "home", "kind": "custom", "project_id": null, "sandbox_id": null,
            "title": "T", "def": { "layout": { "columns": 12 }, "widgets": [
                {"id": "w-x", "type": "scripted", "config": {}, "x": 0, "y": 0, "w": 6, "h": 4}
            ]}
        });
        let (status, json) = ask(
            &app,
            "POST",
            "/api/v1/dashboards",
            &alice,
            Some(scripted_vide),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["code"].as_str(), Some("VNL-APP-DSH-011"));

        let scripted_valide = serde_json::json!({
            "scope": "home", "kind": "custom", "project_id": null, "sandbox_id": null,
            "title": "T", "def": { "layout": { "columns": 12 }, "widgets": [
                {"id": "w-x", "type": "scripted", "config": {"script": "1"}, "x": 0, "y": 0, "w": 6, "h": 4}
            ]}
        });
        let (status, json) = ask(
            &app,
            "POST",
            "/api/v1/dashboards",
            &alice,
            Some(scripted_valide),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "scripted valide : {json}");
    }

    /// Scenario: `deux personnalisations du même écran sont refusées 009` — le hook refuse en
    /// 422 avant l'écriture ; un SECOND complementary sur le même projet passe (`FR-5`) ; une
    /// insertion SQL contournant le hook est rejetée par l'index d'unicité.
    #[tokio::test]
    async fn deux_personnalisations_du_meme_ecran_sont_refusees() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;
        let alice_id = user(&db, "alice").await;
        let p1 = projet(&db, alice_id, "p1").await;

        let premier = {
            let mut b = body(&deux_widgets(), "project", "custom");
            b["project_id"] = serde_json::json!(p1);
            b
        };
        let (status, json) = ask(
            &app,
            "POST",
            "/api/v1/dashboards",
            &alice,
            Some(premier.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "première personnalisation : {json}"
        );

        let (status, json) = ask(&app, "POST", "/api/v1/dashboards", &alice, Some(premier)).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "le hook refuse le doublon avant l'écriture : {json}"
        );
        assert_eq!(json["code"].as_str(), Some("VNL-APP-DSH-009"));

        // Un SECOND complementary sur le même projet passe (FR-5, pas d'unicité là-dessus).
        let complementary = {
            let mut b = body(&deux_widgets(), "project", "complementary");
            b["project_id"] = serde_json::json!(p1);
            b
        };
        let (status, json) = ask(
            &app,
            "POST",
            "/api/v1/dashboards",
            &alice,
            Some(complementary.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "premier complementary : {json}"
        );
        let (status, json) = ask(
            &app,
            "POST",
            "/api/v1/dashboards",
            &alice,
            Some(complementary),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "second complementary : {json}");

        // Insertion en contournant le hook : l'index partiel refuse le second `custom`.
        let brut = db
            .execute_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                "INSERT INTO vanyline_dashboards (owner_id, scope, kind, project_id, sandbox_id, title, def) \
                 VALUES (?, 'project', 'custom', ?, NULL, 'T', '{\"layout\": {\"columns\": 12}, \"widgets\": []}')",
                vec![alice_id.into(), p1.into()],
            ))
            .await;
        let erreur = brut.expect_err("l'index d'unicité refuse l'insertion brute");
        assert!(
            erreur.to_string().to_uppercase().contains("UNIQUE"),
            "le rejet vient de l'index : {erreur}"
        );
    }

    /// Scenario: `la course sur un même écran rend 409` — partie base de la course : le hook
    /// voit l'écran libre (`Ok`), un autre écrivain insère la personnalisation, et l'INSERT
    /// suivant est rejeté par l'index d'unicité (jamais une deuxième ligne). Le rendu HTTP 409
    /// `MRD-REST-007` est celui de miryad-core, non atteignable ici sans contourner le hook.
    #[tokio::test]
    async fn la_course_sur_un_meme_ecran_est_arretee_par_l_index() {
        use miryad_core::resource::MiryadResource;
        let db = real_db().await;
        let alice_id = user(&db, "alice").await;
        let p1 = projet(&db, alice_id, "p1").await;
        let principal = miryad_core::auth::AuthPrincipal {
            subject: "alice".to_string(),
            email: None,
            preferred_username: None,
            source: miryad_core::auth::PrincipalSource::ApiToken { token_id: 1 },
        };
        let active = ActiveModel {
            id: NotSet,
            owner_id: sea_orm::Set(alice_id),
            scope: sea_orm::Set("project".to_string()),
            kind: sea_orm::Set("custom".to_string()),
            project_id: sea_orm::Set(Some(p1)),
            sandbox_id: sea_orm::Set(None),
            title: sea_orm::Set("Mon tableau".to_string()),
            def: sea_orm::Set(serde_json::json!({"layout": {"columns": 12}, "widgets": []})),
        };
        let active = Entity::before_create(&db, active, &principal)
            .await
            .expect("le hook voit l'écran libre");
        db.execute_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            "INSERT INTO vanyline_dashboards (owner_id, scope, kind, project_id, sandbox_id, title, def) \
             VALUES (?, 'project', 'custom', ?, NULL, 'Concurrent', '{\"layout\": {\"columns\": 12}, \"widgets\": []}')",
            vec![alice_id.into(), p1.into()],
        ))
        .await
        .expect("l'autre écrivain insère");
        let erreur = Entity::insert(active)
            .exec(&db)
            .await
            .expect_err("l'index refuse la seconde ligne");
        assert!(
            erreur.to_string().to_uppercase().contains("UNIQUE"),
            "le rejet vient de l'index d'unicité : {erreur}"
        );
        assert_eq!(
            Entity::find().count(&db).await.expect("compte"),
            1,
            "jamais une deuxième ligne"
        );
    }

    /// Scenario: `la vue de sandbox est clée par projet et retient la sandbox d'origine` —
    /// la sandbox d'origine est vidée à sa suppression (ligne conservée), et supprimer le
    /// PROJET supprime la ligne.
    #[tokio::test]
    async fn la_vue_de_sandbox_est_clee_par_projet() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;
        let alice_id = user(&db, "alice").await;
        let p1 = projet(&db, alice_id, "p1").await;
        let s1 = sandbox(&db, alice_id, p1, "s1").await;

        let mut b = body(&deux_widgets(), "sandbox", "custom");
        b["project_id"] = serde_json::json!(p1);
        b["sandbox_id"] = serde_json::json!(s1);
        let (status, json) = ask(&app, "POST", "/api/v1/dashboards", &alice, Some(b)).await;
        assert_eq!(status, StatusCode::CREATED, "{json}");

        crate::db::entities::sandboxes::Entity::delete_by_id(s1)
            .exec(&db)
            .await
            .expect("la sandbox se supprime");
        let lignes = Entity::find().all(&db).await.expect("relisible");
        assert_eq!(lignes.len(), 1, "la ligne subsiste");
        assert_eq!(lignes[0].sandbox_id, None, "sandbox_id vidé");
        assert_eq!(
            lignes[0].project_id,
            Some(p1),
            "la clé d'écran reste le projet"
        );

        crate::db::entities::projects::Entity::delete_by_id(p1)
            .exec(&db)
            .await
            .expect("le projet se supprime");
        assert_eq!(
            Entity::find().count(&db).await.expect("compte"),
            0,
            "supprimer le projet supprime la ligne"
        );
    }

    /// Scenario: `un complementary d'un autre utilisateur n'est pas visible` (`AC-6`).
    #[tokio::test]
    async fn un_complementary_d_un_autre_utilisateur_n_est_pas_visible() {
        let (app, db) = app_with_token().await;
        let ana = token(&db, "ana").await;
        let ben = token(&db, "ben").await;
        let ana_id = user(&db, "ana").await;
        let p1 = projet(&db, ana_id, "p1").await;

        let mut complementary = body(&deux_widgets(), "project", "complementary");
        complementary["project_id"] = serde_json::json!(p1);
        let (status, json) = ask(
            &app,
            "POST",
            "/api/v1/dashboards",
            &ana,
            Some(complementary),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{json}");

        let (status, liste) = ask(&app, "GET", "/api/v1/dashboards", &ben, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            liste["items"],
            serde_json::json!([]),
            "aucune ligne d'ana ne filtre (OwnerOnly, AC-6)"
        );
    }

    /// Scenario « la liste filtre et trie par colonnes déclarées » (ticket #78).
    #[tokio::test]
    async fn la_liste_filtre_et_trie_par_colonnes_declarees() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;
        let alice_id = user(&db, "alice").await;
        let web = projet(&db, alice_id, "web").await;

        let mut accueil = body(&deux_widgets(), "home", "custom");
        accueil["title"] = serde_json::json!("Accueil");
        let (status, _) = ask(&app, "POST", "/api/v1/dashboards", &alice, Some(accueil)).await;
        assert_eq!(status, StatusCode::CREATED);
        for titre in ["B", "A"] {
            let mut ligne = body(&deux_widgets(), "project", "complementary");
            ligne["project_id"] = serde_json::json!(web);
            ligne["title"] = serde_json::json!(titre);
            let (status, json) = ask(&app, "POST", "/api/v1/dashboards", &alice, Some(ligne)).await;
            assert_eq!(status, StatusCode::CREATED, "{json}");
        }

        let (status, liste) = ask(
            &app,
            "GET",
            &format!("/api/v1/dashboards?kind=complementary&project_id={web}&sort=title&order=asc"),
            &alice,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{liste}");
        let titres: Vec<&str> = liste["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|l| l["title"].as_str())
            .collect();
        assert_eq!(titres, vec!["A", "B"]);

        let (status, _) = ask(&app, "GET", "/api/v1/dashboards?def=x", &alice, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Verrou de la forme sérialisée du payload (création du frontend sans `id`/`owner_id`).
    #[test]
    fn payload_du_frontend_sans_id_ni_owner() {
        let b = body_home(&deux_widgets());
        let model: Model = serde_json::from_value(b).expect("le corps de création se désérialise");
        assert_eq!(model.id, 0);
        assert_eq!(model.owner_id, 0);
        assert_eq!(model.scope, "home");
        assert_eq!(model.kind, "custom");
        assert_eq!(model.project_id, None);
        assert_eq!(model.sandbox_id, None);
    }

    /// Le hook ne normalise rien : contrat de la ressource vérifié, conversion en
    /// `ActiveModel` sans écriture (motif avant-hook des sœurs).
    #[test]
    fn contrat_de_ressource() {
        assert_eq!(Entity::resource_name(), "dashboards");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::OwnerOnly);
        assert!(matches!(Entity::owner_column(), Some(Column::OwnerId)));
        assert_eq!(
            Entity::filterable_columns().len(),
            3,
            "scope, kind, project_id"
        );
        let _ = Model {
            id: 1,
            owner_id: 1,
            scope: "home".to_string(),
            kind: "custom".to_string(),
            project_id: None,
            sandbox_id: None,
            title: "T".to_string(),
            def: serde_json::json!({"layout": {"columns": 12}, "widgets": []}),
        }
        .into_active_model();
    }
}
