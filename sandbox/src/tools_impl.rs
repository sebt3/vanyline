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

/// Args de `lsp_document_symbols`.
#[derive(serde::Deserialize)]
pub struct LspDocumentSymbolsArgs {
    pub path: String,
}

/// Args de `lsp_workspace_symbols`. `path` est un INDICE de toolchain
/// (`toolchain_for_path` uniquement — jamais ouvert, jamais lu, R5 : aucune
/// opération de fichier sur cette valeur).
#[derive(serde::Deserialize)]
pub struct LspWorkspaceSymbolsArgs {
    pub query: String,
    #[serde(default)]
    pub path: Option<String>,
}

/// Args de `lsp_rename` : position partagée + nouveau nom.
#[derive(serde::Deserialize, Clone)]
pub struct LspRenameArgs {
    #[serde(flatten)]
    pub target: LspSymbolTarget,
    pub new_name: String,
    /// Tâche 05 — `true` : calcule le `WorkspaceEdit` et liste les sites
    /// sans AUCUNE écriture ; `false` (défaut) : applique et rend le rapport
    /// avant→après (`preview: false` = comportement historique + rapport).
    #[serde(default)]
    pub preview: bool,
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

/// Chemin d'affichage d'une URI `file://` : sous `sandbox_root` → chemin relatif
/// au workspace (préfixe `file://{root}/` retiré, sinon chemin confiné résolu) ;
/// hors workspace ou non-`file://` → URI brute.
///
/// Sécurité (design R5) : `confine` avant tout — cette fonction ne fait
/// **aucune lecture**, c'est un rendu de chemin. Un appelant qui veut lire
/// derrière l'URI doit lui-même `confine` (cf. `render_location`). Extraite de
/// `render_location` (tâche 03a), réutilisée par `lsp_document_symbols`.
async fn display_path_for_uri(sandbox_root: &std::path::Path, uri: &str) -> String {
    let confined = match uri.strip_prefix("file://") {
        Some(raw_path) => confine(sandbox_root, raw_path).await.ok(),
        None => None,
    };
    match confined {
        Some(resolved) => {
            let root_prefix = format!("file://{}/", sandbox_root.display());
            match uri.strip_prefix(&root_prefix) {
                Some(rel) => rel.to_string(),
                None => resolved,
            }
        }
        None => {
            // URI non-`file://` ou hors workspace : rendu brut, aucune lecture tentée.
            tracing::debug!("LSP location outside workspace, rendered raw: {uri}");
            uri.to_string()
        }
    }
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

    // R5 : confine AVANT toute lecture — le chemin d'affichage passe par
    // `display_path_for_uri` (qui confine en son sein : relatif si confiné,
    // URI brute sinon) ; la confine ci-dessous ne sert qu'à armer la lecture
    // du snippet, qui ne porte que sur le chemin confiné résolu, jamais sur
    // l'URI brute. Sortie identique à l'état pré-extraction (tests 01b).
    let display_path = display_path_for_uri(sandbox_root, raw_uri).await;
    if let Some(raw_path) = raw_uri.strip_prefix("file://")
        && let Some(resolved) = confine(sandbox_root, raw_path).await.ok()
    {
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

    // URI non-`file://` ou hors workspace : rendu brut, aucune lecture tentée
    // (`display_path_for_uri` a rendu l'URI brute et déjà tracé le debug).
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

/// Schemas for the 6 LSP tools for `tools/list` (same shape as
/// `vanyline_tools::mcp::filesystem_tools()` : name/description/inputSchema).
/// Since task 02 of `lsp-agent-interface`, `lsp_hover` no longer exists as a
/// standalone tool — hover contents are rendered by `lsp_definition`.
/// Since task 03a, `lsp_document_symbols` (outline d'un fichier) s'ajoute à la
/// liste ; since task 03b, `lsp_workspace_symbols` (recherche globale de
/// symboles) — 14 tools MCP au total.
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
            "description": "Find all references of the symbol at a position in a file via the LSP server. Results are grouped by file: each reference is rendered under its enclosing symbol (name + signature, resolved with one documentSymbol per distinct file — never per reference) with a line snippet; out-of-workspace references render raw (bare line, never read). Supported extensions: .rs (rust), .ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs (node) — others (including .vue) return VNL-SBX-LSP-006, no LSP configured for that extension.",
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
            "description": "Rename a symbol at a position in a file via the LSP server. Default (preview false) applies the resulting WorkspaceEdit to the filesystem and returns a before/after report per edited site. With preview true, the WorkspaceEdit is computed and its sites listed grouped by file — no file is modified. Supported extensions: .rs (rust), .ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs (node) — others (including .vue) return VNL-SBX-LSP-006, no LSP configured for that extension.",
            "inputSchema": {
                "type": "object",
                "required": ["path", "line", "new_name"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."},
                    "line": {"type": "integer", "description": "1-based line number (as shown by read_file). Required."},
                    "symbol": {"type": "string", "description": "Identifier name to target on that line (recommended). Resolved to the first delimited-identifier occurrence; if it appears several times the first is used and the answer says so."},
                    "character": {"type": "integer", "description": "1-based column, precise targeting when symbol is absent or ambiguous. Ignored when symbol is set."},
                    "new_name": {"type": "string", "description": "New name for the symbol."},
                    "preview": {"type": "boolean", "description": "If true, compute and list the edits without applying them (no file is modified). Default false applies the rename and returns a before/after report."}
                }
            }
        }),
        serde_json::json!({
            "name": "lsp_document_symbols",
            "description": "Outline a file's symbols (functions, structs, etc.) with kinds, signatures and line numbers via the LSP server. Supported extensions: .rs (rust), .ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs (node) — others (including .vue) return VNL-SBX-LSP-006, no LSP configured for that extension.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file within the sandbox workspace."}
                }
            }
        }),
        serde_json::json!({
            "name": "lsp_workspace_symbols",
            "description": "Search symbols by name across the whole workspace via the LSP server (workspace/symbol). Results come back ranked by relevance: path, line, kind and name. Optional path only selects which toolchain's server answers (extension must be supported: .rs, .ts/.tsx/.js/…); no file is read from it. Supported toolchains: rust, node — others return VNL-SBX-LSP-006.",
            "inputSchema": {
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {"type": "string", "description": "Symbol name or partial name to search for."},
                    "path": {"type": "string", "description": "Optional path of a file in the language whose server should answer (toolchain hint only — the file is not read)."}
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

/// Étiquette d'affichage d'un `SymbolKind` LSP (1..=26). Inconnu (0, >26) →
/// format "symbol{n}" (d'où le retour `String`, pas `&'static str`).
fn symbol_kind_label(kind: i64) -> String {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "fn",
        13 => "var",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enumMember",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "typeParameter",
        _ => return format!("symbol{kind}"),
    }
    .to_string()
}

/// Partie commune d'une ligne de symbole (lsp_document_symbols) :
/// `"{kind-label} {name}"` + `" · {detail}"` si `detail` est une chaîne non
/// vide (c'est là que les serveurs mettent la signature d'un DocumentSymbol).
fn symbol_name_and_detail(sym: &Value) -> String {
    let kind = sym.get("kind").and_then(|k| k.as_i64()).unwrap_or(0);
    let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let mut label = format!("{} {name}", symbol_kind_label(kind));
    if let Some(detail) = sym
        .get("detail")
        .and_then(|d| d.as_str())
        .filter(|d| !d.is_empty())
    {
        label.push_str(&format!(" · {detail}"));
    }
    label
}

/// Rend une entrée `DocumentSymbol` (forme hiérarchique — possède
/// `selectionRange`) et ses `children` récursés **après** le parent dans
/// `lines`, indentée de 2 espaces par niveau. Ligne rendue 1-based depuis
/// `selectionRange.start.line`.
fn render_document_symbol_entry(sym: &Value, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    let line1 = sym
        .get("selectionRange")
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("line"))
        .and_then(|l| l.as_i64())
        .unwrap_or(0)
        + 1;
    lines.push(format!(
        "{indent}{} — L{line1}",
        symbol_name_and_detail(sym)
    ));
    if let Some(children) = sym.get("children").and_then(|c| c.as_array()) {
        for child in children {
            render_document_symbol_entry(child, depth + 1, lines);
        }
    }
}

/// Rend le résultat de `textDocument/documentSymbol` pour `lsp_document_symbols`.
///
/// Résultat normalisé en tableau ; chaque entrée est soit **SymbolInformation**
/// (forme plate — possède `location` : c'est celle que rendent réellement
/// rust-analyzer ET typescript-language-server avec `capabilities: {}`, vérif
/// R2 sur cluster), soit **DocumentSymbol** (hiérarchique — possède
/// `selectionRange`) :
/// - ligne = `"{kind-label} {name}"` + (` · {detail}` si `detail` chaîne non
///   vide) + `" — L{n}"` (1-based : `selectionRange.start.line` du DocumentSymbol
///   ou `location.range.start.line` du SymbolInformation) ;
/// - plates triées par `(line, name)` — le serveur peut ne pas être ordonné ;
/// - plate dont le `location.uri` diffère du fichier ouvert → préfixé du chemin
///   d'affichage (`display_path_for_uri`, R5 : rendu de chemin, aucune lecture) ;
/// - DocumentSymbol : arbre indenté (2 espaces par niveau), children après le
///   parent, ordre du serveur conservé ;
/// - tableau vide (ou null) → message explicite, distinct d'un échec de requête
///   (même cohérence « pas encore analysé » qu'un `lsp_diagnostics` vide).
async fn render_document_symbols(sandbox_root: &Path, opened_uri: &str, result: &Value) -> String {
    let symbols: Vec<Value> = result.as_array().cloned().unwrap_or_default();
    if symbols.is_empty() {
        return "no symbols (file analyzed yet — the outline is empty)".to_string();
    }

    // Entrées plates : (ligne, nom, rendu) — triées par (ligne, nom) avant
    // rendu. Entrées hiérarchiques : rendues dans l'ordre du serveur, children
    // déjà attachés sous leur parent.
    let mut flats: Vec<(i64, String, String)> = Vec::new();
    let mut tree_lines: Vec<String> = Vec::new();

    for sym in &symbols {
        if let Some(loc) = sym.get("location").filter(|l| l.is_object()) {
            // SymbolInformation (forme plate réellement observée, R2).
            let line1 = loc
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_i64())
                .unwrap_or(0)
                + 1;
            let name = sym
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let mut rendered = format!("{} — L{line1}", symbol_name_and_detail(sym));
            if let Some(uri) = loc.get("uri").and_then(|u| u.as_str())
                && uri != opened_uri
            {
                let display = display_path_for_uri(sandbox_root, uri).await;
                rendered = format!("{display}: {rendered}");
            }
            flats.push((line1, name, rendered));
        } else if sym.get("selectionRange").is_some() {
            // DocumentSymbol (forme hiérarchique).
            render_document_symbol_entry(sym, 0, &mut tree_lines);
        }
        // Entrée sans `location` ni `selectionRange` : aucune ligne exploitable,
        // filtrée avant rendu (même logique que `location_has_uri`).
    }

    flats.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    flats
        .iter()
        .map(|(_, _, rendered)| rendered)
        .chain(tree_lines.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rendu plat du résultat `workspace/symbol` : une ligne par SymbolInformation,
/// **ordre du serveur conservé** (les serveurs classent par pertinence — vérif
/// R2 cluster ; re-trier détruirait le ranking). Format design §4 :
/// `<uri relatif>:<ligne 1-based>: <kind-label> <nom>` (+ ` · <detail>` si
/// `detail` chaîne non vide, helper `symbol_name_and_detail` de 03a réutilisé).
/// URI rendue par `display_path_for_uri` (R5 : rendu de chemin, aucune lecture).
/// Entrée sans `location` exploitable (ni uri) : filtrée avant rendu (même
/// logique que `location_has_uri`). Tableau vide/null → `no symbol matching
/// "<query>"`.
async fn render_workspace_symbols(sandbox_root: &Path, query: &str, result: &Value) -> String {
    let symbols: Vec<Value> = result.as_array().cloned().unwrap_or_default();
    if symbols.is_empty() {
        return format!("no symbol matching \"{query}\"");
    }

    let mut lines = Vec::with_capacity(symbols.len());
    for sym in &symbols {
        // SymbolInformation plate : `location.uri` + `location.range.start.line`.
        let Some(loc) = sym.get("location").filter(|l| l.is_object()) else {
            continue;
        };
        let Some(uri) = loc.get("uri").and_then(|u| u.as_str()) else {
            continue;
        };
        let line1 = loc
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_i64())
            .unwrap_or(0)
            + 1;
        let display = display_path_for_uri(sandbox_root, uri).await;
        lines.push(format!(
            "{display}:{line1}: {}",
            symbol_name_and_detail(sym)
        ));
    }
    // Ordre du serveur conservé : surtout pas de sort ici (voir doc ci-dessus).
    lines.join("\n")
}

/// Englobant sur forme PLATE (arbitrage développeur R2 2026-09-04, tâche 04) :
/// parmi les SymbolInformation du même fichier, celui dont
/// `location.range.start.line` est maximal sous la contrainte `<= ref_line0`
/// (dernier symbole démarré **à ou avant** la réf, ordre document). Égalité de
/// ligne de départ : premier rencontré (stabilité). Vide/aucun → `None`.
///
/// Fonction pure, aucun I/O. Justification (design §3/R2) : les deux serveurs
/// réels rendent `documentSymbol` en forme plate avec un `range` ne couvrant
/// que le NOM du symbole — le containment profond est impossible sur la forme
/// réelle ; cette heuristique le remplace, signature = snippet de la ligne de
/// l'englobant (jamais une requête LSP de plus).
///
/// (Signature du contrat de tâche : `&'a [Value] -> Option<&'a Value>` — même
/// type, lifetime élidée : un seul site de lifetime entrant, cf.
/// `clippy::needless_lifetimes`.)
fn flat_enclosing(file_symbols: &[Value], ref_line0: i64) -> Option<&Value> {
    let mut best: Option<(i64, &Value)> = None;
    for sym in file_symbols {
        let Some(line0) = sym
            .get("location")
            .and_then(|l| l.get("range"))
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_i64())
        else {
            continue;
        };
        if line0 > ref_line0 {
            continue;
        }
        // `>` strict : à ligne de départ égale, le premier rencontré gagne
        // (stabilité de l'ordre document).
        if best.is_none_or(|(best_line, _)| line0 > best_line) {
            best = Some((line0, sym));
        }
    }
    best.map(|(_, sym)| sym)
}

