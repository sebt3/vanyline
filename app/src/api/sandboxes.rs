use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use vanyline_crds::{Sandbox, SandboxSpec};

use crate::{
    api::conversations::get_or_create_user,
    api::owners,
    auth::middleware::AuthUser,
    error::AppError,
    k8s,
    AppState,
};

/// Body de `POST /api/sandboxes`. `name` porte le nom du CRD Sandbox ;
/// `#[serde(flatten)]` passe le reste (`SandboxSpec`) tel quel (passthrough).
/// Le handler vérifie que `spec.project` appartient à l'Owner de l'utilisateur.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSandboxBody {
    pub name: String,
    #[serde(flatten)]
    pub spec: SandboxSpec,
}

#[derive(Deserialize)]
pub struct SuspendBody {
    pub suspended: bool,
}

pub async fn list_sandboxes(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Sandbox>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let owner = match owners::resolve_owner_name(&state, db_user.id).await? {
        Some(o) => o,
        None => return Ok(Json(Vec::new())), // aucun Owner -> liste vide
    };
    let client = k8s::client(&state).await?;
    let projects = client.list_projects().await?;
    let owner_projects: Vec<String> = projects
        .into_iter()
        .filter(|p| p.spec.owner == owner)
        .filter_map(|p| p.metadata.name.clone())
        .collect();
    let sandboxes = client.list_sandboxes().await?;
    Ok(Json(
        sandboxes
            .into_iter()
            .filter(|s| owner_projects.contains(&s.spec.project))
            .collect(),
    ))
}

pub async fn get_sandbox(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<Sandbox>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let owner = match owners::resolve_owner_name(&state, db_user.id).await? {
        Some(o) => o,
        None => return Err(AppError::Forbidden),
    };
    let client = k8s::client(&state).await?;
    let sandbox = client.get_sandbox(&name).await?;
    let project = client.get_project(&sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    Ok(Json(sandbox))
}

pub async fn create_sandbox(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateSandboxBody>,
) -> Result<(StatusCode, Json<Sandbox>), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let owner = match owners::resolve_owner_name(&state, db_user.id).await? {
        Some(o) => o,
        None => return Err(AppError::Forbidden),
    };
    let client = k8s::client(&state).await?;
    let project = client.get_project(&body.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    let sandbox = client.create_sandbox(&body.name, body.spec).await?;
    Ok((StatusCode::CREATED, Json(sandbox)))
}

pub async fn delete_sandbox(
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
    let sandbox = client.get_sandbox(&name).await?;
    let project = client.get_project(&sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    client.delete_sandbox(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_sandbox_suspended(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<SuspendBody>,
) -> Result<Json<Sandbox>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let owner = match owners::resolve_owner_name(&state, db_user.id).await? {
        Some(o) => o,
        None => return Err(AppError::Forbidden),
    };
    let client = k8s::client(&state).await?;
    let sandbox = client.get_sandbox(&name).await?;
    let project = client.get_project(&sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    let updated = client.set_sandbox_suspended(&name, body.suspended).await?;
    Ok(Json(updated))
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
            .route("/sandboxes", get(list_sandboxes).post(create_sandbox))
            .route("/sandboxes/{name}", get(get_sandbox).delete(delete_sandbox))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_sandboxes_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/sandboxes")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_sandbox_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/sandboxes/my-sandbox")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_sandbox_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/sandboxes")
            .method("POST")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}