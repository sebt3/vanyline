use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::{Agent, AgentMode, ModelProfile, Provider, ProviderType, SkillMeta, SkillSelection, Toolset};
use crate::error::VnyError;
use crate::event::{ChatTurnResult, EventSink};
use crate::store::ConfigStore;

/// Assemble le system prompt final d'un tour d'agent, dans l'ordre fixe décrit
/// par le design (section "Session engine") :
///
/// 1. `agent.system_prompt`
/// 2. `toolset.prompt` de chaque toolset RÉFÉRENCÉ par `agent.toolsets`, DANS
///    L'ORDRE de `agent.toolsets` (pas l'ordre de `all_toolsets`) — un nom de
///    `agent.toolsets` absent de `all_toolsets` est silencieusement ignoré (la
///    validation "toolset inconnu" est la responsabilité de l'appelant via
///    `ConfigStore::get_toolset`, tâche 3 ; cette fonction est pure et ne
///    remonte pas d'erreur)
/// 3. Index des skills — calculé à partir de `agent.skills` et `all_skills` :
///    - `SkillSelection::None` -> pas de section
///    - `SkillSelection::Auto` -> tous les `all_skills`, dans leur ordre d'origine
///    - `SkillSelection::Named(names)` -> les `all_skills` dont le nom est dans
///      `names`, dans l'ordre de `all_skills` (pas l'ordre de `names`)
///      Si la liste résultante est vide (ex. `Auto` mais `all_skills` vide),
///      la section est omise entièrement — pas de bloc vide.
///      Format : une ligne `- {name} : {description}` par skill, PAS de ligne
///      d'en-tête (le design ne mentionne que le format des lignes).
/// 4. `workspace_context` si `Some(s)` et `!s.trim().is_empty()` (une chaîne
///    vide ou uniquement des espaces ne produit pas de section)
///
/// Les sections non vides sont jointes par `"\n\n"` (une ligne blanche entre
/// chaque section). Aucune section vide n'introduit de séparateur superflu
/// (ex. si aucun toolset n'a de prompt ET aucun skill n'est sélectionné, le
/// résultat est juste `agent.system_prompt` + éventuellement le contexte
/// workspace, sans lignes blanches en trop).
pub fn assemble_system_prompt(
    agent: &Agent,
    all_toolsets: &[Toolset],
    all_skills: &[SkillMeta],
    workspace_context: Option<&str>,
) -> String {
    let mut sections = Vec::new();

    // 1. System prompt de l'agent (toujours inclus)
    sections.push(agent.system_prompt.clone());

    // 2. Prompts des toolsets, dans l'ordre de agent.toolsets
    let mut toolset_sections = String::new();
    for name in &agent.toolsets {
        let prompt = all_toolsets
            .iter()
            .find(|t| t.name == *name)
            .and_then(|t| t.prompt.as_ref())
            .map(|p| p.trim())
            .filter(|p| !p.is_empty());
        if let Some(p) = prompt {
            if !toolset_sections.is_empty() {
                toolset_sections.push_str("\n\n");
            }
            toolset_sections.push_str(p);
        }
    }
    if !toolset_sections.is_empty() {
        sections.push(toolset_sections);
    }

    // 3. Index des skills
    let selected_skills = resolve_skill_index(&agent.skills, all_skills);
    if !selected_skills.is_empty() {
        let lines: Vec<_> = selected_skills
            .iter()
            .map(|s| format!("- {} : {}", s.name, s.description))
            .collect();
        sections.push(lines.join("\n"));
    }

    // 4. Workspace context
    if let Some(ctx) = workspace_context {
        let trimmed = ctx.trim();
        if !trimmed.is_empty() {
            sections.push(trimmed.to_string());
        }
    }

    let result = sections.join("\n\n");
    // Si system_prompt était vide et qu'il n'y a rien d'autre, retourner ""
    if agent.system_prompt.is_empty() && sections.len() == 1 {
        String::new()
    } else {
        result
    }
}

