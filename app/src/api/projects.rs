use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::time::Duration;

use vanyline_crds::{EgressRule, Owner, Project, ProjectSpec};
use vanyline_lib::k8s::VnlK8sClient;

use miryad_core::auth::{AuthPrincipal, AuthUser};
use miryad_core::users::resolve_user;

use crate::{AppState, api::owners, error::AppError, k8s};

/// Body de `POST /api/projects`. Reprend les champs de `ProjectSpec` SAUF `owner`,
/// qui est dérivé de l'utilisateur authentifié (décision développeur : owner dérivé).
/// `#[serde(rename_all = "camelCase")]` aligne le JSON sur les conventions CRD.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectBody {
    pub name: String,
    pub repo_url: String,
    #[serde(default)]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub storage_size: Option<String>,
    #[serde(default)]
    pub storage_class: Option<String>,
    #[serde(default)]
    pub storage_access_mode: Option<String>,
    #[serde(default)]
    pub git_secret: Option<String>,
    #[serde(default)]
    pub caches: Option<Vec<String>>,
    #[serde(default)]
    pub fetch_interval: Option<String>,
    #[serde(default)]
    pub egress: Vec<EgressRule>,
}

pub async fn list_projects(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Project>>, AppError> {
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Ok(Json(Vec::new())); // aucun Owner -> liste vide
    };
    let client = k8s::client(&state).await?;
    let projects = client.list_projects(&owner).await?;
    Ok(Json(
        projects
            .into_iter()
            .filter(|p| p.spec.owner == owner)
            .collect(),
    ))
}

