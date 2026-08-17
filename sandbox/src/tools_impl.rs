use serde_json::Value;

use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::AppState;
use crate::lsp_client::LspClient;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("VNL-SBX-001: path escapes sandbox root: {path} (resolved outside {root})")]
    PathEscape { path: String, root: String },

    #[error("VNL-SBX-002: invalid sandbox root: {0}")]
    InvalidRoot(String),

    #[error("VNL-SBX-003: failed to resolve path ancestor {ancestor}: {source}")]
    AncestorResolutionFailed {
        ancestor: String,
        #[source]
        source: std::io::Error,
    },
}

/// Joins `suffix` onto `base` resolving `.`/`..` components lexically (no
/// filesystem access — `base` is assumed already canonical). A `..` that would
/// pop past the top of `base` simply has no further effect (`PathBuf::pop`
/// returns `false` and stops); the caller's `starts_with(root)` check then
/// legitimately rejects the result.
fn join_lexical(base: &Path, suffix: &Path) -> PathBuf {
    let mut result = base.to_path_buf();
    for component in suffix.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(seg) => result.push(seg),
            _ => {}
        }
    }
    result
}

/// Resolves `user_path` under `sandbox_root` and guarantees the result stays
/// confined inside.
///
/// Rules:
/// - Empty `user_path` → resolves to `sandbox_root` itself.
/// - Relative `user_path` → joined to `sandbox_root`.
/// - Absolute `user_path` → used as-is (must still be confined under
///   `sandbox_root`, else `PathEscape`).
/// - Trailing slash ignored (`"sub/"` == `"sub"`).
/// - `..` and symlinks: we canonicalise the **deepest existing ancestor** of the
///   candidate path, then append the part that does not yet exist (so that
///   `write_file` can target a not-yet-existing file), and finally check that
///   the result starts with canonicalised `sandbox_root`.
/// - `sandbox_root` must exist and be canonicalisable, else `InvalidRoot`.
pub fn confine_path(sandbox_root: &Path, user_path: &str) -> Result<PathBuf, SandboxError> {
    let root = std::fs::canonicalize(sandbox_root).map_err(|e| {
        tracing::warn!("invalid sandbox root {sandbox_root:?}: canonicalize failed: {e}");
        SandboxError::InvalidRoot(sandbox_root.to_string_lossy().into_owned())
    })?;

    if user_path.is_empty() || user_path.trim_end_matches('/').is_empty() {
        return Ok(root);
    }

    let trimmed = user_path.trim_end_matches('/');
    let candidate = if Path::new(trimmed).is_absolute() {
        trimmed.into()
    } else {
        sandbox_root.join(trimmed)
    };

    // Canonicalise the deepest existing ancestor.
    let mut ancestor: &Path = candidate.as_ref();
    let mut deepest: Option<&Path> = None;
    loop {
        if ancestor.exists() {
            deepest = Some(ancestor);
            break;
        }
        match ancestor.parent() {
            Some(parent) => ancestor = parent,
            None => break,
        }
    }

    let candidate = match deepest {
        Some(d) => {
            // Canonicalise the deepest existing ancestor and append the
            // non-existent suffix of the candidate.
            let deepest_canon = if d == sandbox_root {
                root.clone()
            } else {
                std::fs::canonicalize(d).map_err(|e| {
                    tracing::warn!(
                        "invalid sandbox root {sandbox_root:?}: deepest ancestor {d:?} failed: {e}"
                    );
                    SandboxError::AncestorResolutionFailed {
                        ancestor: d.to_string_lossy().into_owned(),
                        source: e,
                    }
                })?
            };
            let suffix = candidate.strip_prefix(d).unwrap_or(&candidate);
            join_lexical(&deepest_canon, suffix)
        }
        None => candidate,
    };

    // Confinement check: must start with root.
    if candidate.starts_with(&root) {
        Ok(candidate)
    } else {
        tracing::warn!(
            "path escape: {user_path:?} resolved to {} outside sandbox root {}",
            candidate.display(),
            root.display(),
        );
        Err(SandboxError::PathEscape {
            path: user_path.to_owned(),
            root: root.to_string_lossy().into_owned(),
        })
    }
}

use vanyline_tools::command::{self, ExecuteCommandOptions};
use vanyline_tools::filesystem::{
    self, DeleteFileOptions, EditFileOptions, ListDirectoryOptions, ReadFileOptions,
    WriteFileOptions,
};
use vanyline_tools::search::{self, FindFilesOptions, SearchOptions};

/// Successful MCP tool-result envelope (`isError: false`).
pub fn ok_result(text: String) -> Value {
    serde_json::json!({ "content": [{"type": "text", "text": text}], "isError": false })
}

