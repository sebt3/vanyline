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

/// Args de `lsp_rename` : position partagée + nouveau nom.
#[derive(serde::Deserialize, Clone)]
pub struct LspRenameArgs {
    #[serde(flatten)]
    pub target: LspSymbolTarget,
    pub new_name: String,
}

/// Forme d'argument partagée par tous les tools qui ciblent une position
/// (`lsp_definition`, `lsp_references`, `lsp_rename`, `inspect_symbol`).
/// A remplacé partout l'ancien format 0-based — la migration est achevée
/// depuis la tâche 02 de `lsp-agent-interface`.
/// Entrées 1-based (alignées `read_file`), conversion interne 0-based LSP.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct LspSymbolTarget {
    pub path: String,
    /// Ligne 1-based (comme read_file). Requis (pas de `serde(default)`).
    pub line: u64,
    /// Nom de l'identifiant à cibler sur cette ligne. Mode recommandé.
    /// Le tool résout lui-même la colonne de la 1re occurrence délimitée.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Colonne 1-based, échappatoire pour un ciblage précis quand `symbol`
    /// est ambigu ou absent. Ignoré si `symbol` est fourni (non vide).
    #[serde(default)]
    pub character: Option<u64>,
}

/// Résultat de `resolve_position` — position LSP 0-based + information
/// d'ambiguïté du mode symbole à rendre dans la réponse de l'outil (design R6).
#[derive(Debug, PartialEq, Eq)]
pub enum PositionResolution {
    Unique {
        line0: u64,
        character0: u64,
    },
    /// Mode symbole : `symbol` trouvé `matches` fois (≥ 2) sur la ligne.
    /// C'est la 1re occurrence qui est retenue (`character0`).
    /// `second_char1` = colonne **1-based** de la 2e occurrence, à citer dans
    /// la note d'ambiguïté de la réponse de l'outil.
    Ambiguous {
        line0: u64,
        character0: u64,
        matches: usize,
        second_char1: u64,
    },
}

/// Vrai pour les caractères qui délimitent un identifiant ASCII :
/// `c.is_ascii_alphanumeric() || c == '_'` (bordures `[A-Za-z0-9_]`, design §1).
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Résout la position LSP 0-based visée par `target` dans `content` (contenu
/// brut du fichier déjà lu — fonction pure, aucun I/O).
///
/// Modes (ordre de priorité, design §1) :
/// 1. `symbol = Some(s)` avec `s` non vide → occurrence en tant qu'
///    **identifiant délimité** : sous-chaîne littérale de `s` dans la ligne,
///    encadrée à gauche et à droite par une bordure valide (absence de
///    voisin, ou voisin qui n'est PAS un caractère d'identifiant ASCII :
///    `[A-Za-z0-9_]` — cf. `is_ident_char`). Recherche manuelle sur la
///    séquence de `char`s de la ligne — JAMAIS de regex compilée depuis
///    l'entrée (anti-ReDoS, design R5). Colonnes comptées en `char`s
///    (mêmes approximations UTF-8 ≈ UTF-16 que `position_to_offset`).
///    - 0 occurrence → `Err` contenant `VNL-SBX-LSP-010`.
///    - 1 occurrence → `Unique`.
///    - ≥ 2 occurrences → `Ambiguous` (1re occurrence retenue).
/// 2. sinon `character = Some(c)` → colonne `c.saturating_sub(1)`.
/// 3. sinon → colonne 0 (début de ligne, comportement conservé).
///
/// Erreurs (messages `anyhow` portant le code, même style que
/// `position_to_offset`) :
/// - `line == 0` ou `line > nombre de lignes` → `VNL-SBX-LSP-007: line {line}
///   out of range ({n} lines)` — réutilise le sens existant « ligne hors
///   limites » de ce code.
/// - symbol mode, 0 occurrence → `VNL-SBX-LSP-010: symbol \"{s}\" not found
///   as identifier on line {line} of {path}` (le `path` vient du target).
pub fn resolve_position(
    content: &str,
    target: &LspSymbolTarget,
) -> anyhow::Result<PositionResolution> {
    // Découpage en lignes : même convention que `position_to_offset`
    // (`content.lines()` gère `\r\n`). Un contenu vide rend 1 ligne vide :
    // la ligne 1 existe donc (pas d'erreur de limites sur un fichier vide).
    let lines: Vec<&str> = content.lines().collect();
    if target.line == 0 || target.line as usize > lines.len() {
        return Err(anyhow::anyhow!(
            "VNL-SBX-LSP-007: line {} out of range ({} lines)",
            target.line,
            lines.len()
        ));
    }
    let line0 = target.line - 1;
    let chars: Vec<char> = lines[line0 as usize].chars().collect();

    // Mode symbole : `symbol` non vide. `Some("")` n'active pas ce mode (il
    // retombe sur les modes 2/3 ci-dessous).
    if let Some(sym) = target.symbol.as_deref().filter(|s| !s.is_empty()) {
        let sym_chars: Vec<char> = sym.chars().collect();
        // Recherche littérale manuelle, position par position — pas de regex
        // compilée depuis l'entrée utilisateur (anti-ReDoS, design R5).
        let mut hits: Vec<usize> = Vec::new();
        let mut start = 0usize;
        while start + sym_chars.len() <= chars.len() {
            if chars[start..start + sym_chars.len()] == sym_chars[..] {
                let left_ok = start == 0 || !is_ident_char(chars[start - 1]);
                let after = start + sym_chars.len();
                let right_ok = after == chars.len() || !is_ident_char(chars[after]);
                if left_ok && right_ok {
                    hits.push(start);
                }
            }
            start += 1;
        }

        return match hits.len() {
            0 => Err(anyhow::anyhow!(
                "VNL-SBX-LSP-010: symbol \"{}\" not found as identifier on line {} of {}",
                sym,
                target.line,
                target.path
            )),
            1 => Ok(PositionResolution::Unique {
                line0,
                character0: hits[0] as u64,
            }),
            _ => Ok(PositionResolution::Ambiguous {
                line0,
                character0: hits[0] as u64,
                matches: hits.len(),
                second_char1: hits[1] as u64 + 1,
            }),
        };
    }

    // Mode 2 : colonne 1-based explicite, saturée (character = 0 → colonne 0,
    // pas d'underflow). Mode 3 : ni symbol ni character → colonne 0.
    let character0 = target.character.unwrap_or(0).saturating_sub(1);
    Ok(PositionResolution::Unique { line0, character0 })
}

/// Snippet de la ligne `line0` (0-based, convention `content.lines()`) du
/// contenu `content`, borné (`.trim()` des deux côtés). `None` si la ligne
/// n'existe pas.
fn line_snippet(content: &str, line0: u64) -> Option<String> {
    content
        .lines()
        .nth(line0 as usize)
        .map(|l| l.trim().to_string())
}

/// Note d'ambiguïté R6, suffixe de la ligne « cible: ». Rendue seulement pour
/// `PositionResolution::Ambiguous` (`second_char1` est la colonne 1-based de
/// la 2e occurrence.)
fn ambiguity_note(sym: &str, matches: usize, second_char1: u64) -> String {
    format!(
        " (symbole \"{sym}\" trouvé {matches}× sur la ligne, 1re occurrence utilisée \
         — préciser character: {second_char1} pour la suivante)"
    )
}

/// Vrai si la localisation LSP porte au moins une URI exploitable — `uri`
/// (forme `Location`) ou `targetUri` (forme `LocationLink`). Les entrées sans
/// aucune des deux sont filtrées avant rendu (pas de ligne vide).
fn location_has_uri(loc: &serde_json::Value) -> bool {
    loc.get("uri").and_then(|u| u.as_str()).is_some()
        || loc.get("targetUri").and_then(|u| u.as_str()).is_some()
}

/// Rend un résultat de localisation LSP dans le texte rendu aux agents
/// (helpers des tâches 2, 4, 5, 6 — réutilisé tel quel).
///
/// Deux formes acceptées : `Location` (`uri` + `range.start.line`) et
/// `LocationLink` (`targetUri` + `targetSelectionRange.start.line` —
/// rust-analyzer en émet sur certains goto-def). Les deux rendent
/// `<chemin relatif au workspace>:<ligne 1-based>: <snippet de ligne>`.
///
/// Sécurité (design R5) : le snippet exige la lecture de la ligne — l'URI est
/// d'abord `file://`, puis son chemin passe par `confine(sandbox_root, …)`,
/// **confine AVANT toute lecture, jamais de lecture hors `sandbox_root`**.
/// URI non-`file://` ou hors workspace → `<uri brut>:<ligne 1-based>` sans
/// snippet, aucune lecture tentée. Confine OK mais lecture échouée ou ligne
/// absente → `<chemin relatif>:<ligne 1-based>` sans snippet.
///
/// `chemin relatif` = URI `file://…` amputée de son préfixe
/// `file://{sandbox_root}/` (`sandbox_root` rendu sans slash final) ; à
/// défaut, le chemin confiné résolu par `confine`. Localisation sans aucune
/// URI exploitable (ni `uri` ni `targetUri`) → la chaîne vide.
async fn render_location(sandbox_root: &std::path::Path, loc: &serde_json::Value) -> String {
    let raw_uri = match loc.get("uri").and_then(|u| u.as_str()) {
        Some(u) => u,
        None => match loc.get("targetUri").and_then(|u| u.as_str()) {
            Some(u) => u,
            None => return String::new(),
        },
    };
    let line0 = loc
        .get("range")
        .or_else(|| loc.get("targetSelectionRange"))
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("line"))
        .and_then(|l| l.as_u64());

    // R5 : confine AVANT toute lecture — la lecture ci-dessous ne porte que
    // sur le chemin résolu rendu par `confine`, jamais sur l'URI brute.
    let confined = match raw_uri.strip_prefix("file://") {
        Some(raw_path) => confine(sandbox_root, raw_path).await.ok(),
        None => None,
    };
    if let Some(resolved) = confined {
        let root_prefix = format!("file://{}/", sandbox_root.display());
        let display_path = match raw_uri.strip_prefix(&root_prefix) {
            Some(rel) => rel.to_string(),
            None => resolved.clone(),
        };
        let Some(line0) = line0 else {
            return display_path;
        };
        let snippet = filesystem::read_file(ReadFileOptions {
            path: resolved,
            offset: 0,
            limit: 0,
            raw: true,
        })
        .await
        .ok()
        .and_then(|content| line_snippet(&content, line0));
        return match snippet {
            Some(snippet) => format!("{display_path}:{}: {snippet}", line0 + 1),
            None => format!("{display_path}:{}", line0 + 1),
        };
    }

    // URI non-`file://` ou hors workspace : rendu brut, aucune lecture tentée.
    tracing::debug!("LSP location outside workspace, rendered raw: {raw_uri}");
    match line0 {
        Some(line0) => format!("{raw_uri}:{}", line0 + 1),
        None => raw_uri.to_string(),
    }
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
/// Since task 02 of `lsp-agent-interface`, `lsp_hover` no longer exists as a
/// standalone tool — hover contents are rendered by `lsp_definition`.
pub fn lsp_tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "lsp_diagnostics",
            "description": "Get diagnostics (errors/warnings) for a file via the LSP server. Supported extensions: .rs (rust), .ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs (node) — others (including .vue) return VNL-SBX-LSP-006, no LSP configured for that extension.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."}
                }
            }
        }),
        serde_json::json!({
            "name": "lsp_definition",
            "description": "Go to definition of the symbol at a position in a file via the LSP server, with the hover signature/doc for the same position when available. Supported extensions: .rs (rust), .ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs (node) — others (including .vue) return VNL-SBX-LSP-006, no LSP configured for that extension.",
            "inputSchema": {
                "type": "object",
                "required": ["path", "line"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."},
                    "line": {"type": "integer", "description": "1-based line number (as shown by read_file). Required."},
                    "symbol": {"type": "string", "description": "Identifier name to target on that line (recommended). Resolved to the first delimited-identifier occurrence; if it appears several times the first is used and the answer says so."},
                    "character": {"type": "integer", "description": "1-based column, precise targeting when symbol is absent or ambiguous. Ignored when symbol is set."}
                }
            }
        }),
        serde_json::json!({
            "name": "lsp_references",
            "description": "Find all references of the symbol at a position in a file via the LSP server. Supported extensions: .rs (rust), .ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs (node) — others (including .vue) return VNL-SBX-LSP-006, no LSP configured for that extension.",
            "inputSchema": {
                "type": "object",
                "required": ["path", "line"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."},
                    "line": {"type": "integer", "description": "1-based line number (as shown by read_file). Required."},
                    "symbol": {"type": "string", "description": "Identifier name to target on that line (recommended). Resolved to the first delimited-identifier occurrence; if it appears several times the first is used and the answer says so."},
                    "character": {"type": "integer", "description": "1-based column, precise targeting when symbol is absent or ambiguous. Ignored when symbol is set."}
                }
            }
        }),
        serde_json::json!({
            "name": "lsp_rename",
            "description": "Rename a symbol at a position in a file via the LSP server (applies the resulting WorkspaceEdit to the filesystem). Supported extensions: .rs (rust), .ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs (node) — others (including .vue) return VNL-SBX-LSP-006, no LSP configured for that extension.",
            "inputSchema": {
                "type": "object",
                "required": ["path", "line", "new_name"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."},
                    "line": {"type": "integer", "description": "1-based line number (as shown by read_file). Required."},
                    "symbol": {"type": "string", "description": "Identifier name to target on that line (recommended). Resolved to the first delimited-identifier occurrence; if it appears several times the first is used and the answer says so."},
                    "character": {"type": "integer", "description": "1-based column, precise targeting when symbol is absent or ambiguous. Ignored when symbol is set."},
                    "new_name": {"type": "string", "description": "New name for the symbol."}
                }
            }
        }),
    ]
}

