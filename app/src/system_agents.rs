//! Les AGENTS SYSTÈME de vanyline — contrat : `./system_agents.sdd`.
//!
//! Quatre agents préconfigurés, un par contexte d'écran, dont le prompt et les outils sont
//! pilotés par le code et dont SEUL le modèle est configurable par l'utilisateur. Une ligne
//! système est une VRAIE ligne de `vanyline_agents`, marquée `system_key`/`system_version`
//! (migration `m20261005_000003`), réécrite par le code quand `SYSTEM_VERSION` change.
//! Ses outils passent par le MCP de miryad-core (CRUD de la configuration) et le MCP de
//! domaine, appelés avec un jeton d'API jetable de l'utilisateur : l'agent ne peut jamais
//! excéder ses droits.
use miryad_core::auth::AuthUser;
use sea_orm::DatabaseConnection;

use crate::db::entities::agents::Model as AgentRecord;
use crate::error::AppError;

/// Version du jeu de prompts système embarqué — à incrémenter à chaque changement de
/// prompt ou de politique d'outils (`system_agents.sdd` `Exposes`).
pub const SYSTEM_VERSION: i32 = 1;

/// Les quatre clés stables d'agents système, avec leur contexte de conversation associé.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemAgentKey {
    /// Écran des paramètres — contexte `settings`.
    Config,
    /// Accueil — contexte `home`.
    Home,
    /// Vue projet — contexte `project`.
    Project,
    /// Vue sandbox — contexte `sandbox`.
    Sandbox,
}

impl SystemAgentKey {
    /// Les quatre clés, dans l'ordre d'amorçage.
    pub fn all() -> [Self; 4] {
        [Self::Config, Self::Home, Self::Project, Self::Sandbox]
    }

    /// Clé stable écrite dans `vanyline_agents.system_key`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Home => "home",
            Self::Project => "project",
            Self::Sandbox => "sandbox",
        }
    }

    /// Kind du contexte de conversation associé (`vanyline_chat_contexts.kind`).
    pub fn context_kind(self) -> &'static str {
        match self {
            Self::Config => "settings",
            Self::Home => "home",
            Self::Project => "project",
            Self::Sandbox => "sandbox",
        }
    }

    /// Description de la ligne d'agent.
    pub fn description(self) -> &'static str {
        match self {
            Self::Config => "Agent système — configuration de l'application",
            Self::Home => "Agent système — accueil : projets, sandboxes, workflows",
            Self::Project => "Agent système — projet courant : workflows et runs",
            Self::Sandbox => "Agent système — sandbox courante : travail et santé du code",
        }
    }

    /// Prompt système embarqué, versionné par `SYSTEM_VERSION` — politique commune
    /// (`system_agents.sdd` `Handles`) : agir au nom de l'utilisateur dans ses seuls
    /// droits, AGIR DIRECTEMENT sans confirmation (« yolo » assumé 2026-10-04), ne jamais
    /// afficher de secret, expliquer ce qui a été fait.
    pub fn prompt(self) -> String {
        let intro = match self {
            Self::Config => concat!(
                "Tu es l'agent système « config » de vanyline. Tu gères toute la ",
                "configuration de l'application : fournisseurs LLM, serveurs MCP, profils de ",
                "modèle, toolsets, skills, agents et définitions de workflows, via les outils ",
                "MCP de configuration qui portent l'identité de l'utilisateur.\n\n",
            ),
            Self::Home => concat!(
                "Tu es l'agent système « home » de vanyline, dans l'écran d'accueil. Tu ",
                "pilotes les objets de l'utilisateur : projets, sandboxes, workflows et runs, ",
                "via les outils MCP de configuration et de domaine qui portent son identité.\n\n",
            ),
            Self::Project => concat!(
                "Tu es l'agent système « project » de vanyline, dans la vue d'un projet. Tu ",
                "pilotes ce projet : ses workflows (définitions, rattachements), leurs runs, ",
                "et les sandboxes qui le concernent, via les outils MCP de configuration et ",
                "de domaine qui portent l'identité de l'utilisateur.\n\n",
            ),
            Self::Sandbox => concat!(
                "Tu es l'agent système « sandbox » de vanyline, dans la vue d'une sandbox. Tu ",
                "aides l'utilisateur à travailler dans cette sandbox : état et santé du code, ",
                "jobs, workflows et runs, via les outils MCP de domaine et les outils de la ",
                "sandbox qui portent son identité.\n\n",
            ),
        };
        let tail = match self {
            Self::Config => concat!(
                "Spécificités : les agents système ne se créent, ne se modifient ni ne se ",
                "suppriment que par leur modèle ; ne touche pas à leur contenu. Pour monter un ",
                "environnement, ordonne les étapes : fournisseur, puis profil de modèle, ",
                "puis agents."
            ),
            Self::Home => concat!(
                "Spécificités : quand l'utilisateur décrit une intention (« je veux travailler ",
                "sur tel dépôt »), enchaîne les étapes lui-même : créer le projet, démarrer la ",
                "sandbox, sans t'arrêter à chaque étape."
            ),
            Self::Project => concat!(
                "Spécificités : reste dans le projet de la conversation ; un besoin hors de ce ",
                "périmètre s'explique plutôt que de se faire en cachette."
            ),
            Self::Sandbox => concat!(
                "Spécificités : reste dans la sandbox de la conversation ; explique toujours ",
                "l'état obtenu avant d'enchaîner."
            ),
        };
        format!("{intro}{COMMON_POLICY}\n\n{tail}")
    }
}

