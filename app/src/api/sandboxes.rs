use axum::{
    Json,
    body::to_bytes,
    extract::{Path, Request, State},
    http::StatusCode,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use vanyline_crds::{Sandbox, SandboxSpec};

use miryad_core::auth::AuthUser;
use miryad_core::users::resolve_user;

use crate::{AppState, api::owners, error::AppError, k8s};

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
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let owner = match owners::resolve_owner_name(&state, principal_user.id).await? {
        Some(o) => o,
        None => return Ok(Json(Vec::new())), // aucun Owner -> liste vide
    };
    let client = k8s::client(&state).await?;
    let projects = client.list_projects().await?;
    let owner_projects: Vec<String> = projects
        .into_iter()
        .filter(|p| p.spec.owner == owner)
        .filter_map(|p| p.metadata.name)
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
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let owner = match owners::resolve_owner_name(&state, principal_user.id).await? {
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
    if body.spec.branch.trim().is_empty() {
        return Err(AppError::SandboxBranchEmpty);
    }
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let owner = match owners::resolve_owner_name(&state, principal_user.id).await? {
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
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let owner = match owners::resolve_owner_name(&state, principal_user.id).await? {
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
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let owner = match owners::resolve_owner_name(&state, principal_user.id).await? {
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

/// Réponse de `POST /api/sandboxes/{name}/ws-ticket` : le ticket court-vécu
/// miné auprès de la sandbox + le host public WS
/// (`{name}.sandboxes.{application.host}`). Le JWT OIDC n'y figure jamais.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WsTicketOut {
    pub ticket: String,
    pub ws_host: String,
}

/// Relais de ticket WS : scoping owner identique à `get_sandbox`, host
/// public résolu via la CR Application (owner → `application_ref` →
/// `spec.host`), ticket miné auprès de la sandbox en présentant le
/// `id_token` OIDC de l'utilisateur (Bearer). Le navigateur reçoit
/// `{ ticket, wsHost }` — jamais le JWT.
pub async fn ws_ticket(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<WsTicketOut>, AppError> {
    // 1. scoping owner identique à get_sandbox/delete_sandbox (déjà en place)
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let owner = match owners::resolve_owner_name(&state, principal_user.id).await? {
        Some(o) => o,
        None => return Err(AppError::Forbidden),
    };
    let client = k8s::client(&state).await?;
    let sandbox = client.get_sandbox(&name).await?;
    let project = client.get_project(&sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }

    // 2. host public depuis la CR Application (chaîne owner -> application_ref)
    let owner_obj = client.get_owner(&owner).await?;
    let Some(app_ref) = owner_obj.spec.application_ref else {
        return Err(AppError::SandboxNotExposed);
    };
    let application = client.get_application(&app_ref).await?;
    let ws_host = format!("{name}.sandboxes.{}", application.spec.host);

    // 3. relais : POST /ws/ticket interne avec le id_token OIDC (Bearer)
    let url = client.sandbox_ws_ticket_url(&name).await?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(AppError::RequestError)?;
    let resp = http
        .post(&url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", user.id_token),
        )
        .send()
        .await?
        .error_for_status()?;
    let body: serde_json::Value = resp.json().await?;
    let ticket = body["ticket"].as_str().ok_or_else(|| {
        AppError::InternalError("VNL-SBX-002: sandbox ticket response missing 'ticket'".into())
    })?;

    Ok(Json(WsTicketOut {
        ticket: ticket.to_string(),
        ws_host,
    }))
}

/// Proxy générique vers la sandbox : construit la requête (méthode, URL,
/// headers optionnels, body, `Authorization: Bearer {token}`),
/// l'envoie, et retourne `(statut, JSON)` de la réponse TELS QUELS — y compris
/// les erreurs HTTP de la sandbox (pas de `error_for_status`, pas de
/// transformation). Le token OIDC n'est jamais renvoyé au navigateur.
async fn proxy_git_request(
    http: &reqwest::Client,
    url: &str,
    method: reqwest::Method,
    headers: Vec<(String, String)>, // content-type, accept, si présents
    body: Bytes,
    token: &str,
) -> Result<(StatusCode, serde_json::Value), AppError> {
    let mut req = http
        .request(method, url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
    for (k, v) in headers {
        req = req.header(&k, &v);
    }
    let resp = req.body(body).send().await?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .map_err(|e| AppError::InternalError(e.to_string()))?;
    // Passthrough réel : un body non-JSON (ex. 404/405 texte brut renvoyé
    // par axum pour une route sandbox non matchée) ne doit pas faire
    // échouer le proxy — on le enveloppe plutôt que de perdre le vrai
    // statut/message derrière une erreur générique.
    let bytes = resp.bytes().await?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "error": String::from_utf8_lossy(&bytes).into_owned() }),
    );
    Ok((status, value))
}

/// Relais REST `ANY /api/sandboxes/{name}/git/{*path}` — scoping owner
/// identique à `ws_ticket` (get_or_create_user → resolve_owner_name →
/// get_sandbox → get_project → assert owner match), URL interne via
/// `sandbox_git_url`, forward méthode + path + query + body vers la sandbox
/// avec le `id_token` OIDC (Bearer), réponse passthrough.
/// Extrait le sous-chemin brut (encore percent-encodé) après
/// `/sandboxes/{name}/git/`, à partir du chemin RAW de la requête — pas
/// via l'extractor `Path`, qui décode le wildcard `{*path}` (vérifié
/// empiriquement : axum décode `%2F` en `/` avant que le handler ne voie la
/// valeur). Décoder puis reconstruire perdrait la distinction entre un `/`
/// séparateur de segments et un `%2F` légitime À L'INTÉRIEUR d'un segment
/// (ex. un nom de branche contenant `/`, comme `feature/x`) — d'où le
/// découpage positionnel sur le chemin brut plutôt qu'une recherche de
/// sous-chaîne (qui serait ambiguë si `name` contenait lui-même "git").
///
/// Rejette (`GitPathInvalid`) tout segment qui, une fois décodé, vaut `.`
/// ou `..` (traversal, y compris `%2e%2e`) ou est vide (`//`) — la requête
/// n'atteint jamais `sandbox_git_url`/reqwest si un segment est invalide,
/// donc la normalisation d'URL du client HTTP ne peut plus faire sortir la
/// requête du préfixe `/git/`.
fn raw_git_tail(uri_path: &str) -> Result<String, AppError> {
    let mut segments = uri_path.trim_start_matches('/').split('/');
    segments.next(); // "sandboxes" (littéral de la route)
    segments.next(); // "{name}"
    segments.next(); // "git" (littéral de la route)
    let tail_segments: Vec<&str> = segments.collect();
    for seg in &tail_segments {
        let decoded = percent_encoding::percent_decode_str(seg)
            .decode_utf8()
            .map_err(|_| AppError::GitPathInvalid)?;
        if decoded.is_empty() || decoded == "." || decoded == ".." {
            return Err(AppError::GitPathInvalid);
        }
    }
    Ok(tail_segments.join("/"))
}

#[axum::debug_handler]
pub async fn git_proxy(
    State(state): State<AppState>,
    user: AuthUser,
    Path((name, _path)): Path<(String, String)>,
    req: Request, // dernier extractor, consuming (cf. task sp.)
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    // 0. Chemin brut validé (voir raw_git_tail) — _path (décodé par
    // l'extractor Path) est délibérément ignoré, cf. doc de raw_git_tail.
    let raw_path = raw_git_tail(req.uri().path())?;

    // 1. scoping owner identique à ws_ticket
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(&state).await?;
    let sandbox = client.get_sandbox(&name).await?;
    let project = client.get_project(&sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }

    // 2. URL interne
    let mut base_url = client.sandbox_git_url(&name, &raw_path).await?;

    // 3. Query string
    let query = req.uri().query().map(String::from);
    let method = req.method().clone();
    if let Some(q) = &query {
        base_url = format!("{base_url}?{q}");
    }

    // 4. Client reqwest (30s : les opérations git réseau peuvent dépasser 10s)
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(AppError::RequestError)?;

    // 4bis. Headers à forwarder : content-type et accept, si présents
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(v) = req.headers().get(axum::http::header::CONTENT_TYPE) {
        headers.push((
            "content-type".to_string(),
            v.to_str().unwrap_or("").to_string(),
        ));
    }
    if let Some(v) = req.headers().get(axum::http::header::ACCEPT) {
        headers.push(("accept".to_string(), v.to_str().unwrap_or("").to_string()));
    }

    // 5. Body (to_bytes — 1 MiB max, git bodies are small)
    let body = to_bytes(req.into_body(), 1024 * 1024)
        .await
        .map_err(|e| AppError::InternalError(e.to_string()))?;

    // 6. Appel proxy + retour passthrough
    let (status, value) =
        proxy_git_request(&http, &base_url, method, headers, body, &user.id_token).await?;

    Ok((status, Json(value)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn raw_git_tail_nominal() {
        assert_eq!(raw_git_tail("/sandboxes/foo/git/status").unwrap(), "status");
        assert_eq!(
            raw_git_tail("/sandboxes/foo/git/branches/feature%2Fx").unwrap(),
            "branches/feature%2Fx"
        );
    }

    #[test]
    fn raw_git_tail_rejects_traversal() {
        assert!(raw_git_tail("/sandboxes/foo/git/../mcp").is_err());
        assert!(raw_git_tail("/sandboxes/foo/git/%2e%2e/mcp").is_err());
        assert!(raw_git_tail("/sandboxes/foo/git/branches/../../ws/ticket").is_err());
    }

    #[test]
    fn raw_git_tail_rejects_empty_segment() {
        assert!(raw_git_tail("/sandboxes/foo/git/branches//x").is_err());
    }

    #[test]
    fn raw_git_tail_name_containing_git_is_not_ambiguous() {
        // Nom de sandbox == "git" : le split positionnel (pas une recherche
        // de sous-chaîne "/git/") ne doit pas se tromper de frontière.
        assert_eq!(
            raw_git_tail("/sandboxes/git/git/branches/x").unwrap(),
            "branches/x"
        );
    }
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::{any, get, post},
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
            cookie_key,
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
        };

        Router::new()
            .route("/sandboxes", get(list_sandboxes).post(create_sandbox))
            .route("/sandboxes/{name}", get(get_sandbox).delete(delete_sandbox))
            .route("/sandboxes/{name}/ws-ticket", post(ws_ticket))
            .route("/sandboxes/{name}/git/{*path}", any(git_proxy))
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

    #[tokio::test]
    async fn ws_ticket_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/sandboxes/my-sandbox/ws-ticket")
            .method("POST")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn git_proxy_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/sandboxes/my-sandbox/git/status")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