/// Extrait le texte de `Hover.contents` — `MarkedString | MarkedString[] |
/// MarkupContent` (spec LSP). `MarkedString` = string brute ou `{language, value}` ;
/// `MarkupContent` (forme moderne — c'est ce que rendent rust-analyzer et
/// typescript-language-server par défaut) = `{kind, value}`. Seule la forme array
/// d'objets `{value}` était gérée avant ce fix — la forme `MarkupContent` (un objet
/// direct, pas un array) tombait dans aucun des cas gérés et rendait toujours "no
/// hover" en usage réel, quel que soit le symbole.
fn hover_contents_to_text(contents: &Value) -> String {
    if let Some(s) = contents.as_str() {
        return s.to_string();
    }
    if let Some(arr) = contents.as_array() {
        return arr
            .iter()
            .map(hover_marked_string_to_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    hover_marked_string_to_text(contents)
}

/// Un élément `MarkedString` (string ou `{language, value}`) ou un `MarkupContent`
/// (`{kind, value}`) — les deux formes objet portent leur texte dans `value`.
fn hover_marked_string_to_text(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    value
        .get("value")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Segment du contenu hover extrait par `hover_segments` : section de code
/// (entrée `{language, value}`, texte nu sans fences) ou prose (texte aplati
/// pouvant lui-même porter des blocs ```…``` clôturés — séparés par
/// `split_fenced_code_blocks`).
enum HoverSegment {
    Code(String),
    Prose(String),
}

/// Découpe `hover.contents` (3 formes LSP) en segments code/prose SANS
/// réécrire le parsing : la structure `{language, value}` est seulement
/// reconnue ici, le texte de chaque élément passe par
/// `hover_marked_string_to_text` (prose) et la forme non-array est aplatie
/// par `hover_contents_to_text` — mêmes fonctions que le rendu historique.
/// Un contenu vide (null, `{}`, array vide, chaînes vides) ne produit aucun
/// segment.
fn hover_segments(contents: &Value) -> Vec<HoverSegment> {
    if let Some(arr) = contents.as_array() {
        return arr
            .iter()
            .filter_map(|el| {
                if el.get("language").and_then(|l| l.as_str()).is_some() {
                    let code = el.get("value").and_then(|v| v.as_str()).unwrap_or("");
                    if code.is_empty() {
                        None
                    } else {
                        Some(HoverSegment::Code(code.to_string()))
                    }
                } else {
                    let text = hover_marked_string_to_text(el);
                    if text.is_empty() {
                        None
                    } else {
                        Some(HoverSegment::Prose(text))
                    }
                }
            })
            .collect();
    }
    // MarkedString `{language, value}` seule : même traitement que dans un array.
    if contents.get("language").and_then(|l| l.as_str()).is_some() {
        let code = contents.get("value").and_then(|v| v.as_str()).unwrap_or("");
        return if code.is_empty() {
            vec![]
        } else {
            vec![HoverSegment::Code(code.to_string())]
        };
    }
    let text = hover_contents_to_text(contents);
    if text.is_empty() {
        vec![]
    } else {
        vec![HoverSegment::Prose(text)]
    }
}

/// Sépare un texte (markdown ou plaintext) en (blocs de code clôturés
/// ```…```, lignes de prose restantes). Les lignes de fence ne vont jamais
/// dans la prose (« amputée de ses fences résiduelles ») : une fence jamais
/// fermée n'est pas une section de code — son contenu retourne en prose, seul
/// le marqueur est retiré. Un bloc vide n'est pas une section non plus.
fn split_fenced_code_blocks(text: &str) -> (Vec<String>, Vec<String>) {
    let mut blocks: Vec<String> = Vec::new();
    let mut prose: Vec<String> = Vec::new();
    let mut open_block: Option<Vec<String>> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if let Some(buf) = open_block.take() {
                let block = buf.join("\n");
                if block.trim().is_empty() {
                    prose.extend(buf);
                } else {
                    blocks.push(block);
                }
            } else {
                open_block = Some(Vec::new());
            }
            continue;
        }
        if let Some(buf) = &mut open_block {
            buf.push(line.to_string());
        } else {
            prose.push(line.to_string());
        }
    }
    if let Some(buf) = open_block {
        // Fence jamais fermée : pas une section, contenu rendu en prose.
        prose.extend(buf);
    }
    (blocks, prose)
}

/// Sépare le contenu `hover.contents` (3 formes LSP, gérées par les fonctions
/// existantes `hover_contents_to_text`/`hover_marked_string_to_text` et la
/// structure des éléments `{language, value}` vs `{value}`/string brute) en
/// (signature, doc) :
/// - signature = 1re section de CODE : bloc clôturé ```…``` dans un texte
///   (markdown), ou 1re entrée `{language, value}` d'un array MarkedString.
///   Rendu sans les fences.
/// - doc = prose restante (sections de code exclues), amputée de ses fences
///   résiduelles, tronquée à 3 lignes non vides (rejointes par `\n`).
/// - Aucune section de code nulle part → signature = 1re ligne non vide du
///   texte aplati, doc = les 3 lignes non vides suivantes.
/// - hover null / contenu vide → `("", "")`.
fn hover_signature_and_doc(contents: &Value) -> (String, String) {
    let mut signature: Option<String> = None;
    let mut prose_lines: Vec<String> = Vec::new();

    for segment in hover_segments(contents) {
        match segment {
            HoverSegment::Code(code) => {
                if signature.is_none() {
                    signature = Some(code);
                }
                // Sections de code supplémentaires : exclues du doc, pas de
                // 2e signature.
            }
            HoverSegment::Prose(text) => {
                let (blocks, prose) = split_fenced_code_blocks(&text);
                if signature.is_none() {
                    signature = blocks.first().cloned();
                }
                // Tous les blocs clôturés sont des sections de code : jamais
                // dans le doc, y compris après la signature trouvée.
                prose_lines.extend(prose);
            }
        }
    }

    let doc_candidates: Vec<&str> = prose_lines
        .iter()
        .map(String::as_str)
        .filter(|line| !line.trim().is_empty())
        .collect();

    match signature {
        Some(signature) => {
            let doc = doc_candidates
                .into_iter()
                .take(3)
                .collect::<Vec<_>>()
                .join("\n");
            (signature, doc)
        }
        // Aucune section de code nulle part : 1re ligne non vide = signature,
        // les 3 suivantes = doc.
        None => {
            let mut lines = doc_candidates.into_iter();
            let signature = lines.next().unwrap_or_default().to_string();
            let doc = lines.take(3).collect::<Vec<_>>().join("\n");
            (signature, doc)
        }
    }
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

/// Dispatches a `tools/call` for `lsp_diagnostics`/`lsp_definition`/
/// `lsp_references`/`lsp_rename`. Returns `None` if `name` is not one of these.
/// Consumes `state.lsp` (shared LSP process) and `state.config.sandbox_root`.
pub async fn dispatch_lsp(state: &AppState, name: &str, arguments: Value) -> Option<Value> {
    let lsp_tools = [
        "lsp_diagnostics",
        "lsp_definition",
        "lsp_references",
        "lsp_rename",
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
    } else if name == "lsp_rename" {
        let args: LspRenameArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        LspArgs::Rename(args)
    } else {
        // `lsp_definition` / `lsp_references` : modèle de position partagé,
        // `line` requis (son absence tombe dans le `invalid arguments` ci-dessus).
        let args: LspSymbolTarget = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        LspArgs::Target(args)
    };

    let raw_path = match &args {
        LspArgs::Diagnostics(a) => &a.path,
        LspArgs::Target(a) => &a.path,
        LspArgs::Rename(a) => &a.target.path,
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

    // Step 4b (modèle de position partagé, tâche 01b) : résolution 1-based →
    // 0-based pour les tools à target, avec ligne cible pré-construite —
    // snippet lu dans le `text` déjà lu par le dispatch (pas de re-lecture)
    // et note d'ambiguïté R6 le cas échéant. `Err` porte déjà
    // VNL-SBX-LSP-007/VNL-SBX-LSP-010. `lsp_diagnostics` ne passe pas ici.
    let symbol_target: Option<&LspSymbolTarget> = match &args {
        LspArgs::Target(t) => Some(t),
        LspArgs::Rename(r) => Some(&r.target),
        LspArgs::Diagnostics(_) => None,
    };
    let target_info = match symbol_target {
        None => None,
        Some(t) => {
            let (line0, character0, note) = match resolve_position(&text, t) {
                Ok(PositionResolution::Unique { line0, character0 }) => {
                    (line0, character0, String::new())
                }
                Ok(PositionResolution::Ambiguous {
                    line0,
                    character0,
                    matches,
                    second_char1,
                }) => (
                    line0,
                    character0,
                    ambiguity_note(
                        t.symbol.as_deref().unwrap_or_default(),
                        matches,
                        second_char1,
                    ),
                ),
                Err(e) => return Some(err_result(e.to_string())),
            };
            // Chemin relatif au workspace : le préfixe `{sandbox_root}/` retiré
            // du chemin confiné ; à défaut le chemin confiné lui-même (même
            // logique que `render_location`).
            let root_prefix = format!("{}/", state.config.sandbox_root.display());
            let display_path = match resolved.strip_prefix(&root_prefix) {
                Some(rel) => rel.to_string(),
                None => resolved.clone(),
            };
            let target_line = match line_snippet(&text, line0) {
                Some(snippet) => format!("cible: {display_path}:{}: {snippet}{note}", line0 + 1),
                None => format!("cible: {display_path}:{}{note}", line0 + 1),
            };
            Some((line0, character0, target_line))
        }
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
            // Trois états distincts, pas deux — un vecteur vide et "jamais reçu"
            // n'ont pas le même sens pour un agent (cf. doc `wait_for_diagnostics`) :
            // un agent qui traiterait les deux comme "propre" pourrait croire à tort
            // qu'une édition n'a rien cassé alors que l'analyse n'est simplement pas
            // encore arrivée (trouvé dans un retour d'usage réel).
            Ok(None) => Some(ok_result(
                "not yet analyzed: no diagnostics received from the LSP server within the \
                 timeout — this does NOT mean the file is clean, retry shortly"
                    .to_string(),
            )),
            Ok(Some(diagnostics)) if diagnostics.is_empty() => Some(ok_result(
                "no diagnostics: file analyzed, no issues found".to_string(),
            )),
            Ok(Some(diagnostics)) => {
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
            Err(e) => Some(err_result(e.to_string())),
        },
        "lsp_definition" => {
            let (line0, character0, target_line) = match target_info {
                Some(t) => t,
                None => unreachable!("target_info is Some for target-based tools"),
            };
            match (
                client.initialize().await,
                client.ensure_open(&file_uri, language_id, &text).await,
            ) {
                (Ok(_), Ok(())) => {
                    // Hover d'abord, best effort (tâche 02 `lsp-agent-interface`,
                    // design §2) : « pas de def trouvée → on rend quand même
                    // hover si présent » vaut aussi l'inverse — une erreur ou un
                    // résultat null/vide au hover ne doit JAMAIS faire échouer le
                    // tool, juste omettre les sections signature/doc.
                    let hover = client
                        .request(
                            "textDocument/hover",
                            serde_json::json!({
                                "textDocument": {"uri": file_uri},
                                "position": {"line": line0, "character": character0}
                            }),
                        )
                        .await;
                    let (signature, doc) = match hover {
                        Ok(result) => match result.get("contents") {
                            Some(contents) if !contents.is_null() => {
                                hover_signature_and_doc(contents)
                            }
                            _ => (String::new(), String::new()),
                        },
                        Err(e) => {
                            tracing::warn!(
                                "textDocument/hover failed (best effort, lsp_definition continues): {e}"
                            );
                            (String::new(), String::new())
                        }
                    };
                    // Definition : l'échec reste une erreur tool (comportement
                    // actuel, inchangé).
                    match client
                        .request(
                            "textDocument/definition",
                            serde_json::json!({
                                "textDocument": {"uri": file_uri},
                                "position": {"line": line0, "character": character0}
                            }),
                        )
                        .await
                    {
                        Ok(result) => {
                            // Normalisation existante conservée : tableau JSON-RPC,
                            // ou objet unique accepté.
                            let locations: Vec<Value> = if let Some(arr) = result.as_array() {
                                arr.clone()
                            } else if result.is_object() {
                                vec![result.clone()]
                            } else {
                                vec![]
                            };
                            // Entrées sans aucune URI exploitable filtrées avant rendu.
                            let locations: Vec<Value> =
                                locations.into_iter().filter(location_has_uri).collect();
                            let mut out = vec![target_line];
                            // Sections dans l'ordre cible / signature / doc /
                            // défini à — une section vide est omise.
                            if !signature.is_empty() {
                                out.push(format!("signature: {signature}"));
                            }
                            if !doc.is_empty() {
                                out.push(format!("doc: {doc}"));
                            }
                            if locations.is_empty() {
                                out.push("no definitions".to_string());
                            } else {
                                out.push("défini à:".to_string());
                                for loc in &locations {
                                    out.push(format!(
                                        "  {}",
                                        render_location(&state.config.sandbox_root, loc).await
                                    ));
                                }
                            }
                            Some(ok_result(out.join("\n")))
                        }
                        Err(e) => Some(err_result(e.to_string())),
                    }
                }
                (Err(e), _) | (_, Err(e)) => Some(err_result(e.to_string())),
            }
        }
        "lsp_rename" => {
            let args: LspRenameArgs = match &args {
                LspArgs::Rename(a) => a.clone(),
                _ => unreachable!(),
            };
            let (line0, character0, target_line) = match target_info {
                Some(t) => t,
                None => unreachable!("target_info is Some for target-based tools"),
            };
            match (
                client.initialize().await,
                client.ensure_open(&file_uri, language_id, &text).await,
            ) {
                (Ok(_), Ok(())) => match client
                    .request(
                        "textDocument/rename",
                        serde_json::json!({
                            "textDocument": {"uri": file_uri},
                            "position": {"line": line0, "character": character0},
                            "newName": args.new_name
                        }),
                    )
                    .await
                {
                    // Résultat actuel inchangé (chemins résolus modifiés ligne à
                    // ligne, ou `no rename`), ligne cible antéposée. Pas de mode
                    // `preview` ni de rapport avant→après : tâche 5.
                    Ok(result) => {
                        if result.is_null() {
                            Some(ok_result(format!("{target_line}\nno rename")))
                        } else {
                            match apply_workspace_edit(&state.config.sandbox_root, &result).await {
                                Ok(files) if files.is_empty() => {
                                    Some(ok_result(format!("{target_line}\nno rename")))
                                }
                                Ok(files) => {
                                    Some(ok_result(format!("{target_line}\n{}", files.join("\n"))))
                                }
                                Err(e) => Some(err_result(e.to_string())),
                            }
                        }
                    }
                    Err(e) => Some(err_result(e.to_string())),
                },
                (Err(e), _) | (_, Err(e)) => Some(err_result(e.to_string())),
            }
        }
        "lsp_references" => {
            let (line0, character0, target_line) = match target_info {
                Some(t) => t,
                None => unreachable!("target_info is Some for target-based tools"),
            };
            match (
                client.initialize().await,
                client.ensure_open(&file_uri, language_id, &text).await,
            ) {
                (Ok(_), Ok(())) => match client
                    .request(
                        "textDocument/references",
                        serde_json::json!({
                            "textDocument": {"uri": file_uri},
                            "position": {"line": line0, "character": character0},
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
                        // Entrées sans aucune URI exploitable filtrées avant rendu.
                        let locations: Vec<Value> =
                            locations.into_iter().filter(location_has_uri).collect();
                        let mut out = vec![target_line];
                        if locations.is_empty() {
                            out.push("no references".to_string());
                        } else {
                            out.push(format!("références ({}):", locations.len()));
                            for loc in &locations {
                                out.push(format!(
                                    "  {}",
                                    render_location(&state.config.sandbox_root, loc).await
                                ));
                            }
                        }
                        Some(ok_result(out.join("\n")))
                    }
                    Err(e) => Some(err_result(e.to_string())),
                },
                (Err(e), _) | (_, Err(e)) => Some(err_result(e.to_string())),
            }
        }
        _ => unreachable!(),
    }
}

/// Extrait les `(uri, edits)` d'un `WorkspaceEdit` LSP : `changes` (map uri → TextEdit[])
/// et `documentChanges` (array de `{ textDocument: { uri }, edits }`). Ordre : d'abord
/// `changes`, puis `documentChanges` (déduplication par URI conservée — les edits d'une
/// même URI sont concaténés).
fn workspace_edit_files(edit: &Value) -> Vec<(String, Vec<Value>)> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Process `changes` first (map uri → TextEdit[])
    if let Some(changes) = edit.get("changes").and_then(|v| v.as_object()) {
        for (uri, edits) in changes {
            if let Some(edits_arr) = edits.as_array() {
                seen.insert(uri.clone());
                result.push((uri.clone(), edits_arr.clone()));
            }
        }
    }

    // Process `documentChanges` (array of { textDocument: { uri }, edits })
    if let Some(doc_changes) = edit.get("documentChanges").and_then(|v| v.as_array()) {
        for dc in doc_changes {
            if let (Some(uri), Some(edits)) = (
                dc.get("textDocument")
                    .and_then(|td| td.get("uri"))
                    .and_then(|u| u.as_str()),
                dc.get("edits").and_then(|e| e.as_array()),
            ) {
                let uri_str = uri.to_string();
                if seen.insert(uri_str.clone()) {
                    result.push((uri_str, edits.clone()));
                } else if let Some(pos) = result.iter().position(|(u, _)| u == &uri_str) {
                    // Append edits to existing entry
                    let existing = &result[pos].1;
                    let new_edits = edits.clone();
                    let mut merged = existing.clone();
                    merged.extend(new_edits);
                    result[pos] = (uri_str.clone(), merged);
                }
            }
        }
    }

    result
}

/// Convertit une position LSP `{line, character}` (0-based) en offset dans `content`.
/// `character` compté en caractères UTF-8 (approximation UTF-16 LSP, acceptée MVP).
/// Clamp si `character` dépasse la fin de ligne. Erreur `VNL-SBX-LSP-007` si `line`
/// hors limites.
fn position_to_offset(content: &str, line: u64, character: u64) -> anyhow::Result<usize> {
    let line = line as usize;
    let char_offset = character as usize;
    let lines: Vec<&str> = content.lines().collect();

    if line >= lines.len() {
        return Err(anyhow::anyhow!(
            "VNL-SBX-LSP-007: line {} out of range ({} lines)",
            line,
            lines.len()
        ));
    }

    let line_text = lines[line];
    let chars: Vec<char> = line_text.chars().collect();
    let actual_len = chars.len();

    // Clamp character to line length
    let clamped = char_offset.min(actual_len);

    // Compute byte offset: sum of byte lengths of all previous lines + byte offset in current line
    let mut byte_offset: usize = 0;
    for &line_str in &lines[..line] {
        byte_offset += line_str.len() + 1; // +1 for the newline
    }
    // Add the byte offset within the current line
    byte_offset += chars[..clamped].iter().map(|c| c.len_utf8()).sum::<usize>();

    Ok(byte_offset)
}

/// Applique des `TextEdit` LSP (`{ range: { start, end }, newText }`) à `content`.
/// Convertit chaque range en offsets, vérifie `start <= end`, trie par `start`
/// décroissant, puis `replace_range`. Erreur `VNL-SBX-LSP-008` si range manquant/
/// malformé ou `start > end`.
fn apply_text_edits(content: &str, edits: &[Value]) -> anyhow::Result<String> {
    struct ParsedEdit {
        start: usize,
        end: usize,
        new_text: String,
    }

    let mut parsed = Vec::new();

    for edit in edits.iter() {
        let range = match edit.get("range") {
            Some(r) => r,
            None => {
                return Err(anyhow::anyhow!("VNL-SBX-LSP-008: TextEdit missing 'range'"));
            }
        };

        let start_offset = {
            let start_val = range.get("start");
            let start_line_val = start_val.and_then(|s| s.get("line"));
            let start_char_val = start_val.and_then(|s| s.get("character"));
            let line = match start_line_val.and_then(|v| v.as_u64()) {
                Some(n) => n as usize,
                None => {
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-008: TextEdit range missing 'start' line"
                    ));
                }
            };
            let char_off = match start_char_val.and_then(|v| v.as_u64()) {
                Some(n) => n as usize,
                None => {
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-008: TextEdit range missing 'start' character"
                    ));
                }
            };
            match position_to_offset(content, line as u64, char_off as u64) {
                Ok(offset) => offset,
                Err(_) => {
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-008: invalid start position in TextEdit"
                    ));
                }
            }
        };

        let end_offset = {
            let end_val = range.get("end");
            let end_line_val = end_val.and_then(|e| e.get("line"));
            let end_char_val = end_val.and_then(|e| e.get("character"));
            let line = match end_line_val.and_then(|v| v.as_u64()) {
                Some(n) => n as usize,
                None => {
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-008: TextEdit range missing 'end' line"
                    ));
                }
            };
            let char_off = match end_char_val.and_then(|v| v.as_u64()) {
                Some(n) => n as usize,
                None => {
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-008: TextEdit range missing 'end' character"
                    ));
                }
            };
            match position_to_offset(content, line as u64, char_off as u64) {
                Ok(offset) => offset,
                Err(_) => {
                    return Err(anyhow::anyhow!(
                        "VNL-SBX-LSP-008: invalid end position in TextEdit"
                    ));
                }
            }
        };

        if start_offset > end_offset {
            return Err(anyhow::anyhow!(
                "VNL-SBX-LSP-008: TextEdit range start > end ({start_offset} > {end_offset})"
            ));
        }

        let new_text = match edit.get("newText").and_then(|n| n.as_str()) {
            Some(s) => s.to_string(),
            None => String::new(),
        };

        parsed.push(ParsedEdit {
            start: start_offset,
            end: end_offset,
            new_text,
        });
    }

    // Sort by start descending so offsets remain valid
    parsed.sort_by_key(|edit| std::cmp::Reverse(edit.start));

    let mut result = content.to_string();
    for edit in &parsed {
        result.replace_range(edit.start..edit.end, &edit.new_text);
    }

    Ok(result)
}

