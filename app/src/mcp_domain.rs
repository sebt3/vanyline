//! Serveur MCP de domaine `POST /mcp/domain` de l'app — contrat : `./mcp_domain.sdd`.
//! JSON-RPC 2.0 « HTTP streamable » POST-only, même forme d'enveloppe que le MCP de la
//! sandbox (`../../sandbox/src/mcp.rs`) : `initialize`, `tools/list`, `tools/call`,
//! notification (sans `id`) ⇒ 202 nu. Authentification par l'extracteur dual
//! [`AuthPrincipal`] de miryad-core (cookie OU jeton d'API, 401 sinon). Chaque outil agit
//! au nom de l'appelant, dans le namespace de son Owner (résolu depuis l'identité, jamais
//! fourni par le client), par les MÊMES fonctions que les routes REST. Sorties bornées à
//! 8 KiB avec marqueur de troncature ; erreur d'outil ⇒ `isError: true` + message
//! `@Display` de l'`@AppError`, jamais de code JSON-RPC (convention sandbox).
//!
//! Tranches signalées (rapport M1 2026-10-11) :
//! - `Exposes` omet le nom de la sandbox à `create_sandbox` alors que la route REST
//!   l'exige (`CreateSandboxBody.name`) : l'outil prend `name` en argument requis.
//! - Les outils `*_workflow` ciblent l'`id` numérique (la route REST est indexée par id) ;
//!   l'argument `workflow` accepte un entier ou sa chaîne.
//! - L'`@AuthUser` passé aux handlers REST est synthétisé depuis l'`@AuthPrincipal`
//!   (`id_token` vide) : les handlers de domaine ne lisent que `subject`/`email`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use miryad_core::auth::{AuthPrincipal, AuthUser};

use vanyline_health::report::{CheckStatus, HealthReport, Issue, Severity};

use crate::workflow::run_context::RunContext;
use crate::workflow::steps::{K8sLocator, RunInfo, SandboxAccess, SandboxLocator};
use crate::{AppState, api, error::AppError};

// ── JSON-RPC 2.0 — motif reproduit de ../../sandbox/src/mcp.rs:16-77 ─────────

/// Requête JSON-RPC 2.0 entrante (params optionnels, `id` absent = notification).
#[derive(Deserialize)]
pub(crate) struct JsonRpcRequest {
    /// Version du protocole, vérifiée par le client (jamais relue ici).
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Identifiant de la requête ; `None` = notification (réponse HTTP 202 nue).
    pub id: Option<Value>,
    /// Méthode appelée (`initialize`, `tools/list`, `tools/call`, …).
    pub method: String,
    /// Paramètres de la méthode (absents = `Value::Null` par défaut serde).
    #[serde(default)]
    pub params: Value,
}

/// Réponse JSON-RPC 2.0 : `result` OU `error` (champs `None` exclus de la sérialisation).
#[derive(Serialize)]
pub(crate) struct JsonRpcResponse {
    /// Toujours `"2.0"`.
    pub jsonrpc: &'static str,
    /// Identifiant écho de la requête.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Résultat de la méthode, absent en cas d'erreur.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Objet d'erreur JSON-RPC, absent en cas de succès.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// Objet `error` d'une réponse JSON-RPC 2.0.
#[derive(Serialize)]
pub(crate) struct RpcError {
    /// Code d'erreur JSON-RPC (`-32601` méthode inconnue, `-32602` params invalides).
    pub code: i32,
    /// Message lisible de l'erreur.
    pub message: String,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// ── Enveloppes d'outil MCP (motif ok_result/err_result de ../../sandbox/src/tools_impl.rs) ──

/// Résultat MCP `isError: false` — texte borné.
fn ok_result(text: &str) -> Value {
    serde_json::json!({ "content": [{"type": "text", "text": text}], "isError": false })
}

/// Résultat MCP `isError: true` — échec d'exécution d'outil, JAMAIS un code JSON-RPC
/// (`mcp_domain.sdd` `Handles`).
fn err_result(text: &str) -> Value {
    serde_json::json!({ "content": [{"type": "text", "text": text}], "isError": true })
}

// ── Bornage 8 KiB ────────────────────────────────────────────────────────────

/// Plafond de toute sortie d'outil (`mcp_domain.sdd` `Handles` : « au plus 8 KiB »).
const OUTPUT_MAX_BYTES: usize = 8 * 1024;

/// Marqueur de troncature ajouté quand le texte dépasse le plafond.
const TRUNCATION_MARKER: &str = "\n… [sortie tronquée — plafond 8 KiB, suite non rendue]";

/// Bornage : au plus [`OUTPUT_MAX_BYTES`] OCTETS TOUT COMPRIS (marque incluse), coupure
/// sur une frontière de ligne si elle tombe après la moitié du corps (listes rendues une
/// ligne par élément, JSON sur frontière d'objet), sinon repli sur la dernière frontière
/// caractérielle — jamais une coupure UTF-8.
fn bounded(text: String) -> String {
    if text.len() <= OUTPUT_MAX_BYTES {
        return text;
    }
    let mut max_body = OUTPUT_MAX_BYTES.saturating_sub(TRUNCATION_MARKER.len());
    while !text.is_char_boundary(max_body) {
        max_body = max_body.saturating_sub(1);
    }
    let mut cut = match text[..max_body].rfind('\n') {
        Some(nl) if nl > max_body / 2 => nl,
        _ => max_body,
    };
    while !text.is_char_boundary(cut) {
        cut = cut.saturating_sub(1);
    }
    format!("{}{TRUNCATION_MARKER}", &text[..cut])
}

// ── Route et dispatch JSON-RPC ───────────────────────────────────────────────

/// Monte `POST /mcp/domain` à la table du router racine (`mcp_domain.sdd` `Exposes`).
pub(crate) fn mcp_domain_router() -> axum::Router<AppState> {
    axum::Router::new().route("/mcp/domain", axum::routing::post(handle))
}

/// Handler `POST /mcp/domain` : notification (pas d'`id`) ⇒ HTTP 202 sans corps ; sinon
/// dispatch de la méthode et réponse JSON-RPC. L'authentification est portée par
/// l'extracteur [`AuthPrincipal`] (401 avant toute exécution).
pub(crate) async fn handle(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    if req.id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let response = match req.method.as_str() {
        "initialize" => handle_initialize(req.id),
        "notifications/initialized" => {
            // Ne devrait pas arriver (pas d'`id`), accepté avec grâce — comme la sandbox.
            return StatusCode::ACCEPTED.into_response();
        }
        "tools/list" => handle_tools_list(req.id),
        "tools/call" => handle_tools_call(req.id, req.params, &state, &principal, &headers).await,
        _ => JsonRpcResponse::err(req.id, -32601, format!("Method not found: {}", req.method)),
    };
    Json(response).into_response()
}

fn handle_initialize(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": env!("CARGO_PKG_NAME"),
                "version": env!("CARGO_PKG_VERSION"),
            }
        }),
    )
}

fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(id, serde_json::json!({ "tools": tools_spec() }))
}

async fn handle_tools_call(
    id: Option<Value>,
    params: Value,
    state: &AppState,
    principal: &AuthPrincipal,
    headers: &HeaderMap,
) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::err(id, -32602, "Missing tool name in params");
    };
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => Value::Object(serde_json::Map::new()),
        Some(v) => v.clone(),
    };
    // Marqueur de l'agent système posé par l'app dans la configuration du MCP forcé
    // (`mcp_domain.sdd` `Handles`) ; sinon déclenchement « api ».
    let agent = headers
        .get("x-vnl-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    match dispatch_tool(state, principal, agent.as_deref(), name, arguments).await {
        Some(envelope) => JsonRpcResponse::ok(id, envelope),
        None => JsonRpcResponse::err(id, -32602, format!("Unknown tool: {name}")),
    }
}

// ── Identité ─────────────────────────────────────────────────────────────────

/// Identité de l'appelant : (clé primaire miryad, Owner K8s résolu depuis l'identité —
/// jamais fourni par le client, `mcp_domain.sdd` `Must not`).
pub(crate) async fn caller_owner(
    state: &AppState,
    principal: &AuthPrincipal,
) -> Result<(i32, Option<String>), AppError> {
    let user = miryad_core::users::resolve_user(
        &state.auth.db,
        &principal.subject,
        principal.email.as_deref(),
    )
    .await
    .map_err(AppError::from)?;
    Ok((
        user.id,
        api::owners::resolve_owner_name(state, user.id).await?,
    ))
}

/// `@AuthUser` de passage pour les handlers REST qui le prennent en extracteur : les
/// handlers de domaine ne lisent que `subject`/`email` (usé par `resolve_user`), le champ
/// `id_token` n'est lu par aucun deux — tranché, cf. doc d'en-tête.
fn auth_user(principal: &AuthPrincipal) -> AuthUser {
    AuthUser {
        subject: principal.subject.clone(),
        email: principal.email.clone(),
        id_token: String::new(),
    }
}

// ── Rendus ───────────────────────────────────────────────────────────────────

/// JSON compact (les listes sont rendues une ligne par élément, bord à bord avec la
/// coupure sur frontière de ligne de [`bounded`]).
fn render_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| format!("(rendu JSON impossible: {e})"))
}