pub async fn get_project(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<Project>, AppError> {
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(&state).await?;
    let project = client.get_project(&owner, &name).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    Ok(Json(project))
}

pub async fn create_project(
    State(state): State<AppState>,
    user: AuthPrincipal,
    Json(body): Json<CreateProjectBody>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let owner = owners::ensure_owner(&state, principal_user.id, &user).await?;
    let spec = ProjectSpec {
        owner: owner.clone(),
        repo_url: body.repo_url,
        default_branch: body.default_branch,
        storage_size: body.storage_size,
        storage_class: body.storage_class,
        storage_access_mode: body.storage_access_mode,
        git_secret: body.git_secret,
        caches: body.caches,
        fetch_interval: body.fetch_interval,
        egress: body.egress,
    };
    let client = k8s::client(&state).await?;
    wait_owner_ready(&client, &owner).await;
    let project = client.create_project(&owner, &body.name, spec).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

pub async fn delete_project(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(&state).await?;
    let project = client.get_project(&owner, &name).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    client.delete_project(&owner, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Délai maximal d'attente que l'Owner soit prêt (namespace + `Ready=True`) avant la
/// création du Project (projects.sdd, 2026-10-19 : 403 du premier Project sur un Owner neuf).
const OWNER_READY_TIMEOUT_SECS: u64 = 30;
/// Période de scrutation de l'Owner pendant l'attente.
const OWNER_READY_POLL_MILLIS: u64 = 500;

/// Vrai quand l'Owner a son `status.namespace` posé ET une condition `Ready` à `True`.
#[must_use]
pub fn owner_is_ready(owner: &Owner) -> bool {
    owner.status.as_ref().is_some_and(|status| {
        status.namespace.is_some()
            && status
                .conditions
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True")
    })
}

/// Scrute `read` toutes les `interval` jusqu'à ce que l'Owner lu soit prêt, bornée à `budget`.
/// Une erreur de lecture vaut « pas prêt ». À l'échéance, rend la main sans erreur (la création
/// du Project est alors tentée quand même).
async fn wait_owner_ready_with<F, Fut, E>(mut read: F, budget: Duration, interval: Duration)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Owner, E>>,
{
    let waited = tokio::time::timeout(budget, async {
        loop {
            if matches!(read().await, Ok(ref owner) if owner_is_ready(owner)) {
                return;
            }
            tokio::time::sleep(interval).await;
        }
    })
    .await;
    if waited.is_err() {
        tracing::warn!("owner pas prêt après {budget:?} : création du project tentée quand même");
    }
}

/// Attend l'Owner `owner` via le client K8s (cf. [`wait_owner_ready_with`]).
async fn wait_owner_ready(client: &VnlK8sClient, owner: &str) {
    wait_owner_ready_with(
        || client.get_owner(owner),
        Duration::from_secs(OWNER_READY_TIMEOUT_SECS),
        Duration::from_millis(OWNER_READY_POLL_MILLIS),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        routing::get,
    };
    use tower::ServiceExt;

    fn test_key() -> cookie::Key {
        cookie::Key::from(&[0u8; 64])
    }

    fn make_app(cookie_key: cookie::Key) -> Router {
        let config = crate::config::Config {
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
        };

        let state = AppState {
            config,
            cookie_key,
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        };

        Router::new()
            .route("/projects", get(list_projects).post(create_project))
            .route("/projects/{name}", get(get_project).delete(delete_project))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_projects_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/projects")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_project_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/projects/my-project")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // Scenario: créer et supprimer sans cookie retournent 401 (projects.sdd, gelé).
    // POST /projects avec corps JSON valide + DELETE /projects/p sans cookie :
    // refus 401 avant le corps des handlers — rien écrit ni demandé au cluster
    // (l'@AppState est hors-ligne : cache k8s `None`, aucun cluster joignable).
    #[tokio::test]
    async fn creer_et_supprimer_sans_cookie_retournent_401() {
        let app = make_app(test_key());
        let post_req = Request::builder()
            .method("POST")
            .uri("/projects")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"p","repoUrl":"u"}"#))
            .unwrap();
        let resp = app.clone().oneshot(post_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let del_req = Request::builder()
            .method("DELETE")
            .uri("/projects/p")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(del_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

#[cfg(test)]
mod owner_ready_tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    use vanyline_crds::{OwnerSpec, OwnerStatus};

    /// Owner de test : `namespace` et `conditions` posés dans le status (`None` => status absent
    /// quand `namespace` est `None` ET `conditions` est `Value::Null`).
    fn owner_with(namespace: Option<&str>, conditions: serde_json::Value) -> Owner {
        let mut owner = Owner::new(
            "alice",
            OwnerSpec {
                home_size: None,
                home_storage_class: None,
                home_access_mode: None,
                project_defaults: None,
                application_ref: None,
                egress: Vec::new(),
            },
        );
        owner.status = Some(OwnerStatus {
            namespace: namespace.map(str::to_string),
            conditions: serde_json::from_value(conditions).expect("conditions valides"),
            ..OwnerStatus::default()
        });
        owner
    }

    fn ready_cond(status: &str) -> serde_json::Value {
        json!([{
            "type": "Ready",
            "status": status,
            "lastTransitionTime": "2026-10-19T00:00:00Z",
            "message": "",
            "reason": "Reconciled"
        }])
    }

    fn owner_ready() -> Owner {
        owner_with(Some("app-alice"), ready_cond("True"))
    }

    fn owner_not_ready() -> Owner {
        owner_with(None, json!([]))
    }

    // Scenario: owner_pret_demande_namespace_et_condition_ready
    #[test]
    fn owner_is_ready_vrai_avec_namespace_et_ready_true() {
        assert!(owner_is_ready(&owner_ready()));
    }

    #[test]
    fn owner_is_ready_faux_sans_namespace() {
        assert!(!owner_is_ready(&owner_with(None, ready_cond("True"))));
    }

    #[test]
    fn owner_is_ready_faux_sans_condition_ready() {
        let autre = json!([{
            "type": "Other", "status": "True", "lastTransitionTime": "2026-10-19T00:00:00Z",
            "message": "", "reason": ""
        }]);
        assert!(!owner_is_ready(&owner_with(Some("app-alice"), autre)));
    }

    #[test]
    fn owner_is_ready_faux_si_ready_false() {
        assert!(!owner_is_ready(&owner_with(
            Some("app-alice"),
            ready_cond("False")
        )));
    }

    #[test]
    fn owner_is_ready_faux_sans_status() {
        let mut owner = owner_ready();
        owner.status = None;
        assert!(!owner_is_ready(&owner));
    }

    // Scenario: attente_de_l_owner_avant_la_creation_du_project
    #[tokio::test(start_paused = true)]
    async fn wait_owner_ready_une_seule_lecture_si_deja_pret() {
        let calls = Cell::new(0u32);
        wait_owner_ready_with(
            || {
                calls.set(calls.get() + 1);
                let owner = owner_ready();
                async move { Ok::<Owner, String>(owner) }
            },
            Duration::from_secs(OWNER_READY_TIMEOUT_SECS),
            Duration::from_millis(500),
        )
        .await;
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_owner_ready_rend_la_main_a_la_troisieme_lecture() {
        let calls = Cell::new(0u32);
        wait_owner_ready_with(
            || {
                calls.set(calls.get() + 1);
                let owner = if calls.get() >= 3 {
                    owner_ready()
                } else {
                    owner_not_ready()
                };
                async move { Ok::<Owner, String>(owner) }
            },
            Duration::from_secs(OWNER_READY_TIMEOUT_SECS),
            Duration::from_millis(500),
        )
        .await;
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_owner_ready_erreur_de_lecture_vaut_non_pret() {
        let calls = Cell::new(0u32);
        wait_owner_ready_with(
            || {
                calls.set(calls.get() + 1);
                let res = if calls.get() >= 2 {
                    Ok(owner_ready())
                } else {
                    Err("404 Not Found".to_string())
                };
                async move { res }
            },
            Duration::from_secs(OWNER_READY_TIMEOUT_SECS),
            Duration::from_millis(500),
        )
        .await;
        assert_eq!(calls.get(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_owner_ready_jamais_pret_rend_la_main_apres_la_borne() {
        let calls = Cell::new(0u32);
        let start = tokio::time::Instant::now();
        wait_owner_ready_with(
            || {
                calls.set(calls.get() + 1);
                let owner = owner_not_ready();
                async move { Ok::<Owner, String>(owner) }
            },
            Duration::from_secs(OWNER_READY_TIMEOUT_SECS),
            Duration::from_millis(500),
        )
        .await;
        assert!(
            start.elapsed() >= Duration::from_secs(OWNER_READY_TIMEOUT_SECS),
            "la borne doit être atteinte avant de rendre la main"
        );
        assert!(calls.get() > 1, "scrutation répétée : {}", calls.get());
    }
}
