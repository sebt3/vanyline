//! Kind `rhai` — contrat : `rhai.sdd`. Le `RhaiStep` de miryad-core (issue #27 : `with_setup`,
//! livré en 0.1.5) enrichi par exécution des trois fonctions globales `mcp_servers()`,
//! `mcp_list_tools(server)`, `mcp_call_tool(server, tool, arguments)`, branchées sur les
//! serveurs MCP de l'utilisateur du run (+ `sandbox` en portée sandbox seulement).
//!
//! Client MCP : JSON-RPC 2.0 « HTTP streamable » POST-only sans état — la forme réelle du
//! serveur de la sandbox (`sandbox/src/mcp.rs`) et celle contractée par le scenario
//! « faux serveur MCP axum ». Écart signalé au rapport : les serveurs MCP externes à session
//! (`Mcp-Session-Id`) ne sont pas atteignables par ce client léger ; le séam d'authentification
//! complet `prefixed_mcp::connect_domain_mcp_server_inner` est privé à `vanyline-lib`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use miryad_core::workflow::RhaiStep;
use serde_json::Value;

use super::RunInfo;
use super::SandboxLocator;
use super::StepEnv;
use crate::config_store::PgConfigStore;
use vanyline_cfgstore::store::ConfigStore;

/// Le nom de kind — celui, fixe, de `RhaiStep` (`rhai_step.rs` de miryad-core, `kind() ==
/// "rhai"`). Verrouillé par exécution dans les tests.
pub(crate) const KIND: &str = "rhai";

/// Plafond d'appels MCP par exécution de script (`rhai.sdd` `Must`).
const MAX_MCP_CALLS: usize = 200;
/// Borne de chaque appel MCP (`rhai.sdd` `Must`).
const MCP_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Un serveur MCP joignable par le script.
#[derive(Clone, Debug)]
pub(crate) struct BridgeServer {
    pub name: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// Contexte partagé des fonctions Rhai d'une exécution.
#[derive(Clone)]
pub(crate) struct McpBridge {
    pub http: reqwest::Client,
    pub handle: tokio::runtime::Handle,
    /// Serveurs visibles par le script (utilisateur + sandbox si portée sandbox) — résolus une
    /// fois par exécution à l'usage (`Must` : aucun état partagé entre exécutions).
    pub servers: Arc<std::sync::OnceLock<Result<Vec<BridgeServer>, String>>>,
    list: Arc<dyn Fn() -> Result<Vec<BridgeServer>, String> + Send + Sync>,
    pub calls: Arc<AtomicUsize>,
}

impl McpBridge {
    /// Résout la liste des serveurs au premier usage, la conserve pour l'exécution.
    fn servers(&self) -> Result<Vec<BridgeServer>, String> {
        self.servers
            .get_or_init(|| (self.list)())
            .clone()
            .map_err(|e| format!("serveurs MCP indisponibles : {e}"))
    }

    /// Garde-fou du plafond d'appels : vrai tant que le quota reste ouvert.
    fn take_call(&self) -> Result<(), String> {
        if self.calls.fetch_add(1, Ordering::SeqCst) >= MAX_MCP_CALLS {
            Err(format!("plafond d'appels MCP ({MAX_MCP_CALLS}) atteint"))
        } else {
            Ok(())
        }
    }
}

/// Composition de la liste visible : les serveurs de l'utilisateur, puis `sandbox` seulement
/// quand le run est de portée sandbox (`rhai.sdd` `Handles`).
#[must_use]
pub(crate) fn plan_servers(
    mut user: Vec<BridgeServer>,
    sandbox: Option<BridgeServer>,
) -> Vec<BridgeServer> {
    if let Some(sbx) = sandbox {
        user.push(sbx);
    }
    user
}

impl McpBridge {
    /// Le serveur nommé, ou l'erreur contractuelle sans distinction (« n'existe pas » vs
    /// « interdit » : même message, aucune énumération d'un serveur hors portée).
    fn find(&self, name: &str) -> Result<BridgeServer, String> {
        self.servers()?
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("serveur MCP inconnu : « {name} »"))
    }

