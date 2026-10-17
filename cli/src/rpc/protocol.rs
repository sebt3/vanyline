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
/// `owner` (UN OWNER = UN NAMESPACE, 2026-10-04) : namespace cible
/// `<app-ns>-<owner>` pour les méthodes project/sandbox (requis pour
/// celles-ci, absent des `owners/*`). Optionnel côté désérialisation pour
/// ne pas casser l'enveloppe des `owners/*` ; les handlers project/sandbox
/// le refusent manquant en `VNL-RPC-000`.
#[derive(Debug, Deserialize)]
pub struct NameParams {
    pub name: String,
    #[serde(default)]
    pub owner: Option<String>,
}

/// Params des méthodes de liste scopées par Owner (`projects/list`,
/// `sandboxes/list`) : la liste est celle du namespace de l'Owner
/// (`<app-ns>-<owner>`, UN OWNER = UN NAMESPACE 2026-10-04).
#[derive(Debug, Deserialize)]
pub struct OwnerParams {
    pub owner: String,
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
/// `owner` (UN OWNER = UN NAMESPACE, 2026-10-04) : namespace cible
/// `<app-ns>-<owner>` où poser la CR, requis (le `SandboxSpec` aplati ne porte
/// pas l'Owner, contrairement au `ProjectSpec`).
#[derive(Debug, Deserialize)]
pub struct SandboxCreateParams {
    pub name: String,
    #[serde(default)]
    pub owner: Option<String>,
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

/// `config/<domain>/create` — `item` est le type de domaine `snake_case` tel quel.
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

/// Params de `config/skills/get` — résolution par nom dans le store FUSIONNÉ
/// (pas de `layer` : la lecture voit la config fusionnée, comme les actions
/// `test`). Le body n'est exposé nulle part ailleurs (`config/skills` = index
/// léger uniquement).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigGetSkillParams {
    pub name: String,
}

/// Params de `config/providers/test` et `config/mcpServers/test` — l'entrée est
/// résolue dans le store fusionné par nom (pas de `layer` : on teste ce que la
/// lecture verrait).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigTestParams {
    pub name: String,
}

/// Résultat de `config/providers/test`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestResult {
    pub models: Vec<String>,
}

