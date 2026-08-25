use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use miryad_core::auth::AuthUser;
use miryad_core::users::resolve_user;

use crate::{
    AppState,
    db::entities::{
        chat_contexts::{
            ActiveModel as ChatContextActiveModel,
            Entity as ChatContextEntity,
            Model as ChatContextModel,
        },
        conversations::{
            ActiveModel as ConversationActiveModel,
            Entity as ConversationEntity,
            Model as ConversationModel,
        },
        messages::{Entity as MessageEntity, Model as MessageModel},
    },
    db::models::User,
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

/// Type alias pour la réponse de `get_messages` : `Message` (sqlx) est déjà
/// importé via `crate::db::models`.
type MessageOut = MessageModel;

fn db_err(e: sea_orm::DbErr) -> AppError {
    AppError::InternalError(format!("VNL-DB-006: {e}"))
}

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

/// Résout `conv.agent_id` en nom (SELECT name FROM vanyline_agents WHERE id = $1),
/// charge le ChatContext depuis vanyline_chat_contexts (N+1 accepté, comme aujourd'hui).
async fn to_output(
    state: &AppState,
    conv: ConversationModel,
) -> Result<ConversationOut, AppError> {
    let db = &state.auth.db;

    let agent_name = match conv.agent_id {
        Some(id) => {
            use crate::db::entities::agents::Entity as AgentEntity;
            AgentEntity::find_by_id(id)
                .one(db)
                .await
                .map_err(db_err)?
                .map(|a| a.name)
        }
        None => None,
    };

    let context = ChatContextEntity::find_by_id(conv.context_id)
        .one(db)
        .await
        .map_err(db_err)?
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
/// (WHERE owner_id = user_id AND name = agent_name).
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
        .map_err(db_err)?
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
        .map_err(db_err)?;

    let conversations = ConversationEntity::find()
        .filter(crate::db::entities::conversations::Column::OwnerId.eq(principal_user.id))
        .order_by_desc(crate::db::entities::conversations::Column::UpdatedAt)
        .all(db)
        .await
        .map_err(db_err)?;

    let mut out: Vec<ConversationOut> = Vec::with_capacity(conversations.len());

    if let Some(ref sandbox_name) = query.sandbox_name {
        for conv in conversations {
            let ctx = ChatContextEntity::find_by_id(conv.context_id)
                .one(db)
                .await
                .map_err(db_err)?
                .ok_or_else(|| {
                    AppError::InternalError("VNL-DB-007: context not found".into())
                })?;

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
        .map_err(db_err)?;

    let agent_id = match &body.agent_name {
        Some(name) => Some(resolve_agent_id(&state, principal_user.id, name).await?),
        None => None,
    };

    // Effect de bord : créer le ChatContext avant la Conversation
    let now = chrono::Utc::now();
    let ctx_active = ChatContextActiveModel {
        id: Set(0),
        kind: Set(body.context.kind.clone()),
        data: Set(body.context.data.clone()),
        created_at: Set(now),
    };
    let ctx_result = ChatContextEntity::insert(ctx_active)
        .exec(db)
        .await
        .map_err(db_err)?;
    let context_id = ctx_result.last_insert_id as i32;

    let conv_active = ConversationActiveModel {
        id: Set(0),
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
        .map_err(db_err)?;
    let conv = ConversationEntity::find_by_id(conv_result.last_insert_id as i32)
        .one(db)
        .await
        .map_err(db_err)?
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
        .map_err(db_err)?;

    let conv = ConversationEntity::find_by_id(id)
        .one(db)
        .await
        .map_err(db_err)?
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
        .map_err(db_err)?;

    let conv = ConversationEntity::find_by_id(id)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(AppError::ConversationNotFound)?;

    if conv.owner_id != principal_user.id {
        return Err(AppError::ConversationAccessDenied);
    }

    let count = ConversationEntity::delete_many()
        .filter(crate::db::entities::conversations::Column::Id.eq(id))
        .filter(crate::db::entities::conversations::Column::OwnerId.eq(principal_user.id))
        .exec(db)
        .await
        .map_err(db_err)?
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
        .map_err(db_err)?;

    let conv = ConversationEntity::find_by_id(id)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(AppError::ConversationNotFound)?;

    if conv.owner_id != principal_user.id {
        return Err(AppError::ConversationAccessDenied);
    }

    let messages = MessageEntity::find()
        .filter(crate::db::entities::messages::Column::ConversationId.eq(id))
        .order_by_asc(crate::db::entities::messages::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?;

    Ok(Json(messages))
}

/// Utilitaire sqlx conservé pour `me.rs`, `owners.rs`, `projects.rs`,
/// `sandboxes.rs`, `ws/chat.rs` — ne pas retirer.
pub async fn get_or_create_user(state: &AppState, auth_user: &AuthUser) -> Result<User, AppError> {
    if let Some(user) = sqlx::query_as::<_, User>("SELECT * FROM users WHERE oidc_sub = $1")
        .bind(&auth_user.subject)
        .fetch_optional(&state.pool)
        .await?
    {
        return Ok(user);
    }

    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (oidc_sub, email) VALUES ($1, $2)
         ON CONFLICT (oidc_sub) DO UPDATE SET email = EXCLUDED.email RETURNING *",
    )
    .bind(&auth_user.subject)
    .bind(auth_user.email.as_deref().unwrap_or(""))
    .fetch_one(&state.pool)
    .await?;
    Ok(user)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
        };

        let state = AppState {
            config,
            cookie_key,
            pool: sqlx::PgPool::connect_lazy("postgres://localhost/test_unused").unwrap(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
        };

        Router::new()
            .route("/conversations", get(list_conversations).post(create_conversation))
            .route(
                "/conversations/{id}",
                get(get_conversation).delete(delete_conversation),
            )
            .route(
                "/conversations/{id}/messages",
                get(get_messages),
            )
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
}