    /// Un appel JSON-RPC 2.0 (POST unique, sans état — la forme réelle de `sandbox/src/mcp.rs`).
    /// Borne : `MCP_CALL_TIMEOUT`. Jamais le contenu des en-têtes dans les erreurs.
    async fn rpc(&self, srv: &BridgeServer, method: &str, params: Value) -> Result<Value, String> {
        let mut req = self
            .http
            .post(&srv.url)
            .timeout(MCP_CALL_TIMEOUT)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }));
        for (k, v) in &srv.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("serveur MCP « {} » injoignable : {e}", srv.name))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!(
                "serveur MCP « {} » : HTTP {} ({} en-tête d'authentification)",
                srv.name,
                status,
                if srv
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                {
                    "avec"
                } else {
                    "sans"
                }
            ));
        }
        let envelope: Value = resp
            .json()
            .await
            .map_err(|e| format!("enveloppe JSON-RPC invalide de « {} » : {e}", srv.name))?;
        if let Some(err) = envelope.get("error") {
            return Err(format!("erreur MCP de « {} » : {err}", srv.name));
        }
        envelope
            .get("result")
            .cloned()
            .ok_or_else(|| format!("enveloppe sans `result` de « {} »", srv.name))
    }
}

/// Rend les trois fonctions globales du pont MCP, enregistrables telles quelles : chaque
/// fermeture ne fait que décliner le pont en erreurs Rhai attrapables (`rhai.sdd` `Exposes`).
/// Le pont est appelant-bloquant : le script tourne déjà sur un thread `spawn_blocking`
/// (pool de `RhaiStep`), le `Handle` de la boucle tokio pilote les appels reqwest.
pub(crate) fn register_mcp_fns(engine: &mut rhai::Engine, bridge: &McpBridge) {
    let err = |m: String| -> Box<rhai::EvalAltResult> { m.into() };
    {
        let b = bridge.clone();
        engine.register_fn(
            "mcp_servers",
            move || -> Result<rhai::Array, Box<rhai::EvalAltResult>> {
                b.server_names().map_err(err)
            },
        );
    }
    {
        let b = bridge.clone();
        engine.register_fn(
            "mcp_list_tools",
            move |server: rhai::ImmutableString| -> Result<rhai::Array, Box<rhai::EvalAltResult>> {
                b.list_tools(server.as_str()).map_err(err)
            },
        );
    }
    {
        let b = bridge.clone();
        engine.register_fn(
            "mcp_call_tool",
            move |server: rhai::ImmutableString,
                  tool: rhai::ImmutableString,
                  args: rhai::Map|
                  -> Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
                b.call_tool(server.as_str(), tool.as_str(), args)
                    .map_err(err)
            },
        );
    }
}

impl McpBridge {
    /// Les noms des serveurs visibles (`mcp_servers()` côté script).
    fn server_names(&self) -> Result<rhai::Array, String> {
        Ok(self
            .servers()?
            .into_iter()
            .map(|s| rhai::Dynamic::from(s.name))
            .collect())
    }

    /// `tools/list` sur le serveur nommé, en array de maps `{name, description, input_schema}`.
    fn list_tools(&self, server: &str) -> Result<rhai::Array, String> {
        let srv = self.find(server)?;
        self.take_call()?;
        let result = self
            .handle
            .block_on(self.rpc(&srv, "tools/list", serde_json::json!({})))?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(tools
            .into_iter()
            .map(|t| {
                let mut m = rhai::Map::new();
                m.insert(
                    "name".into(),
                    json_to_dynamic(&t.get("name").cloned().unwrap_or(Value::Null)),
                );
                m.insert(
                    "description".into(),
                    json_to_dynamic(&t.get("description").cloned().unwrap_or(Value::Null)),
                );
                m.insert(
                    "input_schema".into(),
                    json_to_dynamic(&t.get("inputSchema").cloned().unwrap_or(Value::Null)),
                );
                rhai::Dynamic::from(m)
            })
            .collect())
    }

