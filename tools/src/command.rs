use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

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

    #[error("command timed out after {0}s")]
    Timeout(u64),
}

pub fn execute(opts: ExecuteCommandOptions) -> BoxedFuture<Result<ExecuteCommandResult, CommandError>> {
    let command = opts.command;
    let timeout_secs = opts.timeout_secs;
    Box::pin(async move {
        if command.is_empty() {
            return Err(CommandError::Empty);
        }

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = cmd
            .spawn()
            .map_err(|e| CommandError::Process(format!("spawn failed: {e}")))?;

        let output = if timeout_secs == 0 {
            child
                .wait_with_output()
                .await
                .map_err(|e| CommandError::Process(format!("{e}")))?
        } else {
            tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
                .await
                .map_err(|_| CommandError::Timeout(timeout_secs))?
                .map_err(|e| CommandError::Process(format!("{e}")))?
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);
        Ok(ExecuteCommandResult {
            stdout,
            stderr,
            exit_code,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_quotes_respected() {
        let result = execute(ExecuteCommandOptions {
            command: r##"echo "hello world""##.to_string(),
            timeout_secs: 5,
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert_eq!(res.stdout, "hello world\n");
    }

    #[tokio::test]
    async fn test_pipe_works() {
        let result = execute(ExecuteCommandOptions {
            command: "echo hi | tr a-z A-Z".to_string(),
            timeout_secs: 5,
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert_eq!(res.stdout.trim(), "HI");
    }

    #[tokio::test]
    async fn test_nonzero_exit_code() {
        let result = execute(ExecuteCommandOptions {
            command: "false".to_string(),
            timeout_secs: 5,
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert_eq!(res.exit_code, 1);
    }

    #[tokio::test]
    async fn test_timeout_efficient() {
        let start = Instant::now();
        let result = execute(ExecuteCommandOptions {
            command: "sleep 5".to_string(),
            timeout_secs: 1,
        })
        .await;
        let elapsed = start.elapsed();

        assert!(matches!(result, Err(CommandError::Timeout(1))));
        assert!(elapsed < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn test_unlimited_timeout() {
        let result = execute(ExecuteCommandOptions {
            command: "echo ok".to_string(),
            timeout_secs: 0,
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert_eq!(res.stdout.trim(), "ok");
    }

    #[tokio::test]
    async fn test_empty_command() {
        let result = execute(ExecuteCommandOptions {
            command: "".to_string(),
            timeout_secs: 5,
        })
        .await;

        assert!(matches!(result, Err(CommandError::Empty)));
    }
}
