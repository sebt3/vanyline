//! Résolution du tableau effectif d'un écran (`FR-2`, `DEC-5`) — contrat : `./resolve.sdd`.
//! La SEULE route HTTP propre à la Solution : lecture seule, jamais d'écriture ; la
//! résolution ne 404 JAMAIS (le défaut est toujours là, `AC-2`).

use axum::{
    Json,
    extract::{Query, State},
};
use miryad_core::auth::AuthUser;
use serde::Deserialize;
use std::str::FromStr;

use super::model::{DashboardDef, Origin, Resolved, Scope};
use crate::AppState;
use crate::error::AppError;

/// Paramètres de `GET /api/dashboards/resolve` : `scope` requis (absent → 400 par la
/// désérialisation axum, hors contrat de marqueur ici) ; `project`/`sandbox` ignorés hors de
/// leur écran. Les écrans sont désignés par NOM (`./resolve.sdd`, `Accepts`).
#[derive(Debug, Deserialize)]
pub struct ResolveQuery {
    /// Écran résolu : `"home"`, `"project"` ou `"sandbox"` (sinon `VNL-APP-DSH-010`).
    pub scope: String,
    /// Nom du projet (écran projet ; ignoré ailleurs).
    #[serde(default)]
    pub project: Option<String>,
    /// Nom de la sandbox (écran sandbox ; ignoré ailleurs).
    #[serde(default)]
    pub sandbox: Option<String>,
}

/// `GET /api/dashboards/resolve` (`./resolve.sdd`) : personnalisation `custom` de
/// l'APPELANT sur cet écran, sinon le défaut livré. Ne résout jamais sur un `owner_id` de
/// requête, n'écrit jamais, ne 404 jamais.
pub async fn handler_resolve(
    user: AuthUser,
    Query(query): Query<ResolveQuery>,
    state: State<AppState>,
) -> Result<Json<Resolved>, AppError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    use crate::db::entities::dashboards::{Column, Entity};

    let scope = Scope::from_str(&query.scope).map_err(|()| ecran_impossible())?;
    let nom_projet = if matches!(scope, Scope::Project) {
        Some(non_vide(query.project).ok_or_else(ecran_impossible)?)
    } else {
        None
    };
    let nom_sandbox = if matches!(scope, Scope::Sandbox) {
        Some(non_vide(query.sandbox).ok_or_else(ecran_impossible)?)
    } else {
        None
    };

    // Identité de l'APPELANT (jamais un owner_id de requête) — motif de
    // `../../api/workflows.rs`.
    let appelant =
        miryad_core::users::resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
            .await?;

    // Traduction des noms d'écran en identifiant + nom de projet (`Handles` de `./resolve.sdd`).
    // La vue de sandbox est celle de son PROJET : la clé d'écran est le projet de la sandbox.
    let (projet_id, nom_projet_ecran, nom_sandbox_ecran) = match scope {
        Scope::Home => (None, None, None),
        Scope::Project => {
            let nom = nom_projet.clone().ok_or_else(ecran_impossible)?;
            let id = projet_de(&state.auth.db, appelant.id, &nom).await?;
            (Some(id), Some(nom), None)
        }
        Scope::Sandbox => {
            let nom = nom_sandbox.clone().ok_or_else(ecran_impossible)?;
            let (id, nom_projet) = projet_de_sandbox(&state.auth.db, appelant.id, &nom).await?;
            (Some(id), Some(nom_projet), Some(nom))
        }
    };

    // Requête NULL-safe par écran (`Handles` de `./resolve.sdd`) : deux branches
    // IS NULL / EQ, équivalent de `IS NOT DISTINCT FROM` sur les deux moteurs.
    let recherche = Entity::find()
        .filter(Column::OwnerId.eq(appelant.id))
        .filter(Column::Scope.eq(scope.as_str()))
        .filter(Column::Kind.eq("custom"));
    let recherche = match projet_id {
        Some(id) => recherche.filter(Column::ProjectId.eq(id)),
        None => recherche.filter(Column::ProjectId.is_null()),
    };
    if let Some(ligne) = recherche.one(&state.auth.db).await? {
        // Défense en profondeur (`Must` de `./resolve.sdd`) : la corruption se VOIT —
        // marqueur journalisé en error, jamais un repli default silencieux.
        let def: DashboardDef = serde_json::from_value(ligne.def).map_err(|erreur| {
            tracing::error!("VNL-APP-DSH-007: définition stockée illisible: {erreur}");
            AppError::InternalError(format!(
                "VNL-APP-DSH-007: stored dashboard definition is unreadable: {erreur}"
            ))
        })?;
        if let Err((code, message)) = super::validate_definition(&def) {
            tracing::error!("{code}: {message}");
            return Err(AppError::InternalError(format!("{code}: {message}")));
        }
        return Ok(Json(Resolved {
            origin: Origin::Custom,
            dashboard: super::model::DashboardView {
                id: Some(ligne.id),
                title: ligne.title,
                scope,
                project: nom_projet_ecran,
                sandbox: nom_sandbox_ecran,
                layout: def.layout,
                widgets: def.widgets,
            },
        }));
    }

    let def = defaut_ou_erreur(super::defaults::default_for(scope))?;
    Ok(Json(Resolved {
        origin: Origin::Default,
        dashboard: super::model::DashboardView {
            id: None,
            title: def.title.clone(),
            scope,
            project: nom_projet_ecran,
            sandbox: nom_sandbox_ecran,
            layout: def.layout,
            widgets: def.widgets.clone(),
        },
    }))
}

