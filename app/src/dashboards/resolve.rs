//! Résolution du tableau effectif d'un écran (`FR-2`, `DEC-5`) — contrat : `./resolve.sdd`.
//! La SEULE route HTTP propre à la Solution : lecture seule, jamais d'écriture ; la
//! résolution ne 404 JAMAIS (le défaut est toujours là, `AC-2`).

use axum::{
    Json,
    extract::{Query, State},
};
use miryad_core::auth::AuthUser;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use super::model::{DashboardDef, Kind, Layout, Scope, Widget};
use crate::AppState;
use crate::error::AppError;

/// Paramètres de `GET /api/dashboards/resolve` : `scope` requis (absent → 400 par la
/// désérialisation axum, hors contrat de marqueur ici) ; `project`/`sandbox` ignorés hors de
/// leur écran.
#[derive(Debug, Deserialize)]
pub struct ResolveQuery {
    /// Écran résolu : `"home"`, `"project"` ou `"sandbox"` (sinon `VNL-APP-DSH-008`).
    pub scope: String,
    /// Nom du projet (écran projet ; ignoré ailleurs).
    #[serde(default)]
    pub project: Option<String>,
    /// Nom de la sandbox (écran sandbox ; ignoré ailleurs).
    #[serde(default)]
    pub sandbox: Option<String>,
}

/// Réponse de résolution : `{origin, dashboard}` (`DEC-5`).
#[derive(Serialize)]
pub struct Resolved {
    /// `"default"` (aucune personnalisation) ou `"custom"` (ligne de l'appelant).
    pub origin: &'static str,
    /// Tableau résolu, clés d'écran enveloppées dans la réponse.
    pub dashboard: DashboardView,
}

/// La définition effective augmentée des clés d'écran (`scope`, `kind`, `project`,
/// `sandbox`) — présentes dans la RÉPONSE, jamais dans le JSON stocké.
#[derive(Serialize)]
pub struct DashboardView {
    /// Id de la ligne pour `custom`, `null` pour `default`.
    pub id: Option<i32>,
    /// Libellé (colonne `title` de la ligne, ou libellé du défaut livré).
    pub title: String,
    /// Écran résolu.
    pub scope: &'static str,
    /// Origine de la définition : `"custom"` ou `"default"`.
    pub kind: Kind,
    /// Projet de l'écran (sinon `null`).
    pub project: Option<String>,
    /// Sandbox de l'écran (sinon `null`).
    pub sandbox: Option<String>,
    /// Grille de la définition.
    pub layout: Layout,
    /// Widgets dans l'ordre de rendu.
    pub widgets: Vec<Widget>,
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
    let project = if matches!(scope, Scope::Project) {
        Some(non_vide(query.project).ok_or_else(ecran_impossible)?)
    } else {
        None
    };
    let sandbox = if matches!(scope, Scope::Sandbox) {
        Some(non_vide(query.sandbox).ok_or_else(ecran_impossible)?)
    } else {
        None
    };

    // Identité de l'APPELANT (jamais un owner_id de requête) — motif de
    // `../../api/workflows.rs`.
    let appelant =
        miryad_core::users::resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
            .await?;

    // Requête NULL-safe par écran (`Handles` de `./resolve.sdd`) : deux branches
    // IS NULL / EQ, équivalent de `IS NOT DISTINCT FROM` sur les deux moteurs.
    let mut recherche = Entity::find()
        .filter(Column::OwnerId.eq(appelant.id))
        .filter(Column::Scope.eq(scope.as_str()))
        .filter(Column::Kind.eq("custom"));
    recherche = match scope {
        Scope::Home => recherche
            .filter(Column::Project.is_null())
            .filter(Column::Sandbox.is_null()),
        Scope::Project => recherche
            .filter(Column::Project.eq(project.clone()))
            .filter(Column::Sandbox.is_null()),
        Scope::Sandbox => recherche
            .filter(Column::Project.is_null())
            .filter(Column::Sandbox.eq(sandbox.clone())),
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
            origin: "custom",
            dashboard: DashboardView {
                id: Some(ligne.id),
                title: ligne.title,
                scope: scope.as_str(),
                kind: Kind::Custom,
                project,
                sandbox,
                layout: def.layout,
                widgets: def.widgets,
            },
        }));
    }

    let def: &'static DashboardDef =
        super::defaults::default_for(scope).ok_or_else(ecran_impossible)?;
    Ok(Json(Resolved {
        origin: "default",
        dashboard: DashboardView {
            id: None,
            title: def.title.clone(),
            scope: scope.as_str(),
            kind: Kind::Default,
            project,
            sandbox,
            layout: def.layout,
            widgets: def.widgets.clone(),
        },
    }))
}

/// Écran impossible et scope inconnu : même marqueur, message fixe sans contenu embarqué
/// (`Raises` de `./resolve.sdd` — la variante `InternalError` est le barème 500 de
/// `../../error.sdd`, cf. l'écart `[?]` posé dans ses Tasks).
fn ecran_impossible() -> AppError {
    AppError::InternalError("VNL-APP-DSH-008: unknown or impossible dashboard screen".into())
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
            project: Set(Some("p1".to_string())),
            sandbox: Set(None),
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

    /// Scenario: `rendre les défauts revient au défaut` (suppression par la voie de
    /// l'API générique, modélisée par `delete_by_id` — le DELETE du CRUD est le contrat de
    /// `../db/entities/dashboards.sdd`).
    #[tokio::test]
    async fn rendre_les_defauts_revient_au_defaut() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let id = insert_custom_hook(&state.auth.db, "alice").await;
        Entity::delete_by_id(id)
            .exec(&state.auth.db)
            .await
            .expect("la ligne se supprime");
        let app = router(state);

        let (status, json) = resolve(&app, &cookie, "scope=project&project=p1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json["origin"],
            serde_json::json!("default"),
            "AC-3 bout en bout"
        );
    }

    /// Scenario: `écran impossible et scope inconnu refusent 008`. ÉCART DE SPEC consigné
    /// (`[?]` de `./resolve.sdd`) : la spec dit 400, le barème de `../../error.sdd` rend
    /// `InternalError` en 500 — test écrit sur 500 + marqueur, variante non réaffectée.
    #[tokio::test]
    async fn ecran_impossible_et_scope_inconnu_refusent_008() {
        let state = state_with(real_db().await);
        let cookie = session_cookie(&state, "alice").await;
        let app = router(state);

        for query in ["scope=project", "scope=global"] {
            let (status, json) = resolve(&app, &cookie, query).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{query}");
            assert!(
                json["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("VNL-APP-DSH-008")),
                "le marqueur 008 doit être rendu pour {query}, obtenu {json}"
            );
        }
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
                    "INSERT INTO vanyline_dashboards (owner_id, scope, kind, project, sandbox, title, def) \
                     VALUES (?, 'project', 'custom', 'p1', NULL, 'Corrompu', ?)",
                    vec![user.id.into(), def.into()],
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
