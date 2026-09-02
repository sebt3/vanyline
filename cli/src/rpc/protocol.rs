use serde::{Deserialize, Serialize};

/// Version de protocole supportée par ce serveur (design doc, section "Cycle de vie").
pub const PROTOCOL_VERSION: u32 = 1;

/// Requête JSON-RPC entrante. `params` défaut à `Value::Null` si absent —
/// chaque méthode fait sa propre désérialisation depuis ce `Value`.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// `None` = notification (pas de réponse attendue). Aucune méthode
    /// client -> serveur de cette tâche n'est une notification : traiter
    /// `None` comme un id `Value::Null` pour construire la réponse quand
    /// même (permissif — le design ne définit pas ce cas explicitement).
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Réponse JSON-RPC sortante (succès XOR erreur — jamais les deux, jamais aucun
/// des deux ; c'est `JsonRpcResponse::success`/`::error` qui garantit ça, pas
/// le type — ne pas construire la struct à la main ailleurs que dans ces
/// deux fonctions).
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorObj>,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(
        id: serde_json::Value,
        code: i64,
        message: impl Into<String>,
        vnl_code: &'static str,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcErrorObj {
                code,
                message: message.into(),
                data: JsonRpcErrorData {
                    code: vnl_code.to_string(),
                },
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcErrorObj {
    pub code: i64,
    pub message: String,
    pub data: JsonRpcErrorData,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcErrorData {
    /// Identifiant unique `VNL-RPC-*` — TOUTE réponse d'erreur en a un
    /// (règle du projet, cf. AGENTS.md "Messages d'erreur avec identifiant unique").
    pub code: String,
}

/// Codes JSON-RPC standard (plage réservée -32768..-32000, spec JSON-RPC 2.0).
pub mod jsonrpc_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const SERVER_ERROR: i64 = -32000;
}

/// Codes `VNL-RPC-*` de cette tâche. Les tâches 2/3 en ajouteront d'autres
/// (busy = VNL-RPC-002 déjà réservé par le design, pas utilisé ici).
pub mod vnl_code {
    pub const MALFORMED_REQUEST: &str = "VNL-RPC-000";
    pub const NOT_INITIALIZED: &str = "VNL-RPC-001";
    pub const BUSY: &str = "VNL-RPC-002";
    pub const UNKNOWN_PROTOCOL_VERSION: &str = "VNL-RPC-003";
    pub const METHOD_NOT_FOUND: &str = "VNL-RPC-004";
    pub const CONVERSATION_NOT_FOUND: &str = "VNL-RPC-005";
    pub const CONFIG_ERROR: &str = "VNL-RPC-006";
    pub const CONVERSATION_STORAGE_ERROR: &str = "VNL-RPC-007";
    pub const NO_AGENT_RESOLVED: &str = "VNL-RPC-008";
    pub const TURN_EXECUTION_ERROR: &str = "VNL-RPC-009";
    pub const K8S_ERROR: &str = "VNL-RPC-010";
    pub const CONFIG_WRITE_ERROR: &str = "VNL-RPC-011";
    pub const CONFIG_NOT_FOUND: &str = "VNL-RPC-012";
    pub const CONFIG_NAME_CONFLICT: &str = "VNL-RPC-013";
    pub const CONFIG_INVALID_NAME: &str = "VNL-RPC-014";
    pub const CONFIG_VALIDATION: &str = "VNL-RPC-015";
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u32,
    #[serde(default)]
    #[allow(dead_code)] // consommé par la tâche 2 (layering de config par workspace)
    pub workspace: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub server_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,
}

/// Vue allégée d'une conversation — résultat de `conversations/list` et
/// `conversations/create`. `conversations/get` retourne la `Conversation`
/// COMPLÈTE de `vanyline_lib` (déjà Serialize) — pas ce type.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub message_count: usize,
}

impl From<&vanyline_lib::Conversation> for ConversationSummary {
    fn from(c: &vanyline_lib::Conversation) -> Self {
        Self {
            id: c.id.to_string(),
            agent: c.agent.clone(),
            title: c.title.clone(),
            message_count: c.messages.len(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ConversationIdParams {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct ConversationCreateParams {
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Enveloppe JSON-RPC 2.0 générique pour une notification (PAS de champ
/// `id` — contrairement à `JsonRpcResponse` — une notification ne répond à
/// aucune requête précise, cf. spec JSON-RPC 2.0). Générique sur `T` pour
/// être réutilisable par de futurs types de notification au-delà de
/// `chat/event`.
#[derive(Debug, Serialize)]
pub struct JsonRpcNotification<T: Serialize> {
    pub jsonrpc: &'static str,
    pub method: &'static str,
    pub params: T,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEventNotificationParams {
    pub conversation_id: String,
    pub seq: u64,
    pub event: vanyline_lib::event::ChatEvent,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendParams {
    pub conversation_id: String,
    pub message: String,
    #[serde(default)]
    pub agent: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendResult {
    pub text: String,
    pub tool_calls: Vec<vanyline_lib::event::ToolCallRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCancelParams {
    pub conversation_id: String,
}

/// Params partagés par `owners/get`, `owners/delete`,
/// `projects/get`, `projects/delete`, `sandboxes/get`, `sandboxes/delete`.
#[derive(Debug, Deserialize)]
pub struct NameParams {
    pub name: String,
}

/// `owners/create` — `spec` aplati (`#[serde(flatten)]`) : le JSON attendu
/// est `{"name": "...", ...champs de OwnerSpec en camelCase...}`, pas un
/// objet `spec` imbriqué. `OwnerSpec` dérive déjà `Deserialize` avec
/// `#[serde(rename_all = "camelCase")]` (cf. `crds/src/lib.rs`) — réutilisé
/// tel quel, pas de DTO parallèle à maintenir.
#[derive(Debug, Deserialize)]
pub struct OwnerCreateParams {
    pub name: String,
    #[serde(flatten)]
    pub spec: vanyline_crds::OwnerSpec,
}

/// `projects/create` — même principe que `OwnerCreateParams` (04a) :
/// `spec` aplati, `ProjectSpec` dérive déjà `Deserialize` en camelCase.
#[derive(Debug, Deserialize)]
pub struct ProjectCreateParams {
    pub name: String,
    #[serde(flatten)]
    pub spec: vanyline_crds::ProjectSpec,
}

/// `sandboxes/create` — même principe que `OwnerCreateParams`/`ProjectCreateParams`.
#[derive(Debug, Deserialize)]
pub struct SandboxCreateParams {
    pub name: String,
    #[serde(flatten)]
    pub spec: vanyline_crds::SandboxSpec,
}

/// Paramètre `layer` optionnel des méthodes `config/*` d'écriture. La valeur
/// JSON est "global" | "workspace" (minuscules). Toute autre valeur -> erreur
/// de désérialisation d'enveloppe -> VNL-RPC-000.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigLayer {
    Global,
    Workspace,
}

/// `config/<domain>/create` — `item` est le type de domaine snake_case tel quel.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigCreateParams {
    #[serde(default)]
    pub layer: Option<ConfigLayer>,
    pub item: serde_json::Value,
}

/// `config/<domain>/update` — `patch` objet partiel (clé absente = inchangée,
/// présente = remplacée, null = efface un optionnel), transmis au store sans
/// inspection côté handler.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdateParams {
    #[serde(default)]
    pub layer: Option<ConfigLayer>,
    pub name: String,
    pub patch: serde_json::Value,
}

/// `config/<domain>/delete`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDeleteParams {
    #[serde(default)]
    pub layer: Option<ConfigLayer>,
    pub name: String,
}

/// `config/skills/create` — `item` est le `SkillMeta` {name, description} ;
/// `body` est le corps du SKILL.md (hors frontmatter), séparé de l'item.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigCreateSkillParams {
    #[serde(default)]
    pub layer: Option<ConfigLayer>,
    pub item: serde_json::Value,
    pub body: String,
}