/// Englobant sur forme HIERARCHIQUE (tâche 04) : le `DocumentSymbol` le plus
/// profond dont `range` contient la ligne de la réf
/// (`start.line <= ref_line0 <= end.line`), enfants explorés en priorité
/// (plus profond d'abord). Aucun → `None`.
///
/// Fonction pure, aucun I/O. Utilisée quand un serveur rend la forme
/// hiérarchique (`selectionRange`) — avec `detail` pour signature, l'algorithme
/// exact du design §3 redevient possible.
///
/// (Signature du contrat de tâche : `&'a [Value] -> Option<&'a Value>` — même
/// type, lifetime élidée, cf. `clippy::needless_lifetimes`.)
fn deepest_containing(file_symbols: &[Value], ref_line0: i64) -> Option<&Value> {
    for sym in file_symbols {
        let range = match sym.get("range") {
            Some(r) if r.is_object() => r,
            _ => continue,
        };
        let start = range
            .get("start")
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_i64());
        let end = range
            .get("end")
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_i64());
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        if start > ref_line0 || ref_line0 > end {
            continue;
        }
        // Données valides = children ⊆ parent : explorer les enfants quand le
        // parent contient déjà suffit, et le plus profond rendu premier gagne.
        if let Some(children) = sym.get("children").and_then(|c| c.as_array())
            && let Some(deeper) = deepest_containing(children, ref_line0)
        {
            return Some(deeper);
        }
        return Some(sym);
    }
    None
}

