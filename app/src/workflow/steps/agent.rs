//! Kind `agent` — contrat : `agent.sdd`. Exécute un agent configuré avec un prompt gabarisé,
//! sans humain, avec les mêmes gestion d'agents, profils, toolsets et MCP que le chat
//! (`vanyline_lib::session::run_agent_turn`, même chemin d'exécution que `ws/chat.rs`).
//!
//! Écart consigné au rapport W2 : `agent.sdd` cite `vanyline_lib::autonomous::run_agent_task`
//! (spec `/lib/src/autonomous.sdd`), qui N'EST PAS IMPLÉMENTÉE (fichier neuf, conception). Ce
//! fichier porte l'équivalent minimal de cette enveloppe (collecteur + timeout + sortie
//! structurée) derrière le séam `pub(crate) AgentTaskRunner`, pour que le pas soit testable
//! sans réseau et branchable sur l'implémentation de `autonomous.rs` quand elle arrive.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use miryad_core::workflow::MiryadWorkflowStep;
use miryad_core::workflow::RunInfo as EngineRunInfo;
use miryad_core::workflow::StepError;
use serde_json::Value;
use vanyline_cfgstore::domain::McpSelection;
use vanyline_cfgstore::domain::McpServer;
use vanyline_cfgstore::store::ConfigStore;

use super::RunInfo;
use super::SandboxLocator;
use super::StepEnv;
use crate::config_store::PgConfigStore;

/// Kind de step `agent`.
pub struct AgentStep {
    env: StepEnv,
}

impl AgentStep {
    /// Le nom de kind contractuel (`agent.sdd` `Exposes`).
    pub(crate) const KIND: &'static str = "agent";

    /// Construit le kind à partir de l'environnement d'exécution.
    pub(crate) fn new(env: StepEnv) -> Self {
        Self { env }
    }
}

/// Plafond du timeout quand la sandbox est injectée : le jeton de l'Owner vit au plus 15 min
/// (`agent.sdd` `Handles`).
const TIMEOUT_CAP_WITH_SANDBOX_SECS: u64 = 780;
/// Plafond sinon.
const TIMEOUT_CAP_SECS: u64 = 3600;
/// Timeout par défaut.
const TIMEOUT_DEFAULT_SECS: u64 = 600;

/// La `config` du kind `agent`.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct AgentConfig {
    /// Nom de l'agent, résolu dans la configuration de l'utilisateur du run.
    pub agent: String,
    /// Gabarit Handlebars strict du prompt.
    pub prompt: String,
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub json: bool,
    pub max_steps: Option<usize>,
}

/// Bornes effectives transmises à l'exécution.
#[derive(Debug, Clone)]
pub(crate) struct AgentLimits {
    pub timeout: Duration,
    pub max_steps: Option<usize>,
}

/// Raison d'arrêt d'une tâche d'agent (mêmes trois états que `autonomous.sdd` `@StopReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentStop {
    Completed,
    TimedOut,
    StepLimit,
}

/// Calcul des bornes effectives : défaut 600 s, dans les DEUX portées ; PLAFONNÉ à 780 s avec
/// sandbox injectée (le jeton de l'Owner vit au plus 15 min), 3 600 s sinon (`agent.sdd`
/// `Handles` — le plafond ne change pas le défaut).
pub(crate) fn limits_for(cfg: &AgentConfig, with_sandbox: bool) -> AgentLimits {
    let cap = if with_sandbox {
        TIMEOUT_CAP_WITH_SANDBOX_SECS
    } else {
        TIMEOUT_CAP_SECS
    };
    let secs = cfg.timeout_secs.unwrap_or(TIMEOUT_DEFAULT_SECS).min(cap);
    AgentLimits {
        timeout: Duration::from_secs(secs),
        max_steps: cfg.max_steps,
    }
}

/// Résultat exploitable d'une tâche d'agent (miroir de `autonomous.sdd` `@TaskOutcome`).
#[derive(Debug, Clone)]
pub(crate) struct AgentTaskOutcome {
    pub stop: AgentStop,
    pub response_text: String,
    pub tool_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub unavailable_servers: Vec<String>,
}

/// Échec d'exécution, pré-classé : configuration (non retryable) vs fournisseur/MCP (retryable).
#[derive(Debug)]
pub(crate) enum AgentTaskFailure {
    Config(String),
    Provider(String),
}