    /// `tools/call` sur le serveur nommé : le texte du résultat est rendu en map Rhai s'il est
    /// JSON, en chaîne sinon ; `isError` lève l'erreur attrapable par le script.
    fn call_tool(
        &self,
        server: &str,
        tool: &str,
        args: rhai::Map,
    ) -> Result<rhai::Dynamic, String> {
        let srv = self.find(server)?;
        self.take_call()?;
        let arguments = dynamic_to_json(&rhai::Dynamic::from(args));
        let result = self.handle.block_on(self.rpc(
            &srv,
            "tools/call",
            serde_json::json!({ "name": tool, "arguments": arguments }),
        ))?;
        let text: String = result
            .get("content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| {
                        if p.get("type").and_then(Value::as_str) == Some("text") {
                            p.get("text").and_then(Value::as_str)
                        } else {
                            None
                        }
                    })
                    .collect::<String>()
            })
            .unwrap_or_default();
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(format!("erreur d'outil « {} » : {tool}", srv.name));
        }
        // Un résultat JSON est converti en map Rhai, jamais rendu comme une chaîne.
        Ok(serde_json::from_str::<Value>(&text)
            .map_or_else(|_| rhai::Dynamic::from(text), |v| json_to_dynamic(&v)))
    }
}
/// Le `RhaiStep` de miryad-core enrichi (`rhai.sdd` `Exposes`) : les serveurs MCP visibles sont
/// ceux de l'utilisateur du run (`PgConfigStore` borné), plus `sandbox` en portée sandbox —
/// résolus une fois par exécution au premier usage des fonctions MCP.
pub fn rhai_step(env: StepEnv) -> RhaiStep {
    RhaiStep::default().with_setup(move |script, config, _inputs| {
        let Ok(run) = RunInfo::from_config(config) else {
            tracing::debug!("step rhai : pas de contexte `_run`, fonctions MCP non enregistrées");
            return;
        };
        let handle = tokio::runtime::Handle::current();
        let http = env.http.clone();
        let db = env.db.clone();
        let locator = Arc::new(super::K8sLocator::new(env.k8s.clone()));
        let owner = run.owner.clone();
        let sandbox = run.sandbox.clone();
        let user_id = run.user_id;
        let list = move || {
            let http_store = PgConfigStore::new(db.clone(), user_id);
            let locator = locator.clone();
            let owner = owner.clone();
            let sandbox = sandbox.clone();
            handle.block_on(async move {
                let user = http_store
                    .list_mcp_servers()
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|s| BridgeServer {
                        name: s.name,
                        url: s.url,
                        headers: s.headers.into_iter().collect(),
                    })
                    .collect();
                let sbx = match &sandbox {
                    Some(name) => {
                        let token = locator.token(&owner).await.map_err(|e| e.message)?;
                        let base = locator
                            .base_url(&owner, name)
                            .await
                            .map_err(|e| e.message)?;
                        Some(BridgeServer {
                            name: "sandbox".to_string(),
                            url: format!("{base}/mcp"),
                            headers: vec![("Authorization".to_string(), format!("Bearer {token}"))],
                        })
                    }
                    None => None,
                };
                Ok(plan_servers(user, sbx))
            })
        };
        let bridge = McpBridge {
            http,
            handle: tokio::runtime::Handle::current(),
            servers: Arc::new(std::sync::OnceLock::new()),
            list: Arc::new(list),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        register_mcp_fns(&mut script.engine, &bridge);
    })
}