/// Liste : en-tête compté + une ligne JSON par élément.
fn render_items<T: Serialize>(label: &str, items: &[T]) -> String {
    let mut lines = vec![format!("{label} ({}):", items.len())];
    lines.extend(items.iter().map(render_json));
    lines.join("\n")
}

fn tool_ok_json<T: Serialize>(value: &T) -> Value {
    ok_result(&bounded(render_json(value)))
}

fn tool_ok_items<T: Serialize>(label: &str, items: &[T]) -> Value {
    ok_result(&bounded(render_items(label, items)))
}

/// Erreur d'outil : message `@Display` de l'`@AppError` (identifiant `VNL-*` inclus,
/// jamais de secret — les `Display` de `error.rs` n'en portent aucun).
fn tool_err_app(err: &AppError) -> Value {
    err_result(&err.to_string())
}

// ── Extraction des arguments (schéma d'appel : isError nommant l'argument) ──

/// Argument chaîne requis.
fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    match args.get(key) {
        None | Some(Value::Null) => Err(format!("argument manquant : « {key} » (chaîne attendue)")),
        Some(Value::String(s)) => Ok(s),
        Some(other) => Err(format!(
            "argument « {key} » : chaîne attendue, obtenu {other}"
        )),
    }
}

/// Argument chaîne optionnel (`None`/absent ⇒ `None`).
fn arg_opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(other) => Err(format!(
            "argument « {key} » : chaîne attendue, obtenu {other}"
        )),
    }
}

/// Argument entier (cible `i32`) : nombre JSON ou sa chaîne ; c'est l'`id` numérique des
/// routes REST — tranché, cf. doc d'en-tête.
fn arg_i32(args: &Value, key: &str) -> Result<i32, String> {
    let raw = match args.get(key) {
        None | Some(Value::Null) => {
            return Err(format!("argument manquant : « {key} » (entier attendu)"));
        }
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => {
            return Err(format!(
                "argument « {key} » : entier attendu, obtenu {other}"
            ));
        }
    };
    raw.parse::<i32>()
        .map_err(|_| format!("argument « {key} » : entier attendu, obtenu {raw:?}"))
}

/// Argument entier optionnel borné (`limit`).
fn arg_opt_usize(args: &Value, key: &str) -> Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|v| Some(usize::try_from(v).unwrap_or(usize::MAX)))
            .ok_or_else(|| format!("argument « {key} » : entier attendu, obtenu {n}")),
        Some(other) => Err(format!(
            "argument « {key} » : entier attendu, obtenu {other}"
        )),
    }
}

/// Enveloppe d'erreur de schéma d'arguments : `isError: true`, message nommant
/// l'argument (jamais un code JSON-RPC — `mcp_domain.sdd` `Handles`).
fn arg_err(tool: &str, msg: &str) -> Value {
    err_result(&format!(
        "arguments invalides pour l'outil « {tool} » : {msg}"
    ))
}

// ── Déclaration tools/list — les 21 outils de l'Exposes ──────────────────────

/// Les 21 outils de l'`Exposes` avec leurs schémas ; mention « destructif » portée par
/// les trois destructifs promis (`delete_project`/`delete_sandbox`/`delete_workflow`).
fn tools_spec() -> Vec<Value> {
    let mut tools = projets_tools();
    tools.extend(sandboxes_tools());
    tools.extend(sante_tools());
    tools.extend(workflows_tools());
    tools.extend(runs_tools());
    tools
}

/// Schéma de l'`id` numérique d'un workflow (la route REST est indexée par id — tranché,
/// cf. doc d'en-tête).
fn workflow_id_property() -> Value {
    serde_json::json!({ "type": "integer", "description": "id numérique du workflow (index REST)" })
}

fn projets_tools() -> Vec<Value> {
    vec![
        serde_json::json!({
            "name": "list_projects",
            "description": "Liste les projets de l'Owner de l'appelant (namespace de l'Owner).",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "get_project",
            "description": "Lit un projet de l'Owner de l'appelant.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "nom du projet" } },
                "required": ["name"]
            }
        }),
        serde_json::json!({
            "name": "create_project",
            "description": "Crée le projet (CR Project) dans le namespace de l'Owner de l'appelant.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "nom du projet" },
                    "repo_url": { "type": "string", "description": "URL du dépôt git" },
                    "default_branch": { "type": "string", "description": "branche par défaut (optionnel)" }
                },
                "required": ["name", "repo_url"]
            }
        }),
        serde_json::json!({
            "name": "delete_project",
            "description": "Supprime le projet (destructif : CR, workspaces et sandboxes liés).",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "nom du projet" } },
                "required": ["name"]
            }
        }),
    ]
}

fn sandboxes_tools() -> Vec<Value> {
    vec![
        serde_json::json!({
            "name": "list_sandboxes",
            "description": "Liste les sandboxes de l'Owner de l'appelant, filtrables par projet.",
            "inputSchema": {
                "type": "object",
                "properties": { "project": { "type": "string", "description": "filtre par projet (optionnel)" } }
            }
        }),
        serde_json::json!({
            "name": "get_sandbox",
            "description": "Lit une sandbox de l'Owner de l'appelant.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "nom de la sandbox" } },
                "required": ["name"]
            }
        }),
        serde_json::json!({
            "name": "create_sandbox",
            "description": "Crée une sandbox (CR Sandbox) sur un projet de l'Owner de l'appelant.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "nom de la CR Sandbox" },
                    "project": { "type": "string", "description": "projet monté" },
                    "branch": { "type": "string", "description": "branche du worktree" },
                    "toolchains": { "type": "array", "description": "outils toolchain (optionnel, même forme que la route REST)" }
                },
                "required": ["name", "project", "branch"]
            }
        }),
        serde_json::json!({
            "name": "delete_sandbox",
            "description": "Supprime la sandbox (destructif : pod, worktree et service supprimés).",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "nom de la sandbox" } },
                "required": ["name"]
            }
        }),
        serde_json::json!({
            "name": "suspend_sandbox",
            "description": "Suspend la sandbox (le pod est supprimé, l'identité et les volumes conservés).",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "nom de la sandbox" } },
                "required": ["name"]
            }
        }),
        serde_json::json!({
            "name": "resume_sandbox",
            "description": "Relance la sandbox suspendue.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "description": "nom de la sandbox" } },
                "required": ["name"]
            }
        }),
    ]
}

fn sante_tools() -> Vec<Value> {
    vec![
        serde_json::json!({
            "name": "code_health",
            "description": "Relis l'état de santé du code d'une sandbox (GET /code-health, lecture seule, jamais de calcul). Par défaut detail « summary » : phase, statut global, une ligne par vérification, totaux de couverture, langages non vérifiés, les 20 premières issues. Sortie texte bornée 8 KiB.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sandbox": { "type": "string", "description": "nom de la sandbox" },
                    "detail": { "type": "string", "enum": ["summary", "issues", "coverage"], "description": "niveau de détail (défaut summary)" },
                    "path": { "type": "string", "description": "préfixe de chemin (issues/coverage)" },
                    "limit": { "type": "integer", "description": "nombre maximum d'entrées rendues (défaut et plafond 200)" }
                },
                "required": ["sandbox"]
            }
        }),
        serde_json::json!({
            "name": "refresh_code_health",
            "description": "Demande le recalcul immédiat de la santé du code d'une sandbox (POST /code-health/refresh). base, si fourni, devient la base sticky.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sandbox": { "type": "string", "description": "nom de la sandbox" },
                    "base": { "type": "string", "description": "référence git de base (optionnel)" }
                },
                "required": ["sandbox"]
            }
        }),
    ]
}

fn workflows_tools() -> Vec<Value> {
    let id_schema = workflow_id_property();
    vec![
        serde_json::json!({
            "name": "list_workflows",
            "description": "Liste les workflows d'un projet (id, nom nu, portée, kinds, nombre d'étapes).",
            "inputSchema": {
                "type": "object",
                "properties": { "project": { "type": "string", "description": "nom du projet" } },
                "required": ["project"]
            }
        }),
        serde_json::json!({
            "name": "get_workflow",
            "description": "Détail d'un workflow du projet, étapes comprises (404 identique inexistant / à un autre).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "nom du projet" },
                    "workflow": id_schema.clone()
                },
                "required": ["project", "workflow"]
            }
        }),
        serde_json::json!({
            "name": "create_workflow",
            "description": "Crée un workflow (définition + rattachement, transaction) sur un projet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "nom du projet" },
                    "name": { "type": "string", "description": "nom du workflow" },
                    "scope": { "type": "string", "enum": ["project", "sandbox"], "description": "portée" },
                    "steps": { "type": "array", "description": "étapes du DAG (même forme que la route REST)" },
                    "description": { "type": "string", "description": "description (optionnelle)" }
                },
                "required": ["project", "name", "scope", "steps"]
            }
        }),
        serde_json::json!({
            "name": "update_workflow",
            "description": "Met à jour la description et/ou les étapes d'un workflow (la portée n'est pas modifiable).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "nom du projet" },
                    "workflow": id_schema.clone(),
                    "steps": { "type": "array", "description": "nouvelles étapes (optionnel)" },
                    "description": { "type": "string", "description": "nouvelle description (optionnelle)" }
                },
                "required": ["project", "workflow"]
            }
        }),
        serde_json::json!({
            "name": "delete_workflow",
            "description": "Supprime le workflow et sa définition (destructif : les runs indexés survivent).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "nom du projet" },
                    "workflow": id_schema.clone()
                },
                "required": ["project", "workflow"]
            }
        }),
        serde_json::json!({
            "name": "launch_workflow",
            "description": "Lance un run du workflow (202 ; triggered_by = agent:<X-Vnl-Agent> ou api).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "nom du projet" },
                    "workflow": id_schema,
                    "sandbox": { "type": "string", "description": "sandbox (exigée en portée sandbox)" }
                },
                "required": ["project", "workflow"]
            }
        }),
    ]
}

