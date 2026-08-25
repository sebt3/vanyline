use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use vanyline_lib::VnyError;

use miryad_core::auth::AuthUser;

use crate::{
    AppState,
    db::models::{ChatContext, Conversation, Message, User},
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

#[derive(Debug, Clone, Serialize)]
pub struct ConversationOut {
    pub id: Uuid,
    pub agent_name: Option<String>,
    pub context: ChatContext,
    pub title: Option<String>,
    pub todo: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Résout `conv.agent_id` (peut être `None` — conversation sans agent) en
/// nom, via une requête séparée (N+1 accepté, cf. Contexte). `None` si
/// `agent_id` est `None` OU si l'agent référencé a été supprimé depuis
/// (FK `ON DELETE SET NULL` empêche normalement ce cas, mais la fonction
/// reste défensive plutôt que de `.unwrap()`).
async fn to_output(state: &AppState, conv: Conversation) -> Result<ConversationOut, AppError> {
    let agent_name = match conv.agent_id {
        Some(id) => {
            sqlx::query_scalar::<_, String>("SELECT name FROM agents WHERE id = $1")
                .bind(id)
                .fetch_optional(&state.pool)
                .await?
        }
        None => None,
    };

    let context = sqlx::query_as::<_, ChatContext>("SELECT * FROM chat_contexts WHERE id = $1")
        .bind(conv.context_id)
        .fetch_one(&state.pool)
        .await?;

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

async fn resolve_agent_id(
    state: &AppState,
    user_id: Uuid,
    agent_name: &str,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM agents WHERE user_id = $1 AND name = $2")
        .bind(user_id)
        .bind(agent_name)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| {
            AppError::UnprocessableReference(VnyError::UnknownReference(
                "agent",
                agent_name.to_string(),
            ))
        })
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
    let db_user = get_or_create_user(&state, &user).await?;
    let conversations = match &query.sandbox_name {
        Some(sandbox_name) => {
            sqlx::query_as::<_, Conversation>(
                "SELECT c.* FROM conversations c
                 JOIN chat_contexts ctx ON ctx.id = c.context_id
                 WHERE c.user_id = $1 AND ctx.kind = 'sandbox' AND ctx.data->>'sandbox_name' = $2
                 ORDER BY c.updated_at DESC",
            )
            .bind(db_user.id)
            .bind(sandbox_name)
            .fetch_all(&state.pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, Conversation>(
                "SELECT * FROM conversations WHERE user_id = $1 ORDER BY updated_at DESC",
            )
            .bind(db_user.id)
            .fetch_all(&state.pool)
            .await?
        }
    };

    let mut out = Vec::with_capacity(conversations.len());
    for conv in conversations {
        out.push(to_output(&state, conv).await?);
    }
    Ok(Json(out))
}

pub async fn create_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateConversation>,
) -> Result<(StatusCode, Json<ConversationOut>), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;

    let agent_id = match &body.agent_name {
        Some(name) => Some(resolve_agent_id(&state, db_user.id, name).await?),
        None => None,
    };

    let context_id: Uuid =
        sqlx::query_scalar("INSERT INTO chat_contexts (kind, data) VALUES ($1, $2) RETURNING id")
            .bind(&body.context.kind)
            .bind(&body.context.data)
            .fetch_one(&state.pool)
            .await?;

    let conv = sqlx::query_as::<_, Conversation>(
        "INSERT INTO conversations (user_id, agent_id, context_id) VALUES ($1, $2, $3) RETURNING *",
    )
    .bind(db_user.id)
    .bind(agent_id)
    .bind(context_id)
    .fetch_one(&state.pool)
    .await?;

    let out = to_output(&state, conv).await?;
    Ok((StatusCode::CREATED, Json(out)))
}

pub async fn get_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ConversationOut>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let conv = sqlx::query_as::<_, Conversation>("SELECT * FROM conversations WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::ConversationNotFound)?;

    if conv.user_id != db_user.id {
        return Err(AppError::ConversationAccessDenied);
    }

    let out = to_output(&state, conv).await?;
    Ok(Json(out))
}

pub async fn delete_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let rows = sqlx::query("DELETE FROM conversations WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(db_user.id)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::ConversationNotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_messages(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Message>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let conv = sqlx::query_as::<_, Conversation>("SELECT * FROM conversations WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::ConversationNotFound)?;

    if conv.user_id != db_user.id {
        return Err(AppError::ConversationAccessDenied);
    }

    let messages = sqlx::query_as::<_, Message>(
        "SELECT * FROM messages WHERE conversation_id = $1 ORDER BY created_at ASC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(messages))
}

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
            .route("/conversations", get(list_conversations))
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