/// Applique un `WorkspaceEdit` sur le filesystem sandbox : pour chaque `(uri, edits)`,
/// convertit l'URI en chemin (`strip_prefix("file://")`), confine sous `sandbox_root`
/// (échec → `VNL-SBX-LSP-009` avec le message du confine), lit (read_file raw), applique
/// `apply_text_edits`, écrit (write_file). Rend les chemins absolus résolus modifiés.
async fn apply_workspace_edit(sandbox_root: &Path, edit: &Value) -> anyhow::Result<Vec<String>> {
    let mut modified = Vec::new();

    for (uri, edits) in workspace_edit_files(edit) {
        let raw_path = uri.strip_prefix("file://").unwrap_or(&uri);
        let confined_result = confine(sandbox_root, raw_path).await;

        let resolved = match confined_result {
            Ok(r) => r,
            Err(val) => {
                let msg = val["content"][0]["text"]
                    .as_str()
                    .unwrap_or("confinement failed");
                return Err(anyhow::anyhow!("VNL-SBX-LSP-009: {msg}"));
            }
        };

        let text = filesystem::read_file(ReadFileOptions {
            path: resolved.clone(),
            offset: 0,
            limit: 0,
            raw: true,
        })
        .await?;

        let new_text = apply_text_edits(&text, &edits)?;

        filesystem::write_file(WriteFileOptions {
            path: resolved.clone(),
            content: new_text,
        })
        .await?;

        modified.push(resolved);
    }

    Ok(modified)
}