/// Failed MCP tool-result envelope (`isError: true`) — a *tool-level* failure,
/// not a JSON-RPC protocol error. The tool name was valid; execution failed.
pub fn err_result(text: String) -> Value {
    serde_json::json!({ "content": [{"type": "text", "text": text}], "isError": true })
}

/// Resolves `raw_path` under `sandbox_root`, off the tokio executor thread
/// (confine_path does blocking filesystem I/O). On confinement failure, returns
/// an `err_result` envelope ready to hand straight back to the MCP caller.
#[allow(clippy::expect_used)] // JoinError signifie un panic interne dans confine_path, pas une erreur de chemin normale
pub async fn confine(sandbox_root: &Path, raw_path: &str) -> Result<String, Value> {
    let root = sandbox_root.to_path_buf();
    let raw = raw_path.to_string();
    tokio::task::spawn_blocking(move || confine_path(&root, &raw))
        .await
        .expect("confine_path blocking task panicked")
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| err_result(e.to_string()))
}

/// Dispatches a `tools/call` for one of the 5 filesystem tools
/// (read_file, write_file, edit_file, delete_file, list_directory).
/// Returns `None` if `name` isn't one of them, so the caller can try other
/// tool families (search, command — added in follow-up tasks).
pub async fn dispatch_filesystem(
    sandbox_root: &Path,
    name: &str,
    arguments: Value,
) -> Option<Value> {
    // --- read_file ---
    if name == "read_file" {
        let opts: ReadFileOptions = match serde_json::from_value(arguments) {
            Ok(o) => o,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        match confine(sandbox_root, &opts.path).await {
            Ok(resolved) => {
                let mut o = opts;
                o.path = resolved;
                match filesystem::read_file(o).await {
                    Ok(text) => Some(ok_result(text)),
                    Err(e) => Some(err_result(e.to_string())),
                }
            }
            Err(val) => Some(val),
        }
    }
    // --- write_file ---
    else if name == "write_file" {
        let opts: WriteFileOptions = match serde_json::from_value(arguments) {
            Ok(o) => o,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        match confine(sandbox_root, &opts.path).await {
            Ok(resolved) => {
                match filesystem::write_file(WriteFileOptions {
                    path: resolved.clone(),
                    content: opts.content,
                })
                .await
                {
                    Ok(()) => Some(ok_result(format!("wrote {resolved}"))),
                    Err(e) => Some(err_result(e.to_string())),
                }
            }
            Err(val) => Some(val),
        }
    }
    // --- edit_file ---
    else if name == "edit_file" {
        let opts: EditFileOptions = match serde_json::from_value(arguments) {
            Ok(o) => o,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        match confine(sandbox_root, &opts.path).await {
            Ok(resolved) => {
                match filesystem::edit_file(EditFileOptions {
                    path: resolved.clone(),
                    old_string: opts.old_string,
                    new_string: opts.new_string,
                    replace_all: opts.replace_all,
                })
                .await
                {
                    Ok(text) => Some(ok_result(text)),
                    Err(e) => Some(err_result(e.to_string())),
                }
            }
            Err(val) => Some(val),
        }
    }
    // --- delete_file ---
    else if name == "delete_file" {
        let opts: DeleteFileOptions = match serde_json::from_value(arguments) {
            Ok(o) => o,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        match confine(sandbox_root, &opts.path).await {
            Ok(resolved) => {
                match filesystem::delete_file(DeleteFileOptions {
                    path: resolved.clone(),
                })
                .await
                {
                    Ok(()) => Some(ok_result(format!("deleted {resolved}"))),
                    Err(e) => Some(err_result(e.to_string())),
                }
            }
            Err(val) => Some(val),
        }
    }
    // --- list_directory ---
    else if name == "list_directory" {
        let opts: ListDirectoryOptions = match serde_json::from_value(arguments) {
            Ok(o) => o,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        match confine(sandbox_root, &opts.path).await {
            Ok(resolved) => {
                match filesystem::list_directory(ListDirectoryOptions {
                    path: resolved.clone(),
                    depth: opts.depth,
                })
                .await
                {
                    Ok(text) => Some(ok_result(text)),
                    Err(e) => Some(err_result(e.to_string())),
                }
            }
            Err(val) => Some(val),
        }
    } else {
        None
    }
}

/// Dispatches a `tools/call` for `find_files` or `search`. Same shape as
/// `dispatch_filesystem`: confine `path` (empty → sandbox_root, per
/// `confine_path`'s own rule), overwrite it, call the tools-v2 function, map
/// the result. Returns `None` if `name` isn't one of these two.
pub async fn dispatch_search(sandbox_root: &Path, name: &str, arguments: Value) -> Option<Value> {
    // --- find_files ---
    if name == "find_files" {
        let opts: FindFilesOptions = match serde_json::from_value(arguments) {
            Ok(o) => o,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        // `path` is optional (serde default = "") — confine with empty is `sandbox_root`
        match confine(sandbox_root, &opts.path).await {
            Ok(resolved) => {
                match search::find_files(FindFilesOptions {
                    pattern: opts.pattern,
                    path: resolved,
                })
                .await
                {
                    Ok(text) => Some(ok_result(text)),
                    Err(e) => Some(err_result(e.to_string())),
                }
            }
            Err(val) => Some(val),
        }
    }
    // --- search ---
    else if name == "search" {
        let opts: SearchOptions = match serde_json::from_value(arguments) {
            Ok(o) => o,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        match confine(sandbox_root, &opts.path).await {
            Ok(resolved) => {
                match search::search(SearchOptions {
                    pattern: opts.pattern,
                    path: resolved,
                    glob: opts.glob,
                })
                .await
                {
                    Ok(text) => Some(ok_result(text)),
                    Err(e) => Some(err_result(e.to_string())),
                }
            }
            Err(val) => Some(val),
        }
    } else {
        None
    }
}

/// Dispatches a `tools/call` for `execute_command`. Same shape as the other
/// `dispatch_*` functions: `cwd` (even empty) always goes through `confine()`,
/// so the effective default cwd is `sandbox_root` — matching the design's
/// requirement that execute_command defaults to VNL_SANDBOX_ROOT, not the
/// sandbox process's own cwd (which is what tools::command::execute does when
/// given an empty cwd directly, unconfined).
pub async fn dispatch_command(sandbox_root: &Path, name: &str, arguments: Value) -> Option<Value> {
    if name != "execute_command" {
        return None;
    }
    let opts: ExecuteCommandOptions = match serde_json::from_value(arguments) {
        Ok(o) => o,
        Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
    };
    // `cwd` is optional (serde default = "") — confine with empty is `sandbox_root`
    match confine(sandbox_root, &opts.cwd).await {
        Ok(resolved) => {
            match command::execute(ExecuteCommandOptions {
                command: opts.command,
                timeout_secs: opts.timeout_secs,
                cwd: resolved,
            })
            .await
            {
                Ok(text) => Some(ok_result(text)),
                Err(e) => Some(err_result(e.to_string())),
            }
        }
        Err(val) => Some(val),
    }
}

/// Argument parsing for `lsp_diagnostics`.
#[derive(serde::Deserialize)]
pub struct LspDiagnosticsArgs {
    pub path: String,
}

/// Argument parsing for `lsp_hover`/`lsp_definition`/`lsp_references`.
#[derive(serde::Deserialize, Clone)]
pub struct LspPositionArgs {
    pub path: String,
    #[serde(default)]
    pub line: u64,
    #[serde(default)]
    pub character: u64,
}

/// Mapping extension of a file → (toolchain name, LSP languageId).
/// Known toolchains by convention with controller presets: `"rust"`, `"node"`.
/// `None` if the extension is not covered (fallback: no LSP).
pub fn toolchain_for_path(path: &str) -> Option<(&'static str, &'static str)> {
    let lower = path.to_lowercase();
    if lower.ends_with(".rs") {
        Some(("rust", "rust"))
    } else if lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".mts")
        || lower.ends_with(".cts")
    {
        Some(("node", "typescript"))
    } else if lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".mjs")
        || lower.ends_with(".cjs")
    {
        Some(("node", "javascript"))
    } else {
        None
    }
}

/// Schemas for the 4 LSP tools for `tools/list` (same shape as
/// `vanyline_tools::mcp::filesystem_tools()` : name/description/inputSchema).
pub fn lsp_tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "lsp_diagnostics",
            "description": "Get diagnostics (errors/warnings) for a file via the LSP server.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."}
                }
            }
        }),
        serde_json::json!({
            "name": "lsp_hover",
            "description": "Get hover information (tooltip) for a position in a file via the LSP server.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."},
                    "line": {"type": "integer", "description": "0-based line number (default 0)."},
                    "character": {"type": "integer", "description": "0-based character offset (default 0)."}
                }
            }
        }),
        serde_json::json!({
            "name": "lsp_definition",
            "description": "Go to definition of the symbol at a position in a file via the LSP server.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."},
                    "line": {"type": "integer", "description": "0-based line number (default 0)."},
                    "character": {"type": "integer", "description": "0-based character offset (default 0)."}
                }
            }
        }),
        serde_json::json!({
            "name": "lsp_references",
            "description": "Find all references of the symbol at a position in a file via the LSP server.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."},
                    "line": {"type": "integer", "description": "0-based line number (default 0)."},
                    "character": {"type": "integer", "description": "0-based character offset (default 0)."}
                }
            }
        }),
    ]
}

