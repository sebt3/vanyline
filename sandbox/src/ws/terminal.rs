//! Terminal WebSocket endpoint: `GET /ws/terminal`.
//!
//! Authentication is performed by a middleware layer (`ws_auth_middleware`)
//! in [`crate::ws::ticket`], which runs before the `WebSocketUpgrade`
//! extractor.
//!
//! Protocol :
//! - frame WS **binaire** entrante → octets écrits sur le stdin du PTY (master writer) ;
//! - octets lus du PTY (master reader) → frame WS **binaire** sortante ;
//! - frame WS **texte** entrante = JSON de contrôle ;
//!   `{"type":"resize","cols":N,"rows":M}` → resize du PTY ; tout autre JSON ignoré ;
//! - Close/erreur/fin de stream → fermer la session et tuer le groupe de processus.

use crate::AppState;
use axum::extract::ws::{Message, WebSocket, Utf8Bytes};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};

const PTY_BUF_SIZE: usize = 16 * 1024;

/// Handler de `GET /ws/terminal`. Le middleware
/// [`crate::ws::ticket::ws_auth_middleware`] a déjà validé et consommé le ticket.
/// Ouvre un PTY, spawn un shell dans `sandbox_root`, puis sert la session bidirectionnelle.
pub async fn handle_ws_terminal(
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(|ws| terminal_session(ws, state))
}