/// Filtre `all_skills` selon `selection` :
/// - `None` -> vide (pas de skills disponibles pour cet agent)
/// - `Auto` -> tous les `all_skills`, dans leur ordre d'origine
/// - `Named(names)` -> uniquement les skills dont le nom est dans `names`,
///   dans l'ordre de `all_skills` (pas l'ordre de `names`)
///
/// Factorisé car utilisé à deux endroits : `assemble_system_prompt` (index
/// texte dans le system prompt) et `resolve_turn_context` (index structuré
/// passé au tool builtin `skill`, tâche 7) — même règle, ne doit pas diverger.
fn resolve_skill_index(selection: &SkillSelection, all_skills: &[SkillMeta]) -> Vec<SkillMeta> {
    match selection {
        SkillSelection::None => Vec::new(),
        SkillSelection::Auto => all_skills.to_vec(),
        SkillSelection::Named(names) => {
            let name_set: std::collections::HashSet<&str> =
                names.iter().map(|n| n.as_str()).collect();
            all_skills
                .iter()
                .filter(|s| name_set.contains(s.name.as_str()))
                .cloned()
                .collect()
        }
    }
}

/// Agents invocables comme subagents (mode `Subagent` ou `All`) — `Primary`
/// exclu. Dans l'ordre de `all_agents`. Sert à l'index du tool builtin `task`
/// et à décider si ce tool doit être exposé du tout (inutile si personne n'est
/// invocable).
fn available_subagents(all_agents: &[Agent]) -> Vec<Agent> {
    all_agents
        .iter()
        .filter(|a| a.mode != AgentMode::Primary)
        .cloned()
        .collect()
}

/// Adapte un `Arc<dyn ToolDyn>` en une valeur `ToolDyn` owned que
/// `ToolServerHandle::add_tool` peut accepter (`impl ToolDyn + 'static` — un
/// `Arc<dyn ToolDyn>` n'implémente pas lui-même `ToolDyn`, il faut un
/// forwarding explicite). Cloner un `Arc` est bon marché — c'est ce qui permet
/// de peupler un nouveau handle à chaque tour sans consommer
/// `SessionContext.local_tools`.
struct ArcToolDyn(Arc<dyn rig_core::tool::ToolDyn>);

impl rig_core::tool::ToolDyn for ArcToolDyn {
    fn name(&self) -> String {
        self.0.name()
    }

    fn definition<'a>(
        &'a self,
        prompt: String,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, rig_core::completion::ToolDefinition> {
        self.0.definition(prompt)
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> rig_core::wasm_compat::WasmBoxedFuture<'a, Result<String, rig_core::tool::ToolError>> {
        self.0.call(args)
    }
}

/// Contexte partagé d'une session : store de config, sink d'événements, local
/// tools fournis par l'hôte (cli : filesystem/command ; app : aucun ; référencés
/// par nom via `Toolset.local_tools`), profondeur max de subagents (utilisée par
/// la tâche 8, pas par cette tâche).
#[derive(Clone)]
pub struct SessionContext {
    pub store: Arc<dyn ConfigStore>,
    pub sink: Arc<dyn EventSink>,
    pub local_tools: HashMap<String, Arc<dyn rig_core::tool::ToolDyn>>,
    pub subagent_depth_max: u8,
}

/// Résultat de la résolution d'un tour, AVANT toute I/O réseau (MCP, LLM) —
/// tout ce qui vient de `ctx.store` uniquement. C'est ce découpage qui rend
/// testable, sans réseau, la partie la plus piégeuse de `run_agent_turn` : la
/// résolution en chaîne agent -> modèle -> provider -> toolsets, et la
/// propagation correcte des erreurs `UnknownReference` à chaque maillon.
#[derive(Debug)]
struct ResolvedTurn {
    profile: ModelProfile,
    provider: Provider,
    resolved_toolsets: Vec<Toolset>,
    system_prompt: String,
    skill_index: Vec<SkillMeta>,
}

/// Résout la chaîne agent -> modèle -> provider -> toolsets et assemble le
/// system prompt. Propage `VnyError::UnknownReference`/`DuplicateName` de
/// `ConfigStore` (tâche 3) sans les envelopper. Aucune I/O réseau — uniquement
/// des appels à `ctx.store` (qui, pour `InMemoryConfigStore`, est purement en
/// mémoire).
async fn resolve_turn_context(
    ctx: &SessionContext,
    agent_name: &str,
    workspace_context: Option<&str>,
) -> Result<ResolvedTurn, VnyError> {
    let agent = ctx.store.get_agent(agent_name).await?;
    let profile = ctx.store.get_model(&agent.model).await?;
    let provider = ctx.store.get_provider(&profile.provider).await?;

    let mut resolved_toolsets = Vec::with_capacity(agent.toolsets.len());
    for name in &agent.toolsets {
        let toolset = ctx.store.get_toolset(name).await?;
        resolved_toolsets.push(toolset);
    }

    let all_skills = ctx.store.list_skills().await?;
    let selected_skills = resolve_skill_index(&agent.skills, &all_skills);
    let system_prompt = assemble_system_prompt(&agent, &resolved_toolsets, &all_skills, workspace_context);

    Ok(ResolvedTurn {
        profile,
        provider,
        resolved_toolsets,
        system_prompt,
        skill_index: selected_skills,
    })
}