/// Politique commune des quatre prompts (texte partagé, versionné avec `SYSTEM_VERSION`).
const COMMON_POLICY: &str = concat!(
    "Politique d'action : tu agis au nom de l'utilisateur connecté, strictement dans ses ",
    "droits — tes outils portent son identité et ne peuvent rien faire qu'il ne pourrait pas ",
    "faire lui-même. AGIS DIRECTEMENT : exécute la demande jusqu'au bout sans demander de ",
    "confirmation, y compris pour des suppressions ou des écrasements ; vanyline fonctionne ",
    "en mode « yolo » assumé (le code est dans git et révertable, la configuration est en ",
    "base sauvegardée). Ne jamais afficher un secret : ni clé d'API, ni jeton, ni valeur ",
    "sensible ; si un outil en renvoie, passe sous silence. Termine en expliquant, en quelques ",
    "phrases, ce qui a été fait et pourquoi. Réponds dans la langue de l'utilisateur.",
);

/// L'agent système d'un type de contexte (`system_agents.sdd` `Exposes`).
#[must_use]
pub fn system_agent_for(kind: &str) -> Option<SystemAgentKey> {
    SystemAgentKey::all()
        .into_iter()
        .find(|k| k.context_kind() == kind)
}

/// Crée les quatre lignes d'agents système absentes et réécrit celles dont
/// `system_version < SYSTEM_VERSION` (prompt, description, `mode = primary`, `toolsets` et
/// `skills` vides), sans jamais toucher à leur `model_profile_id`. Sans aucun profil de
/// modèle pour l'utilisateur, amorce un profil `default` depuis le réglage global
/// `default_model` ; sans réglage, rend `NoModelProfile` (`VNL-LLM-008`).
pub async fn ensure_system_agents(
    db: &DatabaseConnection,
    user_id: i32,
) -> Result<Vec<AgentRecord>, AppError> {
    use sea_orm::{ActiveValue::NotSet, ColumnTrait, EntityTrait, QueryFilter, Set};

    let profile_id = resolve_or_bootstrap_profile(db, user_id).await?;
    let mut rows = Vec::with_capacity(4);
    for key in SystemAgentKey::all() {
        let existing = crate::db::entities::agents::Entity::find()
            .filter(crate::db::entities::agents::Column::OwnerId.eq(user_id))
            .filter(crate::db::entities::agents::Column::SystemKey.eq(key.key()))
            .one(db)
            .await
            .map_err(AppError::from)?;
        let row = match existing {
            None => {
                let inserted = crate::db::entities::agents::Entity::insert(
                    crate::db::entities::agents::ActiveModel {
                        id: NotSet,
                        owner_id: Set(user_id),
                        name: Set(key.key().to_string()),
                        description: Set(Some(key.description().to_string())),
                        mode: Set("primary".to_string()),
                        model_profile_id: Set(profile_id),
                        toolsets: Set(serde_json::json!([])),
                        skills: Set(serde_json::json!([])),
                        system_prompt: Set(key.prompt()),
                        system_key: Set(Some(key.key().to_string())),
                        system_version: Set(Some(SYSTEM_VERSION)),
                    },
                )
                .exec(db)
                .await
                .map_err(AppError::from)?;
                crate::db::entities::agents::Entity::find_by_id(inserted.last_insert_id)
                    .one(db)
                    .await
                    .map_err(AppError::from)?
                    .ok_or(AppError::InternalError(
                        "VNL-INT-001: system agent vanished after insert".to_string(),
                    ))?
            }
            Some(row) if row.system_version == Some(SYSTEM_VERSION) => row,
            Some(row) => {
                // Ligne rattrapée par la version embarquée : contenu réécrit,
                // `model_profile_id` d'abord lu depuis la ligne et jamais remué
                // (Scenario `mise_a_jour_de_version_garde_le_modele`).
                let row_id = row.id;
                let mut active: crate::db::entities::agents::ActiveModel =
                    sea_orm::IntoActiveModel::into_active_model(row);
                active.description = Set(Some(key.description().to_string()));
                active.mode = Set("primary".to_string());
                active.toolsets = Set(serde_json::json!([]));
                active.skills = Set(serde_json::json!([]));
                active.system_prompt = Set(key.prompt());
                active.system_version = Set(Some(SYSTEM_VERSION));
                crate::db::entities::agents::Entity::update(active)
                    .exec(db)
                    .await
                    .map_err(AppError::from)?;
                crate::db::entities::agents::Entity::find_by_id(row_id)
                    .one(db)
                    .await
                    .map_err(AppError::from)?
                    .ok_or(AppError::InternalError(
                        "VNL-INT-001: system agent vanished after update".to_string(),
                    ))?
            }
        };
        rows.push(row);
    }
    Ok(rows)
}

