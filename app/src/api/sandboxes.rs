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
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Ok(Json(Vec::new())); // aucun Owner -> liste vide
    };
    let client = k8s::client(&state).await?;
    let projects = client.list_projects(&owner).await?;
    let owner_projects: Vec<String> = projects
        .into_iter()
        .filter(|p| p.spec.owner == owner)
        .filter_map(|p| p.metadata.name)
        .collect();
    let sandboxes = client.list_sandboxes(&owner).await?;
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
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(&state).await?;
    let sandbox = client.get_sandbox(&owner, &name).await?;
    let project = client.get_project(&owner, &sandbox.spec.project).await?;
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
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(&state).await?;
    let project = client.get_project(&owner, &body.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    let sandbox = client.create_sandbox(&owner, &body.name, body.spec).await?;
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
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(&state).await?;
    let sandbox = client.get_sandbox(&owner, &name).await?;
    let project = client.get_project(&owner, &sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    client.delete_sandbox(&owner, &name).await?;
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
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(&state).await?;
    let sandbox = client.get_sandbox(&owner, &name).await?;
    let project = client.get_project(&owner, &sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    let updated = client
        .set_sandbox_suspended(&owner, &name, body.suspended)
        .await?;
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
/// `spec.host`), ticket miné auprès de la sandbox en présentant le jeton du
/// `ServiceAccount` de l'Owner (`VnlK8sClient::owner_token`, Bearer — décision
/// 2026-10-04 : l'`id_token` de l'utilisateur n'est JAMAIS transmis à la
/// sandbox). Le navigateur reçoit `{ ticket, wsHost }` — jamais le JWT.
pub async fn ws_ticket(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<WsTicketOut>, AppError> {
    // 1. scoping owner identique à get_sandbox/delete_sandbox (déjà en place)
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let Some(owner) = owners::resolve_owner_name(&state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(&state).await?;
    let sandbox = client.get_sandbox(&owner, &name).await?;
    let project = client.get_project(&owner, &sandbox.spec.project).await?;
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

    // 3. jeton d'Owner : demandé SEULEMENT ici, après la chaîne de scoping
    // complète — un non-owner n'a jamais atteint ce point et n'obtient
    // jamais de jeton (spec `Must`, 2026-10-04).
    let owner_token = client.owner_token(&owner).await?;

    // 4. relais : POST /ws/ticket interne avec le jeton du SA de l'Owner (Bearer)
    let url = client.sandbox_ws_ticket_url(&owner, &name).await?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(AppError::RequestError)?;
    let resp = http
        .post(&url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", owner_token.token),
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
/// transformation). Le `token` est le jeton du SA de l'Owner (obtenu par
/// `VnlK8sClient::owner_token`) ; il n'est jamais renvoyé au navigateur.
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
/// identique à `ws_ticket` (`get_or_create_user` → `resolve_owner_name` →
/// `get_sandbox` → `get_project` → assert owner match), URL interne via
/// `sandbox_git_url`, forward méthode + path + query + body vers la sandbox
/// en présentant le jeton du SA de l'Owner (`owner_token`, Bearer — décision
/// 2026-10-04 : jamais l'`id_token` de l'utilisateur), réponse passthrough.
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
    let sandbox = client.get_sandbox(&owner, &name).await?;
    let project = client.get_project(&owner, &sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }

    // 2. URL interne
    let mut base_url = client.sandbox_git_url(&owner, &name, &raw_path).await?;

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

    // 5bis. Jeton d'Owner : demandé seulement ici, la chaîne de scoping est
    // complète — un non-owner a été rejeté plus haut et n'obtient jamais de
    // jeton (spec `Must`, 2026-10-04).
    let owner_token = client.owner_token(&owner).await?;

    // 6. Appel proxy + retour passthrough
    let (status, value) =
        proxy_git_request(&http, &base_url, method, headers, body, &owner_token.token).await?;

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
        }
    }

    /// `AuthUser` de test avec un `id_token` reconnaissable : les Scenarios
    /// d'auth 2026-10-04 verrouillent qu'il ne sort JAMAIS vers la sandbox.
    fn test_user() -> AuthUser {
        AuthUser {
            subject: "alice".to_string(),
            email: Some("alice@example.com".to_string()),
            id_token: "id-token-utilisateur".to_string(),
        }
    }

    /// État de test branché sur une vraie base (migrations appliquées,
    /// `db::test_support::real_db`) — mêmes conventions que `conversations.rs`.
    fn test_state_with_db(db: sea_orm::DatabaseConnection) -> AppState {
        AppState {
            config: test_config(),
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
        }
    }

    /// État de test sur base simulée SANS résultat programmé : toute
    /// interrogation de la base (ex. `resolve_user` atteinte) échoue — les
    /// tests de refus pré-scoping en déduisent que le handler n'a rien lu.
    fn test_state_mock_db() -> AppState {
        AppState {
            config: test_config(),
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
        }
    }

    fn make_app(cookie_key: cookie::Key) -> Router {
        let state = AppState {
            config: test_config(),
            cookie_key,
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
        };

        Router::new()
            .route("/sandboxes", get(list_sandboxes).post(create_sandbox))
            .route("/sandboxes/{name}", get(get_sandbox).delete(delete_sandbox))
            .route("/sandboxes/{name}/ws-ticket", post(ws_ticket))
            .route("/sandboxes/{name}/suspend", post(set_sandbox_suspended))
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

    // ── Scenario: les_six_handlers_sans_session_retournent_401 (6e route :
    // `/suspend`, harnais enrichi — cf. Tasks `[!]` harnais) ──
    #[tokio::test]
    async fn set_sandbox_suspended_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/sandboxes/my-sandbox/suspend")
            .method("POST")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"suspended":true}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── Scenario: raw_git_tail_rejette_le_segment_vide_et_le_decodage_utf8_invalide ──
    #[test]
    fn raw_git_tail_rejette_le_segment_vide_et_le_decodage_utf8_invalide() {
        assert!(
            matches!(
                raw_git_tail("/sandboxes/foo/git/branches//x"),
                Err(AppError::GitPathInvalid)
            ),
            "un segment vide doit rendre GitPathInvalid"
        );
        assert!(
            matches!(
                raw_git_tail("/sandboxes/foo/git/%ff%fe"),
                Err(AppError::GitPathInvalid)
            ),
            "un segment dont decode_utf8 échoue doit rendre GitPathInvalid"
        );
    }

    // ── Scenario: raw_git_tail_sous_chemin_totalement_absent_retourne_vide
    // (asymétrie avec `/git/` — segment vide → Err — verrouillée telle quelle,
    // cf. Tasks `[?]`) ──
    #[test]
    fn raw_git_tail_sous_chemin_totalement_absent_retourne_vide() {
        assert_eq!(raw_git_tail("/sandboxes/foo/git").unwrap(), "");
    }

    // ── Scenario: ws_ticket_ok_rend_ticket_et_wsHost_sans_exposer_le_jwt
    // (enveloppe verrouillée hors cluster : exactement 2 clés `ticket`/`wsHost`,
    // ni jeton, ni expires_in_secs. Le relais lui-même suppose la sandbox —
    // Scenario d'intégration cluster, cf. Tasks `[!]`) ──
    #[test]
    fn ws_ticket_ok_rend_ticket_et_ws_host_sans_exposer_le_jwt() {
        let out = WsTicketOut {
            ticket: "t32".to_string(),
            ws_host: "s1.sandboxes.app.example.com".to_string(),
        };
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"ticket": "t32", "wsHost": "s1.sandboxes.app.example.com"})
        );
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["ticket", "wsHost"], "exactement 2 clés");
    }

    // ── Scenario: liste_sans_owner_retourne_200_vide_et_pas_403 ──
    // Base réelle sans `vanyline_owner_links` : 200 liste vide ET aucun client
    // K8s découvert (le `None` court-circuite avant `k8s::client`).
    #[tokio::test]
    async fn liste_sans_owner_retourne_200_vide_et_pas_403() {
        let db = crate::db::test_support::real_db().await;
        let state = test_state_with_db(db);
        let Json(list) = list_sandboxes(State(state.clone()), test_user())
            .await
            .expect("list_sandboxes sans owner doit rendre Ok");
        assert!(list.is_empty(), "aucun Owner -> liste vide");
        assert!(
            state.k8s.lock().await.is_none(),
            "aucun client K8s ne doit être découvert quand l'Owner est absent"
        );
    }

    // ── Scenario: creation_branch_vide_ou_blanc_est_rejetee_avant_tout_acces_k8s
    // Utilisateur authentifié SANS owner lié : le contrôle `branch` précède
    // `resolve_owner_name` (sinon ce serait 403 Forbidden) et précède K8s. ──
    #[tokio::test]
    async fn creation_branch_vide_ou_blanc_est_rejetee_avant_tout_acces_k8s() {
        let db = crate::db::test_support::real_db().await;
        let state = test_state_with_db(db);
        for branch in ["", "   "] {
            let body: CreateSandboxBody = serde_json::from_str(&format!(
                r#"{{"name":"s","project":"p","branch":"{branch}"}}"#
            ))
            .unwrap();
            let err = create_sandbox(State(state.clone()), test_user(), Json(body))
                .await
                .expect_err("branch vide/blanc doit être rejeté");
            assert!(
                matches!(err, AppError::SandboxBranchEmpty),
                "branch={branch:?} : attendu SandboxBranchEmpty (422), obtenu {err:?}"
            );
        }
        assert!(
            state.k8s.lock().await.is_none(),
            "aucun accès K8s avant le contrôle de branch"
        );
    }

    // ── Scenario: git_proxy_traversee_rejete_400_sans_appel_aval ──
    // Base simulée SANS résultat programmé : si `resolve_user` était atteint
    // (contrairement au contrat « raw_git_tail en première ligne »), l'erreur
    // serait InternalError (VNL-DB-006) et non GitPathInvalid.
    #[tokio::test]
    async fn git_proxy_traversee_rejete_400_sans_appel_aval() {
        let state = test_state_mock_db();
        let mut req = Request::new(Body::empty());
        *req.uri_mut() = "/sandboxes/s1/git/%2e%2e/mcp".parse().unwrap();
        let err = git_proxy(
            State(state),
            test_user(),
            Path(("s1".to_string(), "..%2F..%2Fmcp".to_string())),
            req,
        )
        .await
        .expect_err("traversal encodé doit être rejeté");
        assert!(
            matches!(err, AppError::GitPathInvalid),
            "attendu GitPathInvalid (400), obtenu {err:?}"
        );
    }

    // ── Serveur de capture « sandbox » pour proxy_git_request ──
    // Les Scenarios ws_ticket/git_proxy qui exigent l'apiserver (owner_token,
    // scoping K8s) restent des Scenarios d'intégration cluster (cf. Tasks
    // `[!]`). Ce harnais teste hors cluster ce que proxy_git_request maîtrise :
    // en-tête Bearer exactement tel qu'on le lui donne, méthode, query brute,
    // corps, passthrough des statuts, enveloppe non-JSON, conservation du
    // percent-encodage.
    #[derive(Clone)]
    struct Captured {
        method: String,
        path_and_query: String,
        authorization: Option<String>,
        content_type: Option<String>,
        body: String,
    }

    /// Démarre un serveur axum sur port éphémère qui capture les requêtes
    /// reçues et répond `(status, content_type, body)`. Retourne l'URL de base
    /// et la liste des captures.
    async fn spawn_sandbox_capture(
        status: u16,
        content_type: &'static str,
        body: &'static str,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<Captured>>>) {
        let cap = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let cap_handler = cap.clone();
        let body = body.to_string();
        let app = axum::Router::new().route(
            "/{*path}",
            axum::routing::any(move |req: axum::extract::Request| {
                let cap = cap_handler.clone();
                let body = body.clone();
                async move {
                    let method = req.method().to_string();
                    let path_and_query = match req.uri().query() {
                        Some(q) => format!("{}?{q}", req.uri().path()),
                        None => req.uri().path().to_string(),
                    };
                    let authorization = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    let seen_ct = req
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    cap.lock().unwrap().push(Captured {
                        method,
                        path_and_query,
                        authorization,
                        content_type: seen_ct,
                        body: String::from_utf8_lossy(&bytes).into_owned(),
                    });
                    (
                        StatusCode::from_u16(status).unwrap(),
                        [(axum::http::header::CONTENT_TYPE, content_type)],
                        bytes::Bytes::from(body),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), cap)
    }

    // ── Scenario: git_proxy_forward_methode_query_bearer_owner_et_passthrough_statut
    // (partie hors cluster : le relais présente le Bearer qu'on lui donne — le
    // jeton du SA de l'Owner — et PAS l'id_token ; la méthode, la query brute,
    // le content-type et le corps transitent ; statut 409 et JSON passthrough).
    // L'enchaînement handler → owner_token(apiserver) est un Scenario
    // d'intégration cluster (Tasks `[!]`). ──
    #[tokio::test]
    async fn git_proxy_forward_methode_query_bearer_owner_et_passthrough_statut() {
        let (base, cap) =
            spawn_sandbox_capture(409, "application/json", r#"{"error":"merge conflict"}"#).await;
        let http = reqwest::Client::new();
        let (status, value) = proxy_git_request(
            &http,
            &format!("{base}/git/commit?retry=1"),
            reqwest::Method::POST,
            vec![("content-type".to_string(), "application/json".to_string())],
            Bytes::from_static(br#"{"message":"m"}"#),
            "owner-sa-jeton-alice",
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CONFLICT, "passthrough du 409");
        assert_eq!(value, serde_json::json!({"error": "merge conflict"}));
        let seen = cap.lock().unwrap().remove(0);
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path_and_query, "/git/commit?retry=1", "query brute");
        assert_eq!(
            seen.authorization.as_deref(),
            Some("Bearer owner-sa-jeton-alice"),
            "le Bearer présenté est le jeton d'Owner passé en paramètre"
        );
        assert!(
            !seen
                .authorization
                .as_deref()
                .unwrap_or_default()
                .contains("id-token-utilisateur"),
            "l'id_token de l'utilisateur ne traverse jamais la sandbox : {:?}",
            seen.authorization
        );
        assert_eq!(seen.content_type.as_deref(), Some("application/json"));
        assert_eq!(seen.body, r#"{"message":"m"}"#, "corps d'origine forwardé");
    }

    // ── Scenario: git_proxy_enveloppe_le_corps_non_json_sans_changer_le_statut ──
    #[tokio::test]
    async fn git_proxy_enveloppe_le_corps_non_json_sans_changer_le_statut() {
        let (base, _cap) = spawn_sandbox_capture(404, "text/plain", "route not found").await;
        let http = reqwest::Client::new();
        let (status, value) = proxy_git_request(
            &http,
            &format!("{base}/git/nope"),
            reqwest::Method::GET,
            Vec::new(),
            Bytes::new(),
            "owner-sa-jeton",
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::NOT_FOUND, "statut d'origine conservé");
        assert_eq!(value, serde_json::json!({"error": "route not found"}));
    }

    // ── Scenario: git_proxy_preserve_le_percent_encodage_d_une_branche ──
    #[tokio::test]
    async fn git_proxy_preserve_le_percent_encodage_d_une_branche() {
        let (base, cap) = spawn_sandbox_capture(200, "application/json", "{}").await;
        let http = reqwest::Client::new();
        proxy_git_request(
            &http,
            &format!("{base}/git/branches/feature%2Fx"),
            reqwest::Method::GET,
            Vec::new(),
            Bytes::new(),
            "owner-sa-jeton",
        )
        .await
        .unwrap();
        let seen = cap.lock().unwrap().remove(0);
        assert_eq!(
            seen.path_and_query, "/git/branches/feature%2Fx",
            "le %2F d'un nom de branche ne doit jamais redevenir un séparateur"
        );
    }
}
