use super::filesystem::*;

use tokio::fs;
use tracing::info;

pub async fn list_directory(opts: ListDirectoryOptions) -> Result<Vec<String>> {
    info!("list_directory: {}", opts.path);
    let mut entries = Vec::new();
    let mut reader = fs::read_dir(&opts.path).await?;
    while let Some(entry) = reader.next_entry().await? {
        entries.push(entry.file_name().to_string_lossy().to_string());
    }
    entries.sort();
    Ok(entries)
}
