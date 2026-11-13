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

/// Ligne de définition portée : clés d'écran (`scope`, `kind`, `project`, `sandbox`),
/// libellé, définition JSON `{layout, widgets}` (`DEC-3`).
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
    /// Écran projet ; NULL ailleurs.
    pub project: Option<String>,
    /// Écran sandbox ; NULL ailleurs.
    pub sandbox: Option<String>,
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

    /// Garde d'écriture FR-3 (règles a→f de `./dashboards.sdd`). Forme async à connexion
    /// générique : miryad-core 0.3.1 (hooks, DEC-2) — logique sans attente rendue par
    /// [`std::future::ready`] (harnais : `manual_async_fn` et `unused_async_trait_impl` se
    /// contredisent sur `async fn` sans `.await`, cf. `../../../../tooling.sdd`).
    fn before_create<C>(
        _db: &C,
        active: Self::ActiveModel,
        _principal: &AuthPrincipal,
    ) -> impl std::future::Future<Output = Result<Self::ActiveModel, HookError>> + Send
    where
        C: sea_orm::ConnectionTrait,
        Self::ActiveModel: Send,
    {
        std::future::ready(valider_creation(active))
    }

    /// La paire effective est revalidée : les champs non posés viennent de la ligne relue
    /// `existing` (prêtée en lecture seule).
    fn before_update<C>(
        _db: &C,
        active: Self::ActiveModel,
        existing: &Self::Model,
        _principal: &AuthPrincipal,
    ) -> impl std::future::Future<Output = Result<Self::ActiveModel, HookError>> + Send
    where
        C: sea_orm::ConnectionTrait,
        Self::ActiveModel: Send,
    {
        std::future::ready(valider_mise_a_jour(active, existing))
    }
}

/// Lecture d'une colonne `String` de l'`ActiveModel` : `NotSet` vaut « absent » (règle du
/// hook : une clé d'écran absente est un manquement 007, jamais une valeur implicite).
fn texte(valeur: &ActiveValue<String>) -> Option<&str> {
    match valeur {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => Some(v.as_str()),
        ActiveValue::NotSet => None,
    }
}

/// Valeur EFFECTIVE d'une colonne `Option<String>` à la création : posée (`NULL` ou valeur)
/// ou non posée — non posé vaut `NULL` (une clé absente est tranchée par les règles b, jamais
/// par une valeur implicite).
fn cle(valeur: &ActiveValue<Option<String>>) -> Option<&str> {
    match valeur {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_deref(),
        ActiveValue::NotSet => None,
    }
}

/// Valeur EFFECTIVE à la mise à jour : posée vaut la valeur posée (`Some`/`NULL` explicite),
/// non posée est REPRIS de la ligne `existing` (prêtée en lecture seule).
fn cle_reprise<'a>(
    valeur: &'a ActiveValue<Option<String>>,
    existante: Option<&'a String>,
) -> Option<&'a str> {
    match valeur {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_deref(),
        ActiveValue::NotSet => existante.map(String::as_str),
    }
}

/// Lecture de la colonne JSON `def`.
fn definition(valeur: &ActiveValue<serde_json::Value>) -> Option<&serde_json::Value> {
    match valeur {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => Some(v),
        ActiveValue::NotSet => None,
    }
}

/// Le 007 contractuel : message fixe, jamais le contenu d'un widget ni la valeur fautives.
fn incoherent(message: &str) -> HookError {
    HookError::with_code("VNL-APP-DSH-007", message)
}