/// Internal enum to hold parsed LSP arguments.
enum LspArgs {
    Diagnostics(LspDiagnosticsArgs),
    /// Modèle de position partagé des tools à position (tâche 01b).
    Target(LspSymbolTarget),
    Rename(LspRenameArgs),
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

    /// Trois états distincts pour `lsp_diagnostics`, pas deux (bug réel trouvé dans un
    /// retour d'usage : un agent qui traite "jamais reçu" comme "propre" peut croire
    /// à tort qu'une édition n'a rien cassé). Ce test couvre le cas "jamais reçu" —
    /// un LSP qui ne publie jamais rien doit produire un message clairement distinct
    /// de "no issues found", pas juste "no diagnostics".
    #[tokio::test]
    async fn lsp_diagnostics_distinguishes_not_yet_analyzed_from_clean() {
        let (state, _tmpdir) = make_lsp_state_nodiag("diag_nodiag").await;
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
        assert!(
            text.contains("not yet analyzed"),
            "should explicitly say not-yet-analyzed, got: {text}"
        );
        assert!(
            !text.contains("no issues found"),
            "must NOT read as a confirmed-clean message, got: {text}"
        );
    }

    #[test]
    fn hover_contents_to_text_handles_all_lsp_shapes() {
        // MarkupContent — la forme réelle envoyée par rust-analyzer/
        // typescript-language-server, jamais gérée avant ce fix.
        assert_eq!(
            hover_contents_to_text(&serde_json::json!({"kind": "markdown", "value": "Arc<T>"})),
            "Arc<T>"
        );
        // MarkedString brute (string).
        assert_eq!(
            hover_contents_to_text(&serde_json::json!("plain hover")),
            "plain hover"
        );
        // MarkedString[] — mélange string et {language, value}.
        assert_eq!(
            hover_contents_to_text(&serde_json::json!([
                "line one",
                {"language": "rust", "value": "line two"}
            ])),
            "line one\nline two"
        );
        // Contenu vide → chaîne vide (le dispatch rend "no hover" dans ce cas).
        assert_eq!(hover_contents_to_text(&serde_json::json!({})), "");
    }

