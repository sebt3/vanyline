use super::command::*;

use tokio::process::Command;
use tracing::info;

pub async fn execute(opts: ExecuteCommandOptions) -> Result<ExecuteCommandResult> {
    if opts.command.is_empty() {
        return Err(CommandError::Empty);
    }

    info!("execute: {} (timeout: {}s)", opts.command, opts.timeout_secs);

    let mut parts = opts.command.split_whitespace();
    let program = parts.next().ok_or(CommandError::Empty)?;
    let args: Vec<String> = parts.map(String::from).collect();

    let mut child = Command::new(program)
        .args(&args)
        .output()
        .await
        .map_err(|e| CommandError::Process(format!("spawn failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&child.stdout).to_string();
    let stderr = String::from_utf8_lossy(&child.stderr).to_string();
    let exit_code = child.status.code().unwrap_or(-1);

    Ok(ExecuteCommandResult {
        stdout,
        stderr,
        exit_code,
    })
}
