use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use vanyline_crds::{EgressRule, Project, ProjectSpec, PvcRef};

use crate::{
    api::conversations::get_or_create_user, api::owners, auth::middleware::AuthUser,
    error::AppError, k8s, AppState,
};

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
    pub existing_pvc: Option<PvcRef>,
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
    let db_user = get_or_create_user(&state, &user).await?;
    let owner = match owners::resolve_owner_name(&state, db_user.id).await? {
        Some(o) => o,
        None => return Ok(Json(Vec::new())), // aucun Owner -> liste vide
    };
    let client = k8s::client(&state).await?;
    let projects = client.list_projects().await?;
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
    let db_user = get_or_create_user(&state, &user).await?;
    let owner = match owners::resolve_owner_name(&state, db_user.id).await? {
        Some(o) => o,
        None => return Err(AppError::Forbidden),
    };
    let client = k8s::client(&state).await?;
    let project = client.get_project(&name).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    Ok(Json(project))
}

pub async fn create_project(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateProjectBody>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let owner = owners::ensure_owner(&state, &db_user).await?;
    let spec = ProjectSpec {
        owner: owner.clone(),
        repo_url: body.repo_url,
        default_branch: body.default_branch,
        existing_pvc: body.existing_pvc,
        storage_size: body.storage_size,
        storage_class: body.storage_class,
        storage_access_mode: body.storage_access_mode,
        git_secret: body.git_secret,
        caches: body.caches,
        fetch_interval: body.fetch_interval,
        egress: body.egress,
    };
    let client = k8s::client(&state).await?;
    let project = client.create_project(&body.name, spec).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

pub async fn delete_project(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let owner = match owners::resolve_owner_name(&state, db_user.id).await? {
        Some(o) => o,
        None => return Err(AppError::Forbidden),
    };
    let client = k8s::client(&state).await?;
    let project = client.get_project(&name).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    client.delete_project(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::auth::MockOidcClient;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
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
        };

        let state = AppState {
            config,
            oidc_client: std::sync::Arc::new(MockOidcClient),
            cookie_key,
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        };

        Router::new()
            .route("/projects", get(list_projects))
            .route("/projects/{name}", get(get_project))
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
}