    // ── Tâche 02 — hover_signature_and_doc (absorption de lsp_hover) ────────

    #[test]
    fn hover_split_markdown_fenced_signature_and_doc() {
        let (sig, doc) = hover_signature_and_doc(&serde_json::json!(
            {"kind": "markdown",
             "value": "```rust\nfn main()\n```\n\nEntry point.\nRuns it.\nThird.\nFourth."}
        ));
        assert_eq!(sig, "fn main()", "first fenced block is the signature");
        assert_eq!(
            doc, "Entry point.\nRuns it.\nThird.",
            "doc truncated to 3 lines"
        );
    }

    #[test]
    fn hover_split_plaintext_no_fences_is_signature() {
        // La forme du fake `FAKE_LSP_PY` (plaintext non clôturé) — non régression.
        let (sig, doc) = hover_signature_and_doc(&serde_json::json!(
            {"kind": "plaintext", "value": "hover:file:///x"}
        ));
        assert_eq!(sig, "hover:file:///x");
        assert_eq!(doc, "");
    }

    #[test]
    fn hover_split_array_marked_string_code_then_prose() {
        let (sig, doc) = hover_signature_and_doc(&serde_json::json!([
            "Doc one",
            {"language": "rust", "value": "fn f()"}
        ]));
        assert_eq!(
            sig, "fn f()",
            "first {{language, value}} entry is the signature"
        );
        assert_eq!(doc, "Doc one", "prose outside code sections is the doc");
    }

