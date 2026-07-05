use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("timeout exceeded: {0}s")]
    Timeout(u64),

    #[error("process error: {0}")]
    Process(String),

    #[error("empty command")]
    Empty,
}

pub type Result<T> = std::result::Result<T, CommandError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteCommandOptions {
    pub command: String,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteCommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