/// Résultat de `config/mcpServers/test`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTestResult {
    pub tools: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ALL_VNL_CODES: [&str; 16] = [
        vnl_code::MALFORMED_REQUEST,
        vnl_code::NOT_INITIALIZED,
        vnl_code::BUSY,
        vnl_code::UNKNOWN_PROTOCOL_VERSION,
        vnl_code::METHOD_NOT_FOUND,
        vnl_code::CONVERSATION_NOT_FOUND,
        vnl_code::CONFIG_ERROR,
        vnl_code::CONVERSATION_STORAGE_ERROR,
        vnl_code::NO_AGENT_RESOLVED,
        vnl_code::TURN_EXECUTION_ERROR,
        vnl_code::K8S_ERROR,
        vnl_code::CONFIG_WRITE_ERROR,
        vnl_code::CONFIG_NOT_FOUND,
        vnl_code::CONFIG_NAME_CONFLICT,
        vnl_code::CONFIG_INVALID_NAME,
        vnl_code::CONFIG_VALIDATION,
    ];

    // Scenario: enveloppe_requete_champs_requis_et_defauts
    #[test]
    fn enveloppe_requete_champs_requis_et_defauts() {
        let ok1: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"method":"m","params":{"a":1}}"#)
                .unwrap();
        assert_eq!(ok1.id, Some(json!(7)));
        assert_eq!(ok1.params, json!({"a": 1}));
        let ok2: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"m"}"#).unwrap();
        assert_eq!(ok2.id, None);
        assert_eq!(ok2.params, serde_json::Value::Null);
        let ok3: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"method":"m"}"#).unwrap();
        assert_eq!(ok3.id, Some(json!(7)));
        assert_eq!(ok3.params, serde_json::Value::Null);
        let e1 = serde_json::from_str::<JsonRpcRequest>(r#"{"id":7,"method":"m"}"#).unwrap_err();
        assert!(e1.to_string().contains("missing field `jsonrpc`"), "{e1}");
        let e2 = serde_json::from_str::<JsonRpcRequest>(r#"{"jsonrpc":"2.0","id":7}"#).unwrap_err();
        assert!(e2.to_string().contains("missing field `method`"), "{e2}");
    }

    // Scenario: enveloppe_requete_ne_valide_ni_la_valeur_de_jsonrpc_ni_la_forme_de_id
    #[test]
    fn enveloppe_requete_ne_valide_ni_la_valeur_de_jsonrpc_ni_la_forme_de_id() {
        let req: JsonRpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"1.0","id":{"a":[1]},"method":"m"}"#).unwrap();
        assert_eq!(req.jsonrpc, "1.0");
        assert_eq!(req.id, Some(json!({"a": [1]})));
        let resp = JsonRpcResponse::success(json!({"a": [1]}), json!({}));
        let wire = serde_json::to_string(&resp).unwrap();
        assert!(wire.contains(r#""id":{"a":[1]}"#), "{wire}");
    }

    // Scenario: enveloppe_reponse_success_xor_error_et_result_null_sur_le_fil
    #[test]
    fn enveloppe_reponse_success_xor_error_et_result_null_sur_le_fil() {
        let ok =
            serde_json::to_value(JsonRpcResponse::success(json!(1), json!({"ok": true}))).unwrap();
        let obj = ok.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["id", "jsonrpc", "result"]);
        assert_eq!(obj["jsonrpc"], "2.0");
        assert_eq!(obj["id"], json!(1));

        let null_result =
            serde_json::to_string(&JsonRpcResponse::success(json!(3), serde_json::Value::Null))
                .unwrap();
        assert!(null_result.contains(r#""result":null"#), "{null_result}");

        let err = serde_json::to_value(JsonRpcResponse::error(
            json!(2),
            jsonrpc_code::SERVER_ERROR,
            "boom",
            vnl_code::CONFIG_ERROR,
        ))
        .unwrap();
        let eobj = err.as_object().unwrap();
        assert!(!eobj.contains_key("result"));
        assert!(eobj.contains_key("error"));
        assert_eq!(err["jsonrpc"], "2.0");
        assert_eq!(err["id"], json!(2));
    }

    // Scenario: objet_erreur_porte_toujours_data_code_et_codes_jsonrpc_reserves
    #[test]
    fn objet_erreur_porte_toujours_data_code_et_codes_jsonrpc_reserves() {
        let cases = [
            (jsonrpc_code::PARSE_ERROR, -32700),
            (jsonrpc_code::METHOD_NOT_FOUND, -32601),
            (jsonrpc_code::SERVER_ERROR, -32000),
        ];
        for (code, expected) in cases {
            let v = serde_json::to_value(JsonRpcResponse::error(
                json!(1),
                code,
                "m",
                vnl_code::MALFORMED_REQUEST,
            ))
            .unwrap();
            assert_eq!(v["error"]["code"], json!(expected));
            assert_eq!(v["error"]["data"]["code"], json!("VNL-RPC-000"));
        }
        for code in ALL_VNL_CODES {
            let v = serde_json::to_value(JsonRpcResponse::error(
                json!(1),
                jsonrpc_code::SERVER_ERROR,
                "m",
                code,
            ))
            .unwrap();
            assert_eq!(v["error"]["data"]["code"], json!(code));
        }
    }

    // Scenario: initialize_params_version_requise_workspace_optionnel
    #[test]
    fn initialize_params_version_requise_workspace_optionnel() {
        let ok1: InitializeParams = serde_json::from_str(r#"{"protocolVersion":1}"#).unwrap();
        assert_eq!(ok1.protocol_version, 1);
        assert_eq!(ok1.workspace, None);
        let ok2: InitializeParams =
            serde_json::from_str(r#"{"protocolVersion":1,"workspace":"/home/dev/projet"}"#)
                .unwrap();
        assert_eq!(ok2.workspace.as_deref(), Some("/home/dev/projet"));
        for bad in [
            "{}",
            r#"{"protocolVersion":"1"}"#,
            r#"{"protocolVersion":-1}"#,
            r#"{"protocolVersion":1,"workspace":42}"#,
        ] {
            assert!(
                serde_json::from_str::<InitializeParams>(bad).is_err(),
                "{bad}"
            );
        }
    }

    // Scenario: initialize_result_omet_les_optionnels_none
    #[test]
    fn initialize_result_omet_les_optionnels_none() {
        let a = serde_json::to_value(InitializeResult {
            protocol_version: 1,
            server_version: "0.0.1".to_string(),
            workspace_root: None,
            default_agent: Some("build".to_string()),
        })
        .unwrap();
        assert_eq!(a["protocolVersion"], json!(1));
        assert_eq!(a["serverVersion"], json!("0.0.1"));
        assert_eq!(a["defaultAgent"], json!("build"));
        assert!(a.get("workspaceRoot").is_none());

        let b = serde_json::to_value(InitializeResult {
            protocol_version: 1,
            server_version: "0.0.1".to_string(),
            workspace_root: Some("/tmp/ws".to_string()),
            default_agent: None,
        })
        .unwrap();
        assert_eq!(b["workspaceRoot"], json!("/tmp/ws"));
        assert!(b.get("defaultAgent").is_none());
    }

    // Scenario: conversation_summary_depuis_conversation
    #[test]
    fn conversation_summary_depuis_conversation() {
        let conv: vanyline_lib::Conversation = serde_json::from_value(json!({
            "id": "00000000-0000-0000-0000-000000000000",
            "agent": null,
            "title": "Refactor auth",
            "messages": [
                {"role": "user", "content": "a"},
                {"role": "assistant", "content": "b"}
            ],
            "todo": null
        }))
        .unwrap();
        let summary = serde_json::to_value(ConversationSummary::from(&conv)).unwrap();
        assert_eq!(summary["id"], json!("00000000-0000-0000-0000-000000000000"));
        assert_eq!(summary["messageCount"], json!(2));
        assert_eq!(summary["title"], json!("Refactor auth"));
        assert!(summary.get("agent").is_none());
        assert!(summary.get("todo").is_none());
        assert!(summary.get("messages").is_none());
    }

    // Scenario: notification_chat_event_sans_id_et_params_camelcase
    #[test]
    fn notification_chat_event_sans_id_et_params_camelcase() {
        let n = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "chat/event",
            params: ChatEventNotificationParams {
                conversation_id: "c1".to_string(),
                seq: 0,
                event: vanyline_lib::event::ChatEvent::Token {
                    content: "Bon".to_string(),
                },
            },
        };
        let v = serde_json::to_value(&n).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["jsonrpc", "method", "params"]);
        assert_eq!(
            v["params"],
            json!({"conversationId": "c1", "seq": 0, "event": {"type": "token", "content": "Bon"}})
        );
    }

    // Scenario: chat_send_params_agent_defaut_et_result_toolcalls_toujours_present
    #[test]
    fn chat_send_params_agent_defaut_et_result_toolcalls_toujours_present() {
        let a: ChatSendParams =
            serde_json::from_str(r#"{"conversationId":"c1","message":"hi"}"#).unwrap();
        assert_eq!(a.agent, None);
        let b: ChatSendParams =
            serde_json::from_str(r#"{"conversationId":"c1","message":"hi","agent":"build"}"#)
                .unwrap();
        assert_eq!(b.agent.as_deref(), Some("build"));
        let c = serde_json::from_str::<ChatSendParams>(r#"{"message":"hi"}"#).unwrap_err();
        assert!(
            c.to_string().contains("missing field `conversationId`"),
            "{c}"
        );
        let res = serde_json::to_value(ChatSendResult {
            text: "t".to_string(),
            tool_calls: Vec::new(),
        })
        .unwrap();
        assert_eq!(res, json!({"text": "t", "toolCalls": []}));
    }

    // Scenario: params_identifiant_et_nom_une_seule_cle_requise_non_validee
    #[test]
    fn params_identifiant_et_nom_une_seule_cle_requise_non_validee() {
        // L'UUID malformé passe le type : la validation est dans handlers.rs.
        assert!(serde_json::from_str::<ConversationIdParams>(r#"{"id":"not-a-uuid"}"#).is_ok());
        assert!(serde_json::from_str::<ConversationIdParams>("{}").is_err());
        assert!(serde_json::from_str::<ConfigGetSkillParams>(r#"{"name":"x"}"#).is_ok());
        assert!(serde_json::from_str::<ConfigGetSkillParams>("{}").is_err());
        assert!(serde_json::from_str::<ConfigTestParams>(r#"{"name":"x"}"#).is_ok());
        assert!(serde_json::from_str::<ConfigTestParams>("{}").is_err());
        // Clé surnuméraire `layer` ignorée silencieusement (arbitrage [?] protocol.sdd).
        assert!(
            serde_json::from_str::<ConfigTestParams>(r#"{"name":"x","layer":"global"}"#).is_ok()
        );
    }

    // Scenario: create_params_k8s_spec_aplatie_camelcase_et_champs_requis
    #[test]
    fn create_params_k8s_spec_aplatie_camelcase_et_champs_requis() {
        let owner: OwnerCreateParams =
            serde_json::from_str(r#"{"name":"alice","homeSize":"2Gi"}"#).unwrap();
        assert_eq!(owner.spec.home_size.as_deref(), Some("2Gi"));
        assert!(
            serde_json::from_str::<ProjectCreateParams>(
                r#"{"name":"demo","owner":"alice","repoUrl":"https://x"}"#
            )
            .is_ok()
        );
        let no_owner =
            serde_json::from_str::<ProjectCreateParams>(r#"{"name":"demo","repoUrl":"https://x"}"#);
        assert!(no_owner.is_err());
        let sb_no_name =
            serde_json::from_str::<SandboxCreateParams>(r#"{"project":"demo","branch":"main"}"#);
        assert!(sb_no_name.is_err());
        let sb: SandboxCreateParams =
            serde_json::from_str(r#"{"name":"sb","project":"demo","branch":"main"}"#).unwrap();
        assert!(sb.spec.toolchains.is_empty());
        assert!(!sb.spec.suspended);
    }

    // Scenario: create_params_cle_spec_imbriquee_silencieusement_ignoree
    #[test]
    fn create_params_cle_spec_imbriquee_silencieusement_ignoree() {
        let p: OwnerCreateParams =
            serde_json::from_str(r#"{"name":"a","spec":{"homeSize":"2Gi"},"inconnu":42}"#).unwrap();
        assert_eq!(p.spec.home_size, None);
    }

    // Scenario: config_layer_accepte_uniquement_les_litteraux_minuscules
    #[test]
    fn config_layer_accepte_uniquement_les_litteraux_minuscules() {
        let parse = |layer: &str| {
            serde_json::from_str::<ConfigDeleteParams>(&format!(
                r#"{{"layer":{layer},"name":"x"}}"#
            ))
        };
        assert!(parse(r#""global""#).is_ok());
        assert!(parse(r#""workspace""#).is_ok());
        assert!(parse("null").is_ok_and(|p| p.layer.is_none()));
        assert!(parse(r#""Global""#).is_err());
        assert!(parse(r#""WORKSPACE""#).is_err());
        assert!(parse(r#""""#).is_err());
    }

    // Scenario: config_params_layer_defaut_contenus_requis_non_inspectes
    #[test]
    fn config_params_layer_defaut_contenus_requis_non_inspectes() {
        let a: ConfigCreateParams = serde_json::from_str(r#"{"item":{"name":"p"}}"#).unwrap();
        assert!(a.layer.is_none());
        assert_eq!(a.item, json!({"name": "p"}));
        let b: ConfigUpdateParams =
            serde_json::from_str(r#"{"name":"n","patch":{"endpoint":"http://x"}}"#).unwrap();
        assert!(b.layer.is_none());
        assert_eq!(b.patch, json!({"endpoint": "http://x"}));
        assert!(serde_json::from_str::<ConfigCreateParams>(r#"{"name":"n"}"#).is_err());
        assert!(serde_json::from_str::<ConfigUpdateParams>(r#"{"patch":{}}"#).is_err());
        let null_item: ConfigCreateParams =
            serde_json::from_str(r#"{"layer":"global","item":null}"#).unwrap();
        assert_eq!(null_item.item, serde_json::Value::Null);
        assert!(
            serde_json::from_str::<ConfigCreateParams>(r#"{"layer":"inexistant","item":{}}"#)
                .is_err()
        );
    }

    // Scenario: config_skills_create_body_requis
    #[test]
    fn config_skills_create_body_requis() {
        assert!(
            serde_json::from_str::<ConfigCreateSkillParams>(
                r##"{"item":{"name":"pdf","description":"d"},"body":"# corps"}"##
            )
            .is_ok()
        );
        assert!(
            serde_json::from_str::<ConfigCreateSkillParams>(
                r#"{"item":{"name":"pdf","description":"d"}}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<ConfigCreateSkillParams>(r#"{"body":"x"}"#).is_err());
    }

    // Scenario: resultats_test_formes_fixes
    #[test]
    fn resultats_test_formes_fixes() {
        assert_eq!(
            serde_json::to_value(ProviderTestResult { models: Vec::new() }).unwrap(),
            json!({"models": []})
        );
        assert_eq!(
            serde_json::to_value(McpTestResult { tools: Vec::new() }).unwrap(),
            json!({"tools": []})
        );
    }

    // Scenario: seize_codes_vnl_rpc_uniques_dans_le_registre
    #[test]
    fn seize_codes_vnl_rpc_uniques_dans_le_registre() {
        let source = include_str!("protocol.rs");
        let production = source.split("#[cfg(test)]").next().unwrap_or_default();
        for code in ALL_VNL_CODES {
            assert_eq!(
                production.matches(&format!("\"{code}\"")).count(),
                1,
                "{code}"
            );
        }
        assert!(!production.contains("VNL-RPC-16"));
        for code in [
            jsonrpc_code::PARSE_ERROR,
            jsonrpc_code::METHOD_NOT_FOUND,
            jsonrpc_code::SERVER_ERROR,
        ] {
            assert!((-32768..=-32000).contains(&code), "{code}");
        }
    }
}