/// Règles a→f de `./dashboards.sdd` sur les VALEURS EFFECTIVES (l'appelant a résolu les
/// champs non posés d'une mise à jour depuis la ligne `existing`). Premier manquement seul,
/// `validate_definition` (001..006) en dernier.
fn valider_ecran(
    kind: Option<&str>,
    scope: Option<&str>,
    project: Option<&str>,
    sandbox: Option<&str>,
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
    // b. cohérence du couple (scope, project, sandbox).
    let coherent = match scope {
        Some("home") => project.is_none() && sandbox.is_none(),
        Some("project") => sandbox.is_none() && project.is_some_and(|p| !p.is_empty()),
        Some("sandbox") => project.is_none() && sandbox.is_some_and(|s| !s.is_empty()),
        _ => false,
    };
    if !coherent {
        return Err(incoherent(
            "invalid screen keys (scope, project, sandbox) (VNL-APP-DSH-007)",
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

/// Logique de `before_create` (sans attente) : clés telles qu'écrites, règles a→f.
fn valider_creation(active: ActiveModel) -> Result<ActiveModel, HookError> {
    valider_ecran(
        texte(&active.kind),
        texte(&active.scope),
        cle(&active.project),
        cle(&active.sandbox),
        texte(&active.title),
        definition(&active.def),
    )?;
    Ok(active)
}

/// Logique de `before_update` (sans attente) : la paire effective est revalidée, les champs
/// non posés sont reprisent de `existing` (lecture seule, jamais normalisée).
fn valider_mise_a_jour(active: ActiveModel, existing: &Model) -> Result<ActiveModel, HookError> {
    valider_ecran(
        Some(texte(&active.kind).unwrap_or(existing.kind.as_str())),
        Some(texte(&active.scope).unwrap_or(existing.scope.as_str())),
        cle_reprise(&active.project, existing.project.as_ref()),
        cle_reprise(&active.sandbox, existing.sandbox.as_ref()),
        Some(texte(&active.title).unwrap_or(existing.title.as_str())),
        definition(&active.def).or(Some(&existing.def)),
    )?;
    Ok(active)
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
    use sea_orm::{EntityTrait, IntoActiveModel};
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

    fn body(widgets: &[serde_json::Value], scope: &str, kind: &str) -> serde_json::Value {
        serde_json::json!({
            "scope": scope,
            "kind": kind,
            "project": null,
            "sandbox": null,
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
        let alice_id = miryad_core::users::resolve_user(&db, "alice", None)
            .await
            .expect("alice")
            .id;

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

    /// Scenario: `les écrans incohérents sont refusés 007` — six créations, aucune ligne
    /// écrite.
    #[tokio::test]
    async fn les_ecrans_incoherents_sont_refuses_007() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;
        let widgets = deux_widgets();

        let six: Vec<serde_json::Value> = vec![
            // (scope=home, project="p1")
            {
                let mut b = body_home(&widgets);
                b["project"] = serde_json::json!("p1");
                b
            },
            // (scope=project, project=NULL)
            body(&widgets, "project", "custom"),
            // (scope=sandbox, sandbox=NULL, project="p1")
            {
                let mut b = body(&widgets, "sandbox", "custom");
                b["project"] = serde_json::json!("p1");
                b
            },
            // kind="default"
            body(&widgets, "home", "default"),
            // kind="complementary" avec scope="home"
            body(&widgets, "home", "complementary"),
            // title vide
            {
                let mut b = body_home(&widgets);
                b["title"] = serde_json::json!("");
                b
            },
        ];
        for (index, b) in six.into_iter().enumerate() {
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

    /// Scenario: `un type de widget inconnu refuse sans lire le registre du frontend`.
    #[tokio::test]
    async fn un_type_de_widget_inconnu_refuse() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;

        let scripted = serde_json::json!({
            "scope": "home", "kind": "custom", "project": null, "sandbox": null,
            "title": "T", "def": { "layout": { "columns": 12 }, "widgets": [
                {"id": "w-x", "type": "scripted", "config": {}, "x": 0, "y": 0, "w": 6, "h": 4}
            ]}
        });
        let (status, json) = ask(&app, "POST", "/api/v1/dashboards", &alice, Some(scripted)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["code"].as_str(), Some("VNL-APP-DSH-003"));

        let config_scalaire = serde_json::json!({
            "scope": "home", "kind": "custom", "project": null, "sandbox": null,
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
    }

    /// Scenario: `deux personnalisations du même écran, c'est 409 pas 422`. ÉCART DE SPEC
    /// consigné (`[?]` de `./dashboards.sdd`) : miryad-core 0.3.1 traduit le `DbErr` de
    /// contrainte en 500 `MRD-REST-003`, pas en 409 — le test verrouille le refus (jamais
    /// 422, jamais 201) et le passage du second complementary.
    #[tokio::test]
    async fn deux_personnalisations_du_meme_ecran_sont_refusees() {
        let (app, db) = app_with_token().await;
        let alice = token(&db, "alice").await;

        let premier = {
            let mut b = body(&deux_widgets(), "project", "custom");
            b["project"] = serde_json::json!("p1");
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
        assert_ne!(status, StatusCode::CREATED, "le doublon ne passe pas");
        assert_ne!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "c'est la contrainte (index partiel), pas le hook : {json}"
        );

        // Un SECOND complementary sur le même projet passe (FR-5, pas d'index là-dessus).
        let complementary = {
            let mut b = body(&deux_widgets(), "project", "complementary");
            b["project"] = serde_json::json!("p1");
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
    }

    /// Scenario: `un complementary d'un autre utilisateur n'est pas visible` (`AC-6`).
    #[tokio::test]
    async fn un_complementary_d_un_autre_utilisateur_n_est_pas_visible() {
        let (app, db) = app_with_token().await;
        let ana = token(&db, "ana").await;
        let ben = token(&db, "ben").await;

        let complementary = {
            let mut b = body(&deux_widgets(), "project", "complementary");
            b["project"] = serde_json::json!("p1");
            b
        };
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

    /// Verrou de la forme sérialisée du payload (création du frontend sans `id`/`owner_id`).
    #[test]
    fn payload_du_frontend_sans_id_ni_owner() {
        let b = body_home(&deux_widgets());
        let model: Model = serde_json::from_value(b).expect("le corps de création se désérialise");
        assert_eq!(model.id, 0);
        assert_eq!(model.owner_id, 0);
        assert_eq!(model.scope, "home");
        assert_eq!(model.kind, "custom");
        assert_eq!(model.project, None);
        assert_eq!(model.sandbox, None);
    }

    /// Le hook ne normalise rien : contrat de la ressource vérifié, conversion en
    /// `ActiveModel` sans écriture (motif avant-hook des sœurs).
    #[test]
    fn contrat_de_ressource() {
        assert_eq!(Entity::resource_name(), "dashboards");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::OwnerOnly);
        assert!(matches!(Entity::owner_column(), Some(Column::OwnerId)));
        let _ = Model {
            id: 1,
            owner_id: 1,
            scope: "home".to_string(),
            kind: "custom".to_string(),
            project: None,
            sandbox: None,
            title: "T".to_string(),
            def: serde_json::json!({"layout": {"columns": 12}, "widgets": []}),
        }
        .into_active_model();
    }
}