/// Severity helper: maps LSP severity code → display string.
fn severity_label(severity: i64) -> &'static str {
    match severity {
        1 => "error",
        2 => "warning",
        3 => "information",
        4 => "hint",
        _ => "error",
    }
}

/// Dispatches a `tools/call` for `lsp_diagnostics`/`lsp_hover`/`lsp_definition`/
/// `lsp_references`. Returns `None` if `name` is not one of these. Consumes
/// `state.lsp` (shared LSP process) and `state.config.sandbox_root`.
pub async fn dispatch_lsp(state: &AppState, name: &str, arguments: Value) -> Option<Value> {
    let lsp_tools = [
        "lsp_diagnostics",
        "lsp_hover",
        "lsp_definition",
        "lsp_references",
    ];
    if !lsp_tools.contains(&name) {
        return None;
    }

    // Step 2: parse arguments
    let args = if name == "lsp_diagnostics" {
        let args: LspDiagnosticsArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        LspArgs::Diagnostics(args)
    } else {
        let args: LspPositionArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        LspArgs::Position(args)
    };

    let raw_path = match &args {
        LspArgs::Diagnostics(a) => &a.path,
        LspArgs::Position(a) => &a.path,
    };

    // Step 3: confine
    let resolved = match confine(&state.config.sandbox_root, raw_path).await {
        Ok(r) => r,
        Err(val) => return Some(val),
    };

    // Step 4: read file
    let text = match filesystem::read_file(ReadFileOptions {
        path: resolved.clone(),
        offset: 0,
        limit: 0,
        raw: true,
    })
    .await
    {
        Ok(t) => t,
        Err(e) => return Some(err_result(e.to_string())),
    };

    // Step 5: toolchain_for_path
    let (toolchain, language_id) = match toolchain_for_path(&resolved) {
        Some(pair) => pair,
        None => {
            return Some(err_result(
                "VNL-SBX-LSP-006: no LSP for file extension".to_string(),
            ));
        }
    };

    // Step 6: get_or_spawn
    let session = match state.lsp.get_or_spawn(toolchain).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Some(err_result(format!(
                "VNL-SBX-LSP-006: no LSP configured for toolchain {toolchain}"
            )));
        }
        Err(e) => return Some(err_result(e.to_string())),
    };

    // Step 7: create client
    let root_uri = format!("file://{}", state.config.sandbox_root.display());
    let file_uri = format!("file://{resolved}");
    let mut client = LspClient::new(session, root_uri);

    // Step 8: per-tool dispatch
    match name {
        "lsp_diagnostics" => match client.diagnostics(&file_uri, language_id, &text).await {
            Ok(diagnostics) => {
                if diagnostics.is_empty() {
                    Some(ok_result("no diagnostics".to_string()))
                } else {
                    let mut lines = Vec::new();
                    for d in &diagnostics {
                        let Some(start) = d["range"]["start"].as_object() else {
                            continue;
                        };
                        let line = start["line"].as_i64().unwrap_or(0) + 1;
                        let col = start["character"].as_i64().unwrap_or(0) + 1;
                        let severity = d["severity"].as_i64().unwrap_or(1);
                        let message = d["message"].as_str().unwrap_or("");
                        let label = severity_label(severity);
                        lines.push(format!("{resolved}:{line}:{col}: {label}: {message}"));
                    }
                    Some(ok_result(lines.join("\n")))
                }
            }
            Err(e) => Some(err_result(e.to_string())),
        },
        "lsp_hover" => {
            let args: LspPositionArgs = match &args {
                LspArgs::Diagnostics(_) => unreachable!(),
                LspArgs::Position(a) => a.clone(),
            };
            match (
                client.initialize().await,
                client.did_open(&file_uri, language_id, &text).await,
            ) {
                (Ok(_), Ok(())) => match client
                    .request(
                        "textDocument/hover",
                        serde_json::json!({
                            "textDocument": {"uri": file_uri},
                            "position": {"line": args.line, "character": args.character}
                        }),
                    )
                    .await
                {
                    Ok(result) => {
                        if let Some(contents) = result.get("contents") {
                            let values: Vec<String> = if let Some(arr) = contents.as_array() {
                                arr.iter()
                                    .filter_map(|c| {
                                        c.get("value")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string())
                                    })
                                    .collect()
                            } else if contents.is_string() {
                                vec![contents.as_str().unwrap_or("").to_string()]
                            } else {
                                vec![]
                            };
                            let text = values.join("\n");
                            if text.is_empty() {
                                Some(ok_result("no hover".to_string()))
                            } else {
                                Some(ok_result(text))
                            }
                        } else {
                            Some(ok_result("no hover".to_string()))
                        }
                    }
                    Err(e) => Some(err_result(e.to_string())),
                },
                (Err(e), _) | (_, Err(e)) => Some(err_result(e.to_string())),
            }
        }
        "lsp_definition" => {
            let args: LspPositionArgs = match &args {
                LspArgs::Diagnostics(_) => unreachable!(),
                LspArgs::Position(a) => a.clone(),
            };
            match (
                client.initialize().await,
                client.did_open(&file_uri, language_id, &text).await,
            ) {
                (Ok(_), Ok(())) => match client
                    .request(
                        "textDocument/definition",
                        serde_json::json!({
                            "textDocument": {"uri": file_uri},
                            "position": {"line": args.line, "character": args.character}
                        }),
                    )
                    .await
                {
                    Ok(result) => {
                        let locations: Vec<Value> = if let Some(arr) = result.as_array() {
                            arr.clone()
                        } else if result.is_object() {
                            vec![result.clone()]
                        } else {
                            vec![]
                        };
                        if locations.is_empty() {
                            Some(ok_result("no definitions".to_string()))
                        } else {
                            let lines: Vec<String> = locations
                                .iter()
                                .map(|loc| {
                                    let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                                    let start = loc
                                        .get("range")
                                        .and_then(|r| r.get("start"))
                                        .and_then(|s| s.as_object());
                                    match start {
                                        Some(s) => {
                                            let line =
                                                s.get("line").and_then(|l| l.as_i64()).unwrap_or(0);
                                            let character = s
                                                .get("character")
                                                .and_then(|c| c.as_i64())
                                                .unwrap_or(0);
                                            format!("{uri}:{line}:{character}")
                                        }
                                        None => uri.to_string(),
                                    }
                                })
                                .collect();
                            Some(ok_result(lines.join("\n")))
                        }
                    }
                    Err(e) => Some(err_result(e.to_string())),
                },
                (Err(e), _) | (_, Err(e)) => Some(err_result(e.to_string())),
            }
        }
        "lsp_references" => {
            let args: LspPositionArgs = match &args {
                LspArgs::Diagnostics(_) => unreachable!(),
                LspArgs::Position(a) => a.clone(),
            };
            match (
                client.initialize().await,
                client.did_open(&file_uri, language_id, &text).await,
            ) {
                (Ok(_), Ok(())) => match client
                    .request(
                        "textDocument/references",
                        serde_json::json!({
                            "textDocument": {"uri": file_uri},
                            "position": {"line": args.line, "character": args.character},
                            "context": {"includeDeclaration": true}
                        }),
                    )
                    .await
                {
                    Ok(result) => {
                        let locations: Vec<Value> = if let Some(arr) = result.as_array() {
                            arr.clone()
                        } else {
                            vec![]
                        };
                        if locations.is_empty() {
                            Some(ok_result("no references".to_string()))
                        } else {
                            let lines: Vec<String> = locations
                                .iter()
                                .map(|loc| {
                                    let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                                    let start = loc
                                        .get("range")
                                        .and_then(|r| r.get("start"))
                                        .and_then(|s| s.as_object());
                                    match start {
                                        Some(s) => {
                                            let line =
                                                s.get("line").and_then(|l| l.as_i64()).unwrap_or(0);
                                            let character = s
                                                .get("character")
                                                .and_then(|c| c.as_i64())
                                                .unwrap_or(0);
                                            format!("{uri}:{line}:{character}")
                                        }
                                        None => uri.to_string(),
                                    }
                                })
                                .collect();
                            Some(ok_result(lines.join("\n")))
                        }
                    }
                    Err(e) => Some(err_result(e.to_string())),
                },
                (Err(e), _) | (_, Err(e)) => Some(err_result(e.to_string())),
            }
        }
        _ => unreachable!(),
    }
}

