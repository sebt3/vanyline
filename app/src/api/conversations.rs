use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    auth::middleware::AuthUser,
    db::models::{Conversation, Message, User},
    error::AppError,
    AppState,
};

#[derive(Deserialize)]
pub struct CreateConversation {
    pub agent_id: Option<Uuid>,
}

pub async fn list_conversations(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<Conversation>>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let conversations = sqlx::query_as::<_, Conversation>(
        "SELECT * FROM conversations WHERE user_id = $1 ORDER BY updated_at DESC",
    )
    .bind(db_user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(conversations))
}

pub async fn create_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateConversation>,
) -> Result<(StatusCode, Json<Conversation>), AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let conv = sqlx::query_as::<_, Conversation>(
        "INSERT INTO conversations (user_id, agent_id) VALUES ($1, $2) RETURNING *",
    )
    .bind(db_user.id)
    .bind(body.agent_id)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(conv)))
}

pub async fn get_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversation>, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let conv = sqlx::query_as::<_, Conversation>(
        "SELECT * FROM conversations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::ConversationNotFound)?;

    if conv.user_id != db_user.id {
        return Err(AppError::ConversationAccessDenied);
    }
    Ok(Json(conv))
}

pub async fn delete_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let db_user = get_or_create_user(&state, &user).await?;
    let rows = sqlx::query(
        "DELETE FROM conversations WHERE id = $1 AND user_id = $2",
    )
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
    let conv = sqlx::query_as::<_, Conversation>(
        "SELECT * FROM conversations WHERE id = $1",
    )
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
    if let Some(user) = sqlx::query_as::<_, User>(
        "SELECT * FROM users WHERE email = $1",
    )
    .bind(&auth_user.email)
    .fetch_optional(&state.pool)
    .await?
    {
        return Ok(user);
    }

    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (oidc_sub, email) VALUES ($1, $2) ON CONFLICT (oidc_sub) DO UPDATE SET email = EXCLUDED.email RETURNING *",
    )
    .bind(&auth_user.email)
    .bind(&auth_user.email)
    .fetch_one(&state.pool)
    .await?;
    Ok(user)
}