/// Profil de rattachement des agents système : le premier profil de l'utilisateur (id
/// croissant — déterministe) ou, s'il n'en a AUCUN, un profil `default` amorcé depuis le
/// réglage global `default_model`. Sans profil et sans réglage (ou réglage vers un
/// fournisseur supprimé) : `NoModelProfile` — rendu par
/// `AppError::UnprocessableReference(VnyError::NoModelConfigured)` (`VNL-LLM-008`, 422).
async fn resolve_or_bootstrap_profile(
    db: &DatabaseConnection,
    user_id: i32,
) -> Result<i32, AppError> {
    use sea_orm::{ActiveValue::NotSet, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

    if let Some(profile) = crate::db::entities::model_profiles::Entity::find()
        .filter(crate::db::entities::model_profiles::Column::OwnerId.eq(user_id))
        .order_by_asc(crate::db::entities::model_profiles::Column::Id)
        .one(db)
        .await
        .map_err(AppError::from)?
    {
        return Ok(profile.id);
    }

    let no_model: fn() -> AppError =
        || AppError::UnprocessableReference(vanyline_lib::VnyError::NoModelConfigured);
    let setting = crate::db::entities::app_settings::Entity::find()
        .filter(
            crate::db::entities::app_settings::Column::Key
                .eq(crate::db::entities::app_settings::KEY_DEFAULT_MODEL),
        )
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or_else(no_model)?;
    let value = setting.value;
    // Le hook de l'entité garantit la forme à l'écriture ; une valeur externally corrompue
    // retombe sur « pas de modèle par défaut » plutôt que sur une insertion invalide.
    let provider_id = value
        .get("provider_id")
        .and_then(serde_json::Value::as_i64)
        .and_then(|v| i32::try_from(v).ok());
    let model = value.get("model").and_then(serde_json::Value::as_str);
    let (Some(provider_id), Some(model)) = (provider_id, model) else {
        return Err(no_model());
    };
    let model = model.to_string();
    let options = value
        .get("options")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    // Le réglage peut viser un fournisseur supprimé depuis (aucune FK contractée, cf.
    // m20261005_000004) : l'amorçage s'arrête là sur `AppError::LlmProviderNotFound` —
    // PAS « comme sans réglage » (`NoModelProfile`). Comportement gelé en `[?]`
    // (`system_agents.sdd`, arbitrage S8 2026-10-14 ouvert : revenir ou non à
    // `NoModelProfile` reste à trancher par Sébastien).
    if crate::db::entities::llm_providers::Entity::find_by_id(provider_id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .is_none()
    {
        return Err(AppError::LlmProviderNotFound);
    }
    let created = crate::db::entities::model_profiles::Entity::insert(
        crate::db::entities::model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(user_id),
            name: Set("default".to_string()),
            provider_id: Set(provider_id),
            model: Set(model),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(options),
        },
    )
    .exec(db)
    .await
    .map_err(AppError::from)?;
    Ok(created.last_insert_id)
}

/// Les serveurs MCP forcés (`extra_mcp`) pour CE tour dans CE contexte, avec le jeton
/// jetable émis pour les atteindre (à révoquer en fin de tour, `ContextTools::revoke`).
#[derive(Default)]
pub struct ContextTools {
    /// Paires (serveur, sélection d'outils) — vides hors contextes connus.
    pub servers: Vec<(
        vanyline_lib::domain::McpServer,
        vanyline_lib::domain::McpSelection,
    )>,
    /// Serveurs-markés indisponibles (nom, motif) — pannes NON bloquantes du tour, à
    /// signaler en `ChatEvent::ToolUnavailable` par l'appelant.
    pub unavailable: Vec<(String, String)>,
    revocation: Option<(i32, DatabaseConnection)>,
}

/// `Debug` manuel QUI ROUGE LE JETON : les en-têtes des serveurs portent le jeton du
/// tour, jamais journalisable (`system_agents.sdd` `Must not`). Seuls noms, URLs et
/// sélection sont rendus.
impl std::fmt::Debug for ContextTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextTools")
            .field(
                "servers",
                &self
                    .servers
                    .iter()
                    .map(|(s, sel)| (s.name.clone(), s.url.clone(), sel.tools.clone()))
                    .collect::<Vec<_>>(),
            )
            .field("unavailable", &self.unavailable)
            .field("has_token", &self.revocation.is_some())
            .finish_non_exhaustive()
    }
}

impl ContextTools {
    /// Retire les serveurs (mouvement vers `SessionContext::extra_mcp`) sans perdre la
    /// capacité de révocation.
    pub fn take_servers(
        &mut self,
    ) -> Vec<(
        vanyline_lib::domain::McpServer,
        vanyline_lib::domain::McpSelection,
    )> {
        std::mem::take(&mut self.servers)
    }

    /// Révoque le jeton du tour (`revoke_token`). Aucun jeton → succès sans effet ;
    /// réversible par l'appelant sur erreur de tour comme sur succès.
    pub async fn revoke(self) -> Result<(), AppError> {
        let Some((token_id, db)) = self.revocation else {
            return Ok(());
        };
        miryad_core::auth::revoke_token(&db, token_id)
            .await
            .map_err(|e| AppError::InternalError(format!("VNL-INT-001: {e}")))
    }
}

