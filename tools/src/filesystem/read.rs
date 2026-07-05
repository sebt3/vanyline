use super::filesystem::*;

use tokio::fs;
use tracing::info;

pub async fn read_file(opts: ReadFileOptions) -> Result<String> {
    info!("read_file: {}", opts.path);
    let content = fs::read_to_string(&opts.path).await?;
    Ok(content)
}