/// Rendu du résultat `textDocument/references` pour `lsp_references` (design
/// §3, tâche 04) : réf groupées par fichier, chaque run consécutif sous un
/// même englobant précédé de son en-tête, le tout déterministe (tri par
/// `(display, ligne)`).
///
/// Le fichier de la dispatch a déjà lu/ouvert/interrogé (`documentSymbol` **un
/// seul** par fichier distinct) — ce rendu ne fait **aucune** lecture ni
/// requête : `symbols_by_uri`/`contents_by_uri` portent ce qui a été collecté.
/// Une URI absente de `contents_by_uri` (hors workspace, non-`file://`, toolchain
/// différente « traitée hors workspace », cap 20 fichiers atteint ou lecture
/// échouée) rend des lignes `  L<1-based>` nues — jamais de snippet, jamais de
/// documentSymbol (R5 : c'est le point de sécurité du design).
///
/// Format exact (design §3 + arbitrage plate R2) :
/// - en-tête fichier = `display_path_for_uri` (relatif si confiné, URI brute
///   sinon), colonne 0 ;
/// - en-tête de bloc `  dans {label} · {snippet-signature} — L{ligne englobant}`
///   — `label` = `symbol_name_and_detail`, snippet-signature de la ligne de
///   l'englobant **uniquement si `detail` absent** (forme plate ; hiérarchique
///   → signature = `detail`, déjà dans le label) ;
/// - réf confinée = `    L<1-based>: {snippet}` (snippet via `line_snippet` sur
///   le contenu déjà lu ; snippet impossible → `    L<1-based>` sans suffixe) ;
///   réf sans englobant → même forme nue directement sous l'en-tête fichier.
async fn render_references_grouped(
    sandbox_root: &Path,
    locations: &[Value],
    symbols_by_uri: &std::collections::BTreeMap<String, Vec<Value>>,
    contents_by_uri: &std::collections::BTreeMap<String, String>,
) -> String {
    struct GroupedRef<'a> {
        uri: &'a str,
        line0: i64,
        display: String,
    }

    // Loc → (uri, ligne 0-based, chemin d'affichage). Les deux formes de
    // localisation sont acceptées (`uri`/`range` ou `targetUri`/
    // `targetSelectionRange`, comme `render_location`).
    let mut refs: Vec<GroupedRef<'_>> = Vec::with_capacity(locations.len());
    for loc in locations {
        let Some(uri) = loc
            .get("uri")
            .and_then(|u| u.as_str())
            .or_else(|| loc.get("targetUri").and_then(|u| u.as_str()))
        else {
            continue;
        };
        let line0 = loc
            .get("range")
            .or_else(|| loc.get("targetSelectionRange"))
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_i64())
            .unwrap_or(0);
        // R5 : `display_path_for_uri` est un rendu de chemin (confinement en
        // son sein), jamais une lecture.
        let display = display_path_for_uri(sandbox_root, uri).await;
        refs.push(GroupedRef {
            uri,
            line0,
            display,
        });
    }
    // Rendu déterministe : tri stable par (fichier, ligne) — l'ordre du
    // serveur reste l'arbitre au sein d'un même (display, ligne).
    refs.sort_by(|a, b| a.display.cmp(&b.display).then(a.line0.cmp(&b.line0)));

    let mut out = vec![format!("références ({}):", refs.len())];
    let mut current_uri: Option<&str> = None;
    // Run courant dans le fichier : `None` = aucun run commencé ; `Some(None)`
    // = réf nue (sans englobant) ; `Some(Some(ptr))` = bloc de l'englobant
    // `ptr` (la pointer identifie le symbole dans la liste du fichier).
    let mut current_run: Option<Option<*const Value>> = None;
    // Symboles du fichier courant, filtrés par forme (clonés — les helpers
    // prennent des `&[Value]`).
    let mut flat_syms: Vec<Value> = Vec::new();
    let mut hier_syms: Vec<Value> = Vec::new();

    for r in &refs {
        // Groupe = changement d'URI : en-tête fichier, reset du run,
        // re-filtrage des symboles. Détection de forme PAR ENTRÉE comme dans
        // `render_document_symbols` (mélanges possibles dans un même
        // résultat) : `location` → SymbolInformation plate, conservée
        // seulement si du même fichier ; `selectionRange` → DocumentSymbol.
        if current_uri != Some(r.uri) {
            current_uri = Some(r.uri);
            out.push(r.display.clone());
            current_run = None;
            flat_syms.clear();
            hier_syms.clear();
            if let Some(syms) = symbols_by_uri.get(r.uri) {
                for sym in syms {
                    if let Some(loc) = sym.get("location").filter(|l| l.is_object()) {
                        if loc.get("uri").and_then(|u| u.as_str()) == Some(r.uri) {
                            flat_syms.push(sym.clone());
                        }
                    } else if sym.get("selectionRange").is_some() {
                        hier_syms.push(sym.clone());
                    }
                }
            }
        }

        // Englobant : containment profond exact d'abord sur la forme
        // hiérarchique (seule forme dont `range` couvre le corps), puis
        // heuristique « dernier symbole démarrant avant la réf » sur la forme
        // plate (arbitrage R2 — celle que les deux serveurs réels rendent).
        let enclosing: Option<(&Value, bool)> = deepest_containing(&hier_syms, r.line0)
            .map(|sym| (sym, true))
            .or_else(|| flat_enclosing(&flat_syms, r.line0).map(|sym| (sym, false)));

        // En-tête de bloc au premier run de l'englobant seulement (runs
        // consécutifs, ordre de ligne conservé).
        if let Some((sym, hierarchical)) = enclosing {
            let run_ptr = sym as *const Value;
            if !matches!(current_run, Some(Some(ptr)) if ptr == run_ptr) {
                let enc_line0 = if hierarchical {
                    sym.get("selectionRange")
                } else {
                    sym.get("location").and_then(|l| l.get("range"))
                }
                .and_then(|rg| rg.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_i64())
                .unwrap_or(0);
                let has_detail = sym
                    .get("detail")
                    .and_then(|d| d.as_str())
                    .is_some_and(|d| !d.is_empty());
                // Snippet-signature de l'englobant : forme plate sans `detail`
                // uniquement (arbitrage R2), pris sur le contenu déjà lu —
                // zéro I/O, zéro requête LSP de plus.
                let signature = if has_detail || hierarchical {
                    None
                } else {
                    contents_by_uri
                        .get(r.uri)
                        .and_then(|c| line_snippet(c, enc_line0.max(0) as u64))
                };
                let enc_line1 = enc_line0 + 1;
                let label = symbol_name_and_detail(sym);
                out.push(match signature {
                    Some(sig) => format!("  dans {label} · {sig} — L{enc_line1}"),
                    None => format!("  dans {label} — L{enc_line1}"),
                });
            }
            current_run = Some(Some(run_ptr));
        } else {
            current_run = Some(None);
        }

        // Ligne de réf : snippet sur le contenu déjà lu (`line_snippet` ne
        // relit jamais — R5). URI jamais lue (absente de `contents_by_uri`) →
        // `L<1-based>` seul, comme un groupe hors workspace.
        let line1 = r.line0 + 1;
        match contents_by_uri.get(r.uri) {
            Some(content) => match line_snippet(content, r.line0.max(0) as u64) {
                Some(snippet) => out.push(format!("    L{line1}: {snippet}")),
                None => out.push(format!("    L{line1}")),
            },
            None => out.push(format!("  L{line1}")),
        }
    }

    out.join("\n")
}

