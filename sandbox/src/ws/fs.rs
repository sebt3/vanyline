//! Filesystem WebSocket endpoint: `GET /ws/fs`.
//!
//! Authentication is performed by a middleware layer (`ws_auth_middleware`)
//! in [`crate::ws::ticket`], which runs before the `WebSocketUpgrade`
//! extractor. This avoids axum's 426 conversion issue when a handler with
//! `WebSocketUpgrade` returns a non-OnUpgrade response (e.g. 401 auth error).

use crate::AppState;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;

/// Query-string par lequel le navigateur présente son ticket au handshake WS.
#[derive(Debug, serde::Deserialize)]
pub struct TicketQuery {
    #[serde(default)]
    pub ticket: Option<String>,
}

/// Handler de `GET /ws/fs`. Le middleware [`crate::ws::ticket::ws_auth_middleware`]
/// a déjà validé et consommé le ticket. Ce handler met simplement en place la
/// session requête/réponse filesystem.
pub async fn handle_ws_fs(
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(|ws| fs_session(state, ws))
}

/// Boucle requête/réponse : chaque frame texte JSON entrante reçoit une frame
/// texte JSON en sortie. Les frames binaires non JSON sont rejetées avec
/// `{"ok":false,"error":"unexpected binary frame"}`.
async fn fs_session(state: AppState, mut ws: WebSocket) {
    loop {
        match ws.recv().await {
            Some(Ok(Message::Text(raw))) => {
                let resp = dispatch_fs_message(&state, &raw).await;
                if ws
                    .send(Message::Text(resp.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
            Some(Ok(_)) => {
                let _ = ws
                    .send(Message::Text(
                        serde_json::json!({ "ok": false, "error": "unexpected binary frame" })
                            .to_string()
                            .into(),
                    ))
                    .await;
            }
        }
    }
}

/// Parse un message JSON entrant et renvoie le JSON de réponse correspondant.
/// Rejette les messages non JSON avec `{"ok":false,"error":"invalid JSON"}`.
/// Une `op` inconnue → `{"ok":false,"error":"unknown op"}`.
pub async fn dispatch_fs_message(state: &AppState, raw: &str) -> serde_json::Value {
    let root = &state.config.sandbox_root;

    let msg: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => {
            return serde_json::json!({ "ok": false, "error": "invalid JSON" });
        }
    };
    let op = match msg["op"].as_str() {
        Some(o) => o.to_string(),
        None => {
            return serde_json::json!({ "ok": false, "error": "missing op" });
        }
    };
    // "root" op — no path required, returns the canonical sandbox root.
    if op == "root" {
        let root_path = match crate::tools_impl::confine_path(root, "") {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }),
        };
        return serde_json::json!({ "ok": true, "root": root_path });
    }
    let path = match msg["path"].as_str() {
        Some(p) => p.to_string(),
        None => return serde_json::json!({ "ok": false, "error": "missing path" }),
    };
    let resolved = match crate::tools_impl::confine_path(root, &path) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }),
    };
    match &op[..] {
        "read" => {
            let offset = msg["offset"].as_u64().unwrap_or(0);
            let limit = msg["limit"].as_u64().unwrap_or(0);
            let raw = msg["raw"].as_bool().unwrap_or(false);
            let opts = vanyline_tools::filesystem::ReadFileOptions {
                path: resolved.clone(),
                offset: offset as usize,
                limit: limit as usize,
                raw,
            };
            match vanyline_tools::filesystem::read_file(opts).await {
                Ok(text) => {
                    let truncated = !raw && text.contains("truncated");
                    serde_json::json!({ "ok": true, "content": text, "truncated": truncated })
                }
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        "write" => {
            let content = match msg["content"].as_str() {
                Some(c) => c.to_string(),
                None => return serde_json::json!({ "ok": false, "error": "missing content" }),
            };
            let opts = vanyline_tools::filesystem::WriteFileOptions {
                path: resolved.clone(),
                content,
            };
            match vanyline_tools::filesystem::write_file(opts).await {
                Ok(()) => serde_json::json!({ "ok": true, "wrote": resolved }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        "edit" => {
            let old_string = match msg["old_string"].as_str() {
                Some(s) => s.to_string(),
                None => return serde_json::json!({ "ok": false, "error": "missing old_string" }),
            };
            let new_string = match msg["new_string"].as_str() {
                Some(s) => s.to_string(),
                None => return serde_json::json!({ "ok": false, "error": "missing new_string" }),
            };
            let replace_all = msg["replace_all"].as_bool().unwrap_or(false);
            let opts = vanyline_tools::filesystem::EditFileOptions {
                path: resolved.clone(),
                old_string,
                new_string,
                replace_all,
            };
            match vanyline_tools::filesystem::edit_file(opts).await {
                Ok(text) => serde_json::json!({ "ok": true, "content": text }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        "delete" => {
            let opts = vanyline_tools::filesystem::DeleteFileOptions {
                path: resolved.clone(),
            };
            match vanyline_tools::filesystem::delete_file(opts).await {
                Ok(()) => serde_json::json!({ "ok": true, "deleted": resolved }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        "mkdir" => {
            let opts = vanyline_tools::filesystem::MkdirOptions {
                path: resolved.clone(),
            };
            match vanyline_tools::filesystem::mkdir(opts).await {
                Ok(()) => serde_json::json!({ "ok": true, "mkdir": resolved }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        "rename" => {
            let to = match msg["to"].as_str() {
                Some(t) => t.to_string(),
                None => return serde_json::json!({ "ok": false, "error": "missing to" }),
            };
            let resolved_to = match crate::tools_impl::confine_path(root, &to) {
                Ok(p) => p.to_string_lossy().into_owned(),
                Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }),
            };
            let opts = vanyline_tools::filesystem::RenameFileOptions {
                path: resolved.clone(),
                to: resolved_to.clone(),
            };
            match vanyline_tools::filesystem::rename_file(opts).await {
                Ok(()) => serde_json::json!({ "ok": true, "renamed": resolved_to }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        "list" => {
            let depth = msg["depth"].as_u64().unwrap_or(0);
            let opts = vanyline_tools::filesystem::ListDirectoryOptions {
                path: resolved.clone(),
                depth: depth as usize,
            };
            match vanyline_tools::filesystem::list_directory(opts).await {
                Ok(text) => {
                    let truncated = text.contains("truncated");
                    serde_json::json!({ "ok": true, "entries": text, "truncated": truncated })
                }
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        _ => serde_json::json!({ "ok": false, "error": "unknown op" }),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// Helper to create an AppState with a fresh TempDir for each test.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn make_state(test_name: &str) -> AppState {
    use crate::{AuthState, config::Config};

    let tmpdir = std::env::temp_dir().join(format!("vanyline-sandbox-fs-test/{}", test_name));
    let sandbox_root = std::path::PathBuf::from(format!("{}/sandbox", tmpdir.display()));

    std::fs::create_dir_all(&sandbox_root).unwrap();
    std::fs::create_dir_all(sandbox_root.join("sub")).unwrap();
    std::fs::write(
        sandbox_root.join("sub/file.txt"),
        "hello\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15\nline16\nline17\nline18\nline19\nline20\n",
    ).unwrap();

    let config = std::sync::Arc::new(Config {
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
        auth: std::sync::Arc::new(auth),
        tickets: crate::ws::ticket::TicketStore::new(),
    }
}

#[cfg(test)]
fn ok(val: &serde_json::Value) -> bool {
    val["ok"].as_bool().unwrap_or(false)
}

// ── Dispatch-tests (no WS needed) ────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    // Test 1: read_file_returns_content
    #[tokio::test]
    async fn read_file_returns_content() {
        let state = make_state("read");
        let resp = dispatch_fs_message(&state, r#"{"op":"read","path":"sub/file.txt"}"#).await;
        assert!(ok(&resp));
        let content = resp["content"].as_str().unwrap();
        assert!(
            content.contains("hello"),
            "content should contain 'hello', got: {content}"
        );
        assert!(!resp["truncated"].as_bool().unwrap());
    }

    // Test 2: read_file_with_limit_truncates
    #[tokio::test]
    async fn read_file_with_limit_truncates() {
        let state = make_state("read_limit");
        let resp =
            dispatch_fs_message(&state, r#"{"op":"read","path":"sub/file.txt","limit":10}"#).await;
        assert!(ok(&resp));
        let content = resp["content"].as_str().unwrap();
        assert!(content.contains("truncated"));
        assert!(resp["truncated"].as_bool().unwrap());
    }

    // Test 3: write_file_creates
    #[tokio::test]
    async fn write_file_creates() {
        let state = make_state("write");
        let resp =
            dispatch_fs_message(&state, r#"{"op":"write","path":"new.txt","content":"abc"}"#).await;
        assert!(ok(&resp));
        let wrote = resp["wrote"].as_str().unwrap();
        assert!(
            wrote.ends_with("/new.txt"),
            "wrote path should end with /new.txt, got {wrote}"
        );

        // Verify file exists on disk
        let sandbox_root = &state.config.sandbox_root;
        assert!(
            sandbox_root.join("new.txt").exists(),
            "new.txt should exist on disk"
        );

        // Clean up for subsequent tests
        let _ = std::fs::remove_file(sandbox_root.join("new.txt"));
    }

    // Test 4: edit_file_replaces
    #[tokio::test]
    async fn edit_file_replaces() {
        let state = make_state("edit");
        let resp = dispatch_fs_message(
            &state,
            r#"{"op":"edit","path":"sub/file.txt","old_string":"hello","new_string":"hi"}"#,
        )
        .await;
        assert!(ok(&resp));
        // edit_file returns "edited /path: N replacement(s)"
        let content = resp["content"].as_str().unwrap();
        assert!(
            content.contains("edited") && content.contains("sandbox/sub/file.txt"),
            "expect edit success message, got: {content}"
        );

        // Verify file content on disk
        let sandbox_root = &state.config.sandbox_root;
        let file_content = std::fs::read_to_string(sandbox_root.join("sub/file.txt")).unwrap();
        assert!(
            file_content.contains("hi\n"),
            "file should contain 'hi', got: {file_content}"
        );
    }

    // Test 5: delete_file_removes
    #[tokio::test]
    async fn delete_file_removes() {
        // Re-create the file fresh (edit might have changed it)
        let tmpdir = std::env::temp_dir().join("vanyline-sandbox-fs-test/delete");
        let sandbox_root = tmpdir.join("sandbox");
        std::fs::create_dir_all(&sandbox_root).ok();
        std::fs::create_dir_all(sandbox_root.join("sub")).ok();
        std::fs::write(sandbox_root.join("sub/file.txt"), "delete_me\n").ok();

        let state = make_state("delete");
        let resp = dispatch_fs_message(&state, r#"{"op":"delete","path":"sub/file.txt"}"#).await;
        assert!(ok(&resp));
        let deleted = resp["deleted"].as_str().unwrap();
        assert!(
            deleted.ends_with("/sub/file.txt"),
            "deleted path should end with /sub/file.txt, got {deleted}"
        );
        assert!(
            !sandbox_root.join("sub/file.txt").exists(),
            "deleted file should not exist on disk"
        );
    }

    // Test 6: list_directory_returns_entries
    #[tokio::test]
    async fn list_directory_returns_entries() {
        let state = make_state("list");
        let resp = dispatch_fs_message(&state, r#"{"op":"list","path":"sub"}"#).await;
        assert!(ok(&resp));
        assert!(resp["entries"].is_string());
        let entries: String = resp["entries"].as_str().unwrap().to_string();
        assert!(
            entries.contains("file.txt"),
            "entries should contain file.txt, got {entries}"
        );
    }

    // Test 7: path_escape_rejected — error contains VNL-SBX-001
    #[tokio::test]
    async fn path_escape_rejected() {
        let state = make_state("escape");
        let resp = dispatch_fs_message(&state, r#"{"op":"read","path":"../../etc/passwd"}"#).await;
        assert!(!ok(&resp));
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("VNL-SBX-001"),
            "expected VNL-SBX-001 in error, got: {err}"
        );
    }

    // Test 8: unknown_op_rejected
    #[tokio::test]
    async fn unknown_op_rejected() {
        let state = make_state("unknown_op");
        let resp = dispatch_fs_message(&state, r#"{"op":"bogus","path":"sub"}"#).await;
        assert!(!ok(&resp));
        assert_eq!(resp["error"].as_str().unwrap(), "unknown op");
    }

    // Test 9: invalid_json_rejected
    #[tokio::test]
    async fn invalid_json_rejected() {
        let state = make_state("invalid_json");
        let resp = dispatch_fs_message(&state, "not json").await;
        assert!(!ok(&resp));
        assert_eq!(resp["error"].as_str().unwrap(), "invalid JSON");
    }

    /// Test 10a: raw mode returns untouched content
    #[tokio::test]
    async fn read_raw_returns_untouched_content() {
        let state = make_state("read_raw");
        let resp =
            dispatch_fs_message(&state, r#"{"op":"read","path":"sub/file.txt","raw":true}"#).await;
        assert!(ok(&resp));
        let content = resp["content"].as_str().unwrap();
        // Raw content must NOT have line-number prefix "    1\t"
        assert!(
            !content.contains("  1\t"),
            "raw content must not be numbered, got: {content}"
        );
        // Should contain the original first line
        assert!(
            content.starts_with("hello\n"),
            "raw content should start with file content, got: {content}"
        );
        // Must not mention truncation
        assert!(!content.contains("truncated"));
        // truncated field must be false
        assert!(!resp["truncated"].as_bool().unwrap());
    }

    /// Test 10b: default (non-raw) still numbers lines — regression test
    #[tokio::test]
    async fn read_default_still_numbered() {
        let state = make_state("read_default");
        let resp = dispatch_fs_message(&state, r#"{"op":"read","path":"sub/file.txt"}"#).await;
        assert!(ok(&resp));
        let content = resp["content"].as_str().unwrap();
        // Default mode must start with numbered line
        assert!(
            content.starts_with("    1\t"),
            "default content should start with numbered line, got: {content}"
        );
    }

    // Test 11: ticket_query_required — missing ticket → 401
    // Test 10b: unknown ticket → 401 (and consumed)
    #[tokio::test]
    async fn ticket_query_required() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

        // Build the full app (public router has /ws/fs with auth middleware).
        let state = make_state("ticket");
        let app = crate::build_app(state.clone());

        // No ticket → 401 (MissingTicket)
        // The ws_auth_middleware runs before WebSocketUpgrade, so it doesn't need
        // full WebSocket headers — a plain HTTP GET suffices.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/ws/fs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "missing ticket → 401"
        );

        // Unknown ticket → 401 (InvalidTicket)
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/ws/fs?ticket=unknown-ticket")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "unknown ticket → 401"
        );

        // Verify the ticket is consumed: second redeem of same ticket must fail
        let claims =
            crate::ws::ticket::redeem_from_query(&state.tickets, Some("unknown-ticket".into()));
        assert!(
            claims.is_err(),
            "unknown ticket should be consumed and return error"
        );
        assert!(matches!(
            claims.unwrap_err(),
            crate::ws::ticket::WsAuthError::InvalidTicket
        ));

        // Verify missing ticket still returns MissingTicket error
        let claims = crate::ws::ticket::redeem_from_query(&state.tickets, None);
        assert!(
            claims.is_err(),
            "missing ticket should return MissingTicket error"
        );
        assert!(matches!(
            claims.unwrap_err(),
            crate::ws::ticket::WsAuthError::MissingTicket
        ));
    }

    /// Test 9: mkdir_creates
    #[tokio::test]
    async fn mkdir_creates() {
        let state = make_state("mkdir_creates");
        let resp = dispatch_fs_message(&state, r#"{"op":"mkdir","path":"newdir"}"#).await;
        assert!(ok(&resp));
        let mkdir_path = resp["mkdir"].as_str().unwrap();
        assert!(
            mkdir_path.ends_with("/newdir"),
            "mkdir path should end with /newdir, got {mkdir_path}"
        );
        let sandbox_root = &state.config.sandbox_root;
        assert!(
            sandbox_root.join("newdir").exists(),
            "newdir should exist on disk"
        );
    }

    /// Test 10: mkdir_creates_parents
    #[tokio::test]
    async fn mkdir_creates_parents() {
        let state = make_state("mkdir_parents");
        let resp = dispatch_fs_message(&state, r#"{"op":"mkdir","path":"a/b/c"}"#).await;
        assert!(ok(&resp));
        let sandbox_root = &state.config.sandbox_root;
        assert!(
            sandbox_root.join("a/b/c").exists(),
            "a/b/c should exist on disk"
        );
    }

    /// Test 11: rename_moves
    #[tokio::test]
    async fn rename_moves() {
        let state = make_state("rename_moves");
        let resp = dispatch_fs_message(
            &state,
            r#"{"op":"rename","path":"sub/file.txt","to":"sub/renamed.txt"}"#,
        )
        .await;
        assert!(ok(&resp));
        let renamed = resp["renamed"].as_str().unwrap();
        assert!(
            renamed.ends_with("/sub/renamed.txt"),
            "renamed path should end with /sub/renamed.txt, got {renamed}"
        );
        let sandbox_root = &state.config.sandbox_root;
        assert!(
            !sandbox_root.join("sub/file.txt").exists(),
            "file.txt should no longer exist"
        );
        assert!(
            sandbox_root.join("sub/renamed.txt").exists(),
            "renamed.txt should exist"
        );
        let content = std::fs::read_to_string(sandbox_root.join("sub/renamed.txt")).unwrap();
        assert!(content.contains("hello"));
    }

    /// Test 12: rename_escape_rejected
    #[tokio::test]
    async fn rename_escape_rejected() {
        let state = make_state("rename_escape");
        let resp = dispatch_fs_message(
            &state,
            r#"{"op":"rename","path":"sub/file.txt","to":"../../etc/passwd"}"#,
        )
        .await;
        assert!(!ok(&resp));
        let err = resp["error"].as_str().unwrap();
        assert!(
            err.contains("VNL-SBX-001"),
            "expected VNL-SBX-001 in error, got: {err}"
        );
    }

    /// Test 13: rename_missing_to_rejected
    #[tokio::test]
    async fn rename_missing_to_rejected() {
        let state = make_state("rename_missing_to");
        let resp = dispatch_fs_message(&state, r#"{"op":"rename","path":"sub/file.txt"}"#).await;
        assert!(!ok(&resp));
        assert_eq!(resp["error"].as_str().unwrap(), "missing to");
    }

    /// Test 14: root_op_returns_canonical_root
    #[tokio::test]
    async fn root_op_returns_canonical_root() {
        let state = make_state("root");
        let resp = dispatch_fs_message(&state, r#"{"op":"root"}"#).await;
        assert!(ok(&resp));
        let root = resp["root"].as_str().unwrap();
        assert!(
            root.ends_with("/sandbox"),
            "root should end with /sandbox, got {root}"
        );
        assert!(!root.is_empty(), "root must not be empty");
    }

    /// Test 15: root_op_needs_no_path
    #[tokio::test]
    async fn root_op_needs_no_path() {
        let state = make_state("root_nopath");
        let resp = dispatch_fs_message(&state, r#"{"op":"root"}"#).await;
        assert!(ok(&resp));
        assert!(
            resp["root"].as_str().is_some(),
            "root op without path should succeed, not return 'missing path'"
        );
    }
}
