use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use sea_orm::{ActiveValue::NotSet, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};

use miryad_core::auth::AuthUser;
use miryad_core::users::resolve_user;

use crate::{
    AppState,
    db::entities::{
        chat_contexts::{
            ActiveModel as ChatContextActiveModel, Entity as ChatContextEntity,
            Model as ChatContextModel,
        },
        conversations::{
            ActiveModel as ConversationActiveModel, Entity as ConversationEntity,
            Model as ConversationModel,
        },
        messages::{Entity as MessageEntity, Model as MessageModel},
    },
    error::AppError,
};

/// Contexte transmis à la création d'une conversation. `kind = "sandbox"` est
/// le seul type géré aujourd'hui (`data = { "sandbox_name": "..." }`) — cf.
/// docs/features/chat-app-fonctionnel.md pour l'extensibilité prévue.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatContextInput {
    pub kind: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CreateConversation {
    pub agent_name: Option<String>,
    pub context: ChatContextInput,
}

/// Type alias pour la réponse de `get_messages` : le Model `SeaORM`
/// `vanyline_messages`.
type MessageOut = MessageModel;

#[derive(Debug, Clone, Serialize)]
pub struct ConversationOut {
    pub id: i32,
    pub agent_name: Option<String>,
    pub context: ChatContextModel,
    pub title: Option<String>,
    pub todo: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Résout `conv.agent_id` en nom (SELECT name FROM `vanyline_agents` WHERE id = $1),
/// charge le `ChatContext` depuis `vanyline_chat_contexts` (N+1 accepté, comme aujourd'hui).
async fn to_output(state: &AppState, conv: ConversationModel) -> Result<ConversationOut, AppError> {
    let db = &state.auth.db;

    let agent_name = match conv.agent_id {
        Some(id) => {
            use crate::db::entities::agents::Entity as AgentEntity;
            AgentEntity::find_by_id(id)
                .one(db)
                .await
                .map_err(AppError::from)?
                .map(|a| a.name)
        }
        None => None,
    };

    let context = ChatContextEntity::find_by_id(conv.context_id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::InternalError("VNL-DB-007: context not found".into()))?;

    Ok(ConversationOut {
        id: conv.id,
        agent_name,
        context,
        title: conv.title,
        todo: conv.todo,
        created_at: conv.created_at,
        updated_at: conv.updated_at,
    })
}

/// Résout `agent_name` en id depuis `vanyline_agents`
/// (WHERE `owner_id` = `user_id` AND name = `agent_name`).
/// Retourne `AppError::UnprocessableReference(UnknownReference("agent", name))`
/// si l'agent n'est pas trouvé.
async fn resolve_agent_id(
    state: &AppState,
    user_id: i32,
    agent_name: &str,
) -> Result<i32, AppError> {
    use crate::db::entities::agents::Column;
    use crate::db::entities::agents::Entity as AgentEntity;

    let db = &state.auth.db;
    let agent = AgentEntity::find()
        .filter(Column::OwnerId.eq(user_id))
        .filter(Column::Name.eq(agent_name))
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| {
            AppError::UnprocessableReference(vanyline_lib::VnyError::UnknownReference(
                "agent",
                agent_name.to_string(),
            ))
        })?;

    Ok(agent.id)
}

/// Filtre optionnel de `list_conversations`. `sandbox_name` est le seul
/// filtre géré aujourd'hui, à l'image du seul `kind = "sandbox"` supporté
/// côté création (cf. `ChatContextInput`).
#[derive(Debug, Deserialize)]
pub struct ListConversationsQuery {
    pub sandbox_name: Option<String>,
}