/// Conversion JSON → `rhai::Dynamic` (maps/tableaux Rhai, pas de chaîne JSON).
pub(crate) fn json_to_dynamic(value: &Value) -> rhai::Dynamic {
    match value {
        Value::Null => rhai::Dynamic::UNIT,
        Value::Bool(b) => rhai::Dynamic::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rhai::Dynamic::from(i)
            } else {
                rhai::Dynamic::from(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => rhai::Dynamic::from(s.clone()),
        Value::Array(items) => {
            rhai::Dynamic::from(items.iter().map(json_to_dynamic).collect::<rhai::Array>())
        }
        Value::Object(map) => {
            let mut m = rhai::Map::new();
            for (k, v) in map {
                m.insert(k.as_str().into(), json_to_dynamic(v));
            }
            rhai::Dynamic::from(m)
        }
    }
}

/// Conversion `rhai::Dynamic` → JSON (arguments `#{…}` des scripts).
pub(crate) fn dynamic_to_json(value: &rhai::Dynamic) -> Value {
    if value.is_unit() {
        return Value::Null;
    }
    if value.is_bool() {
        return Value::Bool(value.as_bool().unwrap_or(false));
    }
    if value.is_int() {
        return Value::from(value.as_int().unwrap_or(0));
    }
    if value.is_float() {
        return serde_json::Number::from_f64(value.as_float().unwrap_or(0.0))
            .map_or(Value::Null, Value::Number);
    }
    if value.is_array() {
        let arr = value.clone().cast::<rhai::Array>();
        return Value::Array(arr.iter().map(dynamic_to_json).collect());
    }
    if value.is_map() {
        let map = value.clone().cast::<rhai::Map>();
        let obj: serde_json::Map<String, Value> = map
            .into_iter()
            .map(|(k, v)| (k.to_string(), dynamic_to_json(&v)))
            .collect();
        return Value::Object(obj);
    }
    Value::String(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use miryad_core::workflow::MiryadWorkflowStep;
    use miryad_core::workflow::RunInfo as EngineRunInfo;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    /// Faux serveur MCP : JSON-RPC 2.0 POST-only (`sandbox/src/mcp.rs`, `tools/src/mcp.rs`).
    /// `tools/list` rend `ping`/`echo`/`boom` ; `echo` renvoie son argument ; `boom` échoue en
    /// `isError`. Enregistre les derniers en-têtes `Authorization` reçus.
    async fn spawn_mcp_fake(auth_seen: Arc<Mutex<Vec<String>>>) -> String {
        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
                let auth_seen = auth_seen.clone();
                async move {
                    if let Some(v) = headers.get(axum::http::header::AUTHORIZATION) {
                        auth_seen
                            .lock()
                            .unwrap()
                            .push(v.to_str().unwrap_or_default().to_string());
                    }
                    let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let id = req.get("id").cloned().unwrap_or(Value::Null);
                    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
                    let result: Value = match method {
                        "initialize" => json!({
                            "protocolVersion": "2024-11-05",
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "fake", "version": "0"},
                        }),
                        "tools/list" => json!({"tools": [
                            {"name": "ping", "description": "renvoie pong",
                             "inputSchema": {"type": "object", "properties": {}}},
                            {"name": "echo", "description": "rend text",
                             "inputSchema": {"type": "object",
                              "properties": {"text": {"type": "string"}}}},
                            {"name": "boom", "description": "échoue toujours",
                             "inputSchema": {"type": "object"}},
                        ]}),
                        "tools/call" => {
                            let name = req
                                .pointer("/params/name")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            if name == "boom" {
                                json!({"isError": true,
                                       "content": [{"type": "text", "text": "l'outil a échoué"}]})
                            } else {
                                let text = req
                                    .pointer("/params/arguments/text")
                                    .and_then(Value::as_str)
                                    .unwrap_or("(rien)");
                                json!({"isError": false,
                                       "content": [{"type": "text", "text": text}]})
                            }
                        }
                        _ => return StatusCode::NOT_FOUND.into_response(),
                    };
                    let envelope = json!({"jsonrpc": "2.0", "id": id, "result": result});
                    axum::Json(envelope).into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}/mcp")
    }

    fn bridge_for(urls: Vec<(&str, &str)>, sandbox: Option<(&str, &str)>) -> McpBridge {
        let servers: Vec<BridgeServer> = urls
            .into_iter()
            .chain(sandbox)
            .map(|(name, url)| BridgeServer {
                name: name.to_string(),
                url: url.to_string(),
                headers: if name == "sandbox" {
                    vec![(
                        "Authorization".to_string(),
                        "Bearer jeton-owner-rhai".to_string(),
                    )]
                } else {
                    vec![]
                },
            })
            .collect();
        let http = reqwest::Client::new();
        let handle = tokio::runtime::Handle::current();
        let list = move || Ok(servers.clone());
        McpBridge {
            http,
            handle,
            servers: Arc::new(std::sync::OnceLock::new()),
            list: Arc::new(list),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn engine(step_id: &str) -> EngineRunInfo {
        EngineRunInfo {
            run_key: "rk".to_string(),
            step_id: step_id.to_string(),
            depth: 0,
        }
    }

    async fn run_script(bridge: McpBridge, script: &str) -> Result<Value, String> {
        let step = RhaiStep::default().with_setup(move |s, _config, _inputs| {
            register_mcp_fns(&mut s.engine, &bridge);
        });
        step.run(&engine("r"), json!({"script": script}), HashMap::new())
            .await
            .map_err(|e| e.message)
    }

    // ── Scenario: lister_les_serveurs ───────────────────────────────────────
    // Deux serveurs configurés ⇒ l'array de leurs noms (+ `sandbox` en portée sandbox —
    // l'assertion différentielle projet/sandbox est le Scenario nommé
    // `pas_de_sandbox_en_portee_projet`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lister_les_serveurs() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_mcp_fake(auth).await;
        let bridge = bridge_for(
            vec![("test", &url), ("autre", &url)],
            Some(("sandbox", &url)),
        );
        let out = run_script(bridge, "#{ names: mcp_servers() }")
            .await
            .expect("mcp_servers() répond");
        assert_eq!(out["names"], json!(["test", "autre", "sandbox"]));
    }

    // ── Scenario: pas_de_sandbox_en_portee_projet ───────────────────────────
    // Même plan global contenant un serveur nommé `sandbox` : deux runs, l'un de portée
    // sandbox, l'autre de portée projet (`bridge_for` = résolution que ferait `rhai_step`
    // selon `_run.sandbox`). Assertion DIFFÉRENTIELLE : `sandbox` visible seulement avec la
    // sandbox, jamais en portée projet.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pas_de_sandbox_en_portee_projet() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_mcp_fake(auth).await;
        let with = run_script(
            bridge_for(vec![("test", &url)], Some(("sandbox", &url))),
            "#{ names: mcp_servers() }",
        )
        .await
        .expect("run de portée sandbox");
        let without = run_script(
            bridge_for(vec![("test", &url)], None),
            "#{ names: mcp_servers() }",
        )
        .await
        .expect("run de portée projet");
        let names = |out: &Value| out["names"].as_array().cloned().unwrap_or_default();
        assert!(
            names(&with).iter().any(|n| n == "sandbox"),
            "`sandbox` visible en portée sandbox : {with}"
        );
        assert!(
            !names(&without).iter().any(|n| n == "sandbox"),
            "`sandbox` jamais en portée projet : {without}"
        );
        assert_eq!(
            names(&with).len(),
            names(&without).len() + 1,
            "même plan : seul l'apport de la sandbox distingue les deux vues"
        );
        // Appoint `plan_servers` (même contrat, sans moteur) : sans entrée sandbox, la liste
        // ne peut pas en contenir une.
        let projet = plan_servers(
            vec![BridgeServer {
                name: "test".to_string(),
                url: url.clone(),
                headers: vec![],
            }],
            None,
        );
        assert!(!projet.iter().any(|s| s.name == "sandbox"));
    }

    // ── Scenario: lister_les_outils ─────────────────────────────────────────
    // `mcp_list_tools("test")` ⇒ array de maps `{name, description, input_schema}`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lister_les_outils() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_mcp_fake(auth).await;
        let bridge = bridge_for(vec![("test", &url)], None);
        let out = run_script(bridge, r#"#{ tools: mcp_list_tools("test") }"#)
            .await
            .expect("mcp_list_tools répond");
        let tools = out["tools"].as_array().expect("un array");
        let ping = tools
            .iter()
            .find(|t| t["name"] == "ping")
            .expect("ping présent");
        assert_eq!(ping["description"], "renvoie pong");
        assert_eq!(ping["input_schema"]["type"], "object");
    }

    // ── Scenario: appeler_un_outil ──────────────────────────────────────────
    // `mcp_call_tool("test", "echo", #{ text: "hi" })` ⇒ le résultat contient `hi`. Le serveur
    // de test ne voit JAMAIS le jeton d'un autre serveur (le Bearer est posé sur `sandbox`
    // seulement, vérifié plus bas).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn appeler_un_outil() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_mcp_fake(auth.clone()).await;
        let bridge = bridge_for(vec![("test", &url)], None);
        let out = run_script(
            bridge,
            "#{ out: mcp_call_tool(\"test\", \"echo\", #{ text: \"hi\" }) }",
        )
        .await
        .expect("echo rend son argument");
        assert!(
            out["out"].as_str().is_some_and(|t| t.contains("hi")),
            "résultat : {out}"
        );
        assert!(
            auth.lock().unwrap().is_empty(),
            "un serveur sans jeton n'en reçoit pas"
        );
    }

    // Appoint `Exposes` : un résultat JSON est converti en map Rhai (pas une chaîne).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resultat_json_converti_en_map() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        // `echo` rend le texte reçu : on lui passe du JSON en texte, il doit être converti.
        let url = spawn_mcp_fake(auth).await;
        let bridge = bridge_for(vec![("test", &url)], None);
        let out = run_script(
            bridge,
            r#"#{ v: mcp_call_tool("test", "echo", #{ text: "{\"a\":1}" }) }"#,
        )
        .await
        .expect("JSON converti");
        assert_eq!(out["v"]["a"], 1, "converti en map, pas en chaîne : {out}");
    }

    // ── Scenario: erreur_d_outil_attrapable ─────────────────────────────────
    // Une erreur d'outil lève une erreur Rhai que le script peut attraper : le step réussit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn erreur_d_outil_attrapable() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_mcp_fake(auth).await;
        let bridge = bridge_for(vec![("test", &url)], None);
        let out = run_script(
            bridge,
            r#"let msg = "rien";
               try { mcp_call_tool("test", "boom", #{}); }
               catch (e) { msg = e; }
               #{ caught: msg != "rien" }"#,
        )
        .await
        .expect("l'erreur est attrapée par le script");
        assert_eq!(out["caught"], true);
    }

    // ── Scenario: serveur_inconnu ───────────────────────────────────────────
    // Nom absent de la liste ⇒ erreur Rhai « serveur MCP inconnu », sans distinguer
    // « n'existe pas » de « interdit ».
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serveur_inconnu() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_mcp_fake(auth).await;
        let bridge = bridge_for(vec![("test", &url)], None);
        let err = run_script(
            bridge,
            "#{ x: mcp_call_tool(\"fantôme\", \"echo\", #{ text: \"hi\" }) }",
        )
        .await
        .expect_err("serveur inconnu = erreur Rhai");
        assert!(
            err.contains("serveur MCP inconnu"),
            "message contractuel sans distinction : {err}"
        );
        let err2 = run_script(
            bridge_for(vec![("test", &url)], None),
            "#{ x: mcp_list_tools(\"fantôme\") }",
        )
        .await
        .expect_err("list_tools aussi refusé");
        assert!(err2.contains("serveur MCP inconnu"), "{err2}");
    }

    // ── Scenario: plafond_d_appels ──────────────────────────────────────────
    // 201 appels ⇒ erreur au 201e.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plafond_d_appels() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_mcp_fake(auth).await;
        let bridge = bridge_for(vec![("test", &url)], None);
        let err = run_script(
            bridge,
            r#"let i = 0;
               while i < 201 { mcp_call_tool("test", "echo", #{ text: "x" }); i += 1; }
               #{ done: true }"#,
        )
        .await
        .expect_err("le 201e appel doit échouer");
        assert!(err.contains("plafond"), "erreur de plafond : {err}");

        // Appoint : 200 appels passent (le compteur démarre après le premier refus).
        let ok = run_script(
            bridge_for(vec![("test", &url)], None),
            r#"let i = 0;
               while i < 200 { mcp_call_tool("test", "echo", #{ text: "x" }); i += 1; }
               #{ done: true }"#,
        )
        .await
        .expect("200 appels sont permis");
        assert_eq!(ok["done"], true);
    }

    // Appoint `Must` « ne jamais exposer le jeton » : l'en-tête Bearer part vers le serveur
    // sandbox, jamais dans la sortie du script.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn jeton_transporte_mais_jamais_expose() {
        let auth = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_mcp_fake(auth.clone()).await;
        let bridge = bridge_for(vec![], Some(("sandbox", &url)));
        let out = run_script(
            bridge,
            "#{ v: mcp_call_tool(\"sandbox\", \"echo\", #{ text: \"via-sandbox\" }) }",
        )
        .await
        .expect("le serveur sandbox est joignable en portée sandbox");
        assert_eq!(out["v"], "via-sandbox");
        assert!(
            !out.to_string().contains("jeton-owner-rhai"),
            "le jeton n'apparaît pas dans la sortie : {out}"
        );
        assert!(
            auth.lock()
                .unwrap()
                .iter()
                .any(|h| h == "Bearer jeton-owner-rhai"),
            "le Bearer bien transmis au serveur : {:?}",
            auth.lock().unwrap()
        );
    }

    // Appoint `kind` : c'est bien le `RhaiStep` de miryad-core (kind « rhai ») et la `config`
    // entière (script + `_run`) est visible du script.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kind_rhai_via_le_rhaistep_de_miryad() {
        let step = RhaiStep::default();
        assert_eq!(MiryadWorkflowStep::kind(&step), "rhai");
        assert_eq!(KIND, "rhai");
    }

    // ── Auxiliaires purs ────────────────────────────────────────────────────
    #[test]
    fn conversions_json_dynamic() {
        use rhai::Dynamic;
        assert_eq!(dynamic_to_json(&json_to_dynamic(&json!(null))), json!(null));
        assert_eq!(dynamic_to_json(&json_to_dynamic(&json!(true))), json!(true));
        assert_eq!(dynamic_to_json(&json_to_dynamic(&json!(-7))), json!(-7));
        assert_eq!(dynamic_to_json(&json_to_dynamic(&json!(2.5))), json!(2.5));
        assert_eq!(
            dynamic_to_json(&json_to_dynamic(&json!("salut"))),
            json!("salut")
        );
        assert_eq!(
            dynamic_to_json(&json_to_dynamic(&json!([1, "deux", null]))),
            json!([1, "deux", null])
        );
        let map = json!({"a": 1, "b": {"c": [true]}});
        assert_eq!(dynamic_to_json(&json_to_dynamic(&map)), map);
        // aller simple inverse : un `#{…}` Rhai devient un objet JSON
        let mut m = rhai::Map::new();
        m.insert("k".into(), Dynamic::from(3_i64));
        assert_eq!(dynamic_to_json(&Dynamic::from(m)), json!({"k": 3}));
    }

    #[test]
    fn plan_sers_expose_plafond_et_ordre() {
        let user = vec![BridgeServer {
            name: "u".to_string(),
            url: "http://u".to_string(),
            headers: vec![],
        }];
        let sbx = BridgeServer {
            name: "sandbox".to_string(),
            url: "http://s/mcp".to_string(),
            headers: vec![("Authorization".to_string(), "Bearer t".to_string())],
        };
        let both = plan_servers(user.clone(), Some(sbx.clone()));
        assert_eq!(
            both.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["u", "sandbox"]
        );
        assert_eq!(plan_servers(user, None).len(), 1);
    }
}