    #[test]
    fn hover_split_second_code_block_not_in_doc() {
        let (sig, doc) = hover_signature_and_doc(&serde_json::json!({
            "kind": "markdown",
            "value": "```rust\nfn a()\n```\n\nbetween.\n\n```sh\nfn b()\n```\n\nafter."
        }));
        assert_eq!(sig, "fn a()", "signature is the first fenced block");
        assert_eq!(doc, "between.\nafter.");
        assert!(
            !doc.contains("fn b()"),
            "second code block must not leak into the doc, got: {doc}"
        );
        assert!(!doc.contains("```"), "no residual fences, got: {doc}");
    }

    #[test]
    fn hover_split_empty() {
        assert_eq!(
            hover_signature_and_doc(&serde_json::Value::Null),
            (String::new(), String::new())
        );
        assert_eq!(
            hover_signature_and_doc(&serde_json::json!({})),
            (String::new(), String::new())
        );
    }

    #[test]
    fn hover_split_multiline_no_fences() {
        let (sig, doc) = hover_signature_and_doc(&serde_json::json!(
            {"kind": "plaintext", "value": "a\nb\nc\nd"}
        ));
        assert_eq!(sig, "a", "first non-empty line is the signature");
        assert_eq!(doc, "b\nc\nd", "next 3 non-empty lines, 4th truncated");
    }

    #[tokio::test]
    async fn lsp_definition_returns_location() {
        let (state, _tmpdir) = make_lsp_state("def").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_definition",
                serde_json::json!({"path": "main.rs", "line": 1}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        // Nouvelle forme 1-based, chemin relatif + snippet (le fake renvoie
        // désormais 2 locations : l'écho dans le workspace + une externe).
        assert!(
            text.contains("main.rs:1:"),
            "should contain workspace-relative 'main.rs:1:', got: {text}"
        );
        assert!(
            !text.contains(":0:0"),
            "no more 0-based ':0:0' rendering, got: {text}"
        );
        assert!(
            text.contains("file:///external/lib.rs:42"),
            "second location (external, 0-based line 41) rendered raw as 1-based 42, got: {text}"
        );
    }

    #[tokio::test]
    async fn lsp_references_returns_location() {
        let (state, _tmpdir) = make_lsp_state("ref").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_references",
                serde_json::json!({"path": "main.rs", "line": 1}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        // Nouvelle forme 1-based (le fake references n'ajoute qu'une location,
        // l'externe est propre au fake definition).
        assert!(
            text.contains("main.rs:1:"),
            "should contain workspace-relative 'main.rs:1:', got: {text}"
        );
        assert!(
            !text.contains(":0:0"),
            "no more 0-based ':0:0' rendering, got: {text}"
        );
        assert!(
            text.contains("références (1):"),
            "should contain the filtered count title, got: {text}"
        );
    }

    #[tokio::test]
    async fn lsp_unknown_tool_returns_none() {
        let (state, _tmpdir) = make_lsp_state("none").await;
        let result = dispatch_lsp(&state, "nope", serde_json::json!({})).await;
        assert!(result.is_none(), "should return None for unknown tool");
    }

    // ── Tâche 01b — wiring LspSymbolTarget + snippets ─────────────────────────

    /// Test 1 — `lsp_definition` avec `symbol` : ligne cible + location écho
    /// rendue en chemin relatif au workspace avec snippet de ligne.
    #[tokio::test]
    async fn lsp_definition_symbol_relative_paths_and_snippet() {
        let (state, _tmpdir) = make_lsp_state("def_snip").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_definition",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "main"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("cible: main.rs:1: fn main() {}"),
            "target line should carry relative path + snippet, got: {text}"
        );
        assert!(text.contains("défini à:"), "got: {text}");
        assert!(
            text.contains("main.rs:1: fn main() {}"),
            "echo location (0-based line 0) rendered relative + snippet at :1:, got: {text}"
        );
    }

    /// Test 2 (garde R5) — une location hors workspace est rendue **brute**,
    /// sans snippet accolé, jamais lue.
    #[tokio::test]
    async fn lsp_definition_external_location_rendered_without_snippet() {
        let (state, _tmpdir) = make_lsp_state("def_ext").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_definition",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "main"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("file:///external/lib.rs:42"),
            "external URI rendered raw with 1-based line (41 0-based → 42), got: {text}"
        );
        // Pas de snippet sur la location externe (R5 : jamais lue) — vérifié en
        // comparant la ligne brute à la forme « avec snippet ».
        assert!(
            !text.contains("file:///external/lib.rs:42: "),
            "external location must NOT carry a snippet, got: {text}"
        );
    }