pub async fn list_conversations(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<ListConversationsQuery>,
) -> Result<Json<Vec<ConversationOut>>, AppError> {
    let db = &state.auth.db;
    let principal_user = resolve_user(db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;

    let conversations = ConversationEntity::find()
        .filter(crate::db::entities::conversations::Column::OwnerId.eq(principal_user.id))
        .order_by_desc(crate::db::entities::conversations::Column::UpdatedAt)
        .all(db)
        .await
        .map_err(AppError::from)?;

    let mut out: Vec<ConversationOut> = Vec::with_capacity(conversations.len());

    if let Some(ref sandbox_name) = query.sandbox_name {
        for conv in conversations {
            let ctx = ChatContextEntity::find_by_id(conv.context_id)
                .one(db)
                .await
                .map_err(AppError::from)?
                .ok_or_else(|| AppError::InternalError("VNL-DB-007: context not found".into()))?;

            if ctx.kind == "sandbox"
                && ctx.data.get("sandbox_name").and_then(|v| v.as_str()) == Some(sandbox_name)
            {
                out.push(to_output(&state, conv).await?);
            }
        }
    } else {
        for conv in conversations {
            out.push(to_output(&state, conv).await?);
        }
    }

    Ok(Json(out))
}

pub async fn create_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateConversation>,
) -> Result<(StatusCode, Json<ConversationOut>), AppError> {
    let db = &state.auth.db;
    let principal_user = resolve_user(db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;

    let agent_id = match &body.agent_name {
        Some(name) => Some(resolve_agent_id(&state, principal_user.id, name).await?),
        None => None,
    };

    // Amorçage à la création (`conversations.sdd` §Must, décidé 2026-10-14) : un contexte
    // système appelle `ensure_system_agents` (idempotent) AVANT de rendre la conversation.
    // En `settings` l'agent système `config` est exigé : la panne (ex. `NoModelProfile`)
    // est rendue telle quelle. Pour `home`/`project`/`sandbox` l'agent système n'est
    // qu'un défaut choisissable et `ws/chat.rs::resolve_extra_mcp` garde son amorçage au
    // tour (les deux chemins convergent) : une panne d'amorçage y est non bloquante,
    // consignée en warn — sinon elle contredirait les Scenario « 201 avec agent_name
    // null » et de régression des ids (création `sandbox` sans profil verrouillée).
    match crate::system_agents::system_agent_for(&body.context.kind) {
        Some(crate::system_agents::SystemAgentKey::Config) => {
            crate::system_agents::ensure_system_agents(db, principal_user.id).await?;
        }
        Some(_) => {
            if let Err(e) = crate::system_agents::ensure_system_agents(db, principal_user.id).await
            {
                tracing::warn!("amorçage des agents système non bloquant : {e}");
            }
        }
        None => {}
    }

    // Effect de bord : créer le ChatContext avant la Conversation
    let now = chrono::Utc::now();
    let ctx_active = ChatContextActiveModel {
        id: NotSet,
        kind: Set(body.context.kind.clone()),
        data: Set(body.context.data.clone()),
        created_at: Set(now),
    };
    let ctx_result = ChatContextEntity::insert(ctx_active)
        .exec(db)
        .await
        .map_err(AppError::from)?;
    let context_id = ctx_result.last_insert_id as i32;

    let conv_active = ConversationActiveModel {
        id: NotSet,
        owner_id: Set(principal_user.id),
        agent_id: Set(agent_id),
        context_id: Set(context_id),
        title: Set(None),
        todo: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let conv_result = ConversationEntity::insert(conv_active)
        .exec(db)
        .await
        .map_err(AppError::from)?;
    let conv = ConversationEntity::find_by_id(conv_result.last_insert_id as i32)
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| {
            AppError::InternalError("VNL-DB-008: created conversation not found".into())
        })?;

    let out = to_output(&state, conv).await?;
    Ok((StatusCode::CREATED, Json(out)))
}

pub async fn get_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<ConversationOut>, AppError> {
    let db = &state.auth.db;
    let principal_user = resolve_user(db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;

    let conv = ConversationEntity::find_by_id(id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or(AppError::ConversationNotFound)?;

    // RBAC : owner check à la main
    if conv.owner_id != principal_user.id {
        return Err(AppError::ConversationAccessDenied);
    }

    let out = to_output(&state, conv).await?;
    Ok(Json(out))
}

pub async fn delete_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<StatusCode, AppError> {
    let db = &state.auth.db;
    let principal_user = resolve_user(db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;

    let conv = ConversationEntity::find_by_id(id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or(AppError::ConversationNotFound)?;

    if conv.owner_id != principal_user.id {
        return Err(AppError::ConversationAccessDenied);
    }

    let count = ConversationEntity::delete_many()
        .filter(crate::db::entities::conversations::Column::Id.eq(id))
        .filter(crate::db::entities::conversations::Column::OwnerId.eq(principal_user.id))
        .exec(db)
        .await
        .map_err(AppError::from)?
        .rows_affected;

    if count == 0 {
        return Err(AppError::ConversationNotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_messages(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<Vec<MessageOut>>, AppError> {
    let db = &state.auth.db;
    let principal_user = resolve_user(db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;

    let conv = ConversationEntity::find_by_id(id)
        .one(db)
        .await
        .map_err(AppError::from)?
        .ok_or(AppError::ConversationNotFound)?;

    if conv.owner_id != principal_user.id {
        return Err(AppError::ConversationAccessDenied);
    }

    let messages = MessageEntity::find()
        .filter(crate::db::entities::messages::Column::ConversationId.eq(id))
        .order_by_asc(crate::db::entities::messages::Column::CreatedAt)
        .all(db)
        .await
        .map_err(AppError::from)?;

    Ok(Json(messages))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
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
        };

        let state = AppState {
            config,
            cookie_key,
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        };

        Router::new()
            .route(
                "/conversations",
                get(list_conversations).post(create_conversation),
            )
            .route(
                "/conversations/{id}",
                get(get_conversation).delete(delete_conversation),
            )
            .route("/conversations/{id}/messages", get(get_messages))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_conversations_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/conversations")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_conversations_filtered_by_sandbox_without_cookie_returns_401() {
        let app = make_app(test_key());
        let req = Request::builder()
            .uri("/conversations?sandbox_name=my-sandbox")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn create_conversation_requires_context() {
        let err = serde_json::from_str::<CreateConversation>(r#"{"agent_name":"foo"}"#)
            .expect_err("context should be required, not optional");
        assert!(err.to_string().contains("context"));
    }

    #[test]
    fn create_conversation_parses_sandbox_context() {
        let body: CreateConversation = serde_json::from_str(
            r#"{"context":{"kind":"sandbox","data":{"sandbox_name":"my-sandbox"}}}"#,
        )
        .unwrap();
        assert_eq!(body.context.kind, "sandbox");
        assert_eq!(
            body.context
                .data
                .get("sandbox_name")
                .and_then(|v| v.as_str()),
            Some("my-sandbox")
        );
    }

    /// Scenario : `création_contexte_settings_amorce_les_agents_système`.
    ///
    /// Création de conversation en contexte `settings` pour un utilisateur ayant un
    /// profil de modèle mais AUCUNE ligne `system_key` : la réponse est 201 et les
    /// QUATRE lignes système (`system-config`, `system-home`, `system-project`,
    /// `system-sandbox`) existent en base pour cet owner — l'amorçage ne tarde plus au
    /// premier tour (décidé 2026-10-14). Le handler est appelé DIRECTEMENT sur `real_db`
    /// + `test_auth_state_with_db` (précédent `id_regression` ; l'authentification HTTP
    /// par cookie est verrouillée telle quelle par les deux tests 401 du module).
    #[tokio::test]
    async fn creation_contexte_settings_amorce_les_agents_systeme() {
        use sea_orm::PaginatorTrait;

        use crate::db::entities::{agents, llm_providers, model_profiles};

        let db = crate::db::test_support::real_db().await;
        let alice = miryad_core::users::resolve_user(&db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        // Profil de modèle (l'amorçage doit aboutir), mais aucune ligne système.
        let provider = llm_providers::Entity::insert(llm_providers::ActiveModel {
            id: NotSet,
            name: Set("prov-settings".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(true),
        })
        .exec(&db)
        .await
        .expect("provider inserts")
        .last_insert_id;
        model_profiles::Entity::insert(model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(alice),
            name: Set("qwen".to_string()),
            provider_id: Set(provider),
            model: Set("qwen".to_string()),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(serde_json::json!({})),
        })
        .exec(&db)
        .await
        .expect("profile inserts");
        assert_eq!(
            agents::Entity::find()
                .filter(agents::Column::SystemKey.is_not_null())
                .count(&db)
                .await
                .expect("count succeeds"),
            0,
            "no system agent row before the creation"
        );

        let state = AppState {
            config: test_config_for_state(),
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db.clone()),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        };
        let user = miryad_core::auth::AuthUser {
            subject: "alice".to_string(),
            email: None,
            id_token: "test-id-token".to_string(),
        };
        let body = CreateConversation {
            agent_name: None,
            context: ChatContextInput {
                kind: "settings".to_string(),
                data: serde_json::json!({}),
            },
        };

        let (status, Json(out)) =
            create_conversation(axum::extract::State(state), user, Json(body))
                .await
                .expect("settings creation succeeds");

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(out.context.kind, "settings");
        let mut rows = agents::Entity::find()
            .filter(agents::Column::SystemKey.is_not_null())
            .all(&db)
            .await
            .expect("find succeeds");
        let mut names: Vec<String> = rows.drain(..).map(|r| r.name).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "system-config",
                "system-home",
                "system-project",
                "system-sandbox"
            ],
            "the four system agents must exist after the settings creation"
        );
    }

    /// `Config` de test pour les états appelant les handlers directement (mêmes
    /// valeurs que `make_app`).
    fn test_config_for_state() -> crate::config::Config {
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

    // ---------------------------------------------------------------------
    // Passe 3 (conversations.sdd) : Scenario sans test. Les routes à `{id}` (lecture,
    // suppression, messages d'autrui) et les statuts 403/404/204 sont CONSIGNÉS dans
    // .claude/memory/haiku-questions/app.md (barème `[!]`), de même que le Scenario
    // d'incohérence de contexte (amplification verrouillée) et le chemin non numérique (`[?]`).
    // ---------------------------------------------------------------------

    use crate::db::entities::{
        agents, chat_contexts, conversations, llm_providers, messages, model_profiles,
    };
    use crate::db::test_support::real_db;
    use axum::response::IntoResponse;
    use miryad_core::auth::AuthUser;
    use sea_orm::{
        ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
        QueryFilter, Set,
    };

    fn alice_user() -> AuthUser {
        AuthUser {
            subject: "alice".to_string(),
            email: None,
            id_token: "test-id-token".to_string(),
        }
    }

    /// En-tête `Cookie` de session scellé pour `subject` (même mécanisme que `local_tools`).
    fn session_cookie(subject: &str) -> String {
        let identity = miryad_core::auth::OidcIdentity {
            id_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.signature".to_string(),
            subject: subject.to_string(),
            email: None,
            preferred_username: None,
        };
        let set_cookie = miryad_core::auth::cookie::build_set_cookie(&identity, &test_key(), false);
        set_cookie
            .split(';')
            .next()
            .expect("cookie has a name=value pair")
            .to_string()
    }

    async fn seed_user(db: &DatabaseConnection, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("user resolves")
            .id
    }

    async fn seed_profile(db: &DatabaseConnection, owner: i32) -> i32 {
        let provider = llm_providers::Entity::insert(llm_providers::ActiveModel {
            id: NotSet,
            name: Set(format!("prov-{owner}")),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(false),
        })
        .exec(db)
        .await
        .expect("provider inserts")
        .last_insert_id;
        model_profiles::Entity::insert(model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(format!("qwen-{owner}")),
            provider_id: Set(provider),
            model: Set("qwen2.5".to_string()),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(serde_json::json!({})),
        })
        .exec(db)
        .await
        .expect("profile inserts")
        .last_insert_id
    }

    async fn seed_agent(db: &DatabaseConnection, owner: i32, name: &str, profile: i32) {
        agents::Entity::insert(agents::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            description: Set(None),
            mode: Set("primary".to_string()),
            model_profile_id: Set(profile),
            toolsets: Set(serde_json::json!([])),
            skills: Set(serde_json::json!("auto")),
            system_prompt: Set(String::new()),
            system_key: Set(None),
            system_version: Set(None),
        })
        .exec(db)
        .await
        .expect("agent inserts");
    }

    async fn seed_context(db: &DatabaseConnection, sandbox_name: &str) -> i32 {
        chat_contexts::Entity::insert(chat_contexts::ActiveModel {
            id: NotSet,
            kind: Set("sandbox".to_string()),
            data: Set(serde_json::json!({"sandbox_name": sandbox_name})),
            created_at: Set(chrono::Utc::now()),
        })
        .exec(db)
        .await
        .expect("context inserts")
        .last_insert_id
    }

    /// Conversation sans agent, `updated_at` fixé (ordre de tri contrôlé).
    async fn seed_conversation(
        db: &DatabaseConnection,
        owner: i32,
        context_id: i32,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> i32 {
        conversations::Entity::insert(conversations::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            agent_id: Set(None),
            context_id: Set(context_id),
            title: Set(None),
            todo: Set(None),
            created_at: Set(updated_at),
            updated_at: Set(updated_at),
        })
        .exec(db)
        .await
        .expect("conversation inserts")
        .last_insert_id
    }

    fn state_with(db: DatabaseConnection) -> AppState {
        AppState {
            config: test_config_for_state(),
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        }
    }

    async fn get_json(app: Router, uri: &str, cookie: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .uri(uri)
            .header("Cookie", cookie)
            .body(Body::empty())
            .expect("valid request");
        let resp = app.oneshot(req).await.expect("router does not fail");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body reads");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn list_router(state: AppState) -> Router {
        Router::new()
            .route(
                "/conversations",
                get(list_conversations).post(create_conversation),
            )
            .route("/conversations/{id}/messages", get(get_messages))
            .with_state(state)
    }

    fn ts(day: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 1, day, 12, 0, 0)
            .single()
            .expect("valid date")
    }

    /// Scenario : `liste_authentifiée_filtre_sur_owner_et_tri_par_updated_at_descendant`.
    #[tokio::test]
    async fn liste_authentifiee_filtre_sur_owner_et_tri_par_updated_at_descendant() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let bob = seed_user(&db, "bob").await;
        let ctx = seed_context(&db, "sb").await;
        let old = seed_conversation(&db, alice, ctx, ts(1)).await;
        let recent = seed_conversation(&db, alice, ctx, ts(2)).await;
        seed_conversation(&db, bob, ctx, ts(3)).await;

        let (status, body) = get_json(
            list_router(state_with(db)),
            "/conversations",
            &session_cookie("alice"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = body.as_array().expect("array body");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], serde_json::json!(recent));
        assert_eq!(items[1]["id"], serde_json::json!(old));
        for item in items {
            assert_eq!(item.as_object().map(serde_json::Map::len), Some(7));
        }
    }

    /// Scenario : `liste_filtrée_par_sandbox_name_selon_kind_et_valeur_exacte`.
    #[tokio::test]
    async fn liste_filtree_par_sandbox_name_selon_kind_et_valeur_exacte() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let sb_a = seed_context(&db, "sb-a").await;
        let sb_b = seed_context(&db, "sb-b").await;
        let other = chat_contexts::Entity::insert(chat_contexts::ActiveModel {
            id: NotSet,
            kind: Set("autre".to_string()),
            data: Set(serde_json::json!({"sandbox_name": "sb-a"})),
            created_at: Set(chrono::Utc::now()),
        })
        .exec(&db)
        .await
        .expect("context inserts")
        .last_insert_id;
        let wanted = seed_conversation(&db, alice, sb_a, ts(1)).await;
        seed_conversation(&db, alice, sb_b, ts(2)).await;
        seed_conversation(&db, alice, other, ts(3)).await;

        let (status, body) = get_json(
            list_router(state_with(db)),
            "/conversations?sandbox_name=sb-a",
            &session_cookie("alice"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = body.as_array().expect("array body");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], serde_json::json!(wanted));
        assert_eq!(items[0]["context"]["kind"], "sandbox");
        assert_eq!(items[0]["context"]["data"]["sandbox_name"], "sb-a");
    }

    /// Scenario : `filtre_sandbox_name_exclut_casse_et_type_différents`.
    #[tokio::test]
    async fn filtre_sandbox_name_exclut_casse_et_type_differents() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let ctx = seed_context(&db, "sb-a").await;
        seed_conversation(&db, alice, ctx, ts(1)).await;
        let numeric = chat_contexts::Entity::insert(chat_contexts::ActiveModel {
            id: NotSet,
            kind: Set("sandbox".to_string()),
            data: Set(serde_json::json!({"sandbox_name": 42})),
            created_at: Set(chrono::Utc::now()),
        })
        .exec(&db)
        .await
        .expect("context inserts")
        .last_insert_id;
        seed_conversation(&db, alice, numeric, ts(2)).await;

        let cookie = session_cookie("alice");
        let (status, body) = get_json(
            list_router(state_with(db.clone())),
            "/conversations?sandbox_name=SB-A",
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!([]));

        // Contexte non-chaîne : jamais pris pour une correspondance.
        conversations::Entity::delete_many()
            .filter(conversations::Column::ContextId.eq(ctx))
            .exec(&db)
            .await
            .expect("delete succeeds");
        let (status, body) = get_json(
            list_router(state_with(db)),
            "/conversations?sandbox_name=42",
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!([]));
    }

    /// Scenario : `création_avec_agent_connu_retourne_201_et_crée_contexte_et_conversation`.
    #[tokio::test]
    async fn creation_avec_agent_connu_retourne_201_et_cree_contexte_et_conversation() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let profile = seed_profile(&db, alice).await;
        seed_agent(&db, alice, "agent-1", profile).await;
        let body: CreateConversation = serde_json::from_value(serde_json::json!({
            "agent_name": "agent-1",
            "context": {"kind": "sandbox", "data": {"sandbox_name": "sb"}}
        }))
        .expect("body parses");

        let (status, Json(out)) =
            create_conversation(State(state_with(db.clone())), alice_user(), Json(body))
                .await
                .expect("creation succeeds");
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(out.agent_name.as_deref(), Some("agent-1"));
        assert!(out.title.is_none());
        assert!(out.todo.is_none());
        assert_eq!(out.created_at, out.updated_at);
        assert_eq!(out.context.kind, "sandbox");
        assert_eq!(out.context.data["sandbox_name"], "sb");
        assert!(out.context.id > 0);
        assert_eq!(
            conversations::Entity::find()
                .count(&db)
                .await
                .expect("count"),
            1
        );
        assert_eq!(
            chat_contexts::Entity::find()
                .count(&db)
                .await
                .expect("count"),
            1
        );
    }

    /// Scenario : `création_sans_agent_name_retourne_201_avec_agent_name_null`.
    #[tokio::test]
    async fn creation_sans_agent_name_retourne_201_avec_agent_name_null() {
        let db = real_db().await;
        seed_user(&db, "alice").await;
        for raw in [
            serde_json::json!({"context": {"kind": "sandbox", "data": {"sandbox_name": "sb"}}}),
            serde_json::json!({"agent_name": null, "context": {"kind": "sandbox", "data": {"sandbox_name": "sb"}}}),
        ] {
            let body: CreateConversation = serde_json::from_value(raw).expect("body parses");
            let (status, Json(out)) =
                create_conversation(State(state_with(db.clone())), alice_user(), Json(body))
                    .await
                    .expect("creation succeeds");
            assert_eq!(status, StatusCode::CREATED);
            assert!(out.agent_name.is_none());
            let row = conversations::Entity::find_by_id(out.id)
                .one(&db)
                .await
                .expect("query runs")
                .expect("conversation row exists");
            assert!(row.agent_id.is_none());
        }
    }

    /// Scenario : `création_avec_agent_inconnu_retourne_422_sans_aucune_écriture`.
    #[tokio::test]
    async fn creation_avec_agent_inconnu_retourne_422_sans_aucune_ecriture() {
        let db = real_db().await;
        seed_user(&db, "alice").await;
        let bob = seed_user(&db, "bob").await;
        let profile_bob = seed_profile(&db, bob).await;
        seed_agent(&db, bob, "agent-1", profile_bob).await;

        for name in ["agent-1", "inexistant"] {
            let body: CreateConversation = serde_json::from_value(serde_json::json!({
                "agent_name": name,
                "context": {"kind": "sandbox", "data": {"sandbox_name": "sb"}}
            }))
            .expect("body parses");
            let err = create_conversation(State(state_with(db.clone())), alice_user(), Json(body))
                .await
                .expect_err("unknown agent is refused");
            assert!(matches!(err, AppError::UnprocessableReference(_)));
            let resp = err.into_response();
            assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body reads");
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("Unknown agent reference"), "body: {text}");
        }
        assert_eq!(
            chat_contexts::Entity::find()
                .count(&db)
                .await
                .expect("count"),
            0
        );
        assert_eq!(
            conversations::Entity::find()
                .count(&db)
                .await
                .expect("count"),
            0
        );
    }

    /// Scenario : `liste_d_un_owner_sans_conversation_retourne_tableau_vide`.
    #[tokio::test]
    async fn liste_d_un_owner_sans_conversation_retourne_tableau_vide() {
        let db = real_db().await;
        seed_user(&db, "alice").await;
        let cookie = session_cookie("alice");
        for uri in ["/conversations", "/conversations?sandbox_name=x"] {
            let (status, body) = get_json(list_router(state_with(db.clone())), uri, &cookie).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, serde_json::json!([]));
        }
    }

    /// Scenario : `messages_triés_en_six_clés_brutes_ou_tableau_vide`.
    #[tokio::test]
    async fn messages_tries_en_six_cles_brutes_ou_tableau_vide() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let ctx = seed_context(&db, "sb").await;
        let with_messages = seed_conversation(&db, alice, ctx, ts(1)).await;
        let empty = seed_conversation(&db, alice, ctx, ts(2)).await;
        // Insertion du plus récent au plus ancien : le tri doit rétablir l'ordre.
        for (day, text) in [(5, "second"), (4, "premier")] {
            messages::Entity::insert(messages::ActiveModel {
                id: NotSet,
                owner_id: Set(alice),
                conversation_id: Set(with_messages),
                role: Set("user".to_string()),
                payload: Set(serde_json::json!({"text": text})),
                created_at: Set(ts(day)),
            })
            .exec(&db)
            .await
            .expect("message inserts");
        }

        let cookie = session_cookie("alice");
        let (status, body) = get_json(
            list_router(state_with(db.clone())),
            &format!("/conversations/{with_messages}/messages"),
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = body.as_array().expect("array body");
        assert_eq!(items.len(), 2);
        for item in items {
            let mut keys: Vec<&str> = item
                .as_object()
                .expect("message is an object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "conversation_id",
                    "created_at",
                    "id",
                    "owner_id",
                    "payload",
                    "role"
                ]
            );
        }
        assert_eq!(items[0]["payload"]["text"], "premier");
        assert_eq!(items[1]["payload"]["text"], "second");

        let (status, body) = get_json(
            list_router(state_with(db)),
            &format!("/conversations/{empty}/messages"),
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!([]));
    }
}

