use axum::{
    Json,
    body::to_bytes,
    extract::{Path, Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use vanyline_crds::{Sandbox, SandboxSpec};
use vanyline_lib::k8s::VnlK8sClient;

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

/// Conversion des pannes TRANSPORT du relais app→sandbox en erreur OPAQUE
/// (`AppError::SandboxRelayUnavailable`, 502 `VNL-SBX-017`) — `./sandboxes.sdd` §Must
/// « Erreurs du RELAIS opaques » (revue 2026-10-14) : le `Display` de `reqwest::Error`
/// porte l'URL interne de la sandbox (`http://sandbox-{name}.{namespace}.svc:…`), qui ne
/// sort JAMAIS de l'app. La panne complète (reqwest, URL) est consignée ici en
/// `tracing::warn!` — jamais dans le message `@Display` rendu au navigateur.
fn map_relay_err(e: &reqwest::Error) -> AppError {
    let url = e.url().map(ToString::to_string).unwrap_or_default();
    tracing::warn!("panne du relais sandbox ({url}): {e}");
    AppError::SandboxRelayUnavailable
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
    let ticket = post_ws_ticket(&http, &url, &owner_token.token).await?;

    Ok(Json(WsTicketOut { ticket, ws_host }))
}

/// `POST /ws/ticket` vers la sandbox, Bearer du jeton d'Owner, et extraction du champ
/// `ticket`. Toute panne de transport (connexion, statut d'erreur, lecture ou décodage du
/// corps en cours de route) passe par `map_relay_err` : jamais de `reqwest::Error` brute
/// vers le navigateur (`./sandboxes.sdd`, Scenario `panne_de_lecture_du_corps_sans_fuite_d_url`).
async fn post_ws_ticket(
    http: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<String, AppError> {
    let resp = http
        .post(url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| map_relay_err(&e))?
        .error_for_status()
        .map_err(|e| map_relay_err(&e))?;
    let body: serde_json::Value = resp.json().await.map_err(|e| map_relay_err(&e))?;
    body["ticket"].as_str().map(str::to_string).ok_or_else(|| {
        AppError::InternalError("VNL-SBX-002: sandbox ticket response missing 'ticket'".into())
    })
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
    // Pannes de TRANSPORT opaques (`map_relay_err`) ; le passthrough statut+JSON de la
    // réponse sandbox en dessous reste TEL QUEL (corps émis par la sandbox, sans URL).
    let resp = req.body(body).send().await.map_err(|e| map_relay_err(&e))?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .map_err(|e| AppError::InternalError(e.to_string()))?;
    // Passthrough réel : un body non-JSON (ex. 404/405 texte brut renvoyé
    // par axum pour une route sandbox non matchée) ne doit pas faire
    // échouer le proxy — on le enveloppe plutôt que de perdre le vrai
    // statut/message derrière une erreur générique.
    let bytes = resp.bytes().await.map_err(|e| map_relay_err(&e))?;
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

    // 1. scoping owner identique à ws_ticket (factorisé dans scope_owned_sandbox)
    let (owner, client) = scope_owned_sandbox(&state, &user, &name).await?;

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

/// Scoping owner complet d'une sandbox (`./sandboxes.sdd` §Must : `AuthUser` →
/// `resolve_user` → `resolve_owner_name` → `get_sandbox` → `get_project` → match
/// d'owner), partagé par `git_proxy` et les relais `/code-health`. Ne demande
/// AUCUN jeton : l'appelant le fait APRÈS ce contrôle.
async fn scope_owned_sandbox(
    state: &AppState,
    user: &AuthUser,
    name: &str,
) -> Result<(String, VnlK8sClient), AppError> {
    let principal_user = resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    let Some(owner) = owners::resolve_owner_name(state, principal_user.id).await? else {
        return Err(AppError::Forbidden);
    };
    let client = k8s::client(state).await?;
    let sandbox = client.get_sandbox(&owner, name).await?;
    let project = client.get_project(&owner, &sandbox.spec.project).await?;
    if project.spec.owner != owner {
        return Err(AppError::Forbidden);
    }
    Ok((owner, client))
}

/// URL interne d'un sous-chemin `/code-health*` de la sandbox, dérivée de son
/// URL `/mcp` (`VnlK8sClient::sandbox_mcp_url`, même service et même port). Une
/// URL MCP d'une autre forme est une erreur interne, jamais une requête devinée.
fn code_health_url(mcp_url: &str, suffix: &str) -> Result<String, AppError> {
    let base = mcp_url
        .strip_suffix("/mcp")
        .ok_or_else(|| AppError::InternalError("URL MCP de sandbox inattendue".into()))?;
    Ok(format!("{base}/code-health{suffix}"))
}

/// Relais REST `GET /api/sandboxes/{name}/code-health` (`./sandboxes.sdd`
/// §Exposes `code_health`) : scoping owner, jeton du SA de l'Owner en Bearer,
/// statut + JSON de la sandbox transmis tels quels. Jamais l'`id_token`.
pub async fn code_health(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<axum::response::Response, AppError> {
    let (owner, client) = scope_owned_sandbox(&state, &user, &name).await?;
    let mcp_url = client.sandbox_mcp_url(&owner, &name).await?;
    let url = code_health_url(&mcp_url, "")?;
    let owner_token = client.owner_token(&owner).await?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(AppError::RequestError)?;
    let (status, value) = proxy_git_request(
        &http,
        &url,
        reqwest::Method::GET,
        Vec::new(),
        Bytes::new(),
        &owner_token.token,
    )
    .await?;
    Ok((status, Json(value)).into_response())
}

/// Corps transmis à la sandbox pour `POST /code-health/refresh` : `None` si la
/// requête n'a pas de corps (la sandbox traite l'absence comme « base et langages
/// inchangés », cf. `sandbox/src/health.rs` `handle_refresh`), sinon le corps reçu
/// tel quel (un JSON invalide est refusé par la sandbox, 400 transmis).
fn refresh_payload(raw: Bytes) -> Option<Bytes> {
    if raw.is_empty() { None } else { Some(raw) }
}

/// Relais REST `POST /api/sandboxes/{name}/code-health/refresh`
/// (`./sandboxes.sdd` §Exposes `code_health_refresh`) : corps JSON transmis tel
/// quel, 202/400/500 de la sandbox transmis tels quels.
pub async fn code_health_refresh(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    body: Bytes,
) -> Result<axum::response::Response, AppError> {
    let (owner, client) = scope_owned_sandbox(&state, &user, &name).await?;
    let mcp_url = client.sandbox_mcp_url(&owner, &name).await?;
    let url = code_health_url(&mcp_url, "/refresh")?;
    // Corps brut (pas d'extracteur `Json`, qui rendrait 415 sans Content-Type) :
    // sans corps, rien n'est transmis, pas même un content-type.
    let (headers, payload) = match refresh_payload(body) {
        Some(raw) => (
            vec![("content-type".to_string(), "application/json".to_string())],
            raw,
        ),
        None => (Vec::new(), Bytes::new()),
    };
    let owner_token = client.owner_token(&owner).await?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(AppError::RequestError)?;
    let (status, value) = proxy_git_request(
        &http,
        &url,
        reqwest::Method::POST,
        headers,
        payload,
        &owner_token.token,
    )
    .await?;
    Ok((status, Json(value)).into_response())
}

#[cfg(test)]
mod tests {
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
            restate_admin_url: None,
            restate_ingress_url: None,
            workflow_listen: "0.0.0.0:9080".to_string(),
            workflow_deployment_url: None,
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
            workflow: None,
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
            workflow: None,
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
            workflow: None,
        };

        Router::new()
            .route("/sandboxes", get(list_sandboxes).post(create_sandbox))
            .route("/sandboxes/{name}", get(get_sandbox).delete(delete_sandbox))
            .route("/sandboxes/{name}/ws-ticket", post(ws_ticket))
            .route("/sandboxes/{name}/suspend", post(set_sandbox_suspended))
            .route("/sandboxes/{name}/git/{*path}", any(git_proxy))
            .route("/sandboxes/{name}/code-health", get(code_health))
            .route(
                "/sandboxes/{name}/code-health/refresh",
                post(code_health_refresh),
            )
            .with_state(state)
    }

    // ── Scenario: code_health_sans_session_retourne_401 ──
    #[tokio::test]
    async fn code_health_sans_session_retourne_401() {
        let app = make_app(test_key());
        let get_req = Request::builder()
            .uri("/sandboxes/s1/code-health")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(get_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "GET sans session");
        let post_req = Request::builder()
            .uri("/sandboxes/s1/code-health/refresh")
            .method("POST")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(post_req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "POST refresh sans session"
        );
    }

    // ── Scenario: code_health_non_owner_n_appelle_jamais_la_sandbox ──
    // Base réelle sans lien Owner : refus 403 AVANT tout client K8s et AVANT
    // toute demande de jeton ; les deux routes sont couvertes.
    #[tokio::test]
    async fn code_health_non_owner_n_appelle_jamais_la_sandbox() {
        let db = crate::db::test_support::real_db().await;
        let state = test_state_with_db(db);
        let Err(err) = code_health(State(state.clone()), test_user(), Path("s1".to_string())).await
        else {
            panic!("non-owner doit être refusé (GET)");
        };
        assert!(
            matches!(err, AppError::Forbidden),
            "GET : attendu Forbidden, obtenu {err:?}"
        );
        let Err(err) = code_health_refresh(
            State(state.clone()),
            test_user(),
            Path("s1".to_string()),
            Bytes::from_static(br#"{"base":"main"}"#),
        )
        .await
        else {
            panic!("non-owner doit être refusé (POST refresh)");
        };
        assert!(
            matches!(err, AppError::Forbidden),
            "POST : attendu Forbidden, obtenu {err:?}"
        );
        assert!(
            state.k8s.lock().await.is_none(),
            "aucun client K8s (donc aucun jeton ni appel sandbox) pour un non-owner"
        );
    }

    // ── Scenario: code_health_relaye_get_avec_jeton_owner_et_passthrough
    // (partie hors cluster : URL dérivée, Bearer du jeton d'Owner, statut 200 et
    // JSON inchangés ; l'obtention du jeton par apiserver reste cluster-manuel) ──
    #[tokio::test]
    async fn code_health_relaye_get_avec_jeton_owner_et_passthrough() {
        let (base, cap) = spawn_sandbox_capture(
            200,
            "application/json",
            r#"{"phase":"fresh","report":null}"#,
        )
        .await;
        let url = code_health_url(&format!("{base}/mcp"), "").unwrap();
        let (status, value) = proxy_git_request(
            &reqwest::Client::new(),
            &url,
            reqwest::Method::GET,
            Vec::new(),
            Bytes::new(),
            "owner-sa-jeton-alice",
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::OK, "statut 200 transmis");
        assert_eq!(value, serde_json::json!({"phase": "fresh", "report": null}));
        let seen = cap.lock().unwrap().remove(0);
        assert_eq!(seen.method, "GET");
        assert_eq!(seen.path_and_query, "/code-health");
        // Égalité EXACTE du header reçu par la sandbox : `Bearer <jeton du SA>`, rien de plus
        // (ni préfixe, ni suffixe, ni autre jeton). Le relais ne reçoit que ce jeton en
        // paramètre : l'exclusion de l'id_token est structurelle à ce niveau.
        assert_eq!(
            seen.authorization.as_deref(),
            Some("Bearer owner-sa-jeton-alice"),
            "le header Authorization reçu est exactement `Bearer <jeton du SA de l'Owner>`"
        );
    }

    // ── Scenario: code_health_refresh_relaye_post_avec_corps_et_statut_202 ──
    #[tokio::test]
    async fn code_health_refresh_relaye_post_avec_corps_et_statut_202() {
        let (base, cap) =
            spawn_sandbox_capture(202, "application/json", r#"{"accepted":true}"#).await;
        let url = code_health_url(&format!("{base}/mcp"), "/refresh").unwrap();
        let (status, value) = proxy_git_request(
            &reqwest::Client::new(),
            &url,
            reqwest::Method::POST,
            vec![("content-type".to_string(), "application/json".to_string())],
            Bytes::from_static(br#"{"base":"main"}"#),
            "owner-sa-jeton-alice",
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::ACCEPTED, "202 transmis");
        assert_eq!(value, serde_json::json!({"accepted": true}));
        let seen = cap.lock().unwrap().remove(0);
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path_and_query, "/code-health/refresh");
        assert_eq!(seen.body, r#"{"base":"main"}"#, "corps transmis tel quel");
        assert_eq!(seen.content_type.as_deref(), Some("application/json"));
        assert_eq!(
            seen.authorization.as_deref(),
            Some("Bearer owner-sa-jeton-alice")
        );
    }

    // ── Scenario: code_health_refresh_sans_corps_est_accepte ──
    // Sans corps ⇒ AUCUN corps transmis à la sandbox (elle traite l'absence comme
    // « base et langages inchangés », cf. sandbox/src/health.rs `handle_refresh`) ;
    // avec corps JSON ⇒ transmis tel quel. Le fil est vérifié : pas de corps, pas de
    // content-type, statut 202 rendu tel quel.
    #[tokio::test]
    async fn code_health_refresh_sans_corps_est_accepte() {
        assert_eq!(
            refresh_payload(Bytes::new()),
            None,
            "corps vide ⇒ rien à transmettre"
        );
        let raw = Bytes::from_static(br#"{"base":"main"}"#);
        assert_eq!(
            refresh_payload(raw.clone()),
            Some(raw),
            "corps JSON transmis tel quel"
        );

        let (base, cap) =
            spawn_sandbox_capture(202, "application/json", r#"{"accepted":true}"#).await;
        let url = code_health_url(&format!("{base}/mcp"), "/refresh").unwrap();
        let (status, _value) = proxy_git_request(
            &reqwest::Client::new(),
            &url,
            reqwest::Method::POST,
            Vec::new(),
            Bytes::new(),
            "owner-sa-jeton-alice",
        )
        .await
        .unwrap();
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "202 rendu tel quel sans corps"
        );
        let seen = cap.lock().unwrap().remove(0);
        assert_eq!(seen.body, "", "aucun corps transmis");
        assert_eq!(seen.content_type, None, "aucun content-type sans corps");
    }

    // ── Scenario: code_health_statut_derreur_de_la_sandbox_transmis ──
    #[tokio::test]
    async fn code_health_statut_derreur_de_la_sandbox_transmis() {
        let (base, _cap) = spawn_sandbox_capture(
            400,
            "application/json",
            r#"{"error":"VNL-SBX-CH-001: invalid ref"}"#,
        )
        .await;
        let url = code_health_url(&format!("{base}/mcp"), "/refresh").unwrap();
        let (status, value) = proxy_git_request(
            &reqwest::Client::new(),
            &url,
            reqwest::Method::POST,
            Vec::new(),
            Bytes::from_static(br#"{"base":"x"}"#),
            "owner-sa-jeton",
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "400 d'origine conservé");
        assert_eq!(
            value,
            serde_json::json!({"error": "VNL-SBX-CH-001: invalid ref"}),
            "JSON d'origine, pas d'error_for_status"
        );
    }

    // ── Scenario: code_health_panne_de_relais_sans_fuite_d_url
    // (partie hors cluster : panne de transport réelle sur le relais code-health
    // → 502 opaque, sans l'URL interne ; la sandbox injoignable en cluster reste
    // cluster-manuel) ──
    #[tokio::test]
    async fn code_health_panne_de_relais_sans_fuite_d_url() {
        use axum::response::IntoResponse;
        let url = code_health_url("http://127.0.0.1:1/mcp", "").unwrap();
        let Err(err) = proxy_git_request(
            &reqwest::Client::new(),
            &url,
            reqwest::Method::GET,
            Vec::new(),
            Bytes::new(),
            "owner-sa-jeton",
        )
        .await
        else {
            panic!("adresse fermée ⇒ panne de transport attendue");
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("corps lisible");
        assert_eq!(
            body.as_ref(),
            br#"{"error":"VNL-SBX-017: sandbox unavailable"}"#.as_slice()
        );
    }

    // ── code_health_url : dérivation de l'URL interne code-health depuis l'URL /mcp ──
    #[test]
    fn code_health_url_derive_depuis_l_url_mcp() {
        assert_eq!(
            code_health_url("http://sandbox-s1.own.svc:3000/mcp", "").unwrap(),
            "http://sandbox-s1.own.svc:3000/code-health"
        );
        assert_eq!(
            code_health_url("http://sandbox-s1.own.svc:3000/mcp", "/refresh").unwrap(),
            "http://sandbox-s1.own.svc:3000/code-health/refresh"
        );
        assert!(code_health_url("http://sandbox-s1.own.svc:3000/autre", "").is_err());
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

    // ── Scenario: panne_de_relais_sans_fuite_d_url ──
    // Une vraie `reqwest::Error` de connexion (adresse fermée) convertie par le helper
    // privé rend 502 avec le corps exact `VNL-SBX-017` — ni l'URL interne (le `Display`
    // reqwest la porte), ni `svc`, ni le message d'origine (revue 2026-10-14).
    #[tokio::test]
    async fn panne_de_relais_sans_fuite_d_url() {
        use axum::response::IntoResponse;

        let err = reqwest::Client::new()
            .get("http://127.0.0.1:1/mcp")
            .send()
            .await
            .expect_err("adresse fermée ⇒ panne de transport réelle");
        assert!(
            err.url()
                .is_some_and(|u| u.to_string().contains("127.0.0.1")),
            "la panne d'origine porte bien l'URL interne (risque verrouillé)"
        );

        let resp = map_relay_err(&err).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("corps lisible");
        assert_eq!(
            body.as_ref(),
            br#"{"error":"VNL-SBX-017: sandbox unavailable"}"#.as_slice(),
            "corps exact, sans fuite"
        );
        let rendu = String::from_utf8_lossy(&body);
        assert!(
            !rendu.contains("127.0.0.1") && !rendu.contains("svc"),
            "ni l'adresse fermée ni le suffixe du service interne ne sortent : {rendu}"
        );
    }

    /// Serveur TCP brut : lit la requête, répond des en-têtes annonçant `Content-Length: 100`,
    /// envoie un morceau de corps puis COUPE la connexion (panne pendant la lecture du corps).
    async fn spawn_coupure_en_cours_de_corps() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let mut recu = Vec::new();
                while !recu.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => recu.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"par",
                    )
                    .await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    // ── Scenario: panne_de_lecture_du_corps_sans_fuite_d_url ──
    // Coupure de la sandbox APRÈS les en-têtes, pendant la lecture du corps : le relais rend
    // 502 `VNL-SBX-017` exact, sans l'URL interne ni le message reqwest d'origine.
    #[tokio::test]
    async fn panne_de_lecture_du_corps_sans_fuite_d_url() {
        use axum::response::IntoResponse;

        let base = spawn_coupure_en_cours_de_corps().await;
        let err = proxy_git_request(
            &reqwest::Client::new(),
            &format!("{base}/code-health"),
            reqwest::Method::GET,
            Vec::new(),
            Bytes::new(),
            "owner-sa-jeton-alice",
        )
        .await
        .expect_err("coupure pendant le corps ⇒ erreur de relais");
        assert!(
            matches!(err, AppError::SandboxRelayUnavailable),
            "panne de lecture ⇒ erreur opaque, obtenu {err:?}"
        );

        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("corps lisible");
        assert_eq!(
            body.as_ref(),
            br#"{"error":"VNL-SBX-017: sandbox unavailable"}"#.as_slice(),
            "corps exact, sans fuite"
        );
    }

    // ── Même défaut sur le relais de ticket WS (`post_ws_ticket`) ──
    #[tokio::test]
    async fn panne_de_lecture_du_ticket_sans_fuite_d_url() {
        use axum::response::IntoResponse;

        let base = spawn_coupure_en_cours_de_corps().await;
        let err = post_ws_ticket(
            &reqwest::Client::new(),
            &format!("{base}/ws/ticket"),
            "jeton",
        )
        .await
        .expect_err("coupure pendant le corps ⇒ erreur de relais");
        assert!(
            matches!(err, AppError::SandboxRelayUnavailable),
            "panne de lecture ⇒ erreur opaque, obtenu {err:?}"
        );

        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("corps lisible");
        let rendu = String::from_utf8_lossy(&body);
        assert!(
            !rendu.contains("127.0.0.1") && !rendu.contains("ws/ticket"),
            "ni l'URL interne ni le message d'origine ne sortent : {rendu}"
        );
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