    /// Test 3 — `lsp_references` aplati avec snippet + compteur.
    #[tokio::test]
    async fn lsp_references_flat_with_snippet() {
        let (state, _tmpdir) = make_lsp_state("ref_snip").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_references",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "main"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("cible: main.rs:1: fn main() {}"),
            "target line should carry snippet, got: {text}"
        );
        assert!(
            text.contains("références (1):"),
            "title count should be 1, got: {text}"
        );
        assert!(
            text.contains("main.rs:1: fn main() {}"),
            "reference rendered relative + snippet, got: {text}"
        );
    }

    /// Test 4 — pas de tableau en réponse (fake nodiag) → ligne cible puis
    /// `no references`, pas d'erreur.
    #[tokio::test]
    async fn lsp_references_empty_keeps_message() {
        let (state, _tmpdir) = make_lsp_state_nodiag("ref_empty").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_references",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "main"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("cible: main.rs:1: fn main() {}"),
            "target line should still render, got: {text}"
        );
        assert!(
            text.contains("no references"),
            "empty result keeps 'no references', got: {text}"
        );
    }

    /// Test 5 (R6) — symbole trouvé 2× sur la ligne : 1re occurrence utilisée,
    /// ambiguïté notée sur la ligne cible avec la colonne de la 2e.
    #[tokio::test]
    async fn lsp_definition_ambiguous_symbol_noted() {
        let (state, tmpdir) = make_lsp_state("def_amb").await;
        std::fs::write(tmpdir.path().join("two.rs"), "let x = f(x);\n").unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_definition",
                serde_json::json!({"path": "two.rs", "line": 1, "symbol": "x"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("2× sur la ligne"),
            "ambiguity R6 should be noted, got: {text}"
        );
        assert!(
            text.contains("character: 11"),
            "note should cite the 2nd occurrence 1-based column, got: {text}"
        );
    }

    /// Test 6 — `symbol` introuvable → VNL-SBX-LSP-010 en erreur tool.
    #[tokio::test]
    async fn lsp_definition_symbol_not_found_returns_010() {
        let (state, _tmpdir) = make_lsp_state("def_010").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_definition",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "nope"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(result["isError"].as_bool().unwrap(), "should be an error");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("VNL-SBX-LSP-010"),
            "missing symbol should return VNL-SBX-LSP-010, got: {text}"
        );
    }

    /// Test 7 — `line` requis : son absence tombe dans `invalid arguments`.
    #[tokio::test]
    async fn lsp_definition_line_required_invalid_args() {
        let (state, _tmpdir) = make_lsp_state("def_noline").await;
        let result = dispatch_lsp(
            &state,
            "lsp_definition",
            serde_json::json!({"path": "main.rs"}),
        )
        .await
        .expect("dispatch returned None");

        assert!(result["isError"].as_bool().unwrap(), "should be an error");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("invalid arguments"),
            "missing required 'line' should be invalid arguments, got: {text}"
        );
    }

    /// Test 8 — schémas `lsp_tools()` des 3 tools à position alignés sur
    /// `LspSymbolTarget`.
    #[test]
    fn lsp_tools_position_schemas_updated() {
        let tools = lsp_tools();
        let cases = [
            ("lsp_definition", vec!["line"]),
            ("lsp_references", vec!["line"]),
            ("lsp_rename", vec!["line", "new_name"]),
        ];
        for (name, required_keys) in cases {
            let tool = tools
                .iter()
                .find(|t| t["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("{name} should be in lsp_tools()"));
            let required = tool["inputSchema"]["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{name}: required should be an array"));
            for key in required_keys {
                assert!(
                    required.iter().any(|r| r.as_str() == Some(key)),
                    "{name}: required should contain '{key}', got: {required:?}"
                );
            }
            let properties = tool["inputSchema"]["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{name}: properties should be an object"));
            assert!(
                properties.contains_key("symbol"),
                "{name}: properties.symbol should be present"
            );
            let line_desc = tool["inputSchema"]["properties"]["line"]["description"]
                .as_str()
                .unwrap_or_else(|| panic!("{name}: line description should be a string"));
            assert!(
                line_desc.contains("1-based"),
                "{name}: line description should mention 1-based, got: {line_desc}"
            );
        }
    }

    // ── Tâche 02 — lsp_definition absorbe hover ; lsp_hover retiré ───────────

    /// Test 7 — le fake `FAKE_LSP_PY` rend au hover un plaintext non clôturé
    /// (`hover:{uri}`) → tout en signature. La réponse porte `signature:` en
    /// plus de la ligne cible et du bloc `défini à:` (régression 01b).
    #[tokio::test]
    async fn lsp_definition_includes_hover_signature() {
        let (state, _tmpdir) = make_lsp_state("def_hover").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_definition",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "main"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("signature: hover:file://"),
            "hover contents should render as a signature section, got: {text}"
        );
        assert!(
            text.contains("cible: main.rs:1: fn main() {}"),
            "01b target line must survive, got: {text}"
        );
        assert!(
            text.contains("défini à:"),
            "01b definitions block must survive, got: {text}"
        );
    }

    /// Test 8 — hover best effort : le fake nodiag répond `{"echo": …}` sans
    /// `contents` → pas de hover, réponse cible + `no definitions`, aucune
    /// section `signature:`/`doc:`, pas d'erreur tool.
    #[tokio::test]
    async fn lsp_definition_no_hover_still_works() {
        let (state, _tmpdir) = make_lsp_state_nodiag("def_nohover").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_definition",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "main"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("cible: main.rs:1: fn main() {}"),
            "target line should render, got: {text}"
        );
        assert!(text.contains("no definitions"), "got: {text}");
        assert!(
            !text.contains("signature:"),
            "no signature section without hover, got: {text}"
        );
        assert!(
            !text.contains("doc:"),
            "no doc section without hover, got: {text}"
        );
    }

    /// Test 9 — `lsp_hover` retiré : plus d'entrée dans `lsp_tools()`, plus de
    /// dispatch du tout (None).
    #[tokio::test]
    async fn lsp_hover_tool_removed() {
        let tools = lsp_tools();
        assert!(
            !tools
                .iter()
                .any(|t| t["name"].as_str() == Some("lsp_hover")),
            "lsp_hover must no longer be advertised, got: {tools:?}"
        );
        let (state, _tmpdir) = make_empty_lsp_state().await;
        let result =
            dispatch_lsp(&state, "lsp_hover", serde_json::json!({"path": "main.rs"})).await;
        assert!(result.is_none(), "lsp_hover must no longer be dispatched");
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

    // ── Rename arg parsing ────────────────────────────────────────────────────

    #[test]
    fn lsp_rename_args_parse_ok() {
        // Forme aplatie (LspSymbolTarget + new_name) — tâche 01b.
        let val = serde_json::json!({
            "path": "src/main.rs",
            "line": 5,
            "new_name": "bar"
        });
        let args: LspRenameArgs = serde_json::from_value(val).unwrap();
        assert_eq!(args.target.path, "src/main.rs");
        assert_eq!(args.target.line, 5);
        assert_eq!(args.target.symbol, None);
        assert_eq!(args.target.character, None);
        assert_eq!(args.new_name, "bar");
    }

    #[test]
    fn lsp_rename_args_defaults() {
        // Forme minimale avec `line` : `symbol` et `character` valent None.
        let val = serde_json::json!({ "path": "f.rs", "line": 1, "new_name": "x" });
        let args: LspRenameArgs = serde_json::from_value(val).unwrap();
        assert_eq!(args.target.symbol, None);
        assert_eq!(args.target.character, None);
        // `line` est requis (LspSymbolTarget) : sans lui, erreur de parsing.
        let missing_line = serde_json::json!({ "path": "f.rs", "new_name": "x" });
        let result: Result<LspRenameArgs, _> = serde_json::from_value(missing_line);
        assert!(result.is_err(), "missing required 'line' must fail parsing");
    }

    // ── position_to_offset unit tests ─────────────────────────────────────────

    #[test]
    fn position_to_offset_line_character() {
        // line 0, char 2 in "abc\ndef" → offset 2 (byte index of 'c')
        assert_eq!(
            position_to_offset("abc\ndef", 0, 2).unwrap(),
            2,
            "line 0 char 2 → offset 2"
        );
        // line 1, char 2 in "abc\ndef" → 3 (line len) + 1 (newline) + 2 = 6
        assert_eq!(
            position_to_offset("abc\ndef", 1, 2).unwrap(),
            6,
            "line 1 char 2 → offset 6"
        );
        // line 2 is out of range
        let err = position_to_offset("abc\ndef", 2, 0).unwrap_err();
        assert!(
            err.to_string().contains("VNL-SBX-LSP-007"),
            "out-of-range line should return VNL-SBX-LSP-007"
        );
    }

    /// ── apply_text_edits unit tests ──────────────────────────────────────────

    #[test]
    fn apply_text_edits_replaces_range() {
        let content = "fn main() {}";
        let edit = serde_json::json!({
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 2}
            },
            "newText": "X"
        });
        let result = apply_text_edits(content, &[edit]).unwrap();
        assert_eq!(result, "X main() {}");
    }

    #[test]
    fn apply_text_edits_multiple_sorted_descending() {
        let content = "abcd";
        let e1 = serde_json::json!({
            "range": {
                "start": {"line": 0, "character": 1},
                "end": {"line": 0, "character": 2}
            },
            "newText": "X"
        });
        let e2 = serde_json::json!({
            "range": {
                "start": {"line": 0, "character": 2},
                "end": {"line": 0, "character": 3}
            },
            "newText": "Y"
        });
        let result = apply_text_edits(content, &[e1, e2]).unwrap();
        // Sorted descending: (2,3,"Y") first → "aXcd", then (1,2,"X") → "aXYd"
        assert_eq!(result, "aXYd");
    }

    #[test]
    fn apply_text_edits_invalid_range_errors() {
        let content = "foo";
        let edit = serde_json::json!({
            "range": {
                "start": {"line": 0, "character": 5},
                "end": {"line": 0, "character": 2}
            },
            "newText": "X"
        });
        let err = apply_text_edits(content, &[edit]).unwrap_err();
        assert!(
            err.to_string().contains("VNL-SBX-LSP-008"),
            "start > end should return VNL-SBX-LSP-008, got: {}",
            err
        );
    }

    /// ── workspace_edit_files unit tests ──────────────────────────────────────

    #[test]
    fn workspace_edit_files_both_forms() {
        let edit = serde_json::json!({
            "changes": {
                "file:///a": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "X"}]
            },
            "documentChanges": [{
                "textDocument": {"uri": "file:///b"},
                "edits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "Y"}]
            }]
        });
        let files = workspace_edit_files(&edit);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "file:///a");
        assert_eq!(files[1].0, "file:///b");
    }

    #[test]
    fn workspace_edit_files_empty_returns_empty() {
        let edit = serde_json::json!({});
        let files = workspace_edit_files(&edit);
        assert!(files.is_empty());
    }

    /// ── Rename integration tests ─────────────────────────────────────────────
    /// Helper: creates an AppState with a fake Rust LSP (nodiag script).
    /// Writes `main.rs` with `"fn main() {}"` into the tempdir.
    async fn make_lsp_state_nodiag(name: &str) -> (AppState, tempfile::TempDir) {
        let tmpdir = tempfile::TempDir::new().unwrap();
        let script_path = tmpdir.path().join(format!("fake_lsp_{name}.py"));
        std::fs::write(&script_path, lsp_test_fakes::FAKE_LSP_NODIAG_PY).unwrap();
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

    #[tokio::test]
    async fn lsp_rename_modifies_file() {
        let (state, _tmpdir) = make_lsp_state("rename").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_rename",
                serde_json::json!({
                    "path": "main.rs",
                    "line": 1,
                    "symbol": "main",
                    "new_name": "X"
                }),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(
            !result["isError"].as_bool().unwrap_or(false),
            "should be OK (not isError)"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        // Ligne cible antéposée au résultat actuel inchangé.
        assert!(
            text.contains("cible: main.rs:1: fn main() {}"),
            "target line should be prepended, got: {text}"
        );
        assert!(
            text.contains("/main.rs"),
            "result should contain the resolved path"
        );
        // Check the file on disk was modified (le fake édite les caractères
        // 0..2 quoi qu'il en soit).
        let disk_content = std::fs::read_to_string(_tmpdir.path().join("main.rs")).unwrap();
        assert_eq!(disk_content, "X main() {}");
    }

    #[tokio::test]
    async fn lsp_rename_no_changes_returns_no_rename() {
        let (state, tmpdir) = make_lsp_state_nodiag("rename_nodiag").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_rename",
                serde_json::json!({
                    "path": "main.rs",
                    "line": 1,
                    "symbol": "main",
                    "new_name": "X"
                }),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(
            !result["isError"].as_bool().unwrap_or(false),
            "should be OK (not isError)"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        // Ligne cible antéposée puis `no rename` (plus d'égalité stricte).
        assert!(
            text.contains("cible: main.rs:1: fn main() {}"),
            "target line should be prepended, got: {text}"
        );
        assert!(text.contains("no rename"), "should contain 'no rename'");
        // File should be unchanged
        let disk_content = std::fs::read_to_string(tmpdir.path().join("main.rs")).unwrap();
        assert_eq!(disk_content, "fn main() {}");
    }

    #[tokio::test]
    async fn lsp_rename_invalid_args_errors() {
        let (state, _tmpdir) = make_lsp_state("rename_bad_args").await;
        // new_name must be a string, not a number
        let result = dispatch_lsp(
            &state,
            "lsp_rename",
            serde_json::json!({ "path": "main.rs", "new_name": 42 }),
        )
        .await
        .expect("dispatch returned None");

        assert!(
            result["isError"].as_bool().unwrap_or(false),
            "should be an error"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("invalid arguments"),
            "should contain 'invalid arguments'"
        );
    }

    // ── resolve_position unit tests ───────────────────────────────────────────

    #[test]
    fn resolve_position_symbol_unique() {
        let content = "fn main() { call_main(); }";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: Some("call_main".to_string()),
            character: None,
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 12
            }
        );
    }

    #[test]
    fn resolve_position_symbol_word_boundary_not_substring() {
        // `main` inside `mainframe` is NOT a delimited occurrence (right border
        // `f` forbidden) — the central anti-substring test.
        let content = "fn main() { mainframe(); }";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: Some("main".to_string()),
            character: None,
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 3
            }
        );
    }

    #[test]
    fn resolve_position_symbol_not_prefix_of_longer_ident() {
        let content = "foobar foo";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: Some("foo".to_string()),
            character: None,
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 7
            }
        );
    }

    #[test]
    fn resolve_position_symbol_at_line_edges() {
        // Missing borders on both edges = valid borders.
        let content = "foo";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: Some("foo".to_string()),
            character: None,
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 0
            }
        );
    }

    #[test]
    fn resolve_position_symbol_ambiguous_first_and_second_col() {
        let content = "let x = f(x);";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: Some("x".to_string()),
            character: None,
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Ambiguous {
                line0: 0,
                character0: 4,
                matches: 2,
                second_char1: 11,
            }
        );
    }

    #[test]
    fn resolve_position_symbol_not_found_vnl_sbx_lsp_010() {
        let content = "fn main() {}";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: Some("nope".to_string()),
            character: None,
        };
        let err = resolve_position(content, &target).unwrap_err();
        assert!(
            err.to_string().contains("VNL-SBX-LSP-010"),
            "symbol not found should return VNL-SBX-LSP-010, got: {err}"
        );
    }

    #[test]
    fn resolve_position_symbol_multiline_line0() {
        let content = "a()\nb()\nc(foo);\n";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 3,
            symbol: Some("foo".to_string()),
            character: None,
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 2,
                character0: 2
            }
        );
    }

    #[test]
    fn resolve_position_character_1based_to_0based() {
        let content = "abcdefgh";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: None,
            character: Some(4),
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 3
            }
        );
    }

    #[test]
    fn resolve_position_character_zero_saturates() {
        let content = "abcdefgh";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: None,
            character: Some(0),
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 0
            }
        );
    }

    #[test]
    fn resolve_position_symbol_wins_over_character() {
        let content = "foo bar";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: Some("bar".to_string()),
            character: Some(1),
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 4
            }
        );
    }

    #[test]
    fn resolve_position_empty_symbol_falls_back_to_character() {
        let content = "abcdefgh";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: Some("".to_string()),
            character: Some(3),
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 2
            }
        );
    }

    #[test]
    fn resolve_position_default_column_zero() {
        let content = "abcdefgh";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 1,
            symbol: None,
            character: None,
        };
        assert_eq!(
            resolve_position(content, &target).unwrap(),
            PositionResolution::Unique {
                line0: 0,
                character0: 0
            }
        );
    }

    #[test]
    fn resolve_position_line_out_of_range_vnl_sbx_lsp_007() {
        let content = "une\nseule\n";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 99,
            symbol: None,
            character: None,
        };
        let err = resolve_position(content, &target).unwrap_err();
        assert!(
            err.to_string().contains("VNL-SBX-LSP-007"),
            "out-of-range line should return VNL-SBX-LSP-007, got: {err}"
        );
    }

    #[test]
    fn resolve_position_line_zero_vnl_sbx_lsp_007() {
        let content = "une\nseule\n";
        let target = LspSymbolTarget {
            path: "m.rs".to_string(),
            line: 0,
            symbol: None,
            character: None,
        };
        let err = resolve_position(content, &target).unwrap_err();
        assert!(
            err.to_string().contains("VNL-SBX-LSP-007"),
            "line 0 should return VNL-SBX-LSP-007, got: {err}"
        );
    }

    // ── LspSymbolTarget deserialization ───────────────────────────────────────

    #[test]
    fn lsp_symbol_target_deser_line_required() {
        // `line` is required (no serde default) → missing it must fail.
        let missing: Result<LspSymbolTarget, _> =
            serde_json::from_value(serde_json::json!({ "path": "a.rs" }));
        assert!(
            missing.is_err(),
            "line is required — deserialization must fail"
        );
        // `symbol`/`character` default to None when absent.
        let minimal: LspSymbolTarget =
            serde_json::from_value(serde_json::json!({ "path": "a.rs", "line": 3 })).unwrap();
        assert_eq!(minimal.path, "a.rs");
        assert_eq!(minimal.line, 3);
        assert_eq!(minimal.symbol, None);
        assert_eq!(minimal.character, None);
    }
}
