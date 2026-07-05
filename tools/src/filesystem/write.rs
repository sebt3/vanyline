use super::filesystem::*;

use tokio::fs;
use tracing::info;

pub async fn write_file(opts: WriteFileOptions) -> Result<()> {
    info!("write_file: {}", opts.path);
    let parent = std::path::Path::new(&opts.path)
        .parent()
        .ok_or(FilesystemError::PathNotAllowed("no parent".into()))?;
    if !parent.is_empty() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(&opts.path, opts.content).await?;
    Ok(())
}