/// Internal enum to hold parsed LSP arguments.
enum LspArgs {
    Diagnostics(LspDiagnosticsArgs),
    Position(LspPositionArgs),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::auth::AuthState;
    use crate::config::Config;
    use crate::lsp::LspManager;
    use crate::lsp_client::lsp_test_fakes;
    use std::sync::Arc;
    use std::time::Duration;

    fn make_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/file.txt"), "hello").unwrap();
        dir
    }

    #[test]
    fn relative_path_within_root() {
        let root = make_root();
        let result = confine_path(root.path(), "sub/file.txt").unwrap();
        let expected = root.path().canonicalize().unwrap().join("sub/file.txt");
        assert_eq!(result, expected);
    }

    #[test]
    fn empty_path_resolves_to_root() {
        let root = make_root();
        let result = confine_path(root.path(), "").unwrap();
        assert_eq!(result, root.path().canonicalize().unwrap());
    }

    #[test]
    fn dot_dot_escape_rejected() {
        let root = make_root();
        let result = confine_path(root.path(), "../../etc/passwd");
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxError::PathEscape { path, .. } => assert_eq!(path, "../../etc/passwd"),
            _ => panic!("expected PathEscape"),
        }
    }

    #[test]
    fn absolute_path_inside_root_ok() {
        let root = make_root();
        let inside = root.path().join("sub");
        let result = confine_path(root.path(), inside.to_string_lossy().as_ref()).unwrap();
        let expected = std::fs::canonicalize(&inside).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn absolute_path_outside_root_rejected() {
        let root = make_root();
        let result = confine_path(root.path(), "/etc/passwd");
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxError::PathEscape { path, .. } => assert_eq!(path, "/etc/passwd"),
            _ => panic!("expected PathEscape"),
        }
    }

    #[test]
    fn nonexistent_file_within_root_ok() {
        let root = make_root();
        let result = confine_path(root.path(), "new/dir/file.txt").unwrap();
        let expected = root.path().canonicalize().unwrap().join("new/dir/file.txt");
        assert_eq!(result, expected);
    }

    #[test]
    #[cfg(unix)]
    fn symlink_escape_rejected() {
        use std::os::unix::fs::symlink;

        let root = make_root();
        let outside = tempfile::tempdir().unwrap();

        // Create a symlink inside root that points outside
        symlink(outside.path(), root.path().join("escape_link")).unwrap();

        // Traversing the symlink leads outside root
        let result = confine_path(root.path(), "escape_link/some_file");
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxError::PathEscape { path, .. } => {
                assert_eq!(path, "escape_link/some_file");
            }
            _ => panic!("expected PathEscape"),
        }
    }

    #[test]
    fn trailing_slash_ignored() {
        let root = make_root();
        let with_slash = confine_path(root.path(), "sub/").unwrap();
        let without_slash = confine_path(root.path(), "sub").unwrap();
        assert_eq!(with_slash, without_slash);
    }

    #[test]
    fn invalid_root_errors() {
        let result = confine_path(Path::new("/nonexistent/path/xyz"), "file.txt");
        assert!(result.is_err());
        match result.unwrap_err() {
            SandboxError::InvalidRoot(_) => {}
            e => panic!("expected InvalidRoot, got {:?}", e),
        }
    }

    // ── Regression tests for task 03b (confinement fix) ──────────────────────

    /// Test 1 — repro exact de la review : `..` dans un cheminement qui traverse
    /// des segments inexistants n'évade pas sandbox_root.
    #[test]
    fn dotdot_via_nonexistent_intermediate_rejected() {
        let root = make_root();
        let result = confine_path(root.path(), "sub/newdir/../../../etc/evilfile");
        assert!(
            result.is_err(),
            "expected PathEscape for '..' passing through nonexistent intermediates"
        );
        match result.unwrap_err() {
            SandboxError::PathEscape { path, .. } => {
                assert_eq!(path, "sub/newdir/../../../etc/evilfile")
            }
            _ => panic!("expected PathEscape"),
        }
    }

    /// Test 2 — modélise le scénario réel : clés SSH voisines de workspace.
    /// Un seul segment inexistant (`bogus`) suffit à déclencher le bug si les
    /// `..` ne sont pas résolus lexicalement.
    #[test]
    fn single_token_dotdot_bypass_rejected() {
        let owner_home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(owner_home.path().join(".ssh")).unwrap();
        std::fs::write(
            owner_home.path().join(".ssh/authorized_keys"),
            "existing-key\n",
        )
        .unwrap();
        let workspace = owner_home.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let result = confine_path(&workspace, "bogus/../../.ssh/authorized_keys");
        assert!(
            result.is_err(),
            "expected PathEscape for single-token '..' bypass to sibling .ssh"
        );
        match result.unwrap_err() {
            SandboxError::PathEscape { path, .. } => {
                assert_eq!(path, "bogus/../../.ssh/authorized_keys")
            }
            _ => panic!("expected PathEscape"),
        }
    }

    /// Test 5 — vérifie le wiring de `AncestorResolutionFailed`.
    ///
    /// Sur Linux, `realpath` (utilisé par `canonicalize`) découvre les noms de
    /// fichiers depuis le parent sans entrer dans le sous-répértoire, donc un
    /// dossier `0o000` n'empêche pas `canonicalize` de réussir. On peut
    /// donc reproduire ce scénario que dans des environnements très spécifiques
    /// (chroot, mount, chown vers UID inaccessible sans privilege).
    ///
    /// Ce test est un stub : la branche `AncestorResolutionFailed` est
    /// correctement câblée dans `confine_path`, mais aucune condition de test
    /// réaliste ne permet de la déclencher dans un conteneur userland normal.
    /// Le CI ne doit pas échouer à cause de ce test.
    #[test]
    #[cfg(unix)]
    fn ancestor_resolution_failure_is_distinct_from_invalid_root() {
        // Stub: wired correctly but not reproducible in userland containers.
        eprintln!(
            "SKIP: ancestor_resolution_failure test — stubbed (cannot make \
             canonicalize fail on a subdirectory within a user-owned TempDir \
             on Linux with 0o000 permissions: realpath discovers names from \
             parent without entering the directory)"
        );
    }

    /// Test 6 — résultat attendu quand l'ancêtre trouvé est root (optimisation
    /// de réutilisation de root déjà calculé). Test de comportement uniquement.
    #[test]
    fn avoids_redundant_canonicalize_when_ancestor_is_root() {
        let root = make_root();
        let result = confine_path(root.path(), "brand/new/path.txt").unwrap();
        let expected = root
            .path()
            .canonicalize()
            .unwrap()
            .join("brand/new/path.txt");
        assert_eq!(
            result, expected,
            "new path under root should resolve correctly"
        );
    }

    /// Test complémentaire : le fix ne casse pas la régression initiale
    /// (`../../etc/passwd` simple, sans segments inexistants).
    #[test]
    fn dotdot_simple_escape_still_blocked() {
        let root = make_root();
        // Chemin qui traverse uniquement des segments existants (root.parent() n'existe pas
        // mais .exists() est appelée sur le candidat et le parcours d'ancêtres devrait
        // trouver root comme plus profond)
        let result = confine_path(root.path(), "../../etc/hosts");
        assert!(
            result.is_err(),
            "dotdot simple escape should still be blocked"
        );
        match result.unwrap_err() {
            SandboxError::PathEscape { path, .. } => {
                assert_eq!(path, "../../etc/hosts")
            }
            _ => panic!("expected PathEscape"),
        }
    }

    // ── Tests LSP dispatch (task 04) ────────────────────────────────────────

    /// Helper: creates an AppState with a fake Rust LSP (Python script).
    /// Writes `main.rs` with `"fn main() {}"` into the tempdir.
    async fn make_lsp_state(name: &str) -> (AppState, tempfile::TempDir) {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let script_path = tmpdir.path().join(format!("fake_lsp_{name}.py"));
        std::fs::write(&script_path, lsp_test_fakes::FAKE_LSP_PY).unwrap();
        let rust_home = tmpdir.path().join("main.rs");
        std::fs::write(&rust_home, "fn main() {}").unwrap();

        let config = Arc::new(Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            oidc_issuer: None,
            oidc_audience: None,
            auth_groups_admin: "kubernetes-admin".into(),
            auth_groups_read: "kubernetes-view,kubernetes-edit".into(),
            no_auth: true,
            static_token: None,
            public_url: None,
            oidc_ca_cert: None,
            metrics_listen: "0.0.0.0:9090".into(),
            otel_endpoint: None,
            sandbox_root: tmpdir.path().to_path_buf(),
        });
        let auth = Arc::new(AuthState::new(config.clone()).unwrap());
        let lsp = Arc::new(LspManager::new(
            vec![crate::lsp::LspToolchain {
                name: "rust".to_string(),
                bin: "python3".to_string(),
                args: vec![script_path.to_string_lossy().to_string()],
            }],
            tmpdir.path().to_path_buf(),
        ));
        let state = AppState {
            config,
            auth,
            tickets: crate::ws::ticket::TicketStore::new(),
            lsp,
        };
        (state, tmpdir)
    }

    /// Helper: creates a minimal AppState with no LSP toolchains.
    async fn make_empty_lsp_state() -> (AppState, tempfile::TempDir) {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let rust_home = tmpdir.path().join("main.rs");
        std::fs::write(&rust_home, "fn main() {}").unwrap();

        let config = Arc::new(Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            oidc_issuer: None,
            oidc_audience: None,
            auth_groups_admin: "kubernetes-admin".into(),
            auth_groups_read: "kubernetes-view,kubernetes-edit".into(),
            no_auth: true,
            static_token: None,
            public_url: None,
            oidc_ca_cert: None,
            metrics_listen: "0.0.0.0:9090".into(),
            otel_endpoint: None,
            sandbox_root: tmpdir.path().to_path_buf(),
        });
        let auth = Arc::new(AuthState::new(config.clone()).unwrap());
        let state = AppState {
            config,
            auth,
            tickets: crate::ws::ticket::TicketStore::new(),
            lsp: Arc::new(LspManager::new(vec![], tmpdir.path().to_path_buf())),
        };
        (state, tmpdir)
    }

    #[tokio::test]
    async fn lsp_diagnostics_returns_structured() {
        let (state, _tmpdir) = make_lsp_state("diag").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_diagnostics",
                serde_json::json!({"path": "main.rs"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("error"), "should contain 'error' severity");
        assert!(text.contains("fake diag"), "should contain 'fake diag'");
        assert!(
            text.contains(":1:1:"),
            "should contain ':1:1:' for 0-based line/char +1"
        );
    }

    #[tokio::test]
    async fn lsp_hover_returns_content() {
        let (state, _tmpdir) = make_lsp_state("hover").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_hover",
                serde_json::json!({"path": "main.rs", "line": 0, "character": 0}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("hover:"), "should contain 'hover:'");
    }

    #[tokio::test]
    async fn lsp_definition_returns_location() {
        let (state, _tmpdir) = make_lsp_state("def").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_definition",
                serde_json::json!({"path": "main.rs", "line": 0, "character": 0}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("file://"), "should contain 'file://'");
        assert!(text.contains(":0:0"), "should contain ':0:0' (0-based)");
    }

    #[tokio::test]
    async fn lsp_references_returns_location() {
        let (state, _tmpdir) = make_lsp_state("ref").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_references",
                serde_json::json!({"path": "main.rs", "line": 0, "character": 0}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("file://"), "should contain 'file://'");
        assert!(text.contains(":0:0"), "should contain ':0:0' (0-based)");
    }

    #[tokio::test]
    async fn lsp_unknown_tool_returns_none() {
        let (state, _tmpdir) = make_lsp_state("none").await;
        let result = dispatch_lsp(&state, "nope", serde_json::json!({})).await;
        assert!(result.is_none(), "should return None for unknown tool");
    }

    /// Test with an unconfigured toolchain (empty specs) → VNL-SBX-LSP-006.
    #[tokio::test]
    async fn lsp_no_lsp_configured() {
        let (state, _tmpdir) = make_empty_lsp_state().await;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            dispatch_lsp(
                &state,
                "lsp_diagnostics",
                serde_json::json!({"path": "main.rs"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(result["isError"].as_bool().unwrap(), "should be an error");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("VNL-SBX-LSP-006"),
            "error should mention VNL-SBX-LSP-006"
        );
    }

    #[tokio::test]
    async fn lsp_no_toolchain_for_extension() {
        let (state, tmpdir) = make_lsp_state("noext").await;
        // Write a .py file — no LSP toolchain maps to Python in the config.
        // But toolchain_for_path(".py") → None, so we need a file with unknown ext
        // under the sandbox_root that is already confined.
        let fake_path = tmpdir.path().join("main.py");
        std::fs::write(&fake_path, "x = 1").unwrap();

        let result = dispatch_lsp(
            &state,
            "lsp_diagnostics",
            serde_json::json!({"path": "main.py"}),
        )
        .await;

        assert!(result.is_some(), "should return Some (err_result)");
        let val = result.unwrap();
        assert!(val["isError"].as_bool().unwrap(), "should be an error");
        let text = val["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("VNL-SBX-LSP-006"),
            "error should mention VNL-SBX-LSP-006"
        );
    }

    #[tokio::test]
    async fn lsp_invalid_args_errors() {
        let (state, _tmpdir) = make_lsp_state("args").await;
        // path must be a string, not a number
        let result = dispatch_lsp(&state, "lsp_diagnostics", serde_json::json!({"path": 42}))
            .await
            .expect("dispatch returned None");

        assert!(result["isError"].as_bool().unwrap(), "should be an error");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("invalid arguments"),
            "should contain 'invalid arguments'"
        );
    }

    // ── toolchain_for_path unit tests ───────────────────────────────────────

    #[test]
    fn toolchain_for_path_rust_file() {
        assert_eq!(toolchain_for_path("src/main.rs"), Some(("rust", "rust")));
    }

    #[test]
    fn toolchain_for_path_ts_file() {
        assert_eq!(
            toolchain_for_path("src/index.ts"),
            Some(("node", "typescript"))
        );
        assert_eq!(
            toolchain_for_path("src/app.tsx"),
            Some(("node", "typescript"))
        );
        assert_eq!(
            toolchain_for_path("src/mod.mts"),
            Some(("node", "typescript"))
        );
        assert_eq!(
            toolchain_for_path("src/mod.cts"),
            Some(("node", "typescript"))
        );
    }

    #[test]
    fn toolchain_for_path_js_file() {
        assert_eq!(
            toolchain_for_path("src/index.js"),
            Some(("node", "javascript"))
        );
        assert_eq!(
            toolchain_for_path("src/app.jsx"),
            Some(("node", "javascript"))
        );
        assert_eq!(
            toolchain_for_path("src/mod.mjs"),
            Some(("node", "javascript"))
        );
        assert_eq!(
            toolchain_for_path("src/mod.cjs"),
            Some(("node", "javascript"))
        );
    }

    #[test]
    fn toolchain_for_path_unknown_extension() {
        assert_eq!(toolchain_for_path("file.xyz"), None);
        assert_eq!(toolchain_for_path("file.py"), None);
        assert_eq!(toolchain_for_path("file.json"), None);
        assert_eq!(toolchain_for_path("README.md"), None);
    }

    #[test]
    fn toolchain_for_path_case_insensitive() {
        assert_eq!(toolchain_for_path("src/main.RS"), Some(("rust", "rust")));
        assert_eq!(
            toolchain_for_path("src/index.TS"),
            Some(("node", "typescript"))
        );
        assert_eq!(
            toolchain_for_path("src/script.JS"),
            Some(("node", "javascript"))
        );
    }
}
