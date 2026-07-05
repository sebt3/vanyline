use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteCommandOptions {
    pub command: String,
    pub timeout_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteCommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("empty command")]
    Empty,

    #[error("process error: {0}")]
    Process(String),
}

pub fn execute(opts: ExecuteCommandOptions) -> BoxedFuture<Result<ExecuteCommandResult, CommandError>> {
    let command = opts.command;
    let _timeout = opts.timeout_secs;
    Box::pin(async move {
        if command.is_empty() {
            return Err(CommandError::Empty);
        }
        let mut parts = command.split_whitespace();
        let program = parts.next().ok_or(CommandError::Empty)?;
        let args: Vec<String> = parts.map(String::from).collect();
        let child = tokio::process::Command::new(program)
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
    })
}

use std::future::Future;
use std::pin::Pin;
pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
