use axum::{extract::State, Json};
use serde::Serialize;

use crate::{auth::middleware::AuthUser, error::AppError, AppState};

#[derive(Serialize)]
pub struct MeResponse {
    pub email: String,
}

pub async fn handler_me(
    _state: State<AppState>,
    user: AuthUser,
) -> Result<Json<MeResponse>, AppError> {
    Ok(Json(MeResponse { email: user.email }))
}