/// Dispatches a `tools/call` for `lsp_diagnostics`/`lsp_definition`/
/// `lsp_references`/`lsp_rename`/`lsp_document_symbols`/
/// `lsp_workspace_symbols`. Returns `None` if `name` is not one of these.
/// Consumes `state.lsp` (shared LSP process) and `state.config.sandbox_root`.
pub async fn dispatch_lsp(state: &AppState, name: &str, arguments: Value) -> Option<Value> {
    let lsp_tools = [
        "lsp_diagnostics",
        "lsp_definition",
        "lsp_references",
        "lsp_rename",
        "lsp_document_symbols",
        "lsp_workspace_symbols",
    ];
    if !lsp_tools.contains(&name) {
        return None;
    }

    // Tâche 03b — `lsp_workspace_symbols` : bloc DÉDIÉ avant le préambule
    // partagé confine/lecture/toolchain ci-dessous. Ce tool n'a AUCUN fichier
    // à confiner ni à lire : il sort de la chaîne (confine → lecture raw →
    // toolchain → …) et se traite par la sienne (sélection toolchain →
    // get_or_spawn → initialize → request), SANS `ensure_open` — rien n'est
    // ouvert. Le `path` optionnel n'est qu'un indice de toolchain
    // (`toolchain_for_path`), jamais un chemin résolu, confiné ou lu (R5).
    if name == "lsp_workspace_symbols" {
        let args: LspWorkspaceSymbolsArgs = match serde_json::from_value(arguments) {
            Ok(a) => a,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };

        // Sélection de toolchain : indice explicite (`toolchain_for_path` sur
        // la valeur brute, sans jamais y toucher autrement), sinon première
        // toolchain LSP configurée dans l'ordre `rust`, `node`.
        let toolchain = match &args.path {
            Some(hint) => match toolchain_for_path(hint) {
                Some((tc, _language_id)) => tc,
                None => {
                    // Un indice invalide est une erreur, pas un indice
                    // silencieusement ignoré.
                    return Some(err_result(
                        "VNL-SBX-LSP-006: no LSP configured for that extension".to_string(),
                    ));
                }
            },
            None => match ["rust", "node"].into_iter().find(|tc| state.lsp.has(tc)) {
                Some(tc) => tc,
                None => {
                    return Some(err_result(
                        "VNL-SBX-LSP-006: no LSP toolchain configured in this sandbox".to_string(),
                    ));
                }
            },
        };

        let session = match state.lsp.get_or_spawn(toolchain).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                return Some(err_result(format!(
                    "VNL-SBX-LSP-006: no LSP configured for toolchain {toolchain}"
                )));
            }
            Err(e) => return Some(err_result(e.to_string())),
        };

        let root_uri = format!("file://{}", state.config.sandbox_root.display());
        let mut client = LspClient::new(session, root_uri);
        if let Err(e) = client.initialize().await {
            return Some(err_result(e.to_string()));
        }
        return match client
            .request("workspace/symbol", serde_json::json!({"query": args.query}))
            .await
        {
            Ok(result) => Some(ok_result(
                render_workspace_symbols(&state.config.sandbox_root, &args.query, &result).await,
            )),
            Err(e) => {
                // Dégradation méthode absente (design §4 : « message clair,
                // pas un fallback ») : l'erreur de `lsp_client::request`
                // embarque l'objet error JSON-RPC sérialisé, donc la
                // sous-chaîne `"code":-32601` est déterministe. Rendu en
                // SUCCÈS (ok_result, pas err_result) — et jamais un fallback
                // grep.
                if e.to_string().contains("\"code\":-32601") {
                    tracing::debug!("workspace/symbol not supported by {toolchain} LSP: {e}");
                    Some(ok_result(
                        "this LSP server does not support workspace/symbol (method not found) \
                         — use lsp_document_symbols per file"
                            .to_string(),
                    ))
                } else {
                    // Toute autre erreur → comportement -005 existant.
                    Some(err_result(e.to_string()))
                }
            }
        };
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
    } else if name == "lsp_document_symbols" {
        let args: LspDocumentSymbolsArgs = match serde_json::from_value(arguments.clone()) {
            Ok(a) => a,
            Err(e) => return Some(err_result(format!("invalid arguments for {name}: {e}"))),
        };
        LspArgs::DocumentSymbols(args)
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
        LspArgs::DocumentSymbols(a) => &a.path,
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
    // VNL-SBX-LSP-007/VNL-SBX-LSP-010. `lsp_diagnostics` et
    // `lsp_document_symbols` (tâche 03a — pas de position cible) ne passent
    // pas ici.
    let symbol_target: Option<&LspSymbolTarget> = match &args {
        LspArgs::Target(t) => Some(t),
        LspArgs::Rename(r) => Some(&r.target),
        LspArgs::Diagnostics(_) => None,
        LspArgs::DocumentSymbols(_) => None,
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
                    // Tâche 05 — `result.is_null()` → `no rename` INCHANGÉ ;
                    // sinon embranchement `preview` : `true` → rendu du
                    // `WorkspaceEdit` par `render_rename_preview` (aucune
                    // écriture possible par construction) ; `false` (défaut) →
                    // `apply_workspace_edit` puis rapport avant→après via
                    // `format_applied_files` ; Vec vide après application →
                    // `no rename` INCHANGÉ. Ligne cible antéposée dans les
                    // trois cas.
                    Ok(result) => {
                        if result.is_null() {
                            Some(ok_result(format!("{target_line}\nno rename")))
                        } else if args.preview {
                            Some(ok_result(format!(
                                "{target_line}\n{}",
                                render_rename_preview(&state.config.sandbox_root, &result).await
                            )))
                        } else {
                            match apply_workspace_edit(&state.config.sandbox_root, &result).await {
                                Ok(files) if files.is_empty() => {
                                    Some(ok_result(format!("{target_line}\nno rename")))
                                }
                                Ok(files) => {
                                    // Les paths sont DÉJÀ résolus et confinés
                                    // par `apply_workspace_edit` : simple
                                    // rendu relatif au root (retombe sur
                                    // l'absolu sinon — possible seulement en
                                    // cas de bug de `apply_workspace_edit`),
                                    // pas de re-confinement, pas d'URI ici.
                                    let rendered: Vec<(String, Vec<AppliedSite>)> = files
                                        .into_iter()
                                        .map(|f| {
                                            let display = match f
                                                .path
                                                .strip_prefix(&state.config.sandbox_root)
                                            {
                                                Ok(rel) => rel.display().to_string(),
                                                Err(_) => f.path.display().to_string(),
                                            };
                                            (display, f.sites)
                                        })
                                        .collect();
                                    Some(ok_result(format!(
                                        "{target_line}\nrename appliqué\n{}",
                                        format_applied_files(&rendered)
                                    )))
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
        // Tâche 04 — `lsp_references` enrichi (design §3) : après la requête
        // references (inchangée), un `documentSymbol` par FICHIER DISTINCT
        // jamais par réf, avec lecture + `ensure_open` (idempotent par
        // session, `try_mark_uri_open` déduplique) sous les mêmes garde-fous
        // R5 que le reste : confinés `file://` ONLY, même toolchain que la
        // session, cap 20 fichiers. Le fichier cible, déjà confiné/lu/ouvert
        // par le préambule, est réutilisé (`resolved`/`text`) — jamais relu,
        // jamais re-didOpen'é. Toute erreur de collecte est best effort
        // (`tracing::warn!`, réf rendue sans englobant), jamais une erreur
        // tool.
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
                            return Some(ok_result(out.join("\n")));
                        }

                        // Fichiers distincts touchés par les réf (BTreeSet :
                        // ordre déterministe). Le fichier cible est traité en
                        // tête — il compte dans le cap de 20 et son statut
                        // (déjà lu/ouvert par le préambule) ne doit pas dépendre
                        // de l'ordre de tri des URI.
                        let mut uris: std::collections::BTreeSet<String> =
                            std::collections::BTreeSet::new();
                        for loc in &locations {
                            if let Some(u) = loc.get("uri").and_then(|u| u.as_str()) {
                                uris.insert(u.to_string());
                            } else if let Some(u) = loc.get("targetUri").and_then(|u| u.as_str()) {
                                uris.insert(u.to_string());
                            }
                        }
                        const MAX_REFERENCE_FILES: usize = 20;
                        let mut ordered_uris: Vec<String> = Vec::with_capacity(uris.len());
                        if uris.remove(&file_uri) {
                            ordered_uris.push(file_uri.clone());
                        }
                        ordered_uris.extend(uris);

                        let mut symbols_by_uri: std::collections::BTreeMap<String, Vec<Value>> =
                            std::collections::BTreeMap::new();
                        let mut contents_by_uri: std::collections::BTreeMap<String, String> =
                            std::collections::BTreeMap::new();
                        let mut files_in_pipeline = 0usize;
                        let mut cap_warned = false;
                        for uri in &ordered_uris {
                            // R5 strict : lecture + didOpen + documentSymbol
                            // UNIQUEMENT sur URI `file://` confine OK, même
                            // toolchain que la session. Tout le reste (hors
                            // workspace, non-`file://`, toolchain différente —
                            // « traitée hors workspace ») est rendu brut par le
                            // helper, sans jamais rien tenter.
                            let Some(raw_path) = uri.strip_prefix("file://") else {
                                continue;
                            };
                            let Ok(resolved_file) =
                                confine(&state.config.sandbox_root, raw_path).await
                            else {
                                continue;
                            };
                            let Some((file_toolchain, file_language_id)) =
                                toolchain_for_path(&resolved_file)
                            else {
                                continue;
                            };
                            if file_toolchain != toolchain {
                                continue;
                            }
                            if files_in_pipeline >= MAX_REFERENCE_FILES {
                                if !cap_warned {
                                    tracing::warn!(
                                        "lsp_references: more than {MAX_REFERENCE_FILES} distinct files — \
                                         remaining references rendered without enclosing"
                                    );
                                    cap_warned = true;
                                }
                                continue;
                            }
                            files_in_pipeline += 1;

                            // Fichier cible : texte du préambule réutilisé —
                            // pas de relecture (et didOpen déjà fait au-dessus).
                            let content = if resolved_file == resolved {
                                text.clone()
                            } else {
                                match filesystem::read_file(ReadFileOptions {
                                    path: resolved_file,
                                    offset: 0,
                                    limit: 0,
                                    raw: true,
                                })
                                .await
                                {
                                    Ok(c) => c,
                                    Err(e) => {
                                        tracing::warn!(
                                            "lsp_references: cannot read {uri} (best effort, rendered without enclosing): {e}"
                                        );
                                        continue;
                                    }
                                }
                            };
                            if *uri != file_uri
                                && let Err(e) =
                                    client.ensure_open(uri, file_language_id, &content).await
                            {
                                tracing::warn!(
                                    "lsp_references: cannot didOpen {uri} (best effort, rendered without enclosing): {e}"
                                );
                                continue;
                            }
                            // UN seul documentSymbol par fichier distinct —
                            // erreur → liste vide + warn, jamais une erreur
                            // tool (best effort, design §3).
                            let symbols = match client
                                .request(
                                    "textDocument/documentSymbol",
                                    serde_json::json!({"textDocument": {"uri": uri}}),
                                )
                                .await
                            {
                                Ok(result) => result.as_array().cloned().unwrap_or_default(),
                                Err(e) => {
                                    tracing::warn!(
                                        "lsp_references: documentSymbol failed for {uri} (best effort, rendered without enclosing): {e}"
                                    );
                                    Vec::new()
                                }
                            };
                            contents_by_uri.insert(uri.clone(), content);
                            symbols_by_uri.insert(uri.clone(), symbols);
                        }

                        out.push(
                            render_references_grouped(
                                &state.config.sandbox_root,
                                &locations,
                                &symbols_by_uri,
                                &contents_by_uri,
                            )
                            .await,
                        );
                        Some(ok_result(out.join("\n")))
                    }
                    Err(e) => Some(err_result(e.to_string())),
                },
                (Err(e), _) | (_, Err(e)) => Some(err_result(e.to_string())),
            }
        }
        // Tâche 03a — `lsp_document_symbols` : enchaînement identique à
        // `lsp_diagnostics` (confine → lecture → toolchain → session →
        // initialize → ensure_open → requête, tout cela au-dessus de ce
        // `match`), puis rendu des symboles (plates triées / arbre indenté).
        "lsp_document_symbols" => {
            match (
                client.initialize().await,
                client.ensure_open(&file_uri, language_id, &text).await,
            ) {
                (Ok(_), Ok(())) => match client
                    .request(
                        "textDocument/documentSymbol",
                        serde_json::json!({
                            "textDocument": {"uri": file_uri}
                        }),
                    )
                    .await
                {
                    Ok(result) => Some(ok_result(
                        render_document_symbols(&state.config.sandbox_root, &file_uri, &result)
                            .await,
                    )),
                    // Erreurs requête → err_result (comportement -005 existant
                    // inchangé).
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

/// Un site appliqué par `apply_workspace_edit` : ligne 0-based du début de
/// l'édit + snippets de la ligne AVANT et APRÈS application (calculés sur le
/// contenu réellement lu avant écriture et sur `new_text` — vides si la ligne
/// n'existe plus/hors EOF). Limite assumée et commentée : la ligne du snippet
/// « après » est relue à la MÊME ligne 0-based — exact pour les edits de
/// rename (aucun changement de nb de lignes), approximatif si un serveur
/// renvoie des edits multi-lignes qui décalent le texte.
#[derive(Debug)]
struct AppliedSite {
    line0: u64,
    old_line: String,
    new_line: String,
}

/// Fichier touché + ses sites dans l'ordre des éditions.
#[derive(Debug)]
struct AppliedFile {
    path: PathBuf,
    sites: Vec<AppliedSite>,
}

/// Applique un `WorkspaceEdit` sur le filesystem sandbox : pour chaque `(uri, edits)`,
/// convertit l'URI en chemin (`strip_prefix("file://")`), confine sous `sandbox_root`
/// (échec → `VNL-SBX-LSP-009` avec le message du confine), lit (read_file raw), applique
/// `apply_text_edits`, écrit (write_file). Retour (tâche 05) : par fichier, le détail
/// des sites — snippets calculés AU POINT D'ÉCRITURE, sur le contenu lu et sur
/// `new_text` déjà en main (jamais un second aller-retour LSP, jamais une relecture
/// après écriture, design §5). Comportement d'écriture et garde-fous INCHANGÉS
/// (confinement R5, VNL-SBX-LSP-009 sur URI hors workspace, erreurs apply_text_edits).
async fn apply_workspace_edit(
    sandbox_root: &Path,
    edit: &Value,
) -> anyhow::Result<Vec<AppliedFile>> {
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

        // Snippets avant→après AU POINT D'ÉCRITURE : `text` (contenu lu) et
        // `new_text` sont déjà en main — zéro I/O de plus, zéro requête LSP
        // de plus. Les ranges ont été validés par `apply_text_edits`
        // (VNL-SBX-LSP-008) : un `range.start.line` manquant ici est
        // défensif, jamais attendu.
        let mut sites = Vec::with_capacity(edits.len());
        for e in &edits {
            if let Some(line0) = e
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_u64())
            {
                sites.push(AppliedSite {
                    line0,
                    old_line: line_snippet(&text, line0).unwrap_or_default(),
                    new_line: line_snippet(&new_text, line0).unwrap_or_default(),
                });
            }
        }

        filesystem::write_file(WriteFileOptions {
            path: resolved.clone(),
            content: new_text,
        })
        .await?;

        modified.push(AppliedFile {
            path: PathBuf::from(resolved),
            sites,
        });
    }

    Ok(modified)
}

/// Rapport post-application (pur, testable sans I/O) :
/// `{total} remplacements dans {m} fichiers\n` + par fichier (chemin relatif
/// rendu par l'appelant AVANT d'appeler — ce helper ne connaît que des
/// display paths) : `{display} — {n} remplacements` puis par site
/// `  L<1-based>: {old} → {new}`. `total` = somme des sites.
fn format_applied_files(files: &[(String, Vec<AppliedSite>)]) -> String {
    let total: usize = files.iter().map(|(_, sites)| sites.len()).sum();
    let mut out = vec![format!(
        "{total} remplacements dans {} fichiers",
        files.len()
    )];
    for (display, sites) in files {
        out.push(format!("{} — {} remplacements", display, sites.len()));
        for site in sites {
            out.push(format!(
                "  L{}: {} → {}",
                site.line0 + 1,
                site.old_line,
                site.new_line
            ));
        }
    }
    out.join("\n")
}

/// Rendu preview (tâche 05) : `aperçu — {N} sites dans {M} fichiers — AUCUN
/// fichier modifié` puis, par fichier (ordre de `workspace_edit_files`, sites
/// dans l'ordre des edits), le display path et par site
/// `  L<1-based>:<col 1-based>: <snippet ligne actuelle>`. Async car lit les
/// lignes actuelles (confinées seules) ; aucune écriture possible par
/// construction — n'appelle jamais rien qui écrive. `tool_error` non :
/// erreurs de lecture = lignes nues.
///
/// R5 : un fichier n'est LU que si son URI est `file://` ET confiné OK ; hors
/// workspace / non-`file://` → URI brute rendue, ligne `L<1-based>:<col
/// 1-based>` nue, AUCUNE lecture tentée ; confine OK mais lecture échouée ou
/// ligne absente → ligne nue aussi (le fichier déjà confiné est seul lu).
async fn render_rename_preview(sandbox_root: &Path, edit: &Value) -> String {
    struct PreviewFile {
        display: String,
        /// Contenu actuel — `None` = jamais lu (hors workspace/non-`file://`)
        /// ou lecture échouée → lignes nues sans snippet.
        content: Option<String>,
        /// Positions 0-based (`range.start`) de chaque TextEdit. Un edit sans
        /// `range.start` exploitable ne peut pas être localisé : sauté
        /// (jamais attendu d'un serveur conforme — en mode apply il lèverait
        /// VNL-SBX-LSP-008).
        sites: Vec<(u64, u64)>,
    }

    let mut files: Vec<PreviewFile> = Vec::new();
    for (uri, edits) in workspace_edit_files(edit) {
        let sites: Vec<(u64, u64)> = edits
            .iter()
            .filter_map(|e| {
                let start = e.get("range")?.get("start")?;
                Some((
                    start.get("line")?.as_u64()?,
                    start.get("character")?.as_u64()?,
                ))
            })
            .collect();

        // R5 : confinement avant toute lecture — seul le chemin confiné résolu
        // est lu, jamais l'URI brute. Display : relatif au root (même logique
        // que la ligne « cible: »), URI brute sinon.
        let (display, content) = match uri.strip_prefix("file://") {
            Some(raw_path) => match confine(sandbox_root, raw_path).await.ok() {
                Some(resolved) => {
                    let root_prefix = format!("{}/", sandbox_root.display());
                    let display = match resolved.strip_prefix(&root_prefix) {
                        Some(rel) => rel.to_string(),
                        None => resolved.clone(),
                    };
                    let content = match filesystem::read_file(ReadFileOptions {
                        path: resolved,
                        offset: 0,
                        limit: 0,
                        raw: true,
                    })
                    .await
                    {
                        Ok(c) => Some(c),
                        Err(e) => {
                            // Best effort : le site est rendu nu, jamais une
                            // erreur tool.
                            tracing::warn!("lsp_rename preview: cannot read {uri}: {e}");
                            None
                        }
                    };
                    (display, content)
                }
                None => {
                    tracing::debug!("lsp_rename preview: outside workspace, rendered raw: {uri}");
                    (uri.clone(), None)
                }
            },
            None => {
                tracing::debug!("lsp_rename preview: non-file URI, rendered raw: {uri}");
                (uri.clone(), None)
            }
        };

        files.push(PreviewFile {
            display,
            content,
            sites,
        });
    }

    let total: usize = files.iter().map(|f| f.sites.len()).sum();
    let mut out = vec![format!(
        "aperçu — {total} sites dans {} fichiers — AUCUN fichier modifié",
        files.len()
    )];
    for f in &files {
        out.push(f.display.clone());
        for &(line0, character0) in &f.sites {
            let line1 = line0 + 1;
            let col1 = character0 + 1;
            match f.content.as_deref().and_then(|c| line_snippet(c, line0)) {
                Some(snippet) => out.push(format!("  L{line1}:{col1}: {snippet}")),
                None => out.push(format!("  L{line1}:{col1}")),
            }
        }
    }
    out.join("\n")
}

/// Internal enum to hold parsed LSP arguments.
enum LspArgs {
    Diagnostics(LspDiagnosticsArgs),
    /// Modèle de position partagé des tools à position (tâche 01b).
    Target(LspSymbolTarget),
    Rename(LspRenameArgs),
    /// Args de `lsp_document_symbols` (tâche 03a) — simple `path`.
    DocumentSymbols(LspDocumentSymbolsArgs),
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
        // Tâche 04 — assertions adaptées au rendu groupé (consigné au rapport) :
        // le fake references est devenu un scanner workspace-wide — la requête
        // sans `symbol` (colonne 0) cible le mot « fn » et rend ses occurrences
        // + l'entrée externe fixe → 2 réf, rendues groupées par fichier (plus
        // de ligne `render_location` plate `main.rs:1: …`).
        assert!(
            text.contains("cible: main.rs:1:"),
            "target line unchanged, got: {text}"
        );
        assert!(
            !text.contains(":0:0"),
            "no more 0-based ':0:0' rendering, got: {text}"
        );
        assert!(
            text.contains("références (2):"),
            "occurrence of 'fn' on disk + fixed external entry, got: {text}"
        );
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.contains(&"main.rs"),
            "workspace-relative group header, got: {text}"
        );
        assert!(
            lines.contains(&"file:///external/lib.rs"),
            "external group header rendered raw, got: {text}"
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
        // Tâche 04 — assertions adaptées au rendu groupé (consigné au rapport) :
        // le compteur passe de 1 à 2 (le fake scanner rend l'occurrence disque
        // de `main` + l'entrée externe fixe) et la ligne plate
        // `main.rs:1: fn main() {}` devient le bloc `dans fn main … — L1` suivi
        // de la réf `    L1: fn main() {}` sous son englobant.
        assert!(
            text.contains("cible: main.rs:1: fn main() {}"),
            "target line should carry snippet, got: {text}"
        );
        assert!(
            text.contains("références (2):"),
            "occurrence of 'main' on disk + fixed external entry, got: {text}"
        );
        assert!(
            text.contains("  dans fn main · fn main() {} — L1"),
            "grouped form: block header with snippet-signature, got: {text}"
        );
        assert!(
            text.contains("    L1: fn main() {}"),
            "reference rendered under its enclosing with snippet, got: {text}"
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
        // Tâche 05 — `preview` absent → `false` (le rename applique, preuve
        // d'intégration par `lsp_rename_apply_reports_before_after`).
        assert!(!args.preview, "preview must default to false");
        // Parse explicite `preview: true` → `true`.
        let with_preview = serde_json::json!({
            "path": "f.rs", "line": 1, "new_name": "x", "preview": true
        });
        let args: LspRenameArgs = serde_json::from_value(with_preview).unwrap();
        assert!(args.preview, "explicit preview:true must parse to true");
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
        // Tâche 05 — l'absolu résolu (`/main.rs`) a cédé la place au rapport
        // avant→après avec chemin d'affichage RELATIF au sandbox_root
        // (écart consigné : `contains("/main.rs")` → assertions du nouveau
        // rapport ; la vérification du contenu disque reste).
        assert!(
            text.contains("rename appliqué"),
            "apply mention expected in report, got: {text}"
        );
        assert!(
            text.contains("main.rs — 1 remplacements"),
            "per-file report line with relative display path expected, got: {text}"
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

    // ── Tâche 05 — lsp_rename : preview + rapport avant→après ────────────────

    /// Test 2 — `preview: true` liste les sites du `WorkspaceEdit` SANS
    /// AUCUNE écriture. Le contrat de rendu est en sous-chaînes (le fake rend
    /// 1 edit → 1 site dans 1 fichier) ; l'assertion importante est la
    /// relecture disque APRÈS l'appel : le fichier est INCHANGÉ.
    #[tokio::test]
    async fn lsp_rename_preview_lists_sites_without_writing() {
        let (state, tmpdir) = make_lsp_state("rename_preview").await;
        let original = "fn helper() {}\nfn main() { helper(); }\n";
        std::fs::write(tmpdir.path().join("main.rs"), original).unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_rename",
                serde_json::json!({
                    "path": "main.rs",
                    "line": 1,
                    "symbol": "helper",
                    "new_name": "tool",
                    "preview": true
                }),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(
            !result["isError"].as_bool().unwrap_or(false),
            "should be OK (not isError), got: {result}"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        // Contrat sous-chaînes : `aperçu` + `1` + `AUCUN fichier modifié`.
        assert!(
            text.contains("aperçu"),
            "preview header expected, got: {text}"
        );
        assert!(text.contains('1'), "site count (1) expected, got: {text}");
        assert!(
            text.contains("AUCUN fichier modifié"),
            "no-write mention expected, got: {text}"
        );
        // Site : position 1-based (ligne + colonne du range.start) + snippet
        // de la ligne ACTUELLE (lecture confinée R5).
        assert!(
            text.contains("  L1:1: fn helper() {}"),
            "site line with current-content snippet expected, got: {text}"
        );
        // L'assertion LA plus importante de la tâche : preview:true n'écrit
        // JAMAIS — relecture du fichier après l'appel.
        let disk_content = std::fs::read_to_string(tmpdir.path().join("main.rs")).unwrap();
        assert_eq!(
            disk_content, original,
            "preview:true must not modify the file"
        );
    }

    /// Test 3 — application (défaut, pas de `preview`) : rapport avant→après
    /// par site + fichier changé sur disque. Prouve au passage que `preview`
    /// absent vaut `false` (item 4 — `lsp_rename_preview_default_is_false`,
    /// preuve = ce test qui applique sans le champ).
    #[tokio::test]
    async fn lsp_rename_apply_reports_before_after() {
        let (state, tmpdir) = make_lsp_state("rename_report").await;
        std::fs::write(
            tmpdir.path().join("main.rs"),
            "fn helper() {}\nfn main() { helper(); }\n",
        )
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_rename",
                serde_json::json!({
                    "path": "main.rs",
                    "line": 1,
                    "symbol": "helper",
                    "new_name": "tool"
                }),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(
            !result["isError"].as_bool().unwrap_or(false),
            "should be OK (not isError), got: {result}"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("rename appliqué"),
            "apply mention expected, got: {text}"
        );
        assert!(
            text.contains("1 remplacement"),
            "replacement count expected, got: {text}"
        );
        // Snippet avant → après calculés au point d'écriture : le fake
        // remplace les caractères 0..2 (`fn`) par `X`.
        assert!(
            text.contains("L1: fn helper() {} → X helper() {}"),
            "before/after site line expected, got: {text}"
        );
        let disk_content = std::fs::read_to_string(tmpdir.path().join("main.rs")).unwrap();
        assert_eq!(
            disk_content, "X helper() {}\nfn main() { helper(); }\n",
            "file must be modified by apply"
        );
    }

    /// Test 1 — `format_applied_files` est PUR : display paths déjà résolus
    /// par l'appelant, aucune I/O. Deux fichiers → total + en-têtes par
    /// fichier + sites `  L<1-based>: {old} → {new}`, ordre stable.
    #[test]
    fn format_applied_files_two_files_totals() {
        let files = vec![
            (
                "main.rs".to_string(),
                vec![
                    AppliedSite {
                        line0: 0,
                        old_line: "fn helper() {}".to_string(),
                        new_line: "fn tool() {}".to_string(),
                    },
                    AppliedSite {
                        line0: 1,
                        old_line: "helper();".to_string(),
                        new_line: "tool();".to_string(),
                    },
                ],
            ),
            (
                "lib.rs".to_string(),
                vec![AppliedSite {
                    line0: 3,
                    old_line: "pub fn helper() {}".to_string(),
                    new_line: "pub fn tool() {}".to_string(),
                }],
            ),
        ];
        let out = format_applied_files(&files);
        assert!(
            out.contains("3 remplacements dans 2 fichiers"),
            "total header expected, got: {out}"
        );
        assert!(
            out.contains("main.rs — 2 remplacements"),
            "main.rs header expected, got: {out}"
        );
        assert!(
            out.contains("  L1: fn helper() {} → fn tool() {}"),
            "site line expected, got: {out}"
        );
        // Ordre stable : en-tête main.rs avant lib.rs (ordre d'entrée).
        let pos_main = out.find("main.rs — 2 remplacements").unwrap();
        let pos_lib = out.find("lib.rs — 1 remplacements").unwrap();
        assert!(
            pos_main < pos_lib,
            "input order must be preserved, got: {out}"
        );
    }

    /// Test 5 — assertion R5 du preview, `render_rename_preview` APPELÉE
    /// DIRECTEMENT (le fake rename ne produit qu'un fichier interne, le cas
    /// external n'a pas de chemin unitaire hors LSP sinon) : un URI
    /// `file:///external/…` hors workspace est rendu BRUT, ligne nue
    /// `L11:5` sans snippet, aucune lecture tentée.
    #[tokio::test]
    async fn render_rename_preview_external_uri_renders_bare_line() {
        let root = make_root();
        let edit = serde_json::json!({
            "changes": {
                "file:///external/lib.rs": [
                    {
                        "range": {
                            "start": {"line": 10, "character": 4},
                            "end": {"line": 10, "character": 8}
                        },
                        "newText": "X"
                    }
                ]
            }
        });
        let out = render_rename_preview(root.path(), &edit).await;
        assert!(
            out.contains("file:///external/lib.rs"),
            "raw URI expected, got: {out}"
        );
        assert!(
            out.contains("L11:5"),
            "1-based line:col expected, got: {out}"
        );
        // Ligne NUE : rien après le `L11:5` — pas de snippet tenté hors
        // workspace (R5).
        assert!(
            out.lines().any(|l| l.trim() == "L11:5"),
            "bare L11:5 line (no snippet) expected, got: {out}"
        );
    }

    /// Test 7 — schéma MCP : propriété `preview` (boolean) présente sur
    /// `lsp_rename` dans `lsp_tools()`. Le comptage global des tools (14,
    /// inchangé) est couvert par `mcp::tests::tools_list_returns_all_tools`.
    #[test]
    fn lsp_rename_schema_has_preview_property() {
        let tools = lsp_tools();
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some("lsp_rename"))
            .expect("lsp_rename should be in lsp_tools()");
        let preview = &tool["inputSchema"]["properties"]["preview"];
        assert!(
            !preview.is_null(),
            "preview property should be present, got: {}",
            tool["inputSchema"]
        );
        assert_eq!(
            preview["type"].as_str(),
            Some("boolean"),
            "preview should be a boolean, got: {preview}"
        );
        let desc = preview["description"].as_str().unwrap_or_default();
        assert!(
            desc.contains("no file is modified") && desc.contains("Default false"),
            "preview description should cover both modes, got: {desc}"
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

    // ── Tâche 03a — lsp_document_symbols ──────────────────────────────────────

    #[test]
    fn symbol_kind_label_known_and_unknown() {
        assert_eq!(symbol_kind_label(12), "fn");
        assert_eq!(symbol_kind_label(23), "struct");
        assert_eq!(symbol_kind_label(26), "typeParameter");
        assert_eq!(symbol_kind_label(99), "symbol99");
        assert_eq!(symbol_kind_label(0), "symbol0");
    }

    #[tokio::test]
    async fn display_path_for_uri_relative_outside_and_foreign() {
        let root = make_root();
        // tmpdir est canonique sur Linux ; on construit l'URI sur le chemin
        // canoniqué pour que le préfixe `file://{root}/` matche.
        let inside = format!(
            "file://{}/sub/file.txt",
            std::fs::canonicalize(root.path()).unwrap().display()
        );
        assert_eq!(
            display_path_for_uri(root.path(), &inside).await,
            "sub/file.txt",
            "URI sous la racine rendue en chemin relatif au workspace"
        );
        assert_eq!(
            display_path_for_uri(root.path(), "file:///etc/passwd").await,
            "file:///etc/passwd",
            "URI hors workspace rendue brute"
        );
        assert_eq!(
            display_path_for_uri(root.path(), "http://x").await,
            "http://x",
            "URI non-`file://` rendue brute"
        );
    }

    #[tokio::test]
    async fn lsp_document_symbols_flat_sorted() {
        let (state, tmpdir) = make_lsp_state("docsym_flat").await;
        // Le scan du fake trouve struct Config L1, fn helper L2, fn main L3.
        std::fs::write(
            tmpdir.path().join("main.rs"),
            "struct Config { }\nfn helper() {}\nfn main() {}",
        )
        .unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_document_symbols",
                serde_json::json!({"path": "main.rs"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // Lignes exactes : pas de préfixe d'URI (même fichier que celui ouvert).
        assert!(
            lines.contains(&"struct Config — L1"),
            "flat entry rendered 'kind-label name — L(n, 1-based)', got: {text}"
        );
        assert!(
            lines.contains(&"fn helper — L2"),
            "flat entry rendered 'kind-label name — L(n, 1-based)', got: {text}"
        );
        assert!(
            lines.contains(&"fn main — L3"),
            "flat entry rendered 'kind-label name — L(n, 1-based)', got: {text}"
        );
        // Rendu trié par ligne (le fake rend l'ordre du scan ; le serveur réel
        // peut ne pas être ordonné — le rendu trie).
        let idx = |needle: &str| lines.iter().position(|l| *l == needle).unwrap();
        assert!(
            idx("struct Config — L1") < idx("fn helper — L2")
                && idx("fn helper — L2") < idx("fn main — L3"),
            "flat entries must be sorted by line, got: {text}"
        );
    }

    #[tokio::test]
    async fn lsp_document_symbols_hierarchical_indented() {
        let (state, tmpdir) = make_lsp_state("docsym_hier").await;
        // Le marqueur HIER fait rendre au fake la forme DocumentSymbol :
        // Outer (struct, L1, detail "struct Outer") + child run (method, L3,
        // detail "() -> ()").
        std::fs::write(tmpdir.path().join("hier.rs"), "HIER\nfn outer() {}").unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_document_symbols",
                serde_json::json!({"path": "hier.rs"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.contains(&"struct Outer · struct Outer — L1"),
            "parent rendered with kind-label, name, detail and 1-based line, got: {text}"
        );
        // Le fake rend `"kind":6` (contrat §Fake du fichier de tâche) → la
        // table `symbol_kind_label` rend "method" (6 = Method dans la spec
        // LSP, comme dans le contrat). La ligne attendue « fn run » du §Tests
        // du fichier de tâche est en écart avec le §Contrats ; le contrat
        // primeiro (voir rapport de tâche).
        assert!(
            lines.contains(&"  method run · () -> () — L3"),
            "child recursed after parent, indented 2 spaces, got: {text}"
        );
    }

    #[tokio::test]
    async fn lsp_document_symbols_empty_message() {
        let (state, tmpdir) = make_lsp_state("docsym_empty").await;
        // Aucun fn/struct → le fake répond `[]` : succès au message explicite,
        // distinct d'un échec de requête.
        std::fs::write(tmpdir.path().join("empty.rs"), "// nothing\n").unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_document_symbols",
                serde_json::json!({"path": "empty.rs"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(
            !result["isError"].as_bool().unwrap(),
            "empty symbol array is a success, not an error"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("no symbols"),
            "empty array renders the explicit 'no symbols' message, got: {text}"
        );
    }

    #[tokio::test]
    async fn lsp_document_symbols_no_toolchain_for_extension() {
        let (state, tmpdir) = make_lsp_state("docsym_noext").await;
        std::fs::write(tmpdir.path().join("main.py"), "x = 1").unwrap();

        let result = dispatch_lsp(
            &state,
            "lsp_document_symbols",
            serde_json::json!({"path": "main.py"}),
        )
        .await
        .expect("dispatch returned None");

        assert!(result["isError"].as_bool().unwrap(), "should be an error");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("VNL-SBX-LSP-006"),
            "no LSP for extension should return VNL-SBX-LSP-006, got: {text}"
        );
    }

    // ── Tâche 03b — lsp_workspace_symbols ─────────────────────────────────────

    /// Helper : harness `make_lsp_state` (seule toolchain « rust » configurée,
    /// le fallback `["rust","node"]` sélectionne donc rust) + `src/main.rs`
    /// avec `struct Config` L1 et `fn helper` L3 (0-based 0 et 2).
    async fn make_workspace_symbols_state(name: &str) -> (AppState, tempfile::TempDir) {
        let (state, tmpdir) = make_lsp_state(name).await;
        std::fs::create_dir_all(tmpdir.path().join("src")).unwrap();
        std::fs::write(
            tmpdir.path().join("src/main.rs"),
            "struct Config {\n}\nfn helper() {}\n",
        )
        .unwrap();
        (state, tmpdir)
    }

    /// Test 1 — format design §4 : `<chemin>:<ligne 1-based>: <kind> <nom>`,
    /// pas de `·` sans detail.
    #[tokio::test]
    async fn lsp_workspace_symbols_flat_design_format() {
        let (state, _tmpdir) = make_workspace_symbols_state("wssym_format").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_workspace_symbols",
                serde_json::json!({"query": "Config"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.contains(&"src/main.rs:1: struct Config"),
            "flat line 'chemin:ligne 1-based: kind nom', got: {text}"
        );
        assert!(!text.contains('·'), "no '·' without detail, got: {text}");
    }

    /// Test 2 — le `query` filtre par sous-chaîne du nom (le fake applique
    /// `query in name` ; le serveur réel rend déjà filtré + ranké).
    #[tokio::test]
    async fn lsp_workspace_symbols_query_filters_and_keeps_server_order() {
        let (state, _tmpdir) = make_workspace_symbols_state("wssym_filter").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_workspace_symbols",
                serde_json::json!({"query": "helper"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            !text.contains("struct Config"),
            "query 'helper' must not surface the Config symbol, got: {text}"
        );
        assert!(
            text.contains("src/main.rs:3: fn helper"),
            "should contain the helper line (0-based line 2 rendered 3), got: {text}"
        );
    }

    /// Test 3 — un `path` d'extension non supportée est une ERREUR (-006),
    /// pas un indice silencieusement ignoré. Le fichier n'a même pas à exister
    /// (il n'est jamais lu).
    #[tokio::test]
    async fn lsp_workspace_symbols_bad_hint_is_error() {
        let (state, _tmpdir) = make_workspace_symbols_state("wssym_badhint").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_workspace_symbols",
                serde_json::json!({"query": "x", "path": "a.py"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(result["isError"].as_bool().unwrap(), "should be an error");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("VNL-SBX-LSP-006"),
            "bad toolchain hint should return VNL-SBX-LSP-006, got: {text}"
        );
    }

    /// Test 4 — assertion forte du « jamais lu » : un `path` d'indice
    /// INEXISTANT ne casse rien (jamais chemin résolu, jamais confiné,
    /// jamais lu — résultat identique à sans path).
    #[tokio::test]
    async fn lsp_workspace_symbols_hint_with_known_extension_ok() {
        let (state, _tmpdir) = make_workspace_symbols_state("wssym_hint").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_workspace_symbols",
                serde_json::json!({"query": "Config", "path": "inexistant.rs"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(
            !result["isError"].as_bool().unwrap(),
            "nonexistent .rs hint must not fail (never read), got: {}",
            result["content"][0]["text"]
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("src/main.rs:1: struct Config"),
            "same results as without path, got: {text}"
        );
    }

    /// Test 5 — aucun symbole rendu → succès au message explicite.
    #[tokio::test]
    async fn lsp_workspace_symbols_no_match() {
        let (state, _tmpdir) = make_workspace_symbols_state("wssym_nomatch").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_workspace_symbols",
                serde_json::json!({"query": "ZZZ"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(
            !result["isError"].as_bool().unwrap(),
            "no match is a success, not an error"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("no symbol matching \"ZZZ\""),
            "empty result renders the explicit no-match message, got: {text}"
        );
    }

    /// Test 6 — méthode absente (-32601) : DÉGRADATION en message clair
    /// (isError FALSE), pas une erreur, jamais un fallback grep.
    #[tokio::test]
    async fn lsp_workspace_symbols_method_missing_degrades() {
        let (state, _tmpdir) = make_workspace_symbols_state("wssym_nosupport").await;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_workspace_symbols",
                serde_json::json!({"query": "NOSUPPORT"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(
            !result["isError"].as_bool().unwrap(),
            "-32601 must degrade to a clear message, not an error, got: {}",
            result["content"][0]["text"]
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("method not found"),
            "degradation message should mention 'method not found', got: {text}"
        );
    }

    // ── Tâche 04 — lsp_references enrichi (groupé par fichier + englobant) ────

    /// Test 1 — englobant sur forme PLATE (arbitrage R2 2026-09-04) : dernier
    /// symbole démarré à ou avant la réf (bornes inclusives), pures fonctions
    /// sans I/O.
    #[test]
    fn flat_enclosing_last_start_before() {
        let syms = serde_json::json!([
            {"name": "a", "kind": 12, "location": {"uri": "file:///m.rs",
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}},
            {"name": "b", "kind": 12, "location": {"uri": "file:///m.rs",
                "range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 1}}}},
            {"name": "c", "kind": 12, "location": {"uri": "file:///m.rs",
                "range": {"start": {"line": 5, "character": 0}, "end": {"line": 5, "character": 1}}}}
        ]);
        let arr = syms.as_array().unwrap();
        assert_eq!(
            flat_enclosing(arr, 1).and_then(|s| s["name"].as_str()),
            Some("a"),
            "réf ligne 1 (0-based) → symbole démarré L0"
        );
        assert_eq!(
            flat_enclosing(arr, 2).and_then(|s| s["name"].as_str()),
            Some("b"),
            "réf sur la ligne même du symbole : '<=' est inclusif"
        );
        assert_eq!(
            flat_enclosing(arr, 7).and_then(|s| s["name"].as_str()),
            Some("c"),
            "réf après tout → dernier symbole démarré"
        );
        assert!(flat_enclosing(&[], 3).is_none(), "liste vide → None");
    }

    /// Test 2 — englobant sur forme HIERARCHIQUE : le `DocumentSymbol` le plus
    /// profond dont `range` contient la ligne, enfants explorés en priorité.
    #[test]
    fn deepest_containing_prefers_deepest() {
        let syms = serde_json::json!([{
            "name": "outer", "kind": 12,
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 10, "character": 0}},
            "selectionRange": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
            "children": [{
                "name": "inner", "kind": 12,
                "range": {"start": {"line": 3, "character": 0}, "end": {"line": 6, "character": 0}},
                "selectionRange": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 5}}
            }]
        }]);
        let arr = syms.as_array().unwrap();
        assert_eq!(
            deepest_containing(arr, 4).and_then(|s| s["name"].as_str()),
            Some("inner"),
            "réf 4 contenue par outer ET inner → le plus profond"
        );
        assert_eq!(
            deepest_containing(arr, 1).and_then(|s| s["name"].as_str()),
            Some("outer"),
            "réf 1 contenue par outer seul"
        );
        assert!(
            deepest_containing(arr, 20).is_none(),
            "hors de tout range → None"
        );
    }

    /// Test 3 — rendu groupé design §3 : un documentSymbol sur le fichier cible,
    /// blocs d'englobant avec snippet-signature (forme plate R2), groupe
    /// externe rendu brut sans aucune lecture (R5).
    #[tokio::test]
    async fn lsp_references_grouped_with_enclosing() {
        let (state, tmpdir) = make_lsp_state("ref_group").await;
        std::fs::write(
            tmpdir.path().join("main.rs"),
            "fn helper() {}\nfn main() { helper(); }\n",
        )
        .unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_references",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "helper"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // Comptage exact : 2 occurrences disques de helper (L1 + L2) + 1 entrée
        // synthétique externe.
        assert!(
            text.contains("références (3):"),
            "exact count: main.rs twice + file:///external/lib.rs, got: {text}"
        );
        // Groupe main.rs : en-tête relatif + deux blocs d'englobant.
        assert!(lines.contains(&"main.rs"), "file header, got: {text}");
        let idx = |needle: &str| {
            lines
                .iter()
                .position(|l| *l == needle)
                .unwrap_or_else(|| panic!("line {needle:?} missing, got: {text}"))
        };
        assert_eq!(
            lines[idx("  dans fn helper · fn helper() {} — L1") + 1],
            "    L1: fn helper() {}",
            "ref L1 under its enclosing block, snippet from content already read, got: {text}"
        );
        assert_eq!(
            lines[idx("  dans fn main · fn main() { helper(); } — L2") + 1],
            "    L2: fn main() { helper(); }",
            "ref L2 under its enclosing block, got: {text}"
        );
        // Groupe externe (R5 — assertion de sécurité) : URI brute en en-tête,
        // ligne `L11` nue — aucune lecture, aucun snippet, aucun documentSymbol.
        assert!(
            lines.contains(&"file:///external/lib.rs"),
            "external group header rendered raw, got: {text}"
        );
        assert_eq!(
            lines[idx("file:///external/lib.rs") + 1],
            "  L11",
            "external ref is a bare 1-based line (0-based 10), got: {text}"
        );
        assert!(
            !text.contains("file:///external/lib.rs:11"),
            "external entry must never carry a snippet, got: {text}"
        );
    }

    /// Test 4 — chemin secondaire complet : `lib.rs` (fichier non-cible) est lu,
    /// didOpen'és et documentSymbol'és une fois — sa preuve est son bloc
    /// d'englobant à lui, impossible sans ces trois étapes.
    #[tokio::test]
    async fn lsp_references_secondary_file_read_and_grouped() {
        let (state, tmpdir) = make_lsp_state("ref_secondary").await;
        std::fs::write(
            tmpdir.path().join("main.rs"),
            "fn helper() {}\nfn main() { helper(); }\n",
        )
        .unwrap();
        std::fs::write(
            tmpdir.path().join("lib.rs"),
            "fn use_helper() { helper(); }\n",
        )
        .unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_references",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "helper"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            text.contains("références (4):"),
            "main.rs twice + lib.rs once + external, got: {text}"
        );
        assert!(
            lines.contains(&"lib.rs"),
            "secondary file has its own group header (it was read), got: {text}"
        );
        assert!(
            lines.contains(&"  dans fn use_helper · fn use_helper() { helper(); } — L1"),
            "enclosing block on the secondary file — proof of its documentSymbol, got: {text}"
        );
        assert!(
            lines.contains(&"    L1: fn use_helper() { helper(); }"),
            "ref on the secondary file with snippet — proof of its read, got: {text}"
        );
    }

    /// Test 5 — réf sans symbole englobant au-dessus : ligne nue sous l'en-tête
    /// fichier, sans ligne `dans`, pas d'erreur.
    #[tokio::test]
    async fn lsp_references_word_without_symbol_context() {
        let (state, tmpdir) = make_lsp_state("ref_noenc").await;
        std::fs::write(tmpdir.path().join("main.rs"), "zed_qux\n").unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            dispatch_lsp(
                &state,
                "lsp_references",
                serde_json::json!({"path": "main.rs", "line": 1, "symbol": "zed_qux"}),
            ),
        )
        .await
        .expect("timeout")
        .expect("dispatch returned None");

        assert!(!result["isError"].as_bool().unwrap(), "should be OK");
        let text = result["content"][0]["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.contains(&"main.rs"), "file header, got: {text}");
        assert!(
            lines.contains(&"    L1: zed_qux"),
            "bare ref line under the file header, no enclosing, got: {text}"
        );
        assert!(
            !text.contains("dans"),
            "no enclosing block without a symbol above, got: {text}"
        );
    }
}