/// Ce que le pas demande à l'exécuteur.
pub(crate) struct AgentTaskRequest {
    pub store: Arc<dyn ConfigStore>,
    pub agent: String,
    pub prompt: String,
    pub limits: AgentLimits,
    pub extra_mcp: Vec<(McpServer, McpSelection)>,
}

/// Séam `pub(crate)` d'exécution de la tâche — la production ([`LibTaskRunner`]) passe par
/// `run_agent_turn` ; les tests injectent un faux exécuteur pour verrouiller tout le pas sans
/// fournisseur réseau (couverture du vrai LLM signalée non verrouillable hors ligne).
#[async_trait::async_trait]
#[allow(clippy::double_must_use)] // async_trait pose `must_use` sur le BoxFuture déjà must_use — tooling.sdd (précédent `trait EventSink`)
pub(crate) trait AgentTaskRunner: Send + Sync {
    async fn run(&self, req: AgentTaskRequest) -> Result<AgentTaskOutcome, AgentTaskFailure>;
}

/// L'exécuteur de production : `run_agent_turn` (`lib/src/session.rs`), enveloppé du timeout et
/// du collecteur d'événements — l'équivalent de `autonomous.rs`, non livré (cf. en-tête).
pub(crate) struct LibTaskRunner;

/// Collecteur d'événements de tour : texte déjà streamé, compteurs d'usage et serveurs MCP
/// indisponibles — la production `LibTaskRunner` relit ce collecteur quand `run_agent_turn` est
/// interrompue par le timeout (arrêt non erreur, sortie partielle).
#[derive(Default)]
struct CollectingSink {
    events: std::sync::Mutex<Vec<vanyline_lib::event::ChatEvent>>,
}

#[async_trait::async_trait]
impl vanyline_lib::event::EventSink for CollectingSink {
    async fn emit(&self, event: vanyline_lib::event::ChatEvent) {
        match self.events.lock() {
            Ok(mut g) => g.push(event),
            Err(e) => tracing::warn!(error = %e, "collecteur d'événements d'agent empoisonné"),
        }
    }
}

impl CollectingSink {
    /// Totaux d'usage vus par le collecteur.
    fn usage(&self) -> (u64, u64) {
        let mut input = 0_u64;
        let mut output = 0_u64;
        if let Ok(g) = self.events.lock() {
            for ev in g.iter() {
                if let vanyline_lib::event::ChatEvent::Usage {
                    input_tokens,
                    output_tokens,
                } = ev
                {
                    input = input.saturating_add(*input_tokens);
                    output = output.saturating_add(*output_tokens);
                }
            }
        }
        (input, output)
    }