/// Identifiant du projet `nom` de l'appelant ; `VNL-APP-DSH-010` s'il n'existe pas pour lui.
async fn projet_de(
    db: &sea_orm::DatabaseConnection,
    owner: i32,
    nom: &str,
) -> Result<i32, AppError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    use crate::db::entities::projects::{Column, Entity};

    Entity::find()
        .filter(Column::OwnerId.eq(owner))
        .filter(Column::Name.eq(nom))
        .one(db)
        .await?
        .map(|projet| projet.id)
        .ok_or_else(ecran_impossible)
}

/// Projet (identifiant et nom) de la sandbox `nom` de l'appelant (`./resolve.sdd` : la vue de
/// sandbox est celle de son projet, déduit d'elle). `VNL-APP-DSH-010` si la sandbox n'existe
/// pas pour lui.
async fn projet_de_sandbox(
    db: &sea_orm::DatabaseConnection,
    owner: i32,
    nom: &str,
) -> Result<(i32, String), AppError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};

    use crate::db::entities::{
        projects,
        sandboxes::{Column, Entity},
    };

    // Le nom n'est unique que par projet : deux correspondances = écran ambigu (échec fermé).
    let mut trouvees = Entity::find()
        .filter(Column::OwnerId.eq(owner))
        .filter(Column::Name.eq(nom))
        .limit(2)
        .all(db)
        .await?;
    if trouvees.len() != 1 {
        return Err(ecran_impossible());
    }
    let sandbox = trouvees.remove(0);
    let projet = projects::Entity::find_by_id(sandbox.project_id)
        .one(db)
        .await?
        .ok_or_else(ecran_impossible)?;
    Ok((projet.id, projet.name))
}

/// Défaut de l'écran, ou faute du serveur `VNL-APP-DSH-008` (500, journalisée) si le défaut
/// livré manque : jamais une faute de la requête (`Raises` de `./resolve.sdd`).
fn defaut_ou_erreur(
    defaut: Option<&'static DashboardDef>,
) -> Result<&'static DashboardDef, AppError> {
    defaut.ok_or_else(|| {
        tracing::error!("VNL-APP-DSH-008: défaut livré absent pour un écran");
        AppError::InternalError("VNL-APP-DSH-008: no shipped default for this screen".into())
    })
}

/// Écran impossible et scope inconnu : `VNL-APP-DSH-010` (400, `DashboardScreenInvalid`), message
/// fixe sans valeur de la requête.
fn ecran_impossible() -> AppError {
    AppError::DashboardScreenInvalid
}