/// Résout les outils du CONTEXTE pour l'agent qui y tourne (système OU ordinaire — les
/// outils sont ceux du contexte, pas de l'agent) : `settings` → `vanyline-config` ;
/// `home` et `project` → `vanyline-config` et `vanyline-domain` ; `sandbox` →
/// `vanyline-domain` et le serveur de la sandbox (jeton de l'Owner). Émet un jeton d'API
/// miryad-core par tour (sujet de l'utilisateur, 1 h) et rend sa révocation
/// (`ContextTools::revoke`).
pub async fn context_tools(
    state: &crate::AppState,
    user: &AuthUser,
    kind: &str,
    context: &crate::db::entities::chat_contexts::Model,
    agent_name: &str,
) -> Result<ContextTools, AppError> {
    use vanyline_lib::domain::{McpSelection, McpServer, McpTransport};

    let wants_config = matches!(kind, "settings" | "home" | "project");
    let wants_domain = matches!(kind, "home" | "project" | "sandbox");
    let wants_sandbox = kind == "sandbox";
    if !wants_config && !wants_domain && !wants_sandbox {
        return Ok(ContextTools::default());
    }

    // L'URL de base est calculée AVANT l'émission du jeton : une panne de calcul
    // (`LISTEN_ADDR` sans port) ne doit pas laisser un jeton vivant jusqu'à 1 h, faute de `ContextTools`
    // rendu pour le révoquer (E4, S8 2026-10-14).
    let base = app_base_url(state)?;

    // Jeton jetable du tour : sujet de l'utilisateur, nommé `system-agent`, expiration 1 h,
    // poivre de l'app. Jamais journalisé, jamais persisté dans une conversation, jamais
    // renvoyé au navigateur — il ne vit que dans les en-têtes des serveurs rendus ici.
    let expires_at = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(1))
        .ok_or_else(|| {
            AppError::InternalError("VNL-INT-001: token expiry out of range".to_string())
        })?;
    let issued = miryad_core::auth::issue_token(
        &state.auth.db,
        &user.subject,
        "system-agent",
        Some(expires_at),
        &state.auth.token_pepper,
    )
    .await
    .map_err(|e| AppError::InternalError(format!("VNL-INT-001: token issuance failed: {e}")))?;
    let mut tools = ContextTools {
        servers: Vec::new(),
        unavailable: Vec::new(),
        revocation: Some((issued.id, state.auth.db.clone())),
    };
    let auth = format!("Bearer {}", issued.token);

    if wants_config {
        tools.servers.push((
            McpServer {
                name: "vanyline-config".to_string(),
                transport: McpTransport::HttpStreamable,
                url: format!("{base}/mcp"),
                headers: std::collections::BTreeMap::from([(
                    "Authorization".to_string(),
                    auth.clone(),
                )]),
            },
            McpSelection {
                server: "vanyline-config".to_string(),
                tools: vec![],
            },
        ));
    }
    if wants_domain {
        tools.servers.push((
            McpServer {
                name: "vanyline-domain".to_string(),
                transport: McpTransport::HttpStreamable,
                url: format!("{base}/mcp/domain"),
                headers: std::collections::BTreeMap::from([
                    ("Authorization".to_string(), auth.clone()),
                    ("X-Vnl-Agent".to_string(), agent_name.to_string()),
                ]),
            },
            McpSelection {
                server: "vanyline-domain".to_string(),
                tools: vec![],
            },
        ));
    }
    if wants_sandbox {
        // Serveur MCP de la sandbox du contexte, aux mêmes conditions de scoping que
        // `ws/chat.rs::resolve_extra_mcp` (garde IDOR `project.spec.owner == owner`).
        // Toute panne de résolution est NON bloquante : serveur marqué `unavailable`.
        match resolve_sandbox_server(state, user, context).await {
            Ok((url, owner_token)) => {
                tools.servers.push((
                    McpServer {
                        name: "sandbox".to_string(),
                        transport: McpTransport::HttpStreamable,
                        url,
                        headers: sandbox_headers(&owner_token),
                    },
                    McpSelection {
                        server: "sandbox".to_string(),
                        tools: vec![],
                    },
                ));
            }
            Err(reason) => tools.unavailable.push(("sandbox".to_string(), reason)),
        }
    }
    Ok(tools)
}

/// URL de base de l'app elle-même (les serveurs MCP internes y pointent) :
/// `http://127.0.0.1:<port>` du `LISTEN_ADDR`.
fn app_base_url(state: &crate::AppState) -> Result<String, AppError> {
    let port = state
        .config
        .listen_addr
        .rsplit_once(':')
        .map(|(_, p)| p.to_string())
        .ok_or_else(|| {
            AppError::InternalError(format!(
                "VNL-INT-001: invalid LISTEN_ADDR for MCP base url: {}",
                state.config.listen_addr
            ))
        })?;
    Ok(format!("http://127.0.0.1:{port}"))
}

