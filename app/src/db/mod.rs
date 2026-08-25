pub mod entities;
pub mod models;

use sqlx::PgPool;

use crate::error::AppError;

pub async fn create_pool(database_url: &str) -> Result<PgPool, AppError> {
    let pool = PgPool::connect(database_url).await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), AppError> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| AppError::InternalError(format!("VNL-DB-002: migration failed: {e}")))?;
    Ok(())
}