/// Le paramètre d'écran, `Some` seulement s'il est présent et non vide.
fn non_vide(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboards::model::Scope;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use miryad_core::users::resolve_user;
    use sea_orm::{ActiveValue::NotSet, ConnectionTrait, EntityTrait, Set, Statement};
    use tower::ServiceExt;

    use crate::db::entities::dashboards::{ActiveModel, Entity, Model};
    use crate::db::test_support::real_db;

    /// `AppState` de test (même forme que `test_state` de `../../main.rs`) sur base réelle.
    fn state_with(db: sea_orm::DatabaseConnection) -> AppState {
        AppState {
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
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    /// Crée l'utilisateur `subject` (via `resolve_user`, source de l'identité) et rend son
    /// cookie de session (même clé que `test_auth_state_with_db`).
    async fn session_cookie(state: &AppState, subject: &str) -> String {
        resolve_user(&state.auth.db, subject, None)
            .await
            .expect("l'utilisateur se résout");
        let identity = miryad_core::auth::OidcIdentity {
            id_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.signature".to_string(),
            subject: subject.to_string(),
            email: None,
            preferred_username: None,
        };
        miryad_core::auth::cookie::build_set_cookie(
            &identity,
            &cookie::Key::from(&[0u8; 64]),
            false,
        )
        .split(';')
        .next()
        .expect("le cookie porte un couple nom=valeur")
        .to_string()
    }

    fn router(state: AppState) -> Router {
        Router::new()
            .route("/api/dashboards/resolve", get(handler_resolve))
            .merge(miryad_core::rest::resource_router::<Entity, AppState>())
            .with_state(state)
    }

    /// Résolution authentifiée : rend le couple (statut, corps JSON).
    async fn resolve(app: &Router, cookie: &str, query: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/dashboards/resolve?{query}"))
                    .header("Cookie", cookie)
                    .body(Body::empty())
                    .expect("requête valide"),
            )
            .await
            .expect("le routeur répond");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("le corps se lit");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    /// Projet `name` de `owner` créé pour le test (identifiant de la clé d'écran).
    async fn projet_de_test(db: &sea_orm::DatabaseConnection, owner: i32, name: &str) -> i32 {
        crate::db::entities::projects::Entity::insert(crate::db::entities::projects::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            missing_since: NotSet,
            created_at: NotSet,
        })
        .exec(db)
        .await
        .expect("le projet s'insère")
        .last_insert_id
    }

    /// Sandbox `name` de `project` pour `owner` (fixture).
    async fn sandbox_de_test(
        db: &sea_orm::DatabaseConnection,
        owner: i32,
        project: i32,
        name: &str,
    ) -> i32 {
        crate::db::entities::sandboxes::Entity::insert(
            crate::db::entities::sandboxes::ActiveModel {
                id: NotSet,
                owner_id: Set(owner),
                project_id: Set(project),
                name: Set(name.to_string()),
                cr_name: Set(format!("cr-{name}")),
                missing_since: NotSet,
                created_at: NotSet,
            },
        )
        .exec(db)
        .await
        .expect("la sandbox s'insère")
        .last_insert_id
    }

    /// Personnalisation `scope=sandbox` du projet `project`, écrite depuis la sandbox `sandbox`
    /// (hook exercé).
    async fn insert_sandbox_custom(
        db: &sea_orm::DatabaseConnection,
        owner: i32,
        project: i32,
        sandbox: i32,
    ) -> i32 {
        use miryad_core::resource::MiryadResource;
        let principal = miryad_core::auth::AuthPrincipal {
            subject: "alice".to_string(),
            email: None,
            preferred_username: None,
            source: miryad_core::auth::PrincipalSource::ApiToken { token_id: 1 },
        };
        let active = ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            scope: Set("sandbox".to_string()),
            kind: Set("custom".to_string()),
            project_id: Set(Some(project)),
            sandbox_id: Set(Some(sandbox)),
            title: Set("Vue sandbox".to_string()),
            def: Set(def_valide()),
        };
        let active = Entity::before_create(db, active, &principal)
            .await
            .expect("le hook accepte la personnalisation de sandbox");
        Entity::insert(active)
            .exec(db)
            .await
            .expect("la ligne s'insère")
            .last_insert_id
    }

    /// Une `def` de deux widgets valide (forme de la colonne JSON).
    fn def_valide() -> serde_json::Value {
        serde_json::json!({
            "layout": { "columns": 12 },
            "widgets": [
                { "id": "w-note", "type": "markdown", "config": { "text": "perso" }, "x": 0, "y": 0, "w": 6, "h": 4 },
                { "id": "w-chat", "type": "chat", "config": {}, "x": 6, "y": 0, "w": 6, "h": 4 }
            ]
        })
    }

    /// Insertion d'une ligne de personnalisation EN EXERCANT LE HOOK de l'entité (le chemin
    /// de création contracté, pas un contournement).
    async fn insert_custom_hook(db: &sea_orm::DatabaseConnection, subject: &str) -> i32 {
        use miryad_core::resource::MiryadResource;
        let user = resolve_user(db, subject, None).await.expect("user");
        let projet = projet_de_test(db, user.id, "p1").await;
        let principal = miryad_core::auth::AuthPrincipal {
            subject: subject.to_string(),
            email: None,
            preferred_username: None,
            source: miryad_core::auth::PrincipalSource::ApiToken { token_id: 1 },
        };
        let active = ActiveModel {
            id: NotSet,
            owner_id: Set(user.id),
            scope: Set("project".to_string()),
            kind: Set("custom".to_string()),
            project_id: Set(Some(projet)),
            sandbox_id: Set(None),
            title: Set("Mon tableau".to_string()),
            def: Set(def_valide()),
        };
        let active = Entity::before_create(db, active, &principal)
            .await
            .expect("le hook accepte la définition valide");
        Entity::insert(active)
            .exec(db)
            .await
            .expect("la ligne s'insère")
            .last_insert_id
    }

    /// Scenario: `sans personnalisation le défaut est rendu avec l'origine default`.
    #[tokio::test]
    async fn sans_personnalisation_le_defaut_est_rendu() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let alice = resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice")
            .id;
        projet_de_test(&state.auth.db, alice, "p1").await;
        let app = router(state);

        let (status, json) = resolve(&app, &cookie, "scope=project&project=p1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["origin"], serde_json::json!("default"));
        assert_eq!(json["dashboard"]["id"], serde_json::Value::Null);
        let attendu = super::super::defaults::default_for(Scope::Project).expect("défaut projet");
        let rendus: Vec<String> = json["dashboard"]["widgets"]
            .as_array()
            .expect("widgets")
            .iter()
            .map(|w| w["type"].as_str().expect("type").to_string())
            .collect();
        assert_eq!(
            rendus,
            attendu
                .widgets
                .iter()
                .map(|w| w.widget_type.clone())
                .collect::<Vec<_>>()
        );
    }

    /// Scenario: `la personnalisation de l'appelant prime`.
    #[tokio::test]
    async fn la_personnalisation_de_l_appelant_prime() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let id = insert_custom_hook(&state.auth.db, "alice").await;
        let app = router(state);

        let (status, json) = resolve(&app, &cookie, "scope=project&project=p1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["origin"], serde_json::json!("custom"));
        assert_eq!(json["dashboard"]["id"], serde_json::json!(id));
        let rendus: Vec<String> = json["dashboard"]["widgets"]
            .as_array()
            .expect("widgets")
            .iter()
            .map(|w| w["id"].as_str().expect("id").to_string())
            .collect();
        assert_eq!(
            rendus,
            ["w-note", "w-chat"],
            "aucun widget du défaut ajouté"
        );
    }

    /// Scenario: `la personnalisation d'un autre utilisateur ne filtre pas`.
    #[tokio::test]
    async fn la_personnalisation_d_un_autre_utilisateur_ne_filtre_pas() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let alice = resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice")
            .id;
        projet_de_test(&state.auth.db, alice, "p1").await;
        let _ = session_cookie(&state, "ben").await;
        let _ = insert_custom_hook(&state.auth.db, "ben").await;
        let app = router(state);

        let (status, json) = resolve(&app, &cookie, "scope=project&project=p1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["origin"],
            serde_json::json!("default"),
            "la ligne d'autrui est invisible (filtre owner, AC-6 inverse)"
        );
    }

    /// Scenario: `rendre les défauts revient au défaut` — le DELETE passe par la voie générique
    /// `/api/v1/dashboards/{id}` (`resource_router`, bout en bout).
    #[tokio::test]
    async fn rendre_les_defauts_revient_au_defaut() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let id = insert_custom_hook(&state.auth.db, "alice").await;
        let app = router(state);

        let (status, json) = resolve(&app, &cookie, "scope=project&project=p1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["origin"],
            serde_json::json!("custom"),
            "avant : personnalisée"
        );

        let supprime = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/dashboards/{id}"))
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .expect("requête valide"),
            )
            .await
            .expect("le routeur répond");
        assert!(
            supprime.status().is_success(),
            "la suppression générique réussit : {}",
            supprime.status()
        );

        let (status, json) = resolve(&app, &cookie, "scope=project&project=p1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["origin"],
            serde_json::json!("default"),
            "AC-3 bout en bout"
        );
    }

    /// Scenario: `écran impossible et scope inconnu refusent 010` — 400 avec le marqueur 010 ;
    /// jamais le défaut en repli, jamais la confirmation qu'une ressource d'autrui existe.
    #[tokio::test]
    async fn ecran_impossible_et_scope_inconnu_refusent_010() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let alice = resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice")
            .id;
        projet_de_test(&state.auth.db, alice, "p1").await;
        let ben = resolve_user(&state.auth.db, "ben", None)
            .await
            .expect("ben")
            .id;
        let pb = projet_de_test(&state.auth.db, ben, "pb").await;
        sandbox_de_test(&state.auth.db, ben, pb, "sb-ben").await;
        let app = router(state);

        for query in [
            "scope=project",
            "scope=project&project=inconnu",
            "scope=sandbox&sandbox=sb-ben",
            "scope=global",
        ] {
            let (status, json) = resolve(&app, &cookie, query).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query} : {json}");
            assert!(
                json["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("VNL-APP-DSH-010")),
                "le marqueur 010 doit être rendu pour {query}, obtenu {json}"
            );
        }
    }

    /// Une sandbox de même nom dans deux projets de l'appelant : écran ambigu, 010.
    #[tokio::test]
    async fn sandbox_ambigue_refuse_010() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let alice = resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice")
            .id;
        let p1 = projet_de_test(&state.auth.db, alice, "p1").await;
        let p2 = projet_de_test(&state.auth.db, alice, "p2").await;
        sandbox_de_test(&state.auth.db, alice, p1, "s1").await;
        crate::db::entities::sandboxes::Entity::insert(
            crate::db::entities::sandboxes::ActiveModel {
                id: NotSet,
                owner_id: Set(alice),
                project_id: Set(p2),
                name: Set("s1".to_string()),
                cr_name: Set("cr-s1-bis".to_string()),
                missing_since: NotSet,
                created_at: NotSet,
            },
        )
        .exec(&state.auth.db)
        .await
        .expect("la seconde sandbox s'insère");
        let app = router(state);
        let (status, json) = resolve(&app, &cookie, "scope=sandbox&sandbox=s1").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        assert!(
            json["error"]
                .as_str()
                .is_some_and(|e| e.contains("VNL-APP-DSH-010")),
            "{json}"
        );
    }

    /// Scenario: `un défaut livré absent est une faute du serveur` — 500 avec `VNL-APP-DSH-008`
    /// (aucun scope livré n'en manque : le cas est porté par la fonction de repli).
    #[tokio::test]
    async fn un_defaut_livre_absent_est_une_faute_du_serveur() {
        use axum::response::IntoResponse;
        let erreur = defaut_ou_erreur(None).expect_err("sans défaut, pas de réponse");
        let reponse = erreur.into_response();
        assert_eq!(reponse.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let octets = axum::body::to_bytes(reponse.into_body(), usize::MAX)
            .await
            .expect("corps lisible");
        let json: serde_json::Value = serde_json::from_slice(&octets).expect("JSON");
        assert!(
            json["error"]
                .as_str()
                .is_some_and(|e| e.contains("VNL-APP-DSH-008")),
            "le marqueur 008 est rendu : {json}"
        );
    }

    /// Scenario: `la vue de sandbox est celle du projet` — la personnalisation écrite depuis
    /// `s1` s'applique à `s2` du même projet ; `sandbox` vaut `s2`, `project` vaut `p1`.
    #[tokio::test]
    async fn la_vue_de_sandbox_est_celle_du_projet() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let alice = resolve_user(&state.auth.db, "alice", None)
            .await
            .expect("alice")
            .id;
        let p1 = projet_de_test(&state.auth.db, alice, "p1").await;
        let s1 = sandbox_de_test(&state.auth.db, alice, p1, "s1").await;
        sandbox_de_test(&state.auth.db, alice, p1, "s2").await;
        insert_sandbox_custom(&state.auth.db, alice, p1, s1).await;
        let app = router(state);

        let (status, json) = resolve(&app, &cookie, "scope=sandbox&sandbox=s2").await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["origin"], serde_json::json!("custom"));
        assert_eq!(json["dashboard"]["scope"], serde_json::json!("sandbox"));
        assert_eq!(json["dashboard"]["sandbox"], serde_json::json!("s2"));
        assert_eq!(json["dashboard"]["project"], serde_json::json!("p1"));
    }

    /// Scenario: `une ligne stockée corrompue est visible, pas silencieuse` — insert en
    /// contournant le hook (SQL brut de test), 25 widgets. Test synchrone à runtime injecté
    /// pour pouvoir capturer les traces (`with_default` synchrone, motif de
    /// `../sessions/events.rs`).
    #[test]
    fn une_ligne_stockee_corrompue_est_visible() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime de test");
        let (journaux, (status, json)) = capture_logs(|| {
            runtime.block_on(async {
                let db = real_db().await;
                let user = resolve_user(&db, "alice", None).await.expect("user");
                let projet = projet_de_test(&db, user.id, "p1").await;
                let widgets: Vec<serde_json::Value> = (0..25)
                    .map(|i| {
                        serde_json::json!({
                            "id": format!("w-{i}"), "type": "chat", "config": {},
                            "x": 0, "y": 0, "w": 1, "h": 1
                        })
                    })
                    .collect();
                let def = serde_json::json!({ "layout": { "columns": 12 }, "widgets": widgets });
                let stmt = Statement::from_sql_and_values(
                    db.get_database_backend(),
                    "INSERT INTO vanyline_dashboards (owner_id, scope, kind, project_id, sandbox_id, title, def) \
                     VALUES (?, 'project', 'custom', ?, NULL, 'Corrompu', ?)",
                    vec![user.id.into(), projet.into(), def.into()],
                );
                db.execute_raw(stmt).await.expect("l'insertion brute passe");
                let rows: Vec<Model> = Entity::find().all(&db).await.expect("relisible");
                assert_eq!(rows.len(), 1, "la ligne corrompue est bien posée");

                let state = state_with(db);
                let cookie = session_cookie(&state, "alice").await;
                let app = router(state);
                resolve(&app, &cookie, "scope=project&project=p1").await
            })
        });
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            json["error"]
                .as_str()
                .is_some_and(|e| e.contains("VNL-APP-DSH-001")),
            "la corruption rend le marqueur, jamais un repli default : {json}"
        );
        assert!(
            json.get("origin").is_none(),
            "jamais un repli default silencieux : {json}"
        );
        assert!(
            journaux.iter().any(|l| l.contains("VNL-APP-DSH-001")),
            "le marqueur doit être journalisé en error : {journaux:?}"
        );
    }

    /// Support : capture des messages `tracing` émis pendant une fermeture (motif de
    /// `../sessions/events.rs`) — les identifiants journalisés sont un contrat de `Handles`.
    fn capture_logs<T>(f: impl FnOnce() -> T) -> (Vec<String>, T) {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        struct BufWriter(Buf);
        impl Write for BufWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .0
                    .lock()
                    .expect("tampon de capture jamais corrompu")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl tracing_subscriber::fmt::MakeWriter<'_> for Buf {
            type Writer = BufWriter;
            fn make_writer(&self) -> Self::Writer {
                BufWriter(self.clone())
            }
        }

        let buf = Buf::default();
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_writer(buf.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::ERROR)
            .finish();
        let sortie = tracing::subscriber::with_default(subscriber, f);
        let octets = {
            let mut garde = buf.0.lock().expect("tampon de capture jamais corrompu");
            std::mem::take(&mut *garde)
        };
        (
            String::from_utf8_lossy(&octets)
                .lines()
                .map(str::to_owned)
                .collect(),
            sortie,
        )
    }
}