    /// Serveurs MCP déclarés indisponibles pendant le tour.
    fn unavailable(&self) -> Vec<String> {
        self.events
            .lock()
            .map(|g| {
                g.iter()
                    .filter_map(|ev| match ev {
                        vanyline_lib::event::ChatEvent::ToolUnavailable { server, .. } => {
                            Some(server.clone())
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Texte partiel streamé à ce point (deltas assistant concaténés).
    fn partial_text(&self) -> String {
        self.events
            .lock()
            .map(|g| {
                g.iter()
                    .filter_map(|ev| match ev {
                        vanyline_lib::event::ChatEvent::Token { content } => Some(content.as_str()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl AgentTaskRunner for LibTaskRunner {
    async fn run(&self, req: AgentTaskRequest) -> Result<AgentTaskOutcome, AgentTaskFailure> {
        let sink = Arc::new(CollectingSink::default());
        let ctx = vanyline_lib::session::SessionContext {
            store: req.store.clone(),
            sink: sink.clone(),
            local_tools: HashMap::new(),
            subagent_depth_max: 0,
            extra_mcp: req.extra_mcp.clone(),
            model_override: None,
            todo_state: Arc::new(std::sync::Mutex::new(None)),
        };
        let outcome = |stop: AgentStop, text: String, tool_calls: usize| AgentTaskOutcome {
            stop,
            response_text: text,
            tool_calls,
            input_tokens: sink.usage().0,
            output_tokens: sink.usage().1,
            unavailable_servers: sink.unavailable(),
        };
        // Timeout appliqué ICI (le collecteur survit à l'annulation : la sortie partielle
        // est rendue, l'arrêt n'est pas une erreur). `max_steps` sans exécuteur livré :
        // `run_agent_turn` n'a pas de limite de pas (autonomous.rs non livré) — signalé.
        let turn = tokio::time::timeout(
            req.limits.timeout,
            vanyline_lib::session::run_agent_turn(&ctx, &req.agent, Vec::new(), &req.prompt, None),
        )
        .await;
        match turn {
            Err(_elapsed) => Ok(outcome(AgentStop::TimedOut, sink.partial_text(), 0)),
            Ok(Ok(res)) => Ok(outcome(
                AgentStop::Completed,
                res.response_text,
                res.tool_calls.len(),
            )),
            Ok(Err(e)) => {
                let failure = match &e {
                    vanyline_lib::VnyError::ConfigError(_)
                    | vanyline_lib::VnyError::LlmProviderNotFound
                    | vanyline_lib::VnyError::UnknownProviderType(_)
                    | vanyline_lib::VnyError::ModelBuildError(_)
                    | vanyline_lib::VnyError::NoProviderConfigured
                    | vanyline_lib::VnyError::NoModelConfigured
                    | vanyline_lib::VnyError::AgentNotFound
                    | vanyline_lib::VnyError::UnknownReference(_, _)
                    | vanyline_lib::VnyError::DuplicateName(_, _) => {
                        AgentTaskFailure::Config(format!("panne de configuration : {e}"))
                    }
                    _ => AgentTaskFailure::Provider(format!("panne du fournisseur : {e}")),
                };
                Err(failure)
            }
        }
    }
}

/// Parse et validation de la config (`agent` et `prompt` requis).
pub(crate) fn parse_config(config: &Value) -> Result<AgentConfig, StepError> {
    let cfg: AgentConfig = serde_json::from_value(config.clone()).map_err(|e| StepError {
        message: format!("config du step `agent` invalide : {e}"),
        retryable: false,
    })?;
    if cfg.agent.trim().is_empty() {
        return Err(StepError {
            message: "config du step `agent` invalide : `agent` est vide".to_string(),
            retryable: false,
        });
    }
    if cfg.prompt.trim().is_empty() {
        return Err(StepError {
            message: "config du step `agent` invalide : `prompt` est vide".to_string(),
            retryable: false,
        });
    }
    Ok(cfg)
}

/// Rendu du gabarit Handlebars STRICT : variables `inputs.<step_id>.<chemin>` et `run.<champ>` ;
/// une variable inconnue est une erreur (jamais une chaîne vide) — `agent.sdd` `Handles`.
pub(crate) fn render_prompt(
    template: &str,
    inputs: &HashMap<String, Value>,
    run: &RunInfo,
) -> Result<String, StepError> {
    let mut hb = handlebars::Handlebars::new();
    hb.set_strict_mode(true);
    hb.register_template_string("prompt", template)
        .map_err(|e| StepError {
            message: format!("gabarit du prompt invalide : {e}"),
            retryable: false,
        })?;
    let run_value = serde_json::to_value(&run.0).map_err(|e| StepError {
        message: format!("contexte de run non sérialisable : {e}"),
        retryable: false,
    })?;
    let data = serde_json::json!({ "inputs": inputs, "run": run_value });
    hb.render("prompt", &data).map_err(|e| StepError {
        message: format!("gabarit du prompt : {e}"),
        retryable: false,
    })
}

/// Rend le premier bloc de code JSON clôturé valide (balisé `json`), à défaut le texte entier
/// trimé si JSON valide, sinon `None` (contrat `extract_json` de `/lib/src/autonomous.sdd`,
/// non livré).
#[must_use]
pub(crate) fn extract_json(text: &str) -> Option<Value> {
    const OPEN: &str = "```json";
    const CLOSE: &str = "```";
    let mut rest = text;
    while let Some(open) = rest.find(OPEN) {
        let after_open = &rest[open.saturating_add(OPEN.len())..];
        let Some(close) = after_open.find(CLOSE) else {
            break;
        };
        let candidate = after_open[..close].trim();
        if let Ok(value) = serde_json::from_str::<Value>(candidate) {
            return Some(value);
        }
        rest = &after_open[close.saturating_add(CLOSE.len())..];
    }
    serde_json::from_str::<Value>(text.trim()).ok()
}

/// Corps du pas : rendu du gabarit (avant tout), bornage `user_id` et résolution de l'agent,
/// injection sandbox, plafond du timeout, exécution, sortie contractuelle (`agent.sdd`
/// `Handles`).
pub(crate) async fn execute_with(
    db: sea_orm::DatabaseConnection,
    locator: Arc<dyn SandboxLocator>,
    runner: Arc<dyn AgentTaskRunner>,
    config: &Value,
    inputs: &HashMap<String, Value>,
    _engine: &EngineRunInfo,
) -> Result<Value, StepError> {
    let cfg = parse_config(config)?;
    let run = RunInfo::from_config(config)?;
    // Le gabarit est rendu AVANT toute résolution ni exécution : un gabarit cassé ne doit
    // jamais déclencher d'appel LLM.
    let prompt = render_prompt(&cfg.prompt, inputs, &run)?;
    // Résolution dans la configuration de l'UTILISATEUR du run (`PgConfigStore` borné) : un
    // workflow ne donne jamais accès à l'agent d'un autre utilisateur.
    let store: Arc<dyn ConfigStore> = Arc::new(PgConfigStore::new(db, run.user_id));
    store.get_agent(&cfg.agent).await.map_err(|e| match e {
        vanyline_cfgstore::CfgStoreError::UnknownReference(_, name)
        | vanyline_cfgstore::CfgStoreError::DuplicateName(_, name) => StepError {
            message: format!(
                "agent introuvable : « {name} » dans la configuration de l'utilisateur du run"
            ),
            retryable: false,
        },
        other => StepError {
            message: format!(
                "résolution de l'agent « {} » impossible : {other}",
                cfg.agent
            ),
            retryable: false,
        },
    })?;
    // Injection sandbox : uniquement en portée sandbox, comme serveur MCP forcé du tour.
    let mut extra_mcp: Vec<(McpServer, McpSelection)> = Vec::new();
    if let Some(sandbox) = &run.sandbox {
        let token = locator.token(&run.owner).await?;
        let base = locator.base_url(&run.owner, sandbox).await?;
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {token}"));
        extra_mcp.push((
            McpServer {
                name: "sandbox".to_string(),
                transport: vanyline_cfgstore::domain::McpTransport::HttpStreamable,
                url: format!("{base}/mcp"),
                headers,
            },
            McpSelection {
                server: "sandbox".to_string(),
                tools: Vec::new(),
            },
        ));
    }
    let limits = limits_for(&cfg, run.sandbox.is_some());
    let outcome = runner
        .run(AgentTaskRequest {
            store,
            agent: cfg.agent.clone(),
            prompt,
            limits,
            extra_mcp,
        })
        .await
        .map_err(|f| match f {
            AgentTaskFailure::Config(m) => StepError {
                message: m,
                retryable: false,
            },
            AgentTaskFailure::Provider(m) => StepError {
                message: m,
                retryable: true,
            },
        })?;
    // Arrêt non complet : pas une erreur, l'arrêt est dans la sortie.
    let stop = match outcome.stop {
        AgentStop::Completed => "completed",
        AgentStop::TimedOut => "timed_out",
        AgentStop::StepLimit => "step_limit",
    };
    let json_out = if cfg.json {
        extract_json(&outcome.response_text).ok_or_else(|| StepError {
            message: "json demandé : aucune charge JSON exploitable dans la réponse de l'agent"
                .to_string(),
            retryable: false,
        })?
    } else {
        Value::Null
    };
    Ok(serde_json::json!({
        "text": outcome.response_text,
        "stop": stop,
        "json": json_out,
        "tool_calls": outcome.tool_calls,
        "usage": {
            "input_tokens": outcome.input_tokens,
            "output_tokens": outcome.output_tokens,
        },
        "unavailable_servers": outcome.unavailable_servers,
    }))
}

#[async_trait::async_trait]
impl MiryadWorkflowStep for AgentStep {
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    async fn run(
        &self,
        run: &EngineRunInfo,
        config: Value,
        inputs: HashMap<String, Value>,
    ) -> Result<Value, StepError> {
        execute_with(
            self.env.db.clone(),
            Arc::new(super::K8sLocator::new(self.env.k8s.clone())),
            Arc::new(LibTaskRunner),
            &config,
            &inputs,
            run,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::workflow::steps::tests::{config_with_run, run_ctx};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Exécuteur faux : mémorise la dernière requête, rend l'outcome programmé.
    #[derive(Clone, Default)]
    struct SpyRunner {
        calls: Arc<AtomicUsize>,
        last: std::sync::Arc<std::sync::Mutex<Option<LastReq>>>,
        outcome: std::sync::Arc<std::sync::Mutex<Option<AgentTaskOutcome>>>,
    }

    #[derive(Clone, Debug)]
    struct LastReq {
        agent: String,
        prompt: String,
        limits: AgentLimits,
        extra: Vec<(String, String, BTreeMap<String, String>)>,
    }

    #[async_trait::async_trait]
    impl AgentTaskRunner for SpyRunner {
        async fn run(&self, req: AgentTaskRequest) -> Result<AgentTaskOutcome, AgentTaskFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().unwrap() = Some(LastReq {
                agent: req.agent.clone(),
                prompt: req.prompt.clone(),
                limits: AgentLimits {
                    timeout: req.limits.timeout,
                    max_steps: req.limits.max_steps,
                },
                extra: req
                    .extra_mcp
                    .into_iter()
                    .map(|(s, _)| (s.name, s.url, s.headers))
                    .collect(),
            });
            self.outcome.lock().unwrap().clone().map_or_else(
                || {
                    Ok(AgentTaskOutcome {
                        stop: AgentStop::Completed,
                        response_text: "ok".to_string(),
                        tool_calls: 0,
                        input_tokens: 3,
                        output_tokens: 2,
                        unavailable_servers: vec![],
                    })
                },
                Ok,
            )
        }
    }

    fn outcome(text: &str, stop: AgentStop) -> AgentTaskOutcome {
        AgentTaskOutcome {
            stop,
            response_text: text.to_string(),
            tool_calls: 1,
            input_tokens: 10,
            output_tokens: 5,
            unavailable_servers: vec!["grafana".to_string()],
        }
    }

    fn fake_locator(base: &str) -> Arc<dyn SandboxLocator> {
        crate::workflow::steps::tests::FakeLocator::running("tok-owner", base)
    }

    fn engine() -> EngineRunInfo {
        EngineRunInfo {
            run_key: "rk".to_string(),
            step_id: "s".to_string(),
            depth: 0,
        }
    }

    fn cfg(
        agent: &str,
        prompt: &str,
        sandbox: Option<&str>,
        extra: serde_json::Map<String, Value>,
    ) -> Value {
        let mut map = extra;
        map.insert("agent".to_string(), json!(agent));
        map.insert("prompt".to_string(), json!(prompt));
        config_with_run(&run_ctx(sandbox), map)
    }

    async fn db_with_agent_owned_by(user_id: i32) -> sea_orm::DatabaseConnection {
        use sea_orm::{ActiveModelTrait, ConnectionTrait, Set};
        let db = crate::db::test_support::real_db().await;
        // FK `model_profiles.owner_id -> miryad_users.id` : l'utilisateur du run doit exister.
        db.execute_unprepared(&format!(
            "INSERT INTO miryad_users (id, subject, created_at) VALUES ({user_id}, 'user-{user_id}', datetime('now'))"
        ))
        .await
        .unwrap();
        let provider = crate::db::entities::llm_providers::ActiveModel {
            name: Set("prov".to_string()),
            provider_type: Set("openai-compatible".to_string()),
            endpoint: Set("http://127.0.0.1:1/v1".to_string()),
            api_key: Set(None),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        let profile = crate::db::entities::model_profiles::ActiveModel {
            owner_id: Set(user_id),
            name: Set("m".to_string()),
            provider_id: Set(provider.id),
            model: Set("gpt-test".to_string()),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(json!({})),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        crate::db::entities::agents::ActiveModel {
            owner_id: Set(user_id),
            name: Set("a".to_string()),
            description: Set(None),
            mode: Set("primary".to_string()),
            model_profile_id: Set(profile.id),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        db
    }

    // ── Scenario: sortie_texte ──────────────────────────────────────────────
    // Réponse « ok », prompt sans variable ⇒ `text == "ok"`, `stop == "completed"`, `json == null`.
    #[tokio::test]
    async fn sortie_texte() {
        let db = db_with_agent_owned_by(7).await; // agent `a` résoluble pour l'utilisateur du run
        let runner = SpyRunner::default();
        let out = execute_with(
            db,
            fake_locator("http://127.0.0.1:1"),
            Arc::new(runner),
            &cfg("a", "bonjour", None, serde_json::Map::new()),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect("prompt littéral, exécution faux runner");
        assert_eq!(out["text"], "ok");
        assert_eq!(out["stop"], "completed");
        assert_eq!(out["json"], Value::Null);
        assert_eq!(out["tool_calls"], 0);
        assert_eq!(out["usage"]["input_tokens"], 3);
        assert_eq!(out["usage"]["output_tokens"], 2);
        assert_eq!(out["unavailable_servers"], json!([]));
    }

    // ── Scenario: json_permis ───────────────────────────────────────────────
    #[tokio::test]
    async fn json_permis() {
        let db = db_with_agent_owned_by(7).await; // agent `a` résoluble pour l'utilisateur du run
        let runner = SpyRunner {
            outcome: std::sync::Arc::new(std::sync::Mutex::new(Some(outcome(
                "Verdict :\n```json\n{\"notify\":true}\n```\nfin",
                AgentStop::Completed,
            )))),
            ..Default::default()
        };
        let out = execute_with(
            db,
            fake_locator("http://127.0.0.1:1"),
            Arc::new(runner),
            &cfg(
                "a",
                "v",
                None,
                serde_json::map::Map::from_iter([("json".to_string(), json!(true))]),
            ),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect("json demandé et trouvé");
        assert_eq!(out["json"], json!({"notify": true}));
    }

    // ── Scenario: json_demande_mais_absent ──────────────────────────────────
    // `json: true` sans JSON exploitable ⇒ erreur non retryable.
    #[tokio::test]
    async fn json_demande_mais_absent() {
        let db = db_with_agent_owned_by(7).await; // agent `a` résoluble pour l'utilisateur du run
        let runner = SpyRunner {
            outcome: std::sync::Arc::new(std::sync::Mutex::new(Some(outcome(
                "pas de json ici",
                AgentStop::Completed,
            )))),
            ..Default::default()
        };
        let err = execute_with(
            db,
            fake_locator("http://127.0.0.1:1"),
            Arc::new(runner),
            &cfg(
                "a",
                "v",
                None,
                serde_json::map::Map::from_iter([("json".to_string(), json!(true))]),
            ),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect_err("json:true sans JSON = erreur");
        assert!(!err.retryable, "jamais exploitable au rejeu : {err}");
    }

    // ── Scenario: variables_du_gabarit ──────────────────────────────────────
    // `Analyse {{inputs.fetch.text}} pour {{run.project}}` ⇒ « Analyse X pour demo ».
    #[tokio::test]
    async fn variables_du_gabarit() {
        let db = db_with_agent_owned_by(7).await; // agent `a` résoluble pour l'utilisateur du run
        let runner = SpyRunner::default();
        let spy: SpyRunner = runner.clone();
        let mut inputs = HashMap::new();
        inputs.insert("fetch".to_string(), json!({"text": "X"}));
        execute_with(
            db,
            fake_locator("http://127.0.0.1:1"),
            Arc::new(runner),
            &cfg(
                "a",
                "Analyse {{inputs.fetch.text}} pour {{run.project}}",
                None,
                serde_json::Map::new(),
            ),
            &inputs,
            &engine(),
        )
        .await
        .expect("variables connues");
        let last = spy.last.lock().unwrap();
        assert_eq!(last.as_ref().unwrap().prompt, "Analyse X pour demo");
        assert_eq!(last.as_ref().unwrap().agent, "a");
    }

    // ── Scenario: variable_inconnue ─────────────────────────────────────────
    // `{{inputs.absent.x}}` ⇒ erreur non retryable, l'agent n'est PAS appelé.
    #[tokio::test]
    async fn variable_inconnue() {
        let db = crate::db::test_support::real_db().await;
        let runner = SpyRunner::default();
        let err = execute_with(
            db,
            fake_locator("http://127.0.0.1:1"),
            Arc::new(runner.clone()),
            &cfg("a", "x {{inputs.absent.x}}", None, serde_json::Map::new()),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect_err("variable inconnue = erreur, pas une chaîne vide");
        assert!(!err.retryable, "gabarit invalide au rejeu : {err}");
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            0,
            "l'agent n'est pas appelé quand le gabarit échoue"
        );
    }

    // ── Scenario: agent_d_un_autre_utilisateur ──────────────────────────────
    // L'agent `a` appartient à l'utilisateur 2 ; le run est celui de l'utilisateur 7 (le
    // contextedefaut de `run_ctx`). `PgConfigStore` borné ⇒ « agent introuvable », non
    // retryable, exécuteur jamais appelé. Appoint positif : avec `_run.user_id = 2`, l'agent
    // est trouvé (le bornage n'est pas un filtre qui refuse tout).
    #[tokio::test]
    async fn agent_d_un_autre_utilisateur() {
        let db = db_with_agent_owned_by(2).await;
        let err = execute_with(
            db.clone(),
            fake_locator("http://127.0.0.1:1"),
            Arc::new(SpyRunner::default()),
            &cfg("a", "v", None, serde_json::Map::new()),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect_err("l'agent d'un autre utilisateur est introuvable");
        assert!(!err.retryable);
        assert!(
            err.message.contains("agent introuvable"),
            "message contractuel : {err}"
        );

        // Appoint positif : même base, run de l'utilisateur 2 ⇒ l'agent est résolu.
        let mut map = serde_json::Map::new();
        map.insert(
            "_run".to_string(),
            json!({
                "run_id": "r2", "user": "bob", "user_id": 2, "owner": "bob",
                "project": "demo", "sandbox": null, "triggered_by": "ui",
                "definition": "d"
            }),
        );
        map.insert("agent".to_string(), json!("a"));
        map.insert("prompt".to_string(), json!("v"));
        let runner = SpyRunner::default();
        execute_with(
            db,
            fake_locator("http://127.0.0.1:1"),
            Arc::new(runner.clone()),
            &Value::Object(map),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect("l'agent de l'utilisateur du run est résolu");
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    }

    // Appoint `Raises` : config sans `agent` ou sans `prompt` ⇒ erreur non retryable.
    #[test]
    fn config_incomplete() {
        let err = parse_config(&json!({"prompt": "v"})).expect_err("agent requis");
        assert!(!err.retryable);
        let err = parse_config(&json!({"agent": "a"})).expect_err("prompt requis");
        assert!(!err.retryable);
    }

    // ── Scenario: sandbox_injectee_seulement_en_portee_sandbox ──────────────
    #[tokio::test]
    async fn sandbox_injectee_seulement_en_portee_sandbox() {
        let db = db_with_agent_owned_by(7).await; // agent `a` résoluble pour l'utilisateur du run
        let base = "http://sandbox-sbx-1.vanyline-alice.svc:3000";

        // Run de portée sandbox : serveur `sandbox` forcé, URL interne …/mcp, Bearer jeton.
        let with = SpyRunner::default();
        execute_with(
            db.clone(),
            fake_locator(base),
            Arc::new(with.clone()),
            &cfg("a", "v", Some("sbx-1"), serde_json::Map::new()),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect("run de portée sandbox");
        let extra = {
            let last = with.last.lock().unwrap();
            last.as_ref().unwrap().extra.clone()
        };
        assert_eq!(extra.len(), 1, "le serveur sandbox est injecté");
        assert_eq!(extra[0].0, "sandbox");
        assert_eq!(extra[0].1, format!("{base}/mcp"), "URL interne MCP");
        assert_eq!(
            extra[0].2.get("Authorization").map(String::as_str),
            Some("Bearer tok-owner"),
            "jeton de l'Owner en en-tête"
        );

        // Run de portée projet : rien d'injecté.
        let without = SpyRunner::default();
        execute_with(
            db,
            fake_locator(base),
            Arc::new(without.clone()),
            &cfg("a", "v", None, serde_json::Map::new()),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect("run de portée projet");
        assert!(
            without
                .last
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .extra
                .is_empty(),
            "jamais de serveur sandbox hors portée sandbox"
        );
    }

    // ── Scenario: timeout_plafonne_avec_sandbox ─────────────────────────────
    // `timeout_secs = 3000` avec sandbox ⇒ 780 s effectifs. Appoints : 3000 sans sandbox ⇒
    // 3000 (sous le plafond 3600) ; absent ⇒ 600 dans les DEUX portées (`agent.sdd` Handles,
    // amendé 2026-10-10 : le plafond 780 ne change pas le défaut) ; absurdement grand sans
    // sandbox ⇒ 3600.
    #[tokio::test]
    async fn timeout_plafonne_avec_sandbox() {
        let db = db_with_agent_owned_by(7).await; // agent `a` résoluble pour l'utilisateur du run
        for (configured, sandbox, want) in [
            (Some(3000_u64), Some("sbx-1"), TIMEOUT_CAP_WITH_SANDBOX_SECS),
            (Some(3000), None, 3000),
            (None, None, TIMEOUT_DEFAULT_SECS),
            (None, Some("sbx-1"), TIMEOUT_DEFAULT_SECS),
            (Some(99_999), None, TIMEOUT_CAP_SECS),
            (Some(99_999), Some("sbx-1"), TIMEOUT_CAP_WITH_SANDBOX_SECS),
        ] {
            let mut extra = serde_json::Map::new();
            if let Some(t) = configured {
                extra.insert("timeout_secs".to_string(), json!(t));
            }
            let runner = SpyRunner::default();
            execute_with(
                db.clone(),
                fake_locator("http://sandbox-s.svc:3000"),
                Arc::new(runner.clone()),
                &cfg("a", "v", sandbox, extra.clone()),
                &HashMap::new(),
                &engine(),
            )
            .await
            .unwrap_or_else(|e| panic!("timeout {configured:?} sandbox {sandbox:?} : {e}"));
            let secs = runner
                .last
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .limits
                .timeout
                .as_secs();
            assert_eq!(
                secs, want,
                "timeout effectif pour {configured:?}/{sandbox:?}"
            );
        }
        // `max_steps` surcharge `TaskLimits` : transmis tel quel.
        let mut extra = serde_json::Map::new();
        extra.insert("max_steps".to_string(), json!(5));
        let runner = SpyRunner::default();
        execute_with(
            db,
            fake_locator("http://127.0.0.1:1"),
            Arc::new(runner.clone()),
            &cfg("a", "v", None, extra),
            &HashMap::new(),
            &engine(),
        )
        .await
        .unwrap();
        assert_eq!(
            runner
                .last
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .limits
                .max_steps,
            Some(5)
        );
    }

    // ── Scenario: arret_non_complete_n_est_pas_une_erreur ───────────────────
    // `stop == "timed_out"` (et `step_limit`) : le step réussit, l'arrêt est dans la sortie.
    #[tokio::test]
    async fn arret_non_complete_n_est_pas_une_erreur() {
        let db = db_with_agent_owned_by(7).await; // agent `a` résoluble pour l'utilisateur du run
        for (stop, want) in [
            (AgentStop::TimedOut, "timed_out"),
            (AgentStop::StepLimit, "step_limit"),
        ] {
            let runner = SpyRunner {
                outcome: std::sync::Arc::new(std::sync::Mutex::new(Some(outcome(
                    "partiel…",
                    stop,
                )))),
                ..Default::default()
            };
            let out = execute_with(
                db.clone(),
                fake_locator("http://127.0.0.1:1"),
                Arc::new(runner),
                &cfg("a", "v", None, serde_json::Map::new()),
                &HashMap::new(),
                &engine(),
            )
            .await
            .unwrap_or_else(|e| panic!("stop {stop:?} ne doit pas être une erreur : {e}"));
            assert_eq!(out["stop"], want);
            assert_eq!(out["text"], "partiel…");
            assert_eq!(out["tool_calls"], 1);
            assert_eq!(
                out["unavailable_servers"],
                json!(["grafana"]),
                "un verdict sans ses outils doit le dire"
            );
        }
    }

    // ── Appoint : pureté de `extract_json` (contrat `autonomous.sdd`) ───────
    #[test]
    fn extract_json_auxiliaire() {
        assert_eq!(
            extract_json("Verdict :\n```json\n{\"notify\":true}\n```\nfin"),
            Some(json!({"notify": true}))
        );
        assert_eq!(
            extract_json("{\"notify\":false}"),
            Some(json!({"notify": false}))
        );
        assert_eq!(extract_json("pas de json ici"), None);
        assert_eq!(
            extract_json("```json\n{invalide\n```\npuis ```json\n{\"ok\":1}\n```"),
            Some(json!({"ok": 1})),
            "bloc invalide puis bloc valide"
        );
    }

    // Appoint `Raises` : panne du fournisseur ⇒ retryable ; erreur de résolution ⇒ non.
    #[tokio::test]
    async fn classification_des_pannes() {
        struct Failing(AgentTaskFailure);
        #[async_trait::async_trait]
        impl AgentTaskRunner for Failing {
            async fn run(
                &self,
                _r: AgentTaskRequest,
            ) -> Result<AgentTaskOutcome, AgentTaskFailure> {
                Err(match &self.0 {
                    AgentTaskFailure::Config(m) => AgentTaskFailure::Config(m.clone()),
                    AgentTaskFailure::Provider(m) => AgentTaskFailure::Provider(m.clone()),
                })
            }
        }
        let db = db_with_agent_owned_by(7).await; // agent `a` résoluble pour l'utilisateur du run
        let err = execute_with(
            db.clone(),
            fake_locator("http://127.0.0.1:1"),
            Arc::new(Failing(AgentTaskFailure::Provider("503".to_string()))),
            &cfg("a", "v", None, serde_json::Map::new()),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect_err("panne fournisseur");
        assert!(err.retryable, "panne du fournisseur ⇒ transitoire : {err}");
        let err = execute_with(
            db,
            fake_locator("http://127.0.0.1:1"),
            Arc::new(Failing(AgentTaskFailure::Config(
                "modèle orphelin".to_string(),
            ))),
            &cfg("a", "v", None, serde_json::Map::new()),
            &HashMap::new(),
            &engine(),
        )
        .await
        .expect_err("erreur de config interne");
        assert!(!err.retryable, "config ⇒ jamais retryable : {err}");
    }
}