/// Nombre max d'allers-retours d'appels d'outils par tour. Sans ça, rig-core
/// retombe sur son défaut interne (0 — un seul aller-retour), ce qui bloque
/// toute tâche nécessitant lire/chercher/éditer plusieurs fichiers dans un
/// même tour. 100 = un vrai filet de sécurité anti-boucle-infinie (modèle
/// confus qui n'arrête jamais d'appeler des outils), pas un plafond de travail
/// — une tâche de code légitime (explorer, chercher, éditer, vérifier sur
/// plusieurs fichiers) ne doit jamais s'en approcher en pratique.
const DEFAULT_MAX_TURNS: usize = 100;

/// Construit l'`Agent` rig-core à partir des `AgentParams` — extrait de
/// `run_turn_with_model` pour être testable sans réseau (aucun appel HTTP tant
/// qu'on n'appelle pas `.stream_chat(..)`/`.prompt(..)` dessus).
fn build_agent<M>(
    model: M,
    params: &crate::model::AgentParams,
    system_prompt: &str,
    handle: rig_core::tool::server::ToolServerHandle,
) -> rig_core::agent::Agent<M>
where
    M: rig_core::completion::CompletionModel + Clone + 'static,
{
    let mut builder = rig_core::agent::AgentBuilder::new(model)
        .preamble(system_prompt)
        .default_max_turns(DEFAULT_MAX_TURNS);
    if let Some(t) = params.temperature {
        builder = builder.temperature(t);
    }
    if let Some(m) = params.max_tokens {
        builder = builder.max_tokens(m);
    }
    if let Some(ap) = params.additional_params.clone() {
        builder = builder.additional_params(ap);
    }
    builder.tool_server_handle(handle).build()
}

/// Construit l'`Agent<M>` rig (préambule + params du profil + handle de tools)
/// et lance le stream via `event::stream_agent_events`. Générique sur `M` car
/// le type concret du modèle diffère selon `provider.provider_type`
/// (`build_ollama_model`/`build_openai_compat_model`, tâche 4) — c'est
/// `run_agent_turn` qui fait ce dispatch et appelle cette fonction dans chaque
/// bras du `match`.
async fn run_turn_with_model<M>(
    ctx: &SessionContext,
    model: M,
    params: crate::model::AgentParams,
    system_prompt: &str,
    handle: rig_core::tool::server::ToolServerHandle,
    history: Vec<rig_core::message::Message>,
    user_msg: &str,
) -> Result<ChatTurnResult, VnyError>
where
    M: rig_core::completion::CompletionModel + Clone + 'static,
    M::StreamingResponse: rig_core::completion::GetTokenUsage,
{
    let agent = build_agent(model, &params, system_prompt, handle);
    crate::event::stream_agent_events(ctx.sink.clone(), agent, history, user_msg).await
}

/// Point d'entrée unique de la session engine, à la profondeur 0 (un appel
/// top-level, jamais un subagent). Voir `run_agent_turn_at_depth` pour
/// l'implémentation — ce raccourci fixe simplement `current_depth = 0`.
pub async fn run_agent_turn(
    ctx: &SessionContext,
    agent_name: &str,
    history: Vec<rig_core::message::Message>,
    user_msg: &str,
    workspace_context: Option<&str>,
) -> Result<ChatTurnResult, VnyError> {
    run_agent_turn_at_depth(ctx, agent_name, history, user_msg, workspace_context, 0).await
}