async fn terminal_session(mut ws: WebSocket, state: AppState) {
    let cwd = state.config.sandbox_root.clone();
    let size = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };

    let (master, mut child) = match spawn_shell(&cwd, size) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!("PTY spawn error: {e}");
            return;
        }
    };

    // Reader: read from PTY master in spawn_blocking, send chunks to async channel.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PTY_BUF_SIZE);
    let mut reader = match master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("PTY reader error: {e}");
            return;
        }
    };
    let _reader_handle = tokio::task::spawn_blocking(move || {
        let mut buf = vec![0u8; PTY_BUF_SIZE];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,    // EOF
                Ok(n) if tx.blocking_send(buf[..n].to_vec()).is_err() => break,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::yield_now();
                }
                Err(_) => break,
            }
        }
    });

    let mut writer = match master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("PTY writer error: {e}");
            return;
        }
    };

    // We no longer need to access master after taking writer, except for resize.
    // The _reader_handle drops the cloned reader when terminal_session ends.

    // Keep the master available for resize via a reference — but we can't borrow
    // mutably while writer mutably borrows. Instead, collect resize commands
    // and apply them lazily. For simplicity, just keep the master around.
    // Actually the writer was taken from master, master is still accessible.
    // Let's just move the writer borrow to the select loop and keep master
    // available for resize too—we'll need to restructure slightly.

    loop {
        // Re-cloning reader is not possible (already consumed). The reader handle
        // was moved into the spawn_blocking task. The writer handle is borrowed
        // mutably here (for write_all), which means we can't access master.resize
        // simultaneously.

        // To allow resize, we restructure: only access master for resize when
        // we receive a text frame (not in the binary write path).
        // Since writer is a separate fd clone, resize works on the master directly.

        tokio::select! {
            biased;

            received = rx.recv() => {
                match received {
                    Some(data) => {
                        if ws.send(Message::Binary(data.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }

            frame = ws.recv() => {
                match frame {
                    Some(Ok(Message::Binary(data))) => {
                        if let Err(e) = writer.write_all(&data) {
                            tracing::warn!("PTY write error: {e}");
                            break;
                        }
                        if let Err(e) = writer.flush() {
                            tracing::warn!("PTY flush error: {e}");
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Some(obj) = parse_text_frame(&text)
                            && let Err(e) = master.resize(obj) {
                                tracing::warn!("PTY resize error: {e}");
                            }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None | Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                        break;
                    }
                }
            }

            else => break,
        }
    }

    kill_process_group(&mut child);
}

/// Parses a text frame as JSON. Returns `Some(PtySize)` for resize frames,
/// `None` for all other cases (including non-JSON and unknown control types).
fn parse_text_frame(text: &Utf8Bytes) -> Option<PtySize> {
    let map: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = map.as_object()?;
    if obj.get("type")?.as_str()? != "resize" {
        return None;
    }
    let cols = obj.get("cols")?.as_u64()? as u16;
    let rows = obj.get("rows")?.as_u64()? as u16;
    Some(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}

/// Opens a PTY and spawns `$SHELL` (or `/bin/bash`) in `cwd`.
///
/// Environnement hérité tel quel. Retourne le master (contrôle/lecture/écriture)
/// et l'enfant (le shell, leader de session — son `process_id()` == pgid).
///
/// Erreurs : ouverture PTY, spawn du shell → `anyhow::Result`.
pub fn spawn_shell(
    cwd: &std::path::Path,
    size: PtySize,
) -> anyhow::Result<(
    Box<dyn portable_pty::MasterPty + Send>,
    Box<dyn portable_pty::Child + Send + Sync>,
)> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(size)?;

    let shell = std::env::var("SHELL").ok().unwrap_or_else(|| {
        "/bin/bash".to_string()
    });

    let mut cmd = CommandBuilder::new(shell);
    cmd.set_controlling_tty(true);
    cmd.cwd(cwd);

    let child = pair.slave.spawn_command(cmd)?;
    Ok((pair.master, child))
}

/// Tue le groupe de processus du shell (descendants inclus), pas seulement le PID
/// direct. Le shell est leader de session (setsid fait pgid == pid) → kill(-pid, SIGKILL).
/// Best-effort : une erreur (déjà mort) n'est pas remontée.
pub fn kill_process_group(child: &mut Box<dyn portable_pty::Child + Send + Sync>) {
    let pid = child.process_id();
    if let Some(pid) = pid {
        unsafe {
            // Safety: we only call libc::kill with a negative process group ID,
            // which is a standard Unix operation. The child process may already be
            // dead — that's handled by ignoring the return value.
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::{AppState, AuthState, build_app, config::Config};
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use std::io::Write;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn make_state(test_name: &str) -> AppState {
        use crate::ws::ticket::TicketStore;

        let tmpdir = std::env::temp_dir()
            .join(format!("vanyline-sandbox-terminal-test/{}", test_name));
        let sandbox_root = tmpdir.join("sandbox");

        std::fs::create_dir_all(&sandbox_root).unwrap();

        let config = Arc::new(Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            oidc_issuer: None,
            oidc_audience: None,
            auth_groups_admin: "kubernetes-admin".into(),
            auth_groups_read: "kubernetes-view".into(),
            no_auth: false,
            static_token: None,
            public_url: None,
            oidc_ca_cert: None,
            metrics_listen: "0.0.0.0:9090".into(),
            otel_endpoint: None,
            sandbox_root: sandbox_root.clone(),
        });
        let auth = AuthState::new(config.clone()).unwrap();

        AppState {
            config,
            auth: Arc::new(auth),
            tickets: TicketStore::new(),
        }
    }

    // Test 1: spawn_shell_writes_and_reads
    #[test]
    fn spawn_shell_writes_and_reads() {
        let state = make_state("pty_hello");

        // Spawn a fresh shell to ensure we start at a clean prompt.
        let (master, _child) = spawn_shell(&state.config.sandbox_root, PtySize::default())
            .expect("spawn_shell should succeed again");

        let mut reader = master
            .try_clone_reader()
            .expect("should have a reader");
        let mut writer = master
            .take_writer()
            .expect("should have a writer");

        writeln!(writer, "echo pty_hello\r").unwrap();

        // Read with timeout
        let start = std::time::Instant::now();
        let mut output = String::new();
        while start.elapsed().as_secs() < 10 {
            let mut buf: [u8; 4096] = [0; 4096];
            match reader.read(&mut buf) {
                Ok(0) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                Ok(n) => {
                    output.push_str(&String::from_utf8_lossy(&buf[..n]).into_owned());
                }
                Err(_) => break,
            }
            if output.contains("pty_hello") {
                break;
            }
            if start.elapsed().as_secs() >= 10 {
                break;
            }
        }

        assert!(
            output.contains("pty_hello"),
            "pty_hello not found in output: {}",
            output.lines().take(20).collect::<Vec<_>>().join("\n")
        );
    }

    // Test 2: kill_process_group_kills_descendants
    #[test]
    fn kill_process_group_kills_descendants() {
        let state = make_state("kill_fg");
        let marker = state.config.sandbox_root.join("_should_not_exist");

        let (master, mut child) = spawn_shell(&state.config.sandbox_root, PtySize::default())
                .expect("spawn_shell should succeed");

        let reader = master
            .try_clone_reader()
            .expect("should have a reader");
        let mut writer = master
            .take_writer()
            .expect("should have a writer");

        // Spawn a background job: sleep 1s then touch the marker file.
        write!(
            writer,
            "(sleep 1 && touch _should_not_exist) &\r\n"
        )
        .unwrap();

        // Wait for the shell to start the background job.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Kill the process group.
        kill_process_group(&mut child);

        // Drop the PTY handles so the marker file can be accessed from the test process.
        drop(reader);
        drop(writer);
        drop(master);
        drop(child);

        // Wait for the sleep+touch to complete (or fail because it was killed).
        std::thread::sleep(std::time::Duration::from_secs(2));

        assert!(
            !marker.exists(),
            "marker file should not exist — background process killed in process group"
        );
    }

    // Test 3: resize_updates_pty_size
    #[test]
    fn resize_updates_pty_size() {
        let tmpdir = std::env::temp_dir().join("vanyline-sandbox-terminal-test/resize_pty_size");
        let sandbox_root = tmpdir.join("sandbox");
        std::fs::create_dir_all(&sandbox_root).unwrap();

        let config = Arc::new(Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            oidc_issuer: None,
            oidc_audience: None,
            auth_groups_admin: "kubernetes-admin".into(),
            auth_groups_read: "kubernetes-view".into(),
            no_auth: false,
            static_token: None,
            public_url: None,
            oidc_ca_cert: None,
            metrics_listen: "0.0.0.0:9090".into(),
            otel_endpoint: None,
            sandbox_root: sandbox_root.clone(),
        });
        let auth = AuthState::new(config.clone()).unwrap();

        let state = AppState {
            config,
            auth: Arc::new(auth),
            tickets: crate::ws::ticket::TicketStore::new(),
        };

        let (master, _child) = spawn_shell(&state.config.sandbox_root, PtySize::default())
            .expect("spawn_shell should succeed");

        // Resize to 40 rows × 100 cols.
        master
            .resize(PtySize {
                rows: 40,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("resize should succeed");

        let size = master.get_size().expect("get_size should succeed");
        assert_eq!(size.rows, 40, "rows should be 40");
        assert_eq!(size.cols, 100, "cols should be 100");
    }

    // Test 4: terminal_ticket_required — missing → 401, unknown → 401
    #[tokio::test]
    async fn terminal_ticket_required() {
        let state = make_state("ticket_terminal");
        let app = build_app(state);

        // No ticket → 401
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/ws/terminal")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "missing ticket → 401");

        // Unknown ticket → 401
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/ws/terminal?ticket=unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "unknown ticket → 401");
    }
}