/// Régression Phase 3 (miryad-core-integration) : `ChatContext`/`Conversation` posaient
/// `id: Set(0)` au lieu de `NotSet` — la première conversation créée système-wide réussissait,
/// la seconde percutait la contrainte de clé primaire. Deux créations successives doivent
/// réussir et obtenir des ids distincts (cf. `docs/features/miryad-core-integration.md`).
#[cfg(test)]
mod id_regression {
    use super::*;
    use axum::extract::State;
    use miryad_core::auth::AuthUser;

    fn test_user() -> AuthUser {
        AuthUser {
            subject: "alice".to_string(),
            email: Some("alice@example.com".to_string()),
            id_token: "test-id-token".to_string(),
        }
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

    #[tokio::test]
    async fn two_successive_conversations_get_distinct_ids() {
        let db = crate::db::test_support::real_db().await;
        let state = AppState {
            config: test_config(),
            cookie_key: cookie::Key::from(&[0u8; 64]),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state_with_db(db),
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        };

        let body = || CreateConversation {
            agent_name: None,
            context: ChatContextInput {
                kind: "sandbox".to_string(),
                data: serde_json::json!({"sandbox_name": "my-sandbox"}),
            },
        };

        let (_, Json(first)) = create_conversation(State(state.clone()), test_user(), Json(body()))
            .await
            .expect("first conversation creates");
        let (_, Json(second)) = create_conversation(State(state), test_user(), Json(body()))
            .await
            .expect("second conversation creates");

        assert_ne!(
            first.id, second.id,
            "two successive conversations must not collide on the same auto-increment id"
        );
    }
}