/// Implémentation réelle de `run_agent_turn`, paramétrée par `current_depth`
/// (0 pour un tour top-level). `pub(crate)`, pas `pub` : seul le tool builtin
/// `task` (`builtin::task`, cette tâche) a besoin d'invoquer un tour imbriqué
/// à `current_depth + 1` ; les hôtes (cli/app) passent toujours par
/// `run_agent_turn` (profondeur 0 implicite). Résout l'agent par nom, assemble
/// le prompt, peuple un handle de tools frais (local + MCP filtrés par
/// toolset + builtins `skill`/`task`) et streame le tour.
pub(crate) async fn run_agent_turn_at_depth(
    ctx: &SessionContext,
    agent_name: &str,
    history: Vec<rig_core::message::Message>,
    user_msg: &str,
    workspace_context: Option<&str>,
    current_depth: u8,
) -> Result<ChatTurnResult, VnyError> {
    let resolved = resolve_turn_context(ctx, agent_name, workspace_context).await?;
    let handle = crate::prefixed_mcp::new_tool_handle();

    let mut mcp_connections: Vec<crate::prefixed_mcp::McpRunningService> = Vec::new();

    for toolset in &resolved.resolved_toolsets {
        // Local tools
        let (found, missing) = crate::prefixed_mcp::select_local_tools(&toolset.local_tools, &ctx.local_tools);
        for name in missing {
            tracing::warn!("local tool not found: {name}");
        }
        for name in found {
            if let Some(tool) = ctx.local_tools.get(name) {
                let wrapped = ArcToolDyn(tool.clone());
                if let Err(e) = handle.add_tool(wrapped).await {
                    tracing::warn!("failed to add local tool {}: {e}", name);
                }
            }
        }
        // MCP
        let mut validated_servers = Vec::new();
        for selection in &toolset.mcp {
            match ctx.store.get_mcp_server(&selection.server).await {
                Ok(server) => validated_servers.push(server),
                Err(e) => {
                    tracing::warn!("mcp server not found: {e}");
                }
            }
        }
        let running = crate::prefixed_mcp::connect_mcp_servers_selected(&toolset.mcp, &validated_servers, &handle).await?;
        mcp_connections.extend(running);
    }

    if !resolved.skill_index.is_empty() {
        let skill_tool = crate::builtin::skill::SkillTool::new(
            ctx.store.clone(),
            ctx.sink.clone(),
            resolved.skill_index.clone(),
        );
        if let Err(e) = handle.add_tool(skill_tool).await {
            tracing::warn!("failed to add builtin skill tool: {e}");
        }
    }

    if current_depth < ctx.subagent_depth_max {
        let all_agents = ctx.store.list_agents().await?;
        let subagents = available_subagents(&all_agents);
        if !subagents.is_empty() {
            let task_tool = crate::builtin::task::TaskTool::new(ctx.clone(), current_depth, subagents);
            if let Err(e) = handle.add_tool(task_tool).await {
                tracing::warn!("failed to add builtin task tool: {e}");
            }
        }
    }

    let params = crate::model::agent_params(&resolved.profile);
    let result = match resolved.provider.provider_type {
        ProviderType::Ollama => {
            let model = crate::model::build_ollama_model(&resolved.provider, &resolved.profile)?;
            run_turn_with_model(ctx, model, params, &resolved.system_prompt, handle, history, user_msg).await
        }
        ProviderType::OpenaiCompatible => {
            let model = crate::model::build_openai_compat_model(&resolved.provider, &resolved.profile)?;
            run_turn_with_model(ctx, model, params, &resolved.system_prompt, handle, history, user_msg).await
        }
    };

    for conn in mcp_connections {
        if let Err(e) = conn.cancel().await {
            tracing::warn!("failed to cleanly cancel mcp connection: {e}");
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_agent(toolsets: Vec<String>, skills: SkillSelection) -> Agent {
        Agent {
            name: "test-agent".to_string(),
            description: None,
            mode: AgentMode::Primary,
            model: "qwen".to_string(),
            toolsets,
            skills,
            system_prompt: "You are helpful.".to_string(),
        }
    }

    // 1. system_prompt_only
    #[test]
    fn system_prompt_only() {
        let agent = sample_agent(Vec::new(), SkillSelection::None);
        let result = assemble_system_prompt(&agent, &[], &[], None);
        assert_eq!(result, "You are helpful.");
        assert!(!result.ends_with('\n'));
        assert!(!result.ends_with('\r'));
    }

    // 2. toolset_prompts_in_agent_order
    #[test]
    fn toolset_prompts_in_agent_order() {
        let toolsets = vec![
            Toolset {
                name: "a".to_string(),
                description: None,
                prompt: Some("PROMPT_A".to_string()),
                local_tools: vec![],
                mcp: vec![],
            },
            Toolset {
                name: "b".to_string(),
                description: None,
                prompt: Some("PROMPT_B".to_string()),
                local_tools: vec![],
                mcp: vec![],
            },
        ];
        let agent = sample_agent(vec!["b".to_string(), "a".to_string()], SkillSelection::None);
        let result = assemble_system_prompt(&agent, &toolsets, &[], None);
        let pos_b = result.find("PROMPT_B").unwrap();
        let pos_a = result.find("PROMPT_A").unwrap();
        assert!(pos_b < pos_a);
    }

    // 3. toolset_without_prompt_contributes_nothing
    #[test]
    fn toolset_without_prompt_contributes_nothing() {
        let toolsets = vec![
            Toolset {
                name: "a".to_string(),
                description: None,
                prompt: None,
                local_tools: vec![],
                mcp: vec![],
            },
            Toolset {
                name: "b".to_string(),
                description: None,
                prompt: None,
                local_tools: vec![],
                mcp: vec![],
            },
        ];
        let agent = sample_agent(vec!["a".to_string(), "b".to_string()], SkillSelection::None);
        let result = assemble_system_prompt(&agent, &toolsets, &[], None);
        assert_eq!(result, "You are helpful.");
    }

    // 4. toolset_unknown_reference_ignored
    #[test]
    fn toolset_unknown_reference_ignored() {
        let toolsets: Vec<Toolset> = vec![];
        let agent = sample_agent(vec!["nonexistent".to_string()], SkillSelection::None);
        let result = assemble_system_prompt(&agent, &toolsets, &[], None);
        assert_eq!(result, "You are helpful.");
    }

    // 5. skills_none_no_section
    #[test]
    fn skills_none_no_section() {
        let skills = vec![
            SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            },
            SkillMeta {
                name: "web".to_string(),
                description: "Web search".to_string(),
            },
        ];
        let agent = sample_agent(Vec::new(), SkillSelection::None);
        let result = assemble_system_prompt(&agent, &[], &skills, None);
        assert!(!result.contains("PDF processing"));
        assert!(!result.contains("Web search"));
    }

    // 6. skills_auto_all_included
    #[test]
    fn skills_auto_all_included() {
        let skills = vec![
            SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            },
            SkillMeta {
                name: "web".to_string(),
                description: "Web search".to_string(),
            },
        ];
        let agent = sample_agent(Vec::new(), SkillSelection::Auto);
        let result = assemble_system_prompt(&agent, &[], &skills, None);
        assert!(result.contains("- pdf : PDF processing"));
        assert!(result.contains("- web : Web search"));
        let pos_pdf = result.find("- pdf : PDF processing").unwrap();
        let pos_web = result.find("- web : Web search").unwrap();
        assert!(pos_pdf < pos_web);
    }

    // 7. skills_named_filters
    #[test]
    fn skills_named_filters() {
        let skills = vec![
            SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            },
            SkillMeta {
                name: "web".to_string(),
                description: "Web search".to_string(),
            },
        ];
        let agent = sample_agent(
            Vec::new(),
            SkillSelection::Named(vec!["web".to_string()]),
        );
        let result = assemble_system_prompt(&agent, &[], &skills, None);
        assert!(result.contains("- web : Web search"));
        assert!(!result.contains("PDF processing"));
    }

    // 8. skills_auto_empty_list_no_section
    #[test]
    fn skills_auto_empty_list_no_section() {
        let agent = sample_agent(Vec::new(), SkillSelection::Auto);
        let result = assemble_system_prompt(&agent, &[], &[], None);
        assert_eq!(result, "You are helpful.");
    }

    // 9. workspace_context_included
    #[test]
    fn workspace_context_included() {
        let agent = sample_agent(Vec::new(), SkillSelection::None);
        let ctx = Some("# AGENTS.md\ncontent");
        let result = assemble_system_prompt(&agent, &[], &[], ctx);
        assert!(result.ends_with("content"));
    }

    // 10. workspace_context_blank_ignored
    #[test]
    fn workspace_context_blank_ignored() {
        let agent = sample_agent(Vec::new(), SkillSelection::None);
        let ctx = Some("   ");
        let result = assemble_system_prompt(&agent, &[], &[], ctx);
        assert_eq!(result, "You are helpful.");
    }

    // 11. full_assembly_order
    #[test]
    fn full_assembly_order() {
        let toolset = vec![Toolset {
            name: "shell".to_string(),
            description: None,
            prompt: Some("## Shell tool".to_string()),
            local_tools: vec![],
            mcp: vec![],
        }];
        let skills = vec![SkillMeta {
            name: "web".to_string(),
            description: "Web search".to_string(),
        }];
        let agent = sample_agent(
            vec!["shell".to_string()],
            SkillSelection::Auto,
        );
        let result = assemble_system_prompt(&agent, &toolset, &skills, Some("# AGENTS.md\ncontent"));

        let system_pos = result.find("You are helpful.").unwrap();
        let toolset_pos = result.find("## Shell tool").unwrap();
        let skill_pos = result.find("- web : Web search").unwrap();
        let context_pos = result.find("# AGENTS.md").unwrap();

        assert!(system_pos < toolset_pos);
        assert!(toolset_pos < skill_pos);
        assert!(skill_pos < context_pos);
    }

    // ---- resolve_turn_context tests ----

    use crate::event::ChatEvent;
    use crate::store::InMemoryConfigStore;

    struct NoopSink;

    #[async_trait::async_trait]
    impl EventSink for NoopSink {
        async fn emit(&self, _event: ChatEvent) {}
    }

    fn test_ctx(store: InMemoryConfigStore) -> SessionContext {
        SessionContext {
            store: Arc::new(store),
            sink: Arc::new(NoopSink),
            local_tools: HashMap::new(),
            subagent_depth_max: 1,
        }
    }

    // 1. success — full chain resolves, prompt contains agent prompt + toolset prompt
    #[tokio::test]
    async fn resolve_turn_context_success() {
        let store = InMemoryConfigStore {
            providers: vec![Provider {
                name: "ollama-local".to_string(),
                provider_type: ProviderType::Ollama,
                endpoint: "http://localhost:11434".to_string(),
                api_key: None,
            }],
            models: vec![ModelProfile {
                name: "qwen2.5".to_string(),
                provider: "ollama-local".to_string(),
                model: "qwen2.5".to_string(),
                temperature: None,
                max_tokens: None,
                options: serde_json::Map::new(),
            }],
            mcp_servers: vec![],
            toolsets: vec![Toolset {
                name: "default".to_string(),
                description: None,
                prompt: Some("TOOLSET_PROMPT".to_string()),
                local_tools: vec![],
                mcp: vec![],
            }],
            agents: vec![Agent {
                name: "test-agent".to_string(),
                description: None,
                mode: AgentMode::Primary,
                model: "qwen2.5".to_string(),
                toolsets: vec!["default".to_string()],
                skills: SkillSelection::Auto,
                system_prompt: "You are helpful.".to_string(),
            }],
            skills: vec![SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            }],
            ..Default::default()
        };
        let ctx = test_ctx(store);
        let resolved = resolve_turn_context(&ctx, "test-agent", None).await.unwrap();
        assert!(resolved.system_prompt.contains("You are helpful."));
        assert!(resolved.system_prompt.contains("TOOLSET_PROMPT"));
        assert_eq!(resolved.resolved_toolsets.len(), 1);
    }

    // 2. unknown agent — empty store
    #[tokio::test]
    async fn resolve_turn_context_unknown_agent() {
        let store = InMemoryConfigStore::default();
        let ctx = test_ctx(store);
        let result = resolve_turn_context(&ctx, "nope", None).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("VNL-CFG-003"));
    }

    // 3. unknown model — agent references non-existent model
    #[tokio::test]
    async fn resolve_turn_context_unknown_model() {
        let store = InMemoryConfigStore {
            providers: vec![Provider {
                name: "p".to_string(),
                provider_type: ProviderType::Ollama,
                endpoint: "http://localhost:11434".to_string(),
                api_key: None,
            }],
            models: vec![],
            mcp_servers: vec![],
            toolsets: vec![],
            agents: vec![Agent {
                name: "broken-model".to_string(),
                description: None,
                mode: AgentMode::Primary,
                model: "nonexistent-model".to_string(),
                toolsets: vec![],
                skills: SkillSelection::Auto,
                system_prompt: "prompt".to_string(),
            }],
            skills: vec![],
            ..Default::default()
        };
        let ctx = test_ctx(store);
        let result = resolve_turn_context(&ctx, "broken-model", None).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("VNL-CFG-003"));
    }

    // 4. unknown provider — model references non-existent provider
    #[tokio::test]
    async fn resolve_turn_context_unknown_provider() {
        let store = InMemoryConfigStore {
            providers: vec![],
            models: vec![ModelProfile {
                name: "m".to_string(),
                provider: "broken-provider".to_string(),
                model: "qwen2.5".to_string(),
                temperature: None,
                max_tokens: None,
                options: serde_json::Map::new(),
            }],
            mcp_servers: vec![],
            toolsets: vec![],
            agents: vec![Agent {
                name: "broken-model".to_string(),
                description: None,
                mode: AgentMode::Primary,
                model: "m".to_string(),
                toolsets: vec![],
                skills: SkillSelection::Auto,
                system_prompt: "prompt".to_string(),
            }],
            skills: vec![],
            ..Default::default()
        };
        let ctx = test_ctx(store);
        let result = resolve_turn_context(&ctx, "broken-model", None).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("VNL-CFG-003"));
    }

    // 5. unknown toolset — agent references non-existent toolset
    #[tokio::test]
    async fn resolve_turn_context_unknown_toolset() {
        let store = InMemoryConfigStore {
            providers: vec![Provider {
                name: "ollama-local".to_string(),
                provider_type: ProviderType::Ollama,
                endpoint: "http://localhost:11434".to_string(),
                api_key: None,
            }],
            models: vec![ModelProfile {
                name: "qwen2.5".to_string(),
                provider: "ollama-local".to_string(),
                model: "qwen2.5".to_string(),
                temperature: None,
                max_tokens: None,
                options: serde_json::Map::new(),
            }],
            mcp_servers: vec![],
            toolsets: vec![],
            agents: vec![Agent {
                name: "test-agent".to_string(),
                description: None,
                mode: AgentMode::Primary,
                model: "qwen2.5".to_string(),
                toolsets: vec!["missing".to_string()],
                skills: SkillSelection::Auto,
                system_prompt: "prompt".to_string(),
            }],
            skills: vec![],
            ..Default::default()
        };
        let ctx = test_ctx(store);
        let result = resolve_turn_context(&ctx, "test-agent", None).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("VNL-CFG-003"));
    }

    // 6. workspace_context included
    #[tokio::test]
    async fn resolve_turn_context_workspace_context_included() {
        let store = InMemoryConfigStore {
            providers: vec![Provider {
                name: "ollama-local".to_string(),
                provider_type: ProviderType::Ollama,
                endpoint: "http://localhost:11434".to_string(),
                api_key: None,
            }],
            models: vec![ModelProfile {
                name: "qwen2.5".to_string(),
                provider: "ollama-local".to_string(),
                model: "qwen2.5".to_string(),
                temperature: None,
                max_tokens: None,
                options: serde_json::Map::new(),
            }],
            mcp_servers: vec![],
            toolsets: vec![Toolset {
                name: "default".to_string(),
                description: None,
                prompt: Some("TOOLSET_PROMPT".to_string()),
                local_tools: vec![],
                mcp: vec![],
            }],
            agents: vec![Agent {
                name: "test-agent".to_string(),
                description: None,
                mode: AgentMode::Primary,
                model: "qwen2.5".to_string(),
                toolsets: vec!["default".to_string()],
                skills: SkillSelection::Auto,
                system_prompt: "You are helpful.".to_string(),
            }],
            skills: vec![SkillMeta {
                name: "pdf".to_string(),
                description: "PDF processing".to_string(),
            }],
            ..Default::default()
        };
        let ctx = test_ctx(store);
        let result = resolve_turn_context(&ctx, "test-agent", Some("# AGENTS.md")).await.unwrap();
        assert!(result.system_prompt.ends_with("# AGENTS.md"));
    }

    // 8. build_agent_sets_default_max_turns
    #[test]
    fn build_agent_sets_default_max_turns() {
        let provider = crate::domain::Provider {
            name: "p".to_string(),
            provider_type: crate::domain::ProviderType::OpenaiCompatible,
            endpoint: "http://localhost:1".to_string(),
            api_key: None,
        };
        let profile = crate::domain::ModelProfile {
            name: "m".to_string(),
            provider: "p".to_string(),
            model: "qwen2.5".to_string(),
            temperature: None,
            max_tokens: None,
            options: serde_json::Map::new(),
        };
        let model = crate::model::build_openai_compat_model(&provider, &profile).unwrap();
        let params = crate::model::agent_params(&profile);
        let handle = crate::new_tool_handle();
        let agent = build_agent(model, &params, "system prompt", handle);
        assert_eq!(agent.default_max_turns, Some(DEFAULT_MAX_TURNS));
    }
}