fn runs_tools() -> Vec<Value> {
    vec![
        serde_json::json!({
            "name": "list_runs",
            "description": "Runs récents de l'appelant (fenêtre 14 jours, limit ≤ 50, plus récent d'abord), filtrables par projet et/ou sandbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "filtre par projet (optionnel)" },
                    "sandbox": { "type": "string", "description": "filtre par sandbox (optionnel)" },
                    "limit": { "type": "integer", "description": "au plus 50 (défaut 50)" }
                }
            }
        }),
        serde_json::json!({
            "name": "get_run",
            "description": "Détail d'un run avec le statut de ses étapes (404 identique inexistant / à un autre).",
            "inputSchema": {
                "type": "object",
                "properties": { "run_id": { "type": "string", "description": "run_id du lancement" } },
                "required": ["run_id"]
            }
        }),
        serde_json::json!({
            "name": "cancel_run",
            "description": "Annule un run (invocation Restate + jobs shell best-effort, 202).",
            "inputSchema": {
                "type": "object",
                "properties": { "run_id": { "type": "string", "description": "run_id du lancement" } },
                "required": ["run_id"]
            }
        }),
    ]
}

// ── Dispatch des outils — MÊMES fonctions que les routes REST ────────────────

/// Dispatch d'un outil ; `None` si le nom est étranger au domaine (⇒ erreur d'enveloppe
/// `-32602` au niveau JSON-RPC, convention de la sandbox, jamais une exécution d'outil).
async fn dispatch_tool(
    state: &AppState,
    principal: &AuthPrincipal,
    agent: Option<&str>,
    name: &str,
    args: Value,
) -> Option<Value> {
    let out = match name {
        // ── projets ─
        "list_projects" => t_list_projects(state, principal).await,
        "get_project" => t_get_project(state, principal, &args).await,
        "create_project" => t_create_project(state, principal, &args).await,
        "delete_project" => t_delete_project(state, principal, &args).await,
        // ── sandboxes ─
        "list_sandboxes" => t_list_sandboxes(state, principal, &args).await,
        "get_sandbox" => t_get_sandbox(state, principal, &args).await,
        "create_sandbox" => t_create_sandbox(state, principal, &args).await,
        "delete_sandbox" => t_delete_sandbox(state, principal, &args).await,
        "suspend_sandbox" => t_set_sandbox_suspended(state, principal, &args, true).await,
        "resume_sandbox" => t_set_sandbox_suspended(state, principal, &args, false).await,
        // ── santé du code (relais interne, Task `[!]` de mcp_domain.sdd) ─
        "code_health" => t_code_health(state, principal, &args).await,
        "refresh_code_health" => t_refresh_code_health(state, principal, &args).await,
        // ── workflows ─
        "list_workflows" => t_list_workflows(state, principal, &args).await,
        "get_workflow" => t_get_workflow(state, principal, &args).await,
        "create_workflow" => t_create_workflow(state, principal, &args).await,
        "update_workflow" => t_update_workflow(state, principal, &args).await,
        "delete_workflow" => t_delete_workflow(state, principal, &args).await,
        "launch_workflow" => t_launch_workflow(state, principal, agent, &args).await,
        "list_runs" => t_list_runs(state, principal, &args).await,
        "get_run" => t_get_run(state, principal, &args).await,
        "cancel_run" => t_cancel_run(state, principal, &args).await,
        _ => return None,
    };
    Some(out)
}

// ── Outils projets ───────────────────────────────────────────────────────────

