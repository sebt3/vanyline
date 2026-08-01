use crate::error::ToolsError;
use crate::filesystem;
use crate::output;

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Instant;

pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Options d'exécution de commande.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ExecuteCommandOptions {
    pub command: String,
    /// 0 = pas de timeout (comportement déjà existant, inchangé).
    #[serde(default)]
    pub timeout_secs: u64,
    /// Répertoire de travail. Vide = hérite du cwd du processus courant.
    #[serde(default)]
    pub cwd: String,
}

/// Retourne un `String` formaté (exit code, duration, stdout, stderr).
/// Erreurs remontent via `ToolsError`.
pub fn execute(opts: ExecuteCommandOptions) -> BoxedFuture<Result<String, ToolsError>> {
    let command = opts.command;
    let timeout_secs = opts.timeout_secs;
    let cwd = opts.cwd;
    Box::pin(async move {
        // 1. command vide → InvalidArgument
        if command.is_empty() {
            return Err(ToolsError::InvalidArgument {
                name: "command".into(),
                reason: "must not be empty".into(),
            });
        }

        // 2. Résolution cwd : not found / not a directory
        let base_dir = if cwd.is_empty() {
            None
        } else {
            let meta = match tokio::fs::metadata(&cwd).await {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(ToolsError::FileNotFound {
                        path: cwd.clone(),
                        hint: filesystem::file_not_found_hint(&cwd),
                    });
                }
                Err(e) => {
                    return Err(ToolsError::Io {
                        path: cwd.clone(),
                        source: e,
                    })
                }
            };
            if !meta.is_dir() {
                return Err(ToolsError::NotADirectory(cwd.clone()));
            }
            Some(cwd)
        };

        let start = Instant::now();

        // 3. Lancer la commande
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);

        if let Some(ref dir) = base_dir {
            cmd.current_dir(dir);
        }

        let child = cmd.spawn().map_err(|e| ToolsError::Io {
            path: format!("command: {command}"),
            source: e,
        })?;
        let child_pid = child.id();

        // 4. Attendre avec timeout optionnel
        let output = if timeout_secs == 0 {
            child.wait_with_output().await.map_err(|e| ToolsError::Io {
                path: format!("command: {command}"),
                source: e,
            })?
        } else {
            match tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                child.wait_with_output(),
            )
            .await
            {
                Ok(Ok(o)) => o,
                Ok(Err(e)) => {
                    return Err(ToolsError::Io {
                        path: format!("command: {command}"),
                        source: e,
                    });
                }
                Err(_) => {
                    if let Some(pid) = child_pid {
                        // SAFETY : kill(2) sur -pid cible le groupe de processus créé par
                        // process_group(0) (pgid == pid du leader) — pas seulement `sh`
                        // mais tous ses descendants. Best-effort : ESRCH (déjà mort) ou
                        // toute autre erreur n'est pas remontée, le timeout reste
                        // l'erreur pertinente à propager à l'appelant.
                        unsafe {
                            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                        }
                    }
                    return Err(ToolsError::CommandTimeout(timeout_secs));
                }
            }
        };

        let duration = start.elapsed();

        // 5. Formatage de la sortie
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        Ok(format!(
            "exit code: {}\nduration: {:.2}s\nstdout:\n{}\nstderr:\n{}",
            exit_code,
            duration.as_secs_f64(),
            output::bound_head_tail(&stdout, output::COMMAND_MAX_BYTES),
            output::bound_head_tail(&stderr, output::COMMAND_MAX_BYTES),
        ))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::time::Duration;
    use std::time::Instant;

    // -----------------------------------------------------------------------
    /// Adapted existing tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_quotes_respected() {
        let result = execute(ExecuteCommandOptions {
            command: r##"echo "hello world""##.to_string(),
            timeout_secs: 5,
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.contains("hello world"));
    }

    #[tokio::test]
    async fn test_pipe_works() {
        let result = execute(ExecuteCommandOptions {
            command: "echo hi | tr a-z A-Z".to_string(),
            timeout_secs: 5,
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.contains("HI"));
    }

    #[tokio::test]
    async fn test_nonzero_exit_code() {
        let result = execute(ExecuteCommandOptions {
            command: "false".to_string(),
            timeout_secs: 5,
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.contains("exit code: 1"));
    }

    #[tokio::test]
    async fn test_timeout_efficient() {
        let start = Instant::now();
        let result = execute(ExecuteCommandOptions {
            command: "sleep 5".to_string(),
            timeout_secs: 1,
            ..Default::default()
        })
        .await;
        let elapsed = start.elapsed();

        assert!(matches!(result, Err(ToolsError::CommandTimeout(1))));
        assert!(elapsed < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn test_unlimited_timeout() {
        let result = execute(ExecuteCommandOptions {
            command: "echo ok".to_string(),
            timeout_secs: 0,
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.contains("ok"));
    }

    #[tokio::test]
    async fn test_empty_command() {
        let result = execute(ExecuteCommandOptions {
            command: "".to_string(),
            timeout_secs: 5,
            ..Default::default()
        })
        .await;

        match result {
            Err(ToolsError::InvalidArgument { name, reason }) => {
                assert_eq!(name, "command");
                assert!(reason.contains("must not be empty"));
            }
            other => panic!("Expected InvalidArgument, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    /// New tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_duration_present() {
        let result = execute(ExecuteCommandOptions {
            command: "echo test".to_string(),
            timeout_secs: 5,
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        // "duration: " followed by a number followed by "s"
        assert!(res.contains("duration: "));
        let duration_line = res.lines().find(|l| l.starts_with("duration: ")).unwrap();
        assert!(duration_line.contains("s"));
    }

    #[tokio::test]
    async fn test_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute(ExecuteCommandOptions {
            command: "pwd".to_string(),
            timeout_secs: 5,
            cwd: dir.path().to_string_lossy().to_string(),
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        let expected = dir.path().canonicalize().unwrap();
        // Extract stdout section (after "stdout:\n")
        let stdout_section = res.split("\nstdout:\n").nth(1).unwrap();
        let pwd_output = stdout_section.lines().next().unwrap();
        let actual = std::path::Path::new(pwd_output.trim())
            .canonicalize()
            .unwrap();
        assert_eq!(expected, actual);
    }

    #[tokio::test]
    async fn test_cwd_not_found() {
        let result = execute(ExecuteCommandOptions {
            command: "echo ok".to_string(),
            timeout_secs: 5,
            cwd: "/nonexistent/path/xyz".to_string(),
        })
        .await;

        match result {
            Err(ToolsError::FileNotFound { path, .. }) => {
                assert!(path.contains("nonexistent"));
            }
            other => panic!("Expected FileNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cwd_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("afile");
        std::fs::write(&file, "data").unwrap();

        let result = execute(ExecuteCommandOptions {
            command: "echo ok".to_string(),
            timeout_secs: 5,
            cwd: file.to_string_lossy().to_string(),
        })
        .await;

        match result {
            Err(ToolsError::NotADirectory(path)) => {
                assert!(path.contains("afile"));
            }
            other => panic!("Expected NotADirectory, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stdout_truncated() {
        let result = execute(ExecuteCommandOptions {
            command: "for i in $(seq 1 5000); do echo \"line $i with padding to make it long xxxxxxxxxxxxxxxxxxxx\"; done".to_string(),
            timeout_secs: 10,
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.contains("bytes truncated"));
    }

    #[tokio::test]
    async fn test_stderr_truncated() {
        let result = execute(ExecuteCommandOptions {
            command: "for i in $(seq 1 5000); do echo \"error $i with padding to make it long xxxxxxxxxxxxxxxxxxxx\" 1>&2; done".to_string(),
            timeout_secs: 10,
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.contains("bytes truncated"));
        // Should be in stderr section
        assert!(res.contains("\nstderr:\n"));
    }

    #[tokio::test]
    async fn test_timeout_kills_process_group_not_just_direct_child() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let marker_str = marker.to_string_lossy().to_string();

        let result = execute(ExecuteCommandOptions {
            command: format!("(sleep 2 && touch {marker_str}) & sleep 100"),
            timeout_secs: 1,
            ..Default::default()
        })
        .await;

        assert!(matches!(result, Err(ToolsError::CommandTimeout(1))));

        // Avant le fix R8, seul `sh` est tué : le sous-shell backgrounded
        // (`&`) survit, se fait réparenter, et crée le marker ~1s après le
        // timeout. Marge large pour éviter la flakiness.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert!(
            !marker.exists(),
            "background grandchild survived the timeout — process group was not killed"
        );
    }
}
