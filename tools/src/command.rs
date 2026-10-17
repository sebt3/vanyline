use crate::error::ToolsError;
use crate::filesystem;
use crate::output;

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Instant;

/// Future boxed `Send` — type d'effacement des outils asynchrones du crate.
pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Verrou des tests qui modifient le cwd du processus (partagé entre modules de test).
#[cfg(test)]
pub(crate) static CWD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Options d'exécution de commande.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ExecuteCommandOptions {
    /// Commande passée à `sh -c`.
    pub command: String,
    /// 0 = pas de timeout (comportement déjà existant, inchangé).
    #[serde(default)]
    pub timeout_secs: u64,
    /// Répertoire de travail. Vide = hérite du cwd du processus courant.
    #[serde(default)]
    pub cwd: String,
    /// Paires d'env additionnelles appliquées au process APRES le reste de la
    /// config, avant `spawn()` (feature python-support : activation `.venv/`
    /// posée par l'appelant sandbox). `#[serde(skip)]` : jamais désérialisé
    /// depuis les arguments du tool `execute_command` — un LLM ne peut pas
    /// injecter d'env. Posé uniquement par du code sandbox (`dispatch_command`).
    /// Vide = comportement historique strict (cli, tests, tous les autres).
    #[serde(skip)]
    pub envs: Vec<(String, String)>,
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
                    });
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

        for (k, v) in &opts.envs {
            cmd.env(k, v);
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
                        // pid_t == i32 et tout pid du noyau tient en i32 : try_from échoue
                        // uniquement hors plage POSIX, où le kill serait sans objet.
                        if let Ok(pgid) = libc::pid_t::try_from(pid) {
                            #[allow(unsafe_code)]
                            // SAFETY: kill(2) sur pgid du process_group(0) — voir commentaire au-dessus
                            unsafe {
                                libc::kill(pgid.wrapping_neg(), libc::SIGKILL);
                            }
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
            command: r#"echo "hello world""#.to_string(),
            timeout_secs: 5,
            envs: Vec::new(),
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
            envs: Vec::new(),
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
            envs: Vec::new(),
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
            envs: Vec::new(),
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
            envs: Vec::new(),
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
            command: String::new(),
            timeout_secs: 5,
            envs: Vec::new(),
            ..Default::default()
        })
        .await;

        match result {
            Err(ToolsError::InvalidArgument { name, reason }) => {
                assert_eq!(name, "command");
                assert!(reason.contains("must not be empty"));
            }
            other => panic!("Expected InvalidArgument, got: {other:?}"),
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
            envs: Vec::new(),
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        // "duration: " followed by a number followed by "s"
        assert!(res.contains("duration: "));
        let duration_line = res.lines().find(|l| l.starts_with("duration: ")).unwrap();
        assert!(duration_line.contains('s'));
    }

    #[tokio::test]
    async fn test_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute(ExecuteCommandOptions {
            command: "pwd".to_string(),
            timeout_secs: 5,
            cwd: dir.path().to_string_lossy().to_string(),
            envs: Vec::new(),
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
            envs: Vec::new(),
        })
        .await;

        match result {
            Err(ToolsError::FileNotFound { path, .. }) => {
                assert!(path.contains("nonexistent"));
            }
            other => panic!("Expected FileNotFound, got: {other:?}"),
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
            envs: Vec::new(),
        })
        .await;

        match result {
            Err(ToolsError::NotADirectory(path)) => {
                assert!(path.contains("afile"));
            }
            other => panic!("Expected NotADirectory, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_stdout_truncated() {
        let result = execute(ExecuteCommandOptions {
            command: "for i in $(seq 1 5000); do echo \"line $i with padding to make it long xxxxxxxxxxxxxxxxxxxx\"; done".to_string(),
            timeout_secs: 10,
            envs: Vec::new(),
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
            envs: Vec::new(),
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
            envs: Vec::new(),
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

    // -----------------------------------------------------------------------
    /// feature python-support : `envs` (overlay `.venv/` posé par l'appelant)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn execute_envs_appliquees() {
        let result = execute(ExecuteCommandOptions {
            command: r#"echo "${VNL_TEST_ENV:-absent}""#.to_string(),
            timeout_secs: 5,
            envs: vec![("VNL_TEST_ENV".into(), "overlay-ok".into())],
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.contains("overlay-ok"), "expected overlay value: {res}");
        assert!(!res.contains("absent"), "env var not applied: {res}");
    }

    // Nom de test imposé verbatim par la tâche 05 (`LISTE` = tête de PATH,
    // accentuation uppercase assumée dans l'énoncé) — d'où l'allow ciblé
    // plutôt qu'un renommage (même précédent que `sandbox/src/lsp.rs`).
    #[allow(non_snake_case)]
    #[tokio::test]
    async fn execute_envs_path_tete_de_LISTE() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("vnl-env-marker");
        std::fs::write(&script, "#!/bin/sh\necho marker-ok\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        // La commande n'existe que dans le tmpdir préfixant PATH : elle ne
        // peut être résolue QUE grâce à l'overlay.
        let result = execute(ExecuteCommandOptions {
            command: "vnl-env-marker".to_string(),
            timeout_secs: 5,
            envs: vec![(
                "PATH".into(),
                format!("{}:/usr/bin:/bin", dir.path().to_string_lossy()),
            )],
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.contains("marker-ok"), "expected PATH head: {res}");
        assert!(res.contains("exit code: 0"), "expected success: {res}");

        // Volet négatif déterministe : sans l'overlay, même commande ⟹ exit
        // code ≠ 0 (résolution impossible, sh renvoie 127 ; pas d'assertion
        // sur le message « not found » qui varie selon le shell).
        let result_without = execute(ExecuteCommandOptions {
            command: "vnl-env-marker".to_string(),
            timeout_secs: 5,
            ..Default::default()
        })
        .await;
        assert!(result_without.is_ok());
        let res_without = result_without.unwrap();
        assert!(
            !res_without.contains("exit code: 0"),
            "command should fail without the PATH overlay: {res_without}"
        );
    }

    #[tokio::test]
    async fn execute_envs_vide_identique() {
        // `envs: Vec::new()` = comportement historique strict : aucune env
        // additionnelle (le `-absent` par défaut du shell doit gagner).
        let result = execute(ExecuteCommandOptions {
            command: r#"echo "${VNL_TEST_ENV:-absent}""#.to_string(),
            timeout_secs: 5,
            envs: Vec::new(),
            ..Default::default()
        })
        .await;

        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(
            res.contains("absent"),
            "expected historical behavior: {res}"
        );
        assert!(!res.contains("overlay-ok"), "no env injected: {res}");
    }

    /// Extrait la section stdout (entre `stdout:\n` et `\nstderr:\n`).
    fn stdout_section(res: &str) -> &str {
        let after = res.split_once("stdout:\n").map_or("", |(_, rest)| rest);
        after
            .split_once("\nstderr:\n")
            .map_or(after, |(out, _)| out)
    }

    /// Extrait la section stderr (après `\nstderr:\n`).
    fn stderr_section(res: &str) -> &str {
        res.split_once("\nstderr:\n").map_or("", |(_, err)| err)
    }

    fn cmd(command: &str, timeout_secs: u64) -> ExecuteCommandOptions {
        ExecuteCommandOptions {
            command: command.to_string(),
            timeout_secs,
            ..Default::default()
        }
    }

    /// Scenario `commande_vide_invalid_argument`.
    #[tokio::test]
    async fn commande_vide_invalid_argument() {
        let err = execute(cmd("", 5)).await.unwrap_err();
        match err {
            ToolsError::InvalidArgument { name, reason } => {
                assert_eq!(name, "command");
                assert!(reason.contains("must not be empty"));
            }
            other => panic!("InvalidArgument attendu, obtenu : {other:?}"),
        }
    }

    /// Scenario `commande_vide_prioritaire_sur_cwd`.
    #[tokio::test]
    async fn commande_vide_prioritaire_sur_cwd() {
        let err = execute(ExecuteCommandOptions {
            command: String::new(),
            timeout_secs: 5,
            cwd: "/nonexistent/path/xyz".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
        assert!(matches!(err, ToolsError::InvalidArgument { .. }));
    }

    /// Scenario `shell_semantique_quotes_et_pipe`.
    #[tokio::test]
    async fn shell_semantique_quotes_et_pipe() {
        let quoted = execute(cmd(r#"echo "hello world""#, 5)).await.unwrap();
        assert!(quoted.contains("hello world"));
        let piped = execute(cmd("echo hi | tr a-z A-Z", 5)).await.unwrap();
        assert!(piped.contains("HI"));
    }

    /// Scenario `exit_code_non_zero_reste_ok`.
    #[tokio::test]
    async fn exit_code_non_zero_reste_ok() {
        let res = execute(cmd("false", 5)).await.unwrap();
        assert!(res.contains("exit code: 1"));
    }

    /// Scenario `exit_code_signal_vaut_minus_un`.
    #[tokio::test]
    async fn exit_code_signal_vaut_minus_un() {
        let res = execute(cmd("kill -9 $$", 5)).await.unwrap();
        assert!(res.contains("exit code: -1"), "{res}");
    }

    /// Scenario `sortie_formatee_quatre_sections`.
    #[tokio::test]
    async fn sortie_formatee_quatre_sections() {
        let res = execute(cmd("echo test", 5)).await.unwrap();
        let rest = res
            .strip_prefix("exit code: 0\nduration: ")
            .unwrap_or_else(|| panic!("en-tête inattendu : {res}"));
        let (duration, after) = rest
            .split_once("s\n")
            .unwrap_or_else(|| panic!("durée non suivie de s : {res}"));
        let decimals = duration.split_once('.').map(|(_, d)| d.len());
        assert_eq!(decimals, Some(2), "durée à 2 décimales : {duration}");
        assert!(duration.parse::<f64>().is_ok());
        assert_eq!(after, "stdout:\ntest\n\nstderr:\n");
    }

    /// Scenario `stream_vides_sections_vides`.
    #[tokio::test]
    async fn stream_vides_sections_vides() {
        let res = execute(cmd("true", 5)).await.unwrap();
        assert!(res.contains("stdout:\n\nstderr:"), "{res}");
    }

    /// Scenario `stderr_capture_dans_section_stderr`.
    #[tokio::test]
    async fn stderr_capture_dans_section_stderr() {
        let res = execute(cmd("echo out; echo err 1>&2", 5)).await.unwrap();
        let out = stdout_section(&res);
        assert!(out.contains("out"));
        assert!(!out.contains("err"));
        assert!(stderr_section(&res).contains("err"));
    }

    /// Scenario `timeout_zero_completion_normale`.
    #[tokio::test]
    async fn timeout_zero_completion_normale() {
        let res = execute(cmd("echo ok", 0)).await.unwrap();
        assert!(res.contains("ok"));
    }

    /// Scenario `timeout_depasse_command_timeout`.
    #[tokio::test]
    async fn timeout_depasse_command_timeout() {
        let start = Instant::now();
        let err = execute(cmd("sleep 5", 1)).await.unwrap_err();
        let elapsed = start.elapsed();
        assert!(matches!(err, ToolsError::CommandTimeout(1)), "{err:?}");
        assert!(elapsed < Duration::from_secs(3), "trop long : {elapsed:?}");
    }

    /// Scenario `timeout_tue_groupe_et_descendants`.
    #[tokio::test]
    async fn timeout_tue_groupe_et_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marqueur");
        let command = format!("(sleep 2 && touch {}) & sleep 100", marker.display());
        let err = execute(cmd(&command, 1)).await.unwrap_err();
        assert!(matches!(err, ToolsError::CommandTimeout(1)), "{err:?}");
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!marker.exists(), "l'arrière-plan a survécu au SIGKILL");
    }

    /// Scenario `timeout_jette_sortie_partielle`.
    #[tokio::test]
    async fn timeout_jette_sortie_partielle() {
        let err = execute(cmd("echo visible; sleep 5", 1)).await.unwrap_err();
        assert!(matches!(err, ToolsError::CommandTimeout(1)), "{err:?}");
        assert!(!err.to_string().contains("visible"));
    }

    /// Scenario `attente_sortie_jusqu_a_fermeture_pipes`.
    #[tokio::test]
    async fn attente_sortie_jusqu_a_fermeture_pipes() {
        let start = Instant::now();
        let res = execute(cmd("sleep 2 & echo vite", 10)).await.unwrap();
        let elapsed = start.elapsed();
        assert!(res.contains("vite"));
        assert!(elapsed >= Duration::from_millis(1800), "{elapsed:?}");
    }

    /// Scenario `cwd_applique_au_process`.
    #[tokio::test]
    async fn cwd_applique_au_process() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let res = execute(ExecuteCommandOptions {
            command: "pwd".into(),
            timeout_secs: 5,
            cwd: dir.path().to_string_lossy().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
        let first = stdout_section(&res).lines().next().unwrap_or_default();
        assert_eq!(std::fs::canonicalize(first).unwrap(), canonical);
    }

    /// Scenario `cwd_vide_herite_du_processus_courant`.
    #[tokio::test]
    async fn cwd_vide_herite_du_processus_courant() {
        let _guard = CWD_LOCK.lock().await;
        let res = execute(cmd("pwd", 5)).await.unwrap();
        let first = stdout_section(&res).lines().next().unwrap_or_default();
        let expected = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        assert_eq!(std::fs::canonicalize(first).unwrap(), expected);
    }

    /// Scenario `cwd_inexistant_file_not_found_avec_hint`.
    #[tokio::test]
    async fn cwd_inexistant_file_not_found_avec_hint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        let missing = dir.path().join("manquant");
        let err = execute(ExecuteCommandOptions {
            command: "pwd".into(),
            timeout_secs: 5,
            cwd: missing.to_string_lossy().to_string(),
            ..Default::default()
        })
        .await
        .unwrap_err();
        match err {
            ToolsError::FileNotFound { path, hint } => {
                assert!(path.contains("manquant"));
                assert!(hint.contains("a.txt") && hint.contains("b.txt"), "{hint}");
            }
            other => panic!("FileNotFound attendu, obtenu : {other:?}"),
        }
    }

    /// Scenario `cwd_fichier_regulier_not_a_directory`.
    #[tokio::test]
    async fn cwd_fichier_regulier_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fichier.txt");
        std::fs::write(&file, "").unwrap();
        let err = execute(ExecuteCommandOptions {
            command: "pwd".into(),
            timeout_secs: 5,
            cwd: file.to_string_lossy().to_string(),
            ..Default::default()
        })
        .await
        .unwrap_err();
        match err {
            ToolsError::NotADirectory(path) => assert!(path.contains("fichier.txt")),
            other => panic!("NotADirectory attendu, obtenu : {other:?}"),
        }
    }

    /// Scenario `cwd_symlink_vers_repertoire_accepte`.
    #[tokio::test]
    async fn cwd_symlink_vers_repertoire_accepte() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("cible");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("lien");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let res = execute(ExecuteCommandOptions {
            command: "pwd".into(),
            timeout_secs: 5,
            cwd: link.to_string_lossy().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
        let first = stdout_section(&res).lines().next().unwrap_or_default();
        assert_eq!(
            std::fs::canonicalize(first).unwrap(),
            std::fs::canonicalize(&target).unwrap()
        );
    }

    /// Scenario `stdout_borne_tete_et_queue_8kib`.
    #[tokio::test]
    async fn stdout_borne_tete_et_queue_8kib() {
        let command = "i=1; while [ $i -le 5000 ]; do echo \"line $i xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"; i=$((i+1)); done";
        let res = execute(cmd(command, 10)).await.unwrap();
        let out = stdout_section(&res);
        assert!(out.contains("bytes truncated"));
        assert!(out.starts_with("line 1 "));
        assert!(out.contains("line 5000 "));
        assert!(
            out.len() <= output::COMMAND_MAX_BYTES + 128,
            "{}",
            out.len()
        );
    }

    /// Scenario `stderr_borne_tete_et_queue_8kib`.
    #[tokio::test]
    async fn stderr_borne_tete_et_queue_8kib() {
        let command = "i=1; while [ $i -le 5000 ]; do echo \"line $i xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\" 1>&2; i=$((i+1)); done";
        let res = execute(cmd(command, 10)).await.unwrap();
        let err = stderr_section(&res);
        assert!(err.contains("bytes truncated"));
        assert!(err.starts_with("line 1 "));
        assert!(err.contains("line 5000 "));
    }

    /// Scenario `utf8_invalide_remapensee_en_fffd`.
    #[tokio::test]
    async fn utf8_invalide_remapensee_en_fffd() {
        let res = execute(cmd("printf '\\377'", 5)).await.unwrap();
        assert!(stdout_section(&res).contains('\u{FFFD}'));
    }

    /// Scenario `envs_appliquees_au_process`.
    #[tokio::test]
    async fn envs_appliquees_au_process() {
        let res = execute(ExecuteCommandOptions {
            command: r#"echo "${VNL_TEST_ENV:-absent}""#.into(),
            timeout_secs: 5,
            envs: vec![("VNL_TEST_ENV".into(), "overlay-ok".into())],
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(res.contains("overlay-ok"));
        assert!(!res.contains("absent"));
    }

    /// Scenario `envs_vides_comportement_historique`.
    #[tokio::test]
    async fn envs_vides_comportement_historique() {
        let res = execute(ExecuteCommandOptions {
            command: r#"echo "${VNL_TEST_ENV:-absent}""#.into(),
            timeout_secs: 5,
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(res.contains("absent"));
        assert!(!res.contains("overlay-ok"));
    }

    /// Scenario `envs_meme_cle_derniere_paire_gagne`.
    #[tokio::test]
    async fn envs_meme_cle_derniere_paire_gagne() {
        let res = execute(ExecuteCommandOptions {
            command: "echo $VNL_DUP".into(),
            timeout_secs: 5,
            envs: vec![
                ("VNL_DUP".into(), "premiere".into()),
                ("VNL_DUP".into(), "seconde".into()),
            ],
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(res.contains("seconde"));
        assert!(!res.contains("premiere"));
    }

    /// Scenario `envs_surcharge_path_tete_de_liste`.
    #[tokio::test]
    async fn envs_surcharge_path_tete_de_liste() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("vnl-env-marker");
        std::fs::write(&script, "#!/bin/sh\necho marker-ok\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let base_path = std::env::var("PATH").unwrap_or_default();

        let with_overlay = execute(ExecuteCommandOptions {
            command: "vnl-env-marker".into(),
            timeout_secs: 5,
            envs: vec![(
                "PATH".into(),
                format!("{}:{base_path}", dir.path().display()),
            )],
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(with_overlay.contains("marker-ok"), "{with_overlay}");
        assert!(with_overlay.contains("exit code: 0"), "{with_overlay}");

        let without = execute(cmd("vnl-env-marker", 5)).await.unwrap();
        assert!(!without.contains("exit code: 0"), "{without}");
    }

    /// Scenario `envs_jamais_deserialisees`.
    #[test]
    fn envs_jamais_deserialisees() {
        let opts: ExecuteCommandOptions =
            serde_json::from_str(r#"{"command":"x","envs":[["FOO","bar"]]}"#).unwrap();
        assert!(opts.envs.is_empty());
    }

    /// Scenario `envs_absentes_de_la_serialisation`.
    #[test]
    fn envs_absentes_de_la_serialisation() {
        let opts = ExecuteCommandOptions {
            command: "x".into(),
            envs: vec![("FOO".into(), "bar".into())],
            ..Default::default()
        };
        let value = serde_json::to_value(&opts).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["command", "cwd", "timeout_secs"]);
    }

    /// Scenario `timeout_secs_et_cwd_defauts_deser`.
    #[test]
    fn timeout_secs_et_cwd_defauts_deser() {
        let opts: ExecuteCommandOptions = serde_json::from_str(r#"{"command":"x"}"#).unwrap();
        assert_eq!(opts.timeout_secs, 0);
        assert!(opts.cwd.is_empty());
    }
}