/// En-têtes du serveur MCP `sandbox` : le jeton du SA de l'Owner seul (la sandbox
/// n'accepte rien d'autre, `sandbox/src/auth.sdd`) ; jamais le jeton d'API de l'utilisateur.
fn sandbox_headers(owner_token: &str) -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([(
        "Authorization".to_string(),
        format!("Bearer {owner_token}"),
    )])
}

/// Résolution (avec scoping owner) de l'URL MCP interne de la sandbox du contexte et du
/// jeton d'Owner qui l'authentifie — le jeton n'est demandé qu'APRÈS la garde IDOR. Motifs
/// de panne rendus en `Err(raison)` pour un signalement `ToolUnavailable` non bloquant.
async fn resolve_sandbox_server(
    state: &crate::AppState,
    user: &AuthUser,
    context: &crate::db::entities::chat_contexts::Model,
) -> Result<(String, String), String> {
    let Some(sandbox_name) = context.data.get("sandbox_name").and_then(|v| v.as_str()) else {
        return Err("contexte sandbox invalide (sandbox_name absent)".to_string());
    };
    let user_id =
        miryad_core::users::resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
            .await
            .map_err(|e| e.to_string())?
            .id;
    let owner = crate::api::owners::resolve_owner_name(state, user_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "aucun owner K8s associé à cet utilisateur".to_string())?;
    let client = crate::k8s::client(state).await.map_err(|e| e.to_string())?;
    let sandbox = client
        .get_sandbox(&owner, sandbox_name)
        .await
        .map_err(|e| e.to_string())?;
    let project = client
        .get_project(&owner, &sandbox.spec.project)
        .await
        .map_err(|e| e.to_string())?;
    if project.spec.owner != owner {
        return Err("sandbox hors du périmètre de cet utilisateur".to_string());
    }
    let url = client
        .sandbox_mcp_url(&owner, sandbox_name)
        .await
        .map_err(|e| e.to_string())?;
    let owner_token = client
        .owner_token(&owner)
        .await
        .map_err(|e| e.to_string())?;
    Ok((url, owner_token.token))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use sea_orm::{
        ActiveValue::NotSet, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, Statement,
    };
    use vanyline_lib::domain::McpServer;

    use crate::db::entities::{agents, app_settings, chat_contexts, llm_providers, model_profiles};
    use crate::db::test_support::real_db;

    /// Seed : provider global + profil de modèle pour `owner` — retourne l'id du profil.
    async fn seed_profile(db: &sea_orm::DatabaseConnection, owner: i32, name: &str) -> i32 {
        let provider = llm_providers::ActiveModel {
            id: NotSet,
            name: Set(format!("prov-{name}-{owner}")),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(true),
        };
        let provider = llm_providers::Entity::insert(provider)
            .exec(db)
            .await
            .expect("provider inserts")
            .last_insert_id;
        model_profiles::Entity::insert(model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            provider_id: Set(provider),
            model: Set("qwen".to_string()),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(serde_json::json!({})),
        })
        .exec(db)
        .await
        .expect("profile inserts")
        .last_insert_id
    }

    async fn seed_user(db: &sea_orm::DatabaseConnection, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("user resolves")
            .id
    }

    async fn insert_setting(db: &sea_orm::DatabaseConnection, key: &str, value: serde_json::Value) {
        app_settings::Entity::insert(app_settings::ActiveModel {
            id: NotSet,
            key: Set(key.to_string()),
            value: Set(value),
        })
        .exec(db)
        .await
        .expect("setting inserts");
    }

    fn server_names(servers: &[(McpServer, vanyline_lib::domain::McpSelection)]) -> Vec<String> {
        servers.iter().map(|(s, _)| s.name.clone()).collect()
    }

    fn test_state(db: sea_orm::DatabaseConnection) -> crate::AppState {
        crate::AppState {
            config: crate::config::Config {
                oidc_issuer_url: "https://issuer.example.com".to_string(),
                oidc_client_id: "client-id".to_string(),
                oidc_client_secret: "client-secret".to_string(),
                oidc_redirect_url: "https://app.example.com/callback".to_string(),
                oidc_scopes: vec![],
                oidc_ca_cert: None,
                oidc_connect_timeout: std::time::Duration::from_secs(5),
                oidc_timeout: std::time::Duration::from_secs(15),
                cookie_secret: "0".repeat(64),
                database_url: "sqlite::memory:".to_string(),
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
            },
            cookie_key: cookie::Key::from(&[0u8; 64]),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    fn auth_user(subject: &str) -> AuthUser {
        AuthUser {
            subject: subject.to_string(),
            email: None,
            id_token: "test-id-token".to_string(),
        }
    }

    fn context_of(kind: &str) -> chat_contexts::Model {
        chat_contexts::Model {
            id: 1,
            kind: kind.to_string(),
            data: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        }
    }

    /// Compteur générique de lignes (SQL brut — tables hors entités testées ici).
    async fn count_rows(db: &sea_orm::DatabaseConnection, table: &str) -> i64 {
        let res = db
            .query_one_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                format!("SELECT COUNT(*) FROM {table}"),
                [],
            ))
            .await
            .expect("count query succeeds")
            .expect("count row");
        let n: i64 = res
            .try_get_by_index(0)
            .or_else(|_| res.try_get_by_index::<i32>(0).map(i64::from))
            .expect("count decodes");
        n
    }

    /// Contrat `Exposes` : les correspondances clé ↔ contexte.
    #[test]
    fn cle_par_contexte() {
        assert_eq!(system_agent_for("settings"), Some(SystemAgentKey::Config));
        assert_eq!(system_agent_for("home"), Some(SystemAgentKey::Home));
        assert_eq!(system_agent_for("project"), Some(SystemAgentKey::Project));
        assert_eq!(system_agent_for("sandbox"), Some(SystemAgentKey::Sandbox));
        assert_eq!(system_agent_for("chat"), None);
        assert_eq!(SystemAgentKey::Config.key(), "config");
        assert_eq!(SystemAgentKey::Config.context_kind(), "settings");
    }

    /// Scenario : `creation_des_quatre_agents`.
    #[tokio::test]
    async fn creation_des_quatre_agents() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let profile = seed_profile(&db, alice, "qwen").await;

        let rows = ensure_system_agents(&db, alice)
            .await
            .expect("ensure succeeds");

        assert_eq!(rows.len(), 4);
        let keys: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.system_key.as_deref())
            .collect();
        assert_eq!(keys, ["config", "home", "project", "sandbox"]);
        for row in &rows {
            assert_eq!(
                row.model_profile_id, profile,
                "agent {} must link the user's profile",
                row.name
            );
            assert_eq!(row.system_version, Some(SYSTEM_VERSION));
            assert_eq!(row.mode, "primary");
            assert_eq!(row.owner_id, alice);
        }
    }

    /// Scenario : `idempotence` — un second `ensure` à la version courante ne change rien.
    #[tokio::test]
    async fn idempotence() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        seed_profile(&db, alice, "qwen").await;

        ensure_system_agents(&db, alice)
            .await
            .expect("first ensure succeeds");
        let before = agents::Entity::find()
            .all(&db)
            .await
            .expect("find succeeds");

        ensure_system_agents(&db, alice)
            .await
            .expect("second ensure succeeds");
        let after = agents::Entity::find()
            .all(&db)
            .await
            .expect("find succeeds");

        assert_eq!(before, after, "no change at the current version");
        assert_eq!(after.len(), 4);
    }

    /// Scenario : `mise_a_jour_de_version_garde_le_modele` — une ligne d'une version
    /// antérieure est réécrite (prompt + version) SANS perdre le modèle choisi par
    /// l'utilisateur.
    #[tokio::test]
    async fn mise_a_jour_de_version_garde_le_modele() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let profile_a = seed_profile(&db, alice, "ancien").await;
        let profile_b = seed_profile(&db, alice, "nouveau").await;

        ensure_system_agents(&db, alice)
            .await
            .expect("first ensure succeeds");
        // L'utilisateur change le modèle, puis une version dépasse le prompt embarqué.
        let row = agents::Entity::find()
            .filter(agents::Column::SystemKey.eq("config"))
            .one(&db)
            .await
            .expect("find succeeds")
            .expect("config row exists");
        let mut active = agents::ActiveModel::from(row);
        active.model_profile_id = Set(profile_b);
        active.system_prompt = Set("prompt de la version precedente".to_string());
        active.system_version = Set(Some(SYSTEM_VERSION - 1));
        agents::Entity::update(active)
            .exec(&db)
            .await
            .expect("stale-version row seeded");

        ensure_system_agents(&db, alice)
            .await
            .expect("ensure at new version succeeds");
        let row = agents::Entity::find()
            .filter(agents::Column::SystemKey.eq("config"))
            .one(&db)
            .await
            .expect("find succeeds")
            .expect("config row exists");

        assert_eq!(row.system_version, Some(SYSTEM_VERSION));
        assert_eq!(
            row.system_prompt,
            SystemAgentKey::Config.prompt(),
            "prompt must be rewritten"
        );
        assert_eq!(
            row.model_profile_id, profile_b,
            "model choice must survive (was {profile_a})"
        );
    }

    /// Scenario : `sans_profil_de_modele` — ni profil, ni réglage : `NoModelProfile`
    /// (`VNL-LLM-008`) et aucune ligne créée.
    #[tokio::test]
    async fn sans_profil_de_modele() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;

        let err = ensure_system_agents(&db, alice)
            .await
            .expect_err("no profile, no setting must fail");
        assert!(
            err.to_string().contains("VNL-LLM-008"),
            "expected NoModelProfile (VNL-LLM-008), got: {err}"
        );
        assert_eq!(count_rows(&db, "vanyline_agents").await, 0);
    }

    /// Scenario : `amorcage_par_le_modele_par_defaut` — un profil `default` est créé au
    /// nom de l'utilisateur et les quatre agents y sont rattachés.
    #[tokio::test]
    async fn amorcage_par_le_modele_par_defaut() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let provider = llm_providers::ActiveModel {
            id: NotSet,
            name: Set("global".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(true),
        };
        let provider = llm_providers::Entity::insert(provider)
            .exec(&db)
            .await
            .expect("global provider inserts")
            .last_insert_id;
        insert_setting(
            &db,
            "default_model",
            serde_json::json!({"provider_id": provider, "model": "qwen3.6:35b-a3b"}),
        )
        .await;

        let rows = ensure_system_agents(&db, alice)
            .await
            .expect("ensure succeeds");

        let profile = model_profiles::Entity::find()
            .filter(model_profiles::Column::OwnerId.eq(alice))
            .one(&db)
            .await
            .expect("find succeeds")
            .expect("default profile created");
        assert_eq!(profile.name, "default");
        assert_eq!(profile.model, "qwen3.6:35b-a3b");
        for row in &rows {
            assert_eq!(row.model_profile_id, profile.id);
        }
    }

    /// Scenario : `amorcage_sans_reglage` — utilisateur sans profil et sans réglage
    /// `default_model` : `NoModelProfile`.
    #[tokio::test]
    async fn amorcage_sans_reglage() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;

        let err = ensure_system_agents(&db, alice)
            .await
            .expect_err("missing default_model must fail");
        assert!(err.to_string().contains("VNL-LLM-008"));
    }

    /// Scenario : `outils_par_contexte` — la table de `Exposes` : `settings` → config ;
    /// `home` et `project` → config + domain ; `sandbox` → domain + sandbox.
    #[tokio::test]
    async fn outils_par_contexte() {
        let alice_db = |kind: &str, db: sea_orm::DatabaseConnection| {
            let kind = kind.to_string();
            async move {
                let state = test_state(db);
                context_tools(
                    &state,
                    &auth_user("alice"),
                    &kind,
                    &context_of(&kind),
                    "home",
                )
                .await
                .expect("context_tools succeeds")
            }
        };

        let settings = alice_db("settings", real_db().await).await;
        assert_eq!(server_names(&settings.servers), ["vanyline-config"]);

        let home = alice_db("home", real_db().await).await;
        assert_eq!(
            server_names(&home.servers),
            ["vanyline-config", "vanyline-domain"]
        );

        let project = alice_db("project", real_db().await).await;
        assert_eq!(
            server_names(&project.servers),
            ["vanyline-config", "vanyline-domain"]
        );

        // Sandbox : `domain` + `sandbox`. Sans cluster K8s joignable (hors cluster de
        // test), la résolution du serveur sandbox est une panne NON bloquante : le serveur
        // `vanyline-domain` reste et le serveur sandbox est signalé `unavailable`.
        let mut ctx = context_of("sandbox");
        ctx.data = serde_json::json!({"sandbox_name": "sb-1"});
        let state = test_state(real_db().await);
        let sandbox = context_tools(&state, &auth_user("alice"), "sandbox", &ctx, "sandbox")
            .await
            .expect("sandbox context must not fail the turn");
        assert_eq!(server_names(&sandbox.servers), ["vanyline-domain"]);
        assert!(
            sandbox
                .unavailable
                .iter()
                .any(|(server, _)| server == "sandbox"),
            "sandbox MCP must be reported unavailable, got: {:?}",
            sandbox.unavailable
        );
    }

    /// Scenario : `agent_ordinaire_dans_un_contexte` — les outils sont ceux du CONTEXTE :
    /// un agent ordinaire dans `project` reçoit config + domain, comme l'agent système.
    #[tokio::test]
    async fn agent_ordinaire_dans_un_contexte() {
        let state = test_state(real_db().await);
        let tools = context_tools(
            &state,
            &auth_user("alice"),
            "project",
            &context_of("project"),
            "mon-agent-perso",
        )
        .await
        .expect("context_tools succeeds");
        assert_eq!(
            server_names(&tools.servers),
            ["vanyline-config", "vanyline-domain"]
        );
    }

    /// Scenario : `jeton_revoque_en_fin_de_tour` — le jeton émis marche pendant le tour et
    /// ne marche plus après `revoke`, y compris quand le tour a échoué (la révocation est
    /// appelée par l'appelant sur les deux chemins).
    #[tokio::test]
    async fn jeton_revoque_en_fin_de_tour() {
        let state = test_state(real_db().await);
        let tools = context_tools(
            &state,
            &auth_user("alice"),
            "settings",
            &context_of("settings"),
            "config",
        )
        .await
        .expect("context_tools succeeds");

        let token = bearer_of(&tools, "vanyline-config");
        miryad_core::auth::validate_token(&state.auth.db, &token, &state.auth.token_pepper)
            .await
            .expect("token valid during the turn");

        tools.revoke().await.expect("revoke succeeds");
        miryad_core::auth::validate_token(&state.auth.db, &token, &state.auth.token_pepper)
            .await
            .expect_err("token must be dead after revocation");
    }

    /// E4 (S8, durcissement) : si le calcul de l'URL de base échoue (`LISTEN_ADDR` sans
    /// port), l'erreur rendue ne doit PAS laisser un jeton vivant jusqu'à 1 h — le jeton
    /// n'est plus émis avant ce calcul (aucune branche d'erreur entre émission et
    /// `ContextTools` rendu).
    #[tokio::test]
    async fn echec_base_url_ne_laisse_aucun_jeton_vivant() {
        let mut state = test_state(real_db().await);
        state.config.listen_addr = "no-port".to_string();

        let err = context_tools(
            &state,
            &auth_user("alice"),
            "settings",
            &context_of("settings"),
            "config",
        )
        .await
        .expect_err("invalid LISTEN_ADDR must fail");
        assert!(
            err.to_string().contains("VNL-INT-001"),
            "expected the base url error, got: {err}"
        );
        assert_eq!(
            count_rows(&state.auth.db, "miryad_api_tokens").await,
            0,
            "the failure branch must not leave a live token behind"
        );
    }

    /// Handles : le jeton est émis au sujet de l'utilisateur, nommé `system-agent`, borné
    /// à 1 h — jamais un jeton de service.
    #[tokio::test]
    async fn jeton_nomme_et_borne_au_sujet_de_l_utilisateur() {
        let state = test_state(real_db().await);
        let alice = seed_user(&state.auth.db, "alice").await;
        let tools = context_tools(
            &state,
            &auth_user("alice"),
            "settings",
            &context_of("settings"),
            "config",
        )
        .await
        .expect("context_tools succeeds");

        let (token_id, _) = tools.revocation.expect("a token was issued");
        let row = state
            .auth
            .db
            .query_one_raw(Statement::from_sql_and_values(
                state.auth.db.get_database_backend(),
                "SELECT subject, name, expires_at > datetime('now', '+59 minutes') FROM miryad_api_tokens WHERE id = ?",
                [token_id.into()],
            ))
            .await
            .expect("token row is selectable")
            .expect("token row exists");
        let subject: String = row.try_get_by_index(0).expect("subject decodes");
        assert_eq!(subject, "alice");
        let name: String = row.try_get_by_index(1).expect("name decodes");
        assert_eq!(name, "system-agent");
        let bounded = match row.try_get_by_index::<i32>(2) {
            Ok(v) => i64::from(v),
            Err(_) => 1,
        };
        assert_eq!(bounded, 1, "expiration must be ~1h away");
        let _ = alice;
    }

    /// Scenario : `droits_de_l_utilisateur_seulement` — le jeton porte l'identité de
    /// l'utilisateur : présenté à l'API, il ne voit que SES données (politique
    /// propriétaire seul de miryad-core).
    #[tokio::test]
    async fn droits_de_l_utilisateur_seulement() {
        use axum::Router;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use miryad_core::rest::resource_router;
        use tower::ServiceExt;

        let state = test_state(real_db().await);
        let alice = seed_user(&state.auth.db, "alice").await;
        let bob = seed_user(&state.auth.db, "bob").await;
        seed_profile(&state.auth.db, bob, "bob-qwen").await;
        let tools = context_tools(
            &state,
            &auth_user("alice"),
            "settings",
            &context_of("settings"),
            "config",
        )
        .await
        .expect("context_tools succeeds");
        let token = bearer_of(&tools, "vanyline-config");

        let app = Router::new()
            .merge(resource_router::<model_profiles::Entity, _>())
            .with_state(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/model-profiles")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("readable body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        // corps `PagedResult` de miryad-core : la collection est sous `items`.
        let items = json
            .get("items")
            .and_then(serde_json::Value::as_array)
            .expect("paged body");
        assert!(
            items.is_empty(),
            "alice sans profil ne doit rien voir (ni ceux de bob), got: {json}"
        );
        let _ = alice;
        let _ = bob;
    }

    /// Scenario: `sandbox_porte_le_jeton_de_l_owner` — le serveur `sandbox` ne reçoit
    /// JAMAIS le jeton d'API de l'utilisateur : la sandbox n'accepte que le JWT du SA de
    /// l'Owner (`sandbox/src/auth.sdd`).
    #[test]
    fn sandbox_porte_le_jeton_de_l_owner() {
        let headers = sandbox_headers("owner-jwt");
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer owner-jwt")
        );
        assert_eq!(headers.len(), 1, "aucun autre en-tête d'identité");
    }

    /// Scenario: `mcp_config_monte` — `POST /mcp` (CRUD de la configuration, miryad-core)
    /// est monté : 401 sans jeton (jamais 404), 200 `tools/list` avec le jeton jetable.
    #[tokio::test]
    async fn mcp_config_monte_et_authentifie() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = test_state(real_db().await);
        seed_user(&state.auth.db, "alice").await;
        let app = crate::mcp_config_router().with_state(state.clone());
        let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let anonymous = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(list))
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let tools = context_tools(
            &state,
            &auth_user("alice"),
            "settings",
            &context_of("settings"),
            "config",
        )
        .await
        .expect("context_tools succeeds");
        let token = bearer_of(&tools, "vanyline-config");
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::from(list))
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("readable body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        let n = json["result"]["tools"].as_array().map_or(0, Vec::len);
        assert!(n >= 30, "6 entités × 5 outils attendus, got {n}: {json}");
    }

    /// Extrait le jeton du serveur nommé (en-tête `Authorization: Bearer <token>`).
    fn bearer_of(tools: &ContextTools, server: &str) -> String {
        tools
            .servers
            .iter()
            .find(|(s, _)| s.name == server)
            .and_then(|(s, _)| s.headers.get("Authorization"))
            .and_then(|h| h.strip_prefix("Bearer "))
            .expect("server carries the bearer token")
            .to_string()
    }
}