async fn t_list_projects(state: &AppState, principal: &AuthPrincipal) -> Value {
    match api::projects::list_projects(State(state.clone()), auth_user(principal)).await {
        Ok(Json(items)) => tool_ok_items("projets", &items),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_get_project(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let name = match arg_str(args, "name") {
        Ok(v) => v,
        Err(msg) => return arg_err("get_project", &msg),
    };
    match api::projects::get_project(
        State(state.clone()),
        auth_user(principal),
        Path(name.to_string()),
    )
    .await
    {
        Ok(Json(project)) => tool_ok_json(&project),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_create_project(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let (name, repo_url) = match (arg_str(args, "name"), arg_str(args, "repo_url")) {
        (Ok(n), Ok(r)) => (n, r),
        (Err(msg), _) | (_, Err(msg)) => return arg_err("create_project", &msg),
    };
    let default_branch = match arg_opt_str(args, "default_branch") {
        Ok(v) => v,
        Err(msg) => return arg_err("create_project", &msg),
    };
    let body = api::projects::CreateProjectBody {
        name: name.to_string(),
        repo_url: repo_url.to_string(),
        default_branch: default_branch.map(str::to_string),
        storage_size: None,
        storage_class: None,
        storage_access_mode: None,
        git_secret: None,
        caches: None,
        fetch_interval: None,
        egress: Vec::new(),
    };
    match api::projects::create_project(State(state.clone()), principal.clone(), Json(body)).await {
        Ok((_created, Json(project))) => tool_ok_json(&project),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_delete_project(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let name = match arg_str(args, "name") {
        Ok(v) => v,
        Err(msg) => return arg_err("delete_project", &msg),
    };
    match api::projects::delete_project(
        State(state.clone()),
        auth_user(principal),
        Path(name.to_string()),
    )
    .await
    {
        Ok(_no_content) => ok_result(&format!("projet « {name} » supprimé")),
        Err(e) => tool_err_app(&e),
    }
}

// ── Outils sandboxes ─────────────────────────────────────────────────────────

async fn t_list_sandboxes(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let project = match arg_opt_str(args, "project") {
        Ok(v) => v,
        Err(msg) => return arg_err("list_sandboxes", &msg),
    };
    match api::sandboxes::list_sandboxes(State(state.clone()), auth_user(principal)).await {
        Ok(Json(items)) => match project {
            Some(p) => {
                let filtered: Vec<_> = items.into_iter().filter(|s| s.spec.project == p).collect();
                tool_ok_items("sandboxes", &filtered)
            }
            None => tool_ok_items("sandboxes", &items),
        },
        Err(e) => tool_err_app(&e),
    }
}

async fn t_get_sandbox(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let name = match arg_str(args, "name") {
        Ok(v) => v,
        Err(msg) => return arg_err("get_sandbox", &msg),
    };
    match api::sandboxes::get_sandbox(
        State(state.clone()),
        auth_user(principal),
        Path(name.to_string()),
    )
    .await
    {
        Ok(Json(sandbox)) => tool_ok_json(&sandbox),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_create_sandbox(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let name = match arg_str(args, "name") {
        Ok(v) => v,
        Err(msg) => return arg_err("create_sandbox", &msg),
    };
    let project = match arg_str(args, "project") {
        Ok(v) => v,
        Err(msg) => return arg_err("create_sandbox", &msg),
    };
    let branch = match arg_str(args, "branch") {
        Ok(v) => v,
        Err(msg) => return arg_err("create_sandbox", &msg),
    };
    let toolchains: Vec<vanyline_crds::Toolchain> = match args.get("toolchains") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(t) => t,
            Err(e) => {
                return arg_err(
                    "create_sandbox",
                    &format!("argument « toolchains » : tableau d'outils attendu ({e})"),
                );
            }
        },
    };
    let body = api::sandboxes::CreateSandboxBody {
        name: name.to_string(),
        spec: vanyline_crds::SandboxSpec {
            project: project.to_string(),
            branch: branch.to_string(),
            toolchains,
            image: None,
            resources: None,
            egress: Vec::new(),
            suspended: false,
        },
    };
    match api::sandboxes::create_sandbox(State(state.clone()), auth_user(principal), Json(body))
        .await
    {
        Ok((_created, Json(sandbox))) => tool_ok_json(&sandbox),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_delete_sandbox(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let name = match arg_str(args, "name") {
        Ok(v) => v,
        Err(msg) => return arg_err("delete_sandbox", &msg),
    };
    match api::sandboxes::delete_sandbox(
        State(state.clone()),
        auth_user(principal),
        Path(name.to_string()),
    )
    .await
    {
        Ok(_no_content) => ok_result(&format!("sandbox « {name} » supprimée")),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_set_sandbox_suspended(
    state: &AppState,
    principal: &AuthPrincipal,
    args: &Value,
    suspended: bool,
) -> Value {
    let tool = if suspended {
        "suspend_sandbox"
    } else {
        "resume_sandbox"
    };
    let name = match arg_str(args, "name") {
        Ok(v) => v,
        Err(msg) => return arg_err(tool, &msg),
    };
    match api::sandboxes::set_sandbox_suspended(
        State(state.clone()),
        auth_user(principal),
        Path(name.to_string()),
        Json(api::sandboxes::SuspendBody { suspended }),
    )
    .await
    {
        Ok(Json(sandbox)) => tool_ok_json(&sandbox),
        Err(e) => tool_err_app(&e),
    }
}

// ── Outils santé du code (relais interne via SandboxAccess, Task `[!]`) ──────

/// Convertit une erreur du séam sandbox en erreur d'outil (message `StepError` verbatim —
/// le contrat `steps/mod.sdd` n'y met jamais de jeton).
fn step_err(e: miryad_core::workflow::StepError) -> AppError {
    AppError::McpError(e.message)
}

/// Ouvre l'accès authentifié à la sandbox sur le séam fourni (contexte de run synthétique
/// « mcp-domain » : le relais n'est pas un run, seuls `owner` et `sandbox` comptent).
async fn open_sandbox_access(
    locator: Arc<dyn SandboxLocator>,
    http: reqwest::Client,
    owner: &str,
    project: &str,
    sandbox: &str,
) -> Result<SandboxAccess, AppError> {
    let ctx = RunContext {
        run_id: format!("mcp-{}", uuid::Uuid::new_v4()),
        user: "mcp-domain".to_string(),
        user_id: 0,
        owner: owner.to_string(),
        project: project.to_string(),
        sandbox: Some(sandbox.to_string()),
        triggered_by: "mcp".to_string(),
        definition: "mcp-domain".to_string(),
    };
    let value = serde_json::to_value(&ctx).map_err(|e| {
        AppError::InternalError(format!(
            "VNL-INT-001: contexte de relais non sérialisable: {e}"
        ))
    })?;
    let info = RunInfo::from_config(&serde_json::json!({ "_run": value })).map_err(step_err)?;
    SandboxAccess::open_with(locator, http, &info)
        .await
        .map_err(step_err)
}

/// Localisateur de production branché sur le client K8s de l'`AppState`.
async fn k8s_locator(state: &AppState) -> Result<Arc<dyn SandboxLocator>, AppError> {
    Ok(Arc::new(K8sLocator::new(crate::k8s::client(state).await?)))
}

/// Relais `GET /code-health` : ouverture authentifiée, lecture, rendu texte borné 8 KiB
/// comme l'outil de la sandbox (`../../sandbox/src/tools_impl.rs`, `render_code_health`).
/// Options de rendu du relais `code_health` (groupées pour rester sous la limite
/// d'arguments du harnais).
pub(crate) struct HealthRender<'a> {
    /// Niveau de détail : `summary` (défaut), `issues` ou `coverage`.
    pub(crate) detail: &'a str,
    /// Préfixe de chemin filtrant issues et fichiers de couverture.
    pub(crate) path_prefix: Option<&'a str>,
    /// Plafond d'entrées rendues.
    pub(crate) limit: usize,
}

pub(crate) async fn code_health_relay(
    locator: Arc<dyn SandboxLocator>,
    http: reqwest::Client,
    owner: &str,
    project: &str,
    sandbox: &str,
    render: &HealthRender<'_>,
) -> Result<String, AppError> {
    let access = open_sandbox_access(locator, http, owner, project, sandbox).await?;
    let state_json = access.get_json("/code-health").await.map_err(step_err)?;
    let text = render_health(&state_json, render.detail, render.path_prefix, render.limit)?;
    Ok(bounded(text))
}

/// Relais `POST /code-health/refresh` (corps `{ base? }`, 202 attendu).
pub(crate) async fn refresh_code_health_relay(
    locator: Arc<dyn SandboxLocator>,
    http: reqwest::Client,
    owner: &str,
    project: &str,
    sandbox: &str,
    base: Option<&str>,
) -> Result<String, AppError> {
    let access = open_sandbox_access(locator, http, owner, project, sandbox).await?;
    let body = match base {
        Some(b) => serde_json::json!({ "base": b }),
        None => serde_json::json!({}),
    };
    access
        .post_json("/code-health/refresh", &body)
        .await
        .map_err(step_err)?;
    Ok("rafraîchissement demandé (202 accepté) — recalcul immédiat planifié".to_string())
}

/// Propriété puis relais : Owner résolu depuis l'identité, possession vérifiée par la
/// MÊME fonction REST que `GET /api/sandboxes/{name}` (le projet de la CR sert d'ancrage
/// au séam), relais interne via `SandboxAccess`.
async fn sandbox_health_context(
    state: &AppState,
    principal: &AuthPrincipal,
    tool: &str,
    args: &Value,
) -> Result<(String, String), Value> {
    let sandbox = match arg_str(args, "sandbox") {
        Ok(v) => v.to_string(),
        Err(msg) => return Err(arg_err(tool, &msg)),
    };
    let (_uid, owner) = caller_owner(state, principal)
        .await
        .map_err(|e| tool_err_app(&e))?;
    let Some(owner) = owner else {
        return Err(tool_err_app(&AppError::Forbidden));
    };
    let project = match api::sandboxes::get_sandbox(
        State(state.clone()),
        auth_user(principal),
        Path(sandbox.clone()),
    )
    .await
    {
        Ok(Json(cr)) => cr.spec.project,
        Err(e) => return Err(tool_err_app(&e)),
    };
    Ok((owner, project))
}

async fn t_code_health(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let detail = match arg_opt_str(args, "detail") {
        Ok(v) => v.unwrap_or("summary"),
        Err(msg) => return arg_err("code_health", &msg),
    };
    if !matches!(detail, "summary" | "issues" | "coverage") {
        return arg_err(
            "code_health",
            "argument « detail » : « summary », « issues » ou « coverage » attendu",
        );
    }
    let path_prefix = match arg_opt_str(args, "path") {
        Ok(v) => v,
        Err(msg) => return arg_err("code_health", &msg),
    };
    let limit = match arg_opt_usize(args, "limit") {
        Ok(v) => v.unwrap_or(HEALTH_RENDER_LIMIT).min(HEALTH_RENDER_LIMIT),
        Err(msg) => return arg_err("code_health", &msg),
    };
    let sandbox = match arg_str(args, "sandbox") {
        Ok(v) => v,
        Err(msg) => return arg_err("code_health", &msg),
    };
    let (owner, project) = match sandbox_health_context(state, principal, "code_health", args).await
    {
        Ok(v) => v,
        Err(envelope) => return envelope,
    };
    let locator = match k8s_locator(state).await {
        Ok(v) => v,
        Err(e) => return tool_err_app(&e),
    };
    match code_health_relay(
        locator,
        reqwest::Client::new(),
        &owner,
        &project,
        sandbox,
        &HealthRender {
            detail,
            path_prefix,
            limit,
        },
    )
    .await
    {
        Ok(text) => ok_result(&text),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_refresh_code_health(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let base = match arg_opt_str(args, "base") {
        Ok(v) => v,
        Err(msg) => return arg_err("refresh_code_health", &msg),
    };
    let sandbox = match arg_str(args, "sandbox") {
        Ok(v) => v,
        Err(msg) => return arg_err("refresh_code_health", &msg),
    };
    let (owner, project) =
        match sandbox_health_context(state, principal, "refresh_code_health", args).await {
            Ok(v) => v,
            Err(envelope) => return envelope,
        };
    let locator = match k8s_locator(state).await {
        Ok(v) => v,
        Err(e) => return tool_err_app(&e),
    };
    match refresh_code_health_relay(
        locator,
        reqwest::Client::new(),
        &owner,
        &project,
        sandbox,
        base,
    )
    .await
    {
        Ok(text) => ok_result(&text),
        Err(e) => tool_err_app(&e),
    }
}

// ── Rendu santé du code (aligné sur l'outil de la sandbox) ───────────────────

/// Défaut et plafond des entrées rendues (mêmes valeurs que `READ_MAX_LINES` de
/// `vanyline_tools::output`, côté sandbox).
const HEALTH_RENDER_LIMIT: usize = 200;

/// Nombre d'issues rendues par le résumé par défaut (outil de la sandbox : « les 20
/// premières »).
const HEALTH_SUMMARY_ISSUES: usize = 20;

/// Étiquettes alignées sur le serde de `@Phase`/`@CheckStatus`/`@CheckKind`/`@Severity`
/// (mêmes textes que l'outil de la sandbox).
fn health_status_label(status: &CheckStatus) -> &'static str {
    match status {
        CheckStatus::Passed => "passed",
        CheckStatus::Failed => "failed",
        CheckStatus::Errored => "errored",
        CheckStatus::Skipped => "skipped",
    }
}

fn health_check_kind_label(kind: &vanyline_health::report::CheckKind) -> &'static str {
    match kind {
        vanyline_health::report::CheckKind::Format => "format",
        vanyline_health::report::CheckKind::Lint => "lint",
        vanyline_health::report::CheckKind::Tests => "tests",
        vanyline_health::report::CheckKind::Coverage => "coverage",
        vanyline_health::report::CheckKind::NewCode => "new_code",
    }
}

fn health_severity_label(severity: &Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

/// Rendu texte du `@HealthState` rendu par `GET /code-health` de la sandbox (summary /
/// issues / coverage, filtre `path` par préfixe, `limit`).
fn render_health(
    state_json: &Value,
    detail: &str,
    path_prefix: Option<&str>,
    limit: usize,
) -> Result<String, AppError> {
    let phase = state_json
        .get("phase")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::McpError(
                "VNL-MCP-001: /code-health a rendu un état sans champ « phase »".to_string(),
            )
        })?;
    let mut lines = vec![format!("phase: {phase}")];
    let Some(report_json) = state_json.get("report").filter(|v| !v.is_null()) else {
        lines.push(
            "rapport absent — premier calcul en cours, réessayez dans quelques instants"
                .to_string(),
        );
        return Ok(lines.join("\n"));
    };
    let report: HealthReport = serde_json::from_value(report_json.clone())
        .map_err(|e| AppError::McpError(format!("VNL-MCP-001: rapport de santé illisible: {e}")))?;
    lines.push(format!(
        "statut global: {}",
        health_status_label(&report.status())
    ));
    match detail {
        "issues" => {
            let matching: Vec<&Issue> = report
                .issues
                .iter()
                .filter(|issue| health_path_matches(&issue.path, path_prefix))
                .collect();
            push_health_issue_list(&mut lines, &matching, limit);
        }
        "coverage" => {
            push_health_coverage_totals(&mut lines, &report);
            if let Some(coverage) = report.coverage.as_ref() {
                let files: Vec<_> = coverage
                    .files
                    .iter()
                    .filter(|file| health_path_matches(&file.path, path_prefix))
                    .collect();
                let shown = files.len().min(limit);
                lines.push(format!("fichiers couverts ({shown} sur {}):", files.len()));
                for file in files.iter().take(shown) {
                    let totals = vanyline_health::report::CoverageTotals {
                        lines_found: file.lines_found,
                        lines_hit: file.lines_hit,
                    };
                    lines.push(format!(
                        "  {}: {}/{} ({:.1}%)",
                        file.path,
                        file.lines_hit,
                        file.lines_found,
                        totals.percent()
                    ));
                }
                let rest = files.len().saturating_sub(shown);
                if rest > 0 {
                    lines.push(format!(
                        "… {rest} fichiers supplémentaires (affinez avec path ou limit)"
                    ));
                }
            }
        }
        _ => {
            lines.push(format!("vérifications ({}):", report.checks.len()));
            for check in &report.checks {
                lines.push(format!(
                    "  {} [{}]: {} ({} ms)",
                    check.id,
                    health_check_kind_label(&check.kind),
                    health_status_label(&check.status),
                    check.duration_ms
                ));
            }
            push_health_coverage_totals(&mut lines, &report);
            if !report.unchecked_languages.is_empty() {
                lines.push(format!(
                    "langages non vérifiés: {}",
                    report.unchecked_languages.join(", ")
                ));
            }
            let matching: Vec<&Issue> = report
                .issues
                .iter()
                .filter(|issue| health_path_matches(&issue.path, path_prefix))
                .collect();
            if matching.is_empty() {
                lines.push("aucune issue".to_string());
            } else {
                push_health_issue_list(&mut lines, &matching, HEALTH_SUMMARY_ISSUES);
            }
        }
    }
    Ok(lines.join("\n"))
}

/// Liste d'issues (en-tête compté, lignes rendues, reliquat annoncé — style de l'outil
/// de la sandbox).
fn push_health_issue_list(lines: &mut Vec<String>, matching: &[&Issue], limit: usize) {
    let shown = matching.len().min(limit);
    lines.push(format!("issues ({shown} sur {}):", matching.len()));
    for issue in matching.iter().take(shown) {
        let code = issue
            .code
            .as_deref()
            .map_or_else(String::new, |c| format!("{c}: "));
        lines.push(format!(
            "  {}:{}:{} {} {code}{}",
            issue.path,
            issue.line,
            issue.column,
            health_severity_label(&issue.severity),
            issue.message
        ));
    }
    let rest = matching.len().saturating_sub(shown);
    if rest > 0 {
        lines.push(format!(
            "… {rest} issues supplémentaires (affinez avec path ou limit)"
        ));
    }
}

/// Totaux de couverture (« pas de mesure » ≠ « 0 % », même règle que la sandbox).
fn push_health_coverage_totals(lines: &mut Vec<String>, report: &HealthReport) {
    match report.coverage.as_ref() {
        Some(coverage) => {
            let totals = coverage.totals();
            lines.push(format!(
                "couverture: {}/{} lignes ({:.1}%)",
                totals.lines_hit,
                totals.lines_found,
                totals.percent()
            ));
        }
        None => lines.push("couverture: aucune mesure".to_string()),
    }
}

/// Filtre `path` : préfixe de chemin (contrat de l'outil de la sandbox).
fn health_path_matches(path: &str, prefix: Option<&str>) -> bool {
    prefix.is_none_or(|p| path.starts_with(p))
}

// ── Outils workflows et runs ─────────────────────────────────────────────────

async fn t_list_workflows(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let project = match arg_str(args, "project") {
        Ok(v) => v,
        Err(msg) => return arg_err("list_workflows", &msg),
    };
    match api::workflows::list_workflows(
        State(state.clone()),
        auth_user(principal),
        Path(project.to_string()),
    )
    .await
    {
        Ok(Json(items)) => tool_ok_items("workflows", &items),
        Err(e) => tool_err_app(&e),
    }
}

/// (projet, id) — arguments communs des outils ciblant un workflow.
fn wf_ref(args: &Value, tool: &str) -> Result<(String, i32), Value> {
    let project = arg_str(args, "project").map_err(|msg| arg_err(tool, &msg))?;
    let id = arg_i32(args, "workflow").map_err(|msg| arg_err(tool, &msg))?;
    Ok((project.to_string(), id))
}

async fn t_get_workflow(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let (project, id) = match wf_ref(args, "get_workflow") {
        Ok(v) => v,
        Err(envelope) => return envelope,
    };
    match api::workflows::get_workflow(
        State(state.clone()),
        auth_user(principal),
        Path((project, id)),
    )
    .await
    {
        Ok(Json(detail)) => tool_ok_json(&detail),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_create_workflow(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let project = match arg_str(args, "project") {
        Ok(v) => v,
        Err(msg) => return arg_err("create_workflow", &msg),
    };
    let name = match arg_str(args, "name") {
        Ok(v) => v,
        Err(msg) => return arg_err("create_workflow", &msg),
    };
    let scope = match arg_str(args, "scope") {
        Ok(v) => v,
        Err(msg) => return arg_err("create_workflow", &msg),
    };
    let description = match arg_opt_str(args, "description") {
        Ok(v) => v,
        Err(msg) => return arg_err("create_workflow", &msg),
    };
    let steps = match args.get("steps") {
        None | Some(Value::Null) => {
            return arg_err(
                "create_workflow",
                "argument manquant : « steps » (tableau d'étapes attendu)",
            );
        }
        Some(v) => {
            match serde_json::from_value::<Vec<miryad_core::workflow::StepDefinition>>(v.clone()) {
                Ok(s) => s,
                Err(e) => {
                    return arg_err(
                        "create_workflow",
                        &format!("argument « steps » : tableau d'étapes attendu ({e})"),
                    );
                }
            }
        }
    };
    let body = api::workflows::CreateWorkflowBody {
        name: name.to_string(),
        scope: scope.to_string(),
        description: description.map(str::to_string),
        steps,
    };
    match api::workflows::create_workflow(
        State(state.clone()),
        auth_user(principal),
        Path(project.to_string()),
        Json(body),
    )
    .await
    {
        Ok((_created, Json(detail))) => tool_ok_json(&detail),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_update_workflow(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let (project, id) = match wf_ref(args, "update_workflow") {
        Ok(v) => v,
        Err(envelope) => return envelope,
    };
    let description = match arg_opt_str(args, "description") {
        Ok(v) => v.map(str::to_string),
        Err(msg) => return arg_err("update_workflow", &msg),
    };
    let steps = match args.get("steps") {
        None | Some(Value::Null) => None,
        Some(v) => {
            match serde_json::from_value::<Vec<miryad_core::workflow::StepDefinition>>(v.clone()) {
                Ok(s) => Some(s),
                Err(e) => {
                    return arg_err(
                        "update_workflow",
                        &format!("argument « steps » : tableau d'étapes attendu ({e})"),
                    );
                }
            }
        }
    };
    let body = api::workflows::UpdateWorkflowBody {
        description,
        steps,
        scope: None,
    };
    match api::workflows::update_workflow(
        State(state.clone()),
        auth_user(principal),
        Path((project, id)),
        Json(body),
    )
    .await
    {
        Ok(Json(detail)) => tool_ok_json(&detail),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_delete_workflow(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let (project, id) = match wf_ref(args, "delete_workflow") {
        Ok(v) => v,
        Err(envelope) => return envelope,
    };
    match api::workflows::delete_workflow(
        State(state.clone()),
        auth_user(principal),
        Path((project, id)),
    )
    .await
    {
        Ok(_no_content) => ok_result(&format!(
            "workflow {id} supprimé (définition et rattachement)"
        )),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_launch_workflow(
    state: &AppState,
    principal: &AuthPrincipal,
    agent: Option<&str>,
    args: &Value,
) -> Value {
    let (project, id) = match wf_ref(args, "launch_workflow") {
        Ok(v) => v,
        Err(envelope) => return envelope,
    };
    let sandbox = match arg_opt_str(args, "sandbox") {
        Ok(v) => v.map(str::to_string),
        Err(msg) => return arg_err("launch_workflow", &msg),
    };
    let triggered_by = match agent {
        Some(a) => format!("agent:{a}"),
        None => "api".to_string(),
    };
    match api::workflows::launch_workflow(
        state,
        &auth_user(principal),
        &project,
        id,
        sandbox,
        &triggered_by,
    )
    .await
    {
        Ok(launch) => tool_ok_json(&launch),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_list_runs(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    // Garde moteur d'abord (même ordre que `list_project_runs`).
    if state.workflow.is_none() {
        return tool_err_app(&AppError::WorkflowEngineNotConfigured);
    }
    let project = match arg_opt_str(args, "project") {
        Ok(v) => v.map(str::to_string),
        Err(msg) => return arg_err("list_runs", &msg),
    };
    let sandbox = match arg_opt_str(args, "sandbox") {
        Ok(v) => v.map(str::to_string),
        Err(msg) => return arg_err("list_runs", &msg),
    };
    // Bornage silencieux identique à la route REST (`bounded_list` : clamp 1..=50).
    let limit = match arg_opt_usize(args, "limit") {
        Ok(v) => v.unwrap_or(50).clamp(1, 50),
        Err(msg) => return arg_err("list_runs", &msg),
    };
    let (user_id, owner) = match caller_owner(state, principal).await {
        Ok(v) => v,
        Err(e) => return tool_err_app(&e),
    };
    let Some(_owner) = owner else {
        return ok_result(&bounded("runs (0):".to_string()));
    };
    match api::workflows::list_runs(
        state,
        user_id,
        project.as_deref(),
        sandbox.as_deref(),
        u32::try_from(limit).unwrap_or(50),
        14,
    )
    .await
    {
        Ok(items) => tool_ok_items("runs", &items),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_get_run(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let run_id = match arg_str(args, "run_id") {
        Ok(v) => v,
        Err(msg) => return arg_err("get_run", &msg),
    };
    match api::workflows::get_run(
        State(state.clone()),
        auth_user(principal),
        Path(run_id.to_string()),
    )
    .await
    {
        Ok(Json(detail)) => tool_ok_json(&detail),
        Err(e) => tool_err_app(&e),
    }
}

async fn t_cancel_run(state: &AppState, principal: &AuthPrincipal, args: &Value) -> Value {
    let run_id = match arg_str(args, "run_id") {
        Ok(v) => v,
        Err(msg) => return arg_err("cancel_run", &msg),
    };
    match api::workflows::cancel_run(
        State(state.clone()),
        auth_user(principal),
        Path(run_id.to_string()),
    )
    .await
    {
        Ok(_accepted) => ok_result(&format!("run {run_id} annulé (202 accepté)")),
        Err(e) => tool_err_app(&e),
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::api;
    use crate::db::entities::{owner_links, workflow_runs};
    use crate::db::test_support::real_db;
    use crate::workflow::steps::SandboxLocator;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::{
        Json,
        extract::{Path, State},
    };
    use miryad_core::auth::{AuthUser, PrincipalSource};
    use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ActiveValue::Set, EntityTrait};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    // ── harnais (mêmes motifs que api/workflows.rs W4b) ──────────────────────

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
            listen_addr: "0.0.0.1:8080".to_string(),
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

    fn state_with(db: sea_orm::DatabaseConnection) -> AppState {
        AppState {
            config: test_config(),
            cookie_key: cookie::Key::from(&[0u8; 64]),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    fn principal(sub: &str) -> AuthPrincipal {
        AuthPrincipal {
            subject: sub.to_string(),
            email: Some(format!("{sub}@example.com")),
            preferred_username: Some(sub.to_string()),
            source: PrincipalSource::Session {
                id_token: String::new(),
            },
        }
    }

    fn auth_user(sub: &str) -> AuthUser {
        AuthUser {
            subject: sub.to_string(),
            email: Some(format!("{sub}@example.com")),
            id_token: format!("id-token-{sub}"),
        }
    }

    /// Appel `handle` complet (extracteur d'identité franchi en fixture) → corps JSON-RPC.
    async fn rpc(
        state: &AppState,
        who: &str,
        headers: HeaderMap,
        req: serde_json::Value,
    ) -> serde_json::Value {
        let resp = handle(
            State(state.clone()),
            principal(who),
            headers,
            Json(serde_json::from_value(req).expect("enveloppe JSON-RPC de fixture")),
        )
        .await;
        let (parts, body) = resp.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        assert_eq!(
            parts.status,
            StatusCode::OK,
            "corps: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).unwrap()
    }

    fn empty_headers() -> HeaderMap {
        HeaderMap::new()
    }

    /// `tools/call` nu — rend l'enveloppe `result` (ou l'objet `error` JSON-RPC).
    async fn call_tool(
        state: &AppState,
        name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        call_tool_h(state, empty_headers(), name, arguments).await
    }

    async fn call_tool_h(
        state: &AppState,
        headers: HeaderMap,
        name: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let resp = rpc(
            state,
            "alice",
            headers,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }),
        )
        .await;
        assert!(
            resp.get("error").is_none(),
            "erreur d'enveloppe inattendue pour l'outil {name}: {resp}"
        );
        resp["result"].clone()
    }

    /// Texte de la première contenu-block d'une enveloppe d'outil.
    fn tool_text(envelope: &serde_json::Value) -> String {
        envelope["content"][0]["text"].as_str().unwrap().to_string()
    }

    // ── faux Restate (motif combiné de api/workflows.rs W4b) ─────────────────

    async fn spawn_fake_engine_base() -> String {
        let counter = std::sync::atomic::AtomicUsize::new(0);
        let counter = Arc::new(counter);
        let app = axum::Router::new()
            .route(
                "/deployments",
                axum::routing::post(|| async {
                    (StatusCode::OK, r#"{"deploymentId":"dp_fake"}"#.to_string())
                }),
            )
            .route(
                "/DagInterpreter/{key}/run/send",
                axum::routing::post(move || {
                    let counter = counter.clone();
                    async move {
                        let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        (
                            StatusCode::ACCEPTED,
                            [(header::CONTENT_TYPE, "application/json")],
                            format!(r#"{{"invocationId":"inv_{n}","status":"Accepted"}}"#),
                        )
                    }
                }),
            )
            .route(
                "/query",
                axum::routing::post(|| async {
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/json")],
                        r#"{"rows":[]}"#.to_string(),
                    )
                }),
            )
            .route(
                "/invocations/{id}/cancel",
                axum::routing::patch(|| async { StatusCode::ACCEPTED }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    async fn spawn_engine() -> crate::workflow::WorkflowRuntime {
        let base = spawn_fake_engine_base().await;
        let cfg = crate::workflow::WorkflowConfigApp {
            admin_url: base.clone(),
            ingress_url: base,
            deployment_url: "http://vanyline.test:9410".to_string(),
            listen: "127.0.0.1:0".to_string(),
        };
        crate::workflow::runtime::start_with(
            cfg,
            miryad_core::workflow::StepRegistry::new(),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(50),
        )
        .await
        .expect("moteur de test démarré")
    }

    /// Base réelle + alice/bob résolus et liés à leur Owner + moteur sur faux Restate.
    async fn seeded_state() -> (AppState, i32, i32) {
        let db = real_db().await;
        let alice = miryad_core::users::resolve_user(&db, "alice", Some("alice@example.com"))
            .await
            .expect("alice resolves");
        let bob = miryad_core::users::resolve_user(&db, "bob", Some("bob@example.com"))
            .await
            .expect("bob resolves");
        for (uid, name) in [(alice.id, "alice"), (bob.id, "bob")] {
            owner_links::ActiveModel {
                id: NotSet,
                user_id: Set(uid),
                k8s_owner_name: Set(Some(name.to_string())),
            }
            .insert(&db)
            .await
            .expect("owner link seeded");
        }
        let mut state = state_with(db);
        state.workflow = Some(spawn_engine().await);
        (state, alice.id, bob.id)
    }

    fn body_json(name: &str, scope: &str, steps: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({"name": name, "scope": scope, "steps": steps})
    }

    fn agent_step(id: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "depends_on": [], "kind": "agent", "config": {}})
    }

    /// Le workflow de `who` (portée projet, un step agent) créé par la route REST — id.
    async fn seed_workflow(state: &AppState, who: &str, name: &str) -> i32 {
        let body: api::workflows::CreateWorkflowBody = serde_json::from_value(body_json(
            name,
            "project",
            &serde_json::json!([agent_step("a")]),
        ))
        .expect("corps de fixture");
        let (_, detail) = api::workflows::create_workflow(
            State(state.clone()),
            auth_user(who),
            Path("demo".to_string()),
            Json(body),
        )
        .await
        .expect("workflow de fixture créé");
        detail.id
    }

    async fn seed_run(state: &AppState, owner_id: i32, run_id: &str) {
        workflow_runs::ActiveModel {
            id: NotSet,
            owner_id: Set(owner_id),
            project_name: Set("demo".to_string()),
            sandbox_name: Set(None),
            definition_id: Set(None),
            definition_name: Set("alice/demo/ci".to_string()),
            run_id: Set(run_id.to_string()),
            run_key: Set(format!("key-{run_id}")),
            invocation_id: Set(format!("inv-{run_id}")),
            scope: Set("project".to_string()),
            triggered_by: Set("ui".to_string()),
            started_at: Set(chrono::Utc::now()),
        }
        .insert(&state.auth.db)
        .await
        .expect("ligne d'index seedée");
    }

    async fn run_rows(state: &AppState) -> Vec<workflow_runs::Model> {
        workflow_runs::Entity::find()
            .all(&state.auth.db)
            .await
            .unwrap()
    }

    // ── Scenario: authentification_exigee ────────────────────────────────────
    // Aucune authentification ⇒ 401 sur n'importe quel appel (le corps JSON-RPC valide
    // est atteint seulement après l'extracteur).
    #[tokio::test]
    async fn authentification_exigee() {
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );
        let app = mcp_domain_router().with_state(state);
        for uri in ["/mcp/domain"] {
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                ))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "uri {uri}");
        }
    }

    // ── forme JSON-RPC (convention sandbox : notification ⇒ 202 nu) ──────────
    #[tokio::test]
    async fn notification_sans_id_rend_202_sans_corps() {
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );
        let resp = handle(
            State(state),
            principal("alice"),
            empty_headers(),
            Json(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: None,
                method: "notifications/initialized".to_string(),
                params: serde_json::Value::Null,
            }),
        )
        .await;
        let (parts, body) = resp.into_parts();
        assert_eq!(parts.status, StatusCode::ACCEPTED);
        assert!(
            axum::body::to_bytes(body, usize::MAX)
                .await
                .unwrap()
                .is_empty(),
            "notification : 202 sans corps"
        );
    }

    #[tokio::test]
    async fn initialize_meme_forme_que_la_sandbox() {
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );
        let resp = rpc(
            &state,
            "alice",
            empty_headers(),
            serde_json::json!({"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}),
        )
        .await;
        assert_eq!(resp["id"], serde_json::json!(7));
        let result = &resp["result"];
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["serverInfo"]["name"].is_string());
        assert!(result["serverInfo"]["version"].is_string());
    }

    #[tokio::test]
    async fn methode_inconnue_rend_erreur_jsonrpc() {
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );
        let resp = rpc(
            &state,
            "alice",
            empty_headers(),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/nope"}),
        )
        .await;
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn outil_inconnu_rend_jsonrpc_32602_comme_la_sandbox() {
        // Convention sandbox/src/mcp.rs:178 — un NOM d'outil inconnu est une erreur
        // d'enveloppe -32602, pas une exécution d'outil.
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );
        let resp = rpc(
            &state,
            "alice",
            empty_headers(),
            serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
                "params":{"name":"nope","arguments":{}}}),
        )
        .await;
        assert_eq!(resp["error"]["code"], -32602);
        assert!(resp["error"]["message"].as_str().unwrap().contains("nope"));
    }

    // ── Scenario: tools_list ─────────────────────────────────────────────────
    // Les 21 outils de l'`Exposes`, avec schémas ; les 3 destructifs
    // (delete_project/delete_sandbox/delete_workflow) portent « destructif », cancel_run
    // ne le porte PAS (lettre de l'Exposes).
    #[tokio::test]
    async fn tools_list_les_21_outils_avec_schemas_et_destructifs_marques() {
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );
        let resp = rpc(
            &state,
            "alice",
            empty_headers(),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await;
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 21, "21 outils promis par l'Exposes");
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "list_projects",
            "get_project",
            "create_project",
            "delete_project",
            "list_sandboxes",
            "get_sandbox",
            "create_sandbox",
            "delete_sandbox",
            "suspend_sandbox",
            "resume_sandbox",
            "code_health",
            "refresh_code_health",
            "list_workflows",
            "get_workflow",
            "create_workflow",
            "update_workflow",
            "delete_workflow",
            "launch_workflow",
            "list_runs",
            "get_run",
            "cancel_run",
        ] {
            assert!(names.contains(&expected), "outil manquant : {expected}");
        }
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            assert_eq!(tool["inputSchema"]["type"], "object", "schéma de {name}");
            assert!(tool["description"].is_string(), "description de {name}");
            let desc = tool["description"].as_str().unwrap();
            let destructif_promis = matches!(
                name,
                "delete_project" | "delete_sandbox" | "delete_workflow"
            );
            assert_eq!(
                desc.contains("destructif"),
                destructif_promis,
                "mention « destructif » de {name}"
            );
        }
    }

    // ── erreurs d'arguments ⇒ isError nommant l'argument ─────────────────────
    #[tokio::test]
    async fn erreur_argument_manquant_nomme_l_argument() {
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );
        let out = call_tool(&state, "get_project", serde_json::json!({})).await;
        assert_eq!(out["isError"], serde_json::json!(true));
        let text = tool_text(&out);
        assert!(
            text.contains("name"),
            "le message nomme l'argument : {text}"
        );
    }

    // ── dispatch : chaque famille atteint la fonction REST partagée ──────────
    // Vérrouillage hors cluster : la requête parvient au chemin REST (erreur de base
    // simulée `VNL-DB-006` ou garde moteur `VNL-WFA-006`), pas à un stub local.
    #[tokio::test]
    async fn dispatch_atteint_les_chemins_rest() {
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );
        let out = call_tool(&state, "get_project", serde_json::json!({"name": "p"})).await;
        assert_eq!(out["isError"], serde_json::json!(true));
        assert!(
            tool_text(&out).contains("VNL-DB-006"),
            "chemin projects.rs atteint"
        );

        let out = call_tool(&state, "get_sandbox", serde_json::json!({"name": "s"})).await;
        assert_eq!(out["isError"], serde_json::json!(true));
        assert!(
            tool_text(&out).contains("VNL-DB-006"),
            "chemin sandboxes.rs atteint"
        );

        let out = call_tool(
            &state,
            "list_workflows",
            serde_json::json!({"project": "d"}),
        )
        .await;
        assert_eq!(out["isError"], serde_json::json!(true));
        assert!(
            tool_text(&out).contains("VNL-WFA-006"),
            "garde moteur de workflows.rs d'abord"
        );

        let out = call_tool(&state, "suspend_sandbox", serde_json::json!({"name": "s"})).await;
        assert_eq!(out["isError"], serde_json::json!(true));
        assert!(
            tool_text(&out).contains("VNL-DB-006"),
            "chemin set_sandbox_suspended atteint"
        );
    }

    // ── Scenario: liste_de_ses_projets_seulement (volets verrouillables hors
    // cluster — l'isolement CR réel est le `[ ]` cluster de la spec) ──────────
    #[tokio::test]
    async fn liste_de_ses_projets_seulement() {
        let (state, alice_id, bob_id) = seeded_state().await;
        // (1) l'Owner vient TOUJOURS de l'identité : chaque principal résout SON Owner.
        let (uid, owner) = caller_owner(&state, &principal("alice")).await.unwrap();
        assert_eq!(uid, alice_id);
        assert_eq!(owner.as_deref(), Some("alice"));
        let (uid, owner) = caller_owner(&state, &principal("bob")).await.unwrap();
        assert_eq!(uid, bob_id);
        assert_eq!(owner.as_deref(), Some("bob"));

        // (2) sans Owner lié, `list_projects` (la FONCTION REST, qui filtre par Owner)
        // rend la liste vide SANS toucher le cluster.
        let db = real_db().await;
        let state2 = state_with(db);
        let out = call_tool(&state2, "list_projects", serde_json::json!({})).await;
        assert_eq!(out["isError"], serde_json::json!(false));
        assert!(
            tool_text(&out).starts_with("projets (0)"),
            "aucun Owner ⇒ liste vide : {}",
            tool_text(&out)
        );
    }

    // ── Scenario: objet_d_autrui (volet mappage verrouillable hors cluster —
    // le get_sandbox/delete_sandbox sur CR réelle est le `[ ]` cluster) ───────
    // `isError: true`, aucune action, et AUCUNE distinction entre « inexistant » et
    // « à un autre » : le texte rendu est identique octet à octet.
    #[tokio::test]
    async fn objet_d_autrui_iserror_identique_inexistant() {
        let (state, _alice, _bob) = seeded_state().await;
        let id = seed_workflow(&state, "bob", "ci").await;

        let autre = call_tool(
            &state,
            "get_workflow",
            serde_json::json!({"project": "demo", "workflow": id}),
        )
        .await;
        let absent = call_tool(
            &state,
            "get_workflow",
            serde_json::json!({"project": "demo", "workflow": 4242}),
        )
        .await;
        assert_eq!(autre["isError"], serde_json::json!(true));
        assert_eq!(absent["isError"], serde_json::json!(true));
        assert_eq!(
            tool_text(&autre),
            tool_text(&absent),
            "aucune fuite d'existence : texte identique"
        );

        // aucune action : le workflow de bob survit
        let out = rpc(
            &state,
            "bob",
            empty_headers(),
            serde_json::json!({"jsonrpc":"2.0","id":9,"method":"tools/call",
                "params":{"name":"get_workflow","arguments":{"project":"demo","workflow":id}}}),
        )
        .await;
        assert_eq!(out["result"]["isError"], serde_json::json!(false));
    }

    // ── Scenario: lancement_trace_par_agent ──────────────────────────────────
    // X-Vnl-Agent: project ⇒ triggered_by = "agent:project" sur la ligne d'index.
    #[tokio::test]
    async fn lancement_trace_par_agent() {
        let (state, _alice, _bob) = seeded_state().await;
        let id = seed_workflow(&state, "alice", "ci").await;
        let mut headers = HeaderMap::new();
        headers.insert("x-vnl-agent", "project".parse().unwrap());
        let out = call_tool_h(
            &state,
            headers,
            "launch_workflow",
            serde_json::json!({"project": "demo", "workflow": id}),
        )
        .await;
        assert_eq!(
            out["isError"],
            serde_json::json!(false),
            "{}",
            tool_text(&out)
        );

        let rows = run_rows(&state).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].triggered_by, "agent:project");
    }

    // Volet « api » du Scenario : sans en-tête agent, la ligne porte "api".
    #[tokio::test]
    async fn lancement_sans_entete_agent_declenche_api() {
        let (state, _alice, _bob) = seeded_state().await;
        let id = seed_workflow(&state, "alice", "ci").await;
        let out = call_tool(
            &state,
            "launch_workflow",
            serde_json::json!({"project": "demo", "workflow": id}),
        )
        .await;
        assert_eq!(
            out["isError"],
            serde_json::json!(false),
            "{}",
            tool_text(&out)
        );
        let rows = run_rows(&state).await;
        assert_eq!(rows[0].triggered_by, "api");
    }

    // ── Scenario: sortie_bornee ──────────────────────────────────────────────
    // 500 runs ⇒ le texte rendu fait au plus 8 KiB avec un marqueur de troncature.
    #[tokio::test]
    async fn sortie_bornee() {
        let (state, alice, _bob) = seeded_state().await;
        for i in 0..500 {
            seed_run(&state, alice, &format!("run-{i:04}")).await;
        }
        let out = call_tool(&state, "list_runs", serde_json::json!({"project": "demo"})).await;
        assert_eq!(
            out["isError"],
            serde_json::json!(false),
            "{}",
            tool_text(&out)
        );
        let text = tool_text(&out);
        assert!(
            text.len() <= 8192,
            "sortie de {} octets > 8 KiB",
            text.len()
        );
        assert!(text.contains("tronqu"), "marqueur de troncature absent");
    }

    // ── Scenario: meme_logique_que_rest (volets verrouillables hors cluster —
    // la CR réellement créée est le `[ ]` cluster de la spec) ─────────────────
    // Mêmes validations, mêmes erreurs : le texte MCP est le message @Display de
    // l'@AppError que la route REST rend aussi, corps-à-corps.
    #[tokio::test]
    async fn meme_logique_que_rest() {
        let state = state_with(
            sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Sqlite).into_connection(),
        );

        // (a) create_sandbox, branche vide ⇒ la validation REST SandboxBranchEmpty,
        // atteinte AVANT toute base (le REST la pose en première ligne).
        let rest_err = api::sandboxes::create_sandbox(
            State(state.clone()),
            auth_user("alice"),
            Json(api::sandboxes::CreateSandboxBody {
                name: "s".to_string(),
                spec: vanyline_crds::SandboxSpec {
                    project: "p".to_string(),
                    branch: "  ".to_string(),
                    toolchains: vec![],
                    image: None,
                    resources: None,
                    egress: vec![],
                    suspended: false,
                },
            }),
        )
        .await
        .expect_err("branche vide ⇒ erreur REST");
        let out = call_tool(
            &state,
            "create_sandbox",
            serde_json::json!({"name": "s", "project": "p", "branch": "  "}),
        )
        .await;
        assert_eq!(out["isError"], serde_json::json!(true));
        assert_eq!(tool_text(&out), rest_err.to_string(), "mêmes erreurs");

        // (b) list_workflows, moteur non configuré ⇒ la garde REST VNL-WFA-006.
        let rest_err = api::workflows::list_workflows(
            State(state.clone()),
            auth_user("alice"),
            Path("demo".to_string()),
        )
        .await
        .expect_err("moteur absent ⇒ garde REST");
        let out = call_tool(
            &state,
            "list_workflows",
            serde_json::json!({"project": "demo"}),
        )
        .await;
        assert_eq!(out["isError"], serde_json::json!(true));
        assert_eq!(tool_text(&out), rest_err.to_string(), "mêmes erreurs");
    }

    // ── relais santé du code (Task `[!]` : séam SandboxAccess + faux serveur) ─

    /// Faux serveur sandbox : sert `/code-health` (200 ou 401) et capture les
    /// `/code-health/refresh`.
    async fn spawn_fake_sandbox(health_body: Option<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let body = Arc::new(health_body);
        let captured_route = captured.clone();
        let app = axum::Router::new()
            .route(
                "/code-health",
                axum::routing::get(move || {
                    let body = body.clone();
                    async move {
                        match body.as_ref() {
                            Some(b) => (
                                StatusCode::OK,
                                [(header::CONTENT_TYPE, "application/json")],
                                b.clone(),
                            ),
                            None => (
                                StatusCode::UNAUTHORIZED,
                                [(header::CONTENT_TYPE, "text/plain")],
                                "unauthorized".to_string(),
                            ),
                        }
                    }
                }),
            )
            .route(
                "/code-health/refresh",
                axum::routing::post(move |body: String| {
                    let captured = captured_route.clone();
                    async move {
                        captured.lock().unwrap().push(body);
                        StatusCode::ACCEPTED
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), captured)
    }

    struct FakeSandboxLocator {
        base: String,
        token: String,
    }

    #[async_trait]
    impl SandboxLocator for FakeSandboxLocator {
        async fn phase(
            &self,
            _owner: &str,
            _sandbox: &str,
        ) -> Result<Option<String>, miryad_core::workflow::StepError> {
            Ok(Some("Running".to_string()))
        }
        async fn token(&self, _owner: &str) -> Result<String, miryad_core::workflow::StepError> {
            Ok(self.token.clone())
        }
        async fn base_url(
            &self,
            _owner: &str,
            _sandbox: &str,
        ) -> Result<String, miryad_core::workflow::StepError> {
            Ok(self.base.clone())
        }
    }

    fn fake_locator(base: &str) -> std::sync::Arc<dyn SandboxLocator> {
        std::sync::Arc::new(FakeSandboxLocator {
            base: base.to_string(),
            token: "T0K3N-SECRET".to_string(),
        })
    }

    /// `HealthState` complet (2 vérifs dont une `failed`, 200 issues) sérialisé comme la
    /// sandbox le rend (`/code-health`).
    fn fake_health_state() -> String {
        let issues: Vec<String> = (0..200)
            .map(|i| {
                format!(
                    r#"{{"check_id":"clippy","path":"src/fichier_{i}.rs","line":{i},"column":1,"severity":"warning","code":"clippy::warn","message":"avertissement de test numéro {i}"}}"#
                )
            })
            .collect();
        format!(
            r#"{{"phase":"fresh","report":{{"revision":"abc123","checks":[
            {{"id":"clippy","kind":"lint","status":"failed","exit_code":1,"duration_ms":12,"output_tail":"x"}},
            {{"id":"fmt","kind":"format","status":"passed","exit_code":0,"duration_ms":3,"output_tail":""}}],
            "coverage":null,"issues":[{}],"new_code":null,"unchecked_languages":["go"]}},
            "computed_at":1760000000000,"stale_since":null,"base":null}}"#,
            issues.join(",")
        )
    }

    #[tokio::test]
    async fn code_health_relay_rend_summary_et_borne_8kib() {
        let (base, _captured) = spawn_fake_sandbox(Some(fake_health_state())).await;
        // summary : phase, statut global, une ligne par vérif, 20 premières issues sur 200.
        let text = code_health_relay(
            fake_locator(&base),
            reqwest::Client::new(),
            "alice",
            "demo",
            "sb",
            &HealthRender {
                detail: "summary",
                path_prefix: None,
                limit: 200,
            },
        )
        .await
        .expect("relais summary");
        assert!(text.starts_with("phase: fresh"), "{text}");
        assert!(text.contains("statut global: failed"));
        assert!(text.contains("issues (20 sur 200)"));
        assert!(text.contains("180 issues supplémentaires"));
        assert!(text.contains("couverture: aucune mesure"));
        assert!(text.contains("langages non vérifiés: go"));

        // detail « issues » avec limit par défaut ⇒ rendu > 8 KiB borné + marqueur.
        let text = code_health_relay(
            fake_locator(&base),
            reqwest::Client::new(),
            "alice",
            "demo",
            "sb",
            &HealthRender {
                detail: "issues",
                path_prefix: None,
                limit: 200,
            },
        )
        .await
        .expect("relais issues");
        assert!(
            text.len() <= 8192,
            "sortie de {} octets > 8 KiB",
            text.len()
        );
        assert!(text.contains("tronqu"));
        assert!(text.is_char_boundary(text.len()));
    }

    #[tokio::test]
    async fn refresh_code_health_relay_poste_base_et_accepte_202() {
        let (base, captured) = spawn_fake_sandbox(None).await;
        let text = refresh_code_health_relay(
            fake_locator(&base),
            reqwest::Client::new(),
            "alice",
            "demo",
            "sb",
            Some("origin/main"),
        )
        .await
        .expect("refresh accepté (202)");
        assert!(text.contains("rafraîchissement"), "{text}");
        let bodies = captured.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(parsed["base"], "origin/main");
    }

    #[tokio::test]
    async fn jeton_jamais_rendu_en_erreur_de_relais() {
        let (base, _captured) = spawn_fake_sandbox(None).await; // 401 côté sandbox
        let err = code_health_relay(
            fake_locator(&base),
            reqwest::Client::new(),
            "alice",
            "demo",
            "sb",
            &HealthRender {
                detail: "summary",
                path_prefix: None,
                limit: 200,
            },
        )
        .await
        .expect_err("401 de la sandbox ⇒ erreur");
        assert!(err.to_string().contains("401"), "{}", err);
        assert!(
            !err.to_string().contains("T0K3N-SECRET"),
            "le jeton fuit : {err}"
        );
    }
}
