use super::filesystem::*;

use tokio::fs;
use tracing::info;

pub async fn delete_file(opts: DeleteFileOptions) -> Result<()> {
    info!("delete_file: {}", opts.path);
    fs::remove_file(&opts.path).await?;
    Ok(())
}

pub async fn create_directory(opts: CreateDirectoryOptions) -> Result<()> {
    info!("create_directory: {}", opts.path);
    fs::create_dir_all(&opts.path).await?;
    Ok(())
}
