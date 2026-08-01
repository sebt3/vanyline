use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

use crate::error::ToolsError;
use crate::output;

pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ReadFileOptions {
    pub path: String,
    /// Offset 0-based en lignes. Défaut 0.
    #[serde(default)]
    pub offset: usize,
    /// Nombre max de lignes retournées. 0 = utiliser `output::READ_MAX_LINES`
    /// (même convention que `ExecuteCommandOptions::timeout_secs`).
    #[serde(default)]
    pub limit: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WriteFileOptions {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EditFileOptions {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteFileOptions {
    pub path: String,
}

// ---------------------------------------------------------------------------
// Helper: FileNotFound hint
// ---------------------------------------------------------------------------

/// Build the `hint` suffix for `FileNotFound`:
/// - parent dir is empty or "." → no extra info
/// - parent dir is listable → `", parent directory contains: [a.txt, b.rs]"`
/// - parent dir doesn't exist → `", parent directory does not exist either"`
pub(crate) fn file_not_found_hint(path: &str) -> String {
    let parent = std::path::Path::new(&path)
        .parent()
        .map(|p| p.as_os_str())
        .filter(|p| !p.is_empty());

    match parent {
        None => String::new(),
        Some(parent)
            if parent.as_encoded_bytes().is_empty() || parent == std::ffi::OsStr::new(".") =>
        {
            String::new()
        }
        Some(parent) => match std::fs::read_dir(parent) {
            Ok(entries) => {
                let names: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                let mut sorted = names;
                sorted.sort();
                if sorted.is_empty() {
                    ", parent directory contains: []".to_string()
                } else {
                    let display = sorted.join(", ");
                    format!(", parent directory contains: [{}]", display)
                }
            }
            Err(_) => ", parent directory does not exist either".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub fn read_file(opts: ReadFileOptions) -> BoxedFuture<Result<String, ToolsError>> {
    let path = opts.path.clone();
    let offset = opts.offset;
    let limit = opts.limit;

    Box::pin(async move {
        // 1. metadata → file not found / io / dir
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolsError::FileNotFound {
                    path: path.clone(),
                    hint: file_not_found_hint(&path),
                });
            }
            Err(e) => {
                return Err(ToolsError::Io {
                    path: path.clone(),
                    source: e,
                })
            }
        };

        if meta.is_dir() {
            return Err(ToolsError::NotAFile(path));
        }

        // 2. read content
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(ToolsError::PermissionDenied(path));
            }
            Err(e) => return Err(ToolsError::Io { path, source: e }),
        };

        // 3. empty file check
        let total = content.lines().count();
        if total == 0 {
            if offset == 0 {
                return Ok(String::new());
            } else {
                return Err(ToolsError::InvalidArgument {
                    name: "offset".into(),
                    reason: "file is empty".into(),
                });
            }
        }

        // 4. offset out of range
        if offset >= total {
            return Err(ToolsError::InvalidArgument {
                name: "offset".into(),
                reason: format!(
                    "file has {} lines, offset {} is out of range",
                    total, offset
                ),
            });
        }

        // 5. effective_limit
        let effective_limit = if limit == 0 {
            output::READ_MAX_LINES
        } else {
            limit
        };

        // 6. number_lines before bounding
        let sliced: Vec<&str> = content.lines().skip(offset).collect();
        let sliced_text = sliced.join("\n");
        let numbered = output::number_lines(&sliced_text, offset + 1);
        let result =
            output::bound_lines(&numbered, offset, effective_limit, output::READ_MAX_BYTES);

        Ok(result)
    })
}

/// Écrit `content` dans `path` de façon atomique : fichier temporaire dans
/// le même répertoire que `path` (même filesystem, `rename` reste
/// atomique), permissions du fichier remplacé préservées s'il existe, puis
/// `rename` par-dessus la cible (R15 — un crash/kill en cours d'écriture ne
/// laisse plus un fichier tronqué, et le mode du fichier n'est plus
/// silencieusement réinitialisé à l'umask du process).
async fn atomic_write(path: &str, content: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path_ref = std::path::Path::new(path);
    let parent = path_ref
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path_ref.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
    })?;
    let tmp_path = parent.join(format!(
        ".{}.vny-tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));

    if let Err(e) = tokio::fs::write(&tmp_path, content.as_ref()).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    if let Ok(existing_meta) = tokio::fs::metadata(path_ref).await {
        if let Err(e) = tokio::fs::set_permissions(&tmp_path, existing_meta.permissions()).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
    }

    if let Err(e) = tokio::fs::rename(&tmp_path, path_ref).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------

pub fn write_file(opts: WriteFileOptions) -> BoxedFuture<Result<(), ToolsError>> {
    let path = opts.path.clone();
    let content = opts.content;

    Box::pin(async move {
        // 1. create parent dirs
        if let Some(parent) = std::path::Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        return Err(ToolsError::PermissionDenied(
                            parent.to_string_lossy().to_string(),
                        ));
                    }
                    return Err(ToolsError::Io {
                        path: parent.to_string_lossy().to_string(),
                        source: e,
                    });
                }
            }
        }

        // 2. check if path exists and is a directory
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            if meta.is_dir() {
                return Err(ToolsError::NotAFile(path));
            }
        }

        // 3. write file
        match atomic_write(&path, content).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(ToolsError::PermissionDenied(path))
            }
            Err(e) => Err(ToolsError::Io {
                path: path.clone(),
                source: e,
            }),
        }
    })
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

pub fn edit_file(opts: EditFileOptions) -> BoxedFuture<Result<String, ToolsError>> {
    let path = opts.path.clone();
    let old_string = opts.old_string.clone();
    let new_string = opts.new_string.clone();
    let replace_all = opts.replace_all;

    Box::pin(async move {
        // 1. empty old_string
        if old_string.is_empty() {
            return Err(ToolsError::InvalidArgument {
                name: "old_string".into(),
                reason: "must not be empty".into(),
            });
        }

        // 2. read file (same error handling as read_file)
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolsError::FileNotFound {
                    path: path.clone(),
                    hint: file_not_found_hint(&path),
                });
            }
            Err(e) => {
                return Err(ToolsError::Io {
                    path: path.clone(),
                    source: e,
                })
            }
        };

        if meta.is_dir() {
            return Err(ToolsError::NotAFile(path.clone()));
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(ToolsError::PermissionDenied(path.clone()));
            }
            Err(e) => return Err(ToolsError::Io { path, source: e }),
        };

        // 3. count occurrences
        let count = content.matches(&old_string).count();

        // 4. no match → EditNoMatch with Levenshtein closest line
        if count == 0 {
            let hint = closest_line_hint(&content, &old_string);
            return Err(ToolsError::EditNoMatch {
                path: path.clone(),
                hint,
            });
        }

        // 5. ambiguous
        if count > 1 && !replace_all {
            return Err(ToolsError::EditAmbiguous {
                path: path.clone(),
                count,
            });
        }

        // 6. perform replacement
        let n = if replace_all { count } else { 1 };
        let new_content = if replace_all {
            content.replace(&old_string, &new_string)
        } else {
            content.replacen(&old_string, &new_string, 1)
        };

        // 7. write back
        match atomic_write(&path, new_content).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(ToolsError::PermissionDenied(path.clone()))
            }
            Err(e) => Err(ToolsError::Io {
                path: path.clone(),
                source: e,
            }),
        }?;

        // 8. return message
        Ok(format!("edited {}: {} replacement(s)", path, n))
    })
}

// ---------------------------------------------------------------------------
// Levenshtein distance & closest line
// ---------------------------------------------------------------------------

/// Classic Levenshtein distance (insertion/deletion/substitution = cost 1).
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let len_a = a_bytes.len();
    let len_b = b_bytes.len();

    if len_a == 0 {
        return len_b;
    }
    if len_b == 0 {
        return len_a;
    }

    // Use a single vector with rolling rows for memory efficiency
    let mut prev: Vec<usize> = (0..=len_b).collect();
    let mut curr: Vec<usize> = vec![0; len_b + 1];

    for i in 1..=len_a {
        curr[0] = i;
        for j in 1..=len_b {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[len_b]
}

/// Find the closest line to `needle` by Levenshtein distance.
/// Compares `needle.trim()` against every line (trimmed) of `content`.
/// Returns the hint string in the format: ". Closest line in file: '...'"
/// If the file is empty (no lines), returns an empty string.
fn closest_line_hint(content: &str, needle: &str) -> String {
    let trimmed = needle.trim();
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return String::new();
    }

    let mut best_line = lines[0].trim();
    let mut best_dist = levenshtein_distance(trimmed, best_line);

    for &line in &lines[1..] {
        let trimmed_line = line.trim();
        let dist = levenshtein_distance(trimmed, trimmed_line);
        if dist < best_dist {
            best_dist = dist;
            best_line = trimmed_line;
        }
    }

    format!(". Closest line in file: '{}'", best_line)
}

// ---------------------------------------------------------------------------
// list_directory v2 — compact tree, bounded by depth
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct ListDirectoryOptions {
    pub path: String,
    /// Recursion depth. 0 = default (1), same convention as `ReadFileOptions::limit`.
    #[serde(default)]
    pub depth: usize,
}

pub fn list_directory(opts: ListDirectoryOptions) -> BoxedFuture<Result<String, ToolsError>> {
    let path = opts.path;
    let path_for_err = path.clone();
    let depth = opts.depth;

    Box::pin(async move {
        let result = tokio::task::spawn_blocking(move || {
            // 1. Resolve path
            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(ToolsError::FileNotFound {
                        path: path.clone(),
                        hint: file_not_found_hint(&path),
                    });
                }
                Err(e) => {
                    return Err(ToolsError::Io {
                        path: path.clone(),
                        source: e,
                    });
                }
            };
            if !meta.is_dir() {
                return Err(ToolsError::NotADirectory(path));
            }

            let effective_depth = if depth == 0 { 1 } else { depth };

            // 2. Build the output tree
            let mut output = Vec::new();
            let mut count = 0usize;
            let max_entries = output::LIST_MAX_ENTRIES;
            let limit_reached = std::cell::Cell::new(false);

            fn recurse(
                dir: &std::path::Path,
                depth_remaining: usize,
                indent_level: usize,
                output: &mut Vec<String>,
                count: &mut usize,
                max_entries: usize,
                limit_reached: &std::cell::Cell<bool>,
            ) {
                if limit_reached.get() || *count >= max_entries {
                    return;
                }

                let mut entries: Vec<(_, _)> = Vec::new();
                let reader = match std::fs::read_dir(dir) {
                    Ok(r) => r,
                    Err(_) => return,
                };
                for entry in reader.flatten() {
                    entries.push((entry.file_name(), entry.path()));
                }
                entries.sort_by(|a, b| a.0.cmp(&b.0));

                for (file_name, full_path) in entries {
                    if limit_reached.get() {
                        break;
                    }

                    let is_dir = full_path.is_dir();
                    let indent = "  ".repeat(indent_level);
                    let suffix = if is_dir { "/" } else { "" };
                    output.push(format!(
                        "{}{}{}",
                        indent,
                        file_name.to_string_lossy(),
                        suffix
                    ));
                    *count += 1;

                    // Check limit after adding the entry
                    if *count >= max_entries {
                        limit_reached.set(true);
                        break;
                    }

                    // Only descend if there are remaining levels
                    if is_dir && depth_remaining > 1 {
                        recurse(
                            &full_path,
                            depth_remaining - 1,
                            indent_level + 1,
                            output,
                            count,
                            max_entries,
                            limit_reached,
                        );
                    }
                }
            }

            recurse(
                std::path::Path::new(&path),
                effective_depth,
                0,
                &mut output,
                &mut count,
                max_entries,
                &limit_reached,
            );

            if output.is_empty() {
                Ok(format!("{} is empty", path))
            } else {
                let mut result = output.join("\n");
                if limit_reached.get() {
                    result.push_str(&format!(
                        "\n[truncated at {} entries — narrow the path or reduce depth]",
                        output::LIST_MAX_ENTRIES
                    ));
                }
                Ok(result)
            }
        })
        .await
        .map_err(|e| ToolsError::Io {
            path: path_for_err,
            source: std::io::Error::other(e.to_string()),
        })?;

        result
    })
}

pub fn delete_file(opts: DeleteFileOptions) -> BoxedFuture<Result<(), ToolsError>> {
    let path = opts.path.clone();
    Box::pin(async move {
        // 1. metadata — not found → FileNotFound, other → Io
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolsError::FileNotFound {
                    path: path.clone(),
                    hint: file_not_found_hint(&path),
                });
            }
            Err(e) => {
                return Err(ToolsError::Io {
                    path: path.clone(),
                    source: e,
                })
            }
        };

        // 2. directory → remove_dir
        if meta.is_dir() {
            match tokio::fs::remove_dir(&path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                    Err(ToolsError::InvalidArgument {
                        name: "path".into(),
                        reason: "directory is not empty".into(),
                    })
                }
                Err(e) => Err(ToolsError::Io { path, source: e }),
            }
        } else {
            // 3. file → remove_file
            match tokio::fs::remove_file(&path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    Err(ToolsError::PermissionDenied(path))
                }
                Err(e) => Err(ToolsError::Io { path, source: e }),
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// -----------------------------------------------------------------------
    /// read_file tests
    /// -----------------------------------------------------------------------

    #[tokio::test]
    async fn read_file_nominal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "line1\nline2\nline3\nline4\nline5\n")
            .await
            .unwrap();

        let result = read_file(ReadFileOptions {
            path: path.to_string_lossy().to_string(),
            offset: 0,
            limit: 0, // use default READ_MAX_LINES
        })
        .await
        .unwrap();

        // 5 lines numbered, no truncation marker
        assert!(!result.contains("truncated"));
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 5);
        for (i, line) in lines.iter().enumerate() {
            assert!(line.starts_with(&format!("{:>5}\t", i + 1)));
        }
    }

    #[tokio::test]
    async fn read_file_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let lines: Vec<String> = (0..10).map(|i| format!("line {}", i)).collect();
        tokio::fs::write(&path, lines.join("\n") + "\n")
            .await
            .unwrap();

        let result = read_file(ReadFileOptions {
            path: path.to_string_lossy().to_string(),
            offset: 5,
            limit: 0,
        })
        .await
        .unwrap();

        let all_lines: Vec<&str> = result.lines().collect();
        assert_eq!(all_lines.len(), 5);
        // Numbering should start at 6 (offset 5 + 1)
        assert!(all_lines[0].starts_with("    6\t"));
    }

    #[tokio::test]
    async fn read_file_limit_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let lines: Vec<String> = (0..300).map(|i| format!("line {}", i)).collect();
        tokio::fs::write(&path, lines.join("\n") + "\n")
            .await
            .unwrap();

        let result = read_file(ReadFileOptions {
            path: path.to_string_lossy().to_string(),
            offset: 0,
            limit: 50,
        })
        .await
        .unwrap();

        // Should contain 50 numbered lines + truncation marker
        // Count actual content lines (excluding the marker line)
        let content_lines = result
            .lines()
            .take_while(|l| !l.contains("truncated"))
            .count();
        assert_eq!(content_lines, 50);
        assert!(result.contains("truncated"));
        assert!(result.contains("offset=50"));
    }

    #[tokio::test]
    async fn read_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.txt");

        let result = read_file(ReadFileOptions {
            path: path.to_string_lossy().to_string(),
            offset: 0,
            limit: 0,
        })
        .await;

        match result {
            Err(ToolsError::FileNotFound { path: p, hint }) => {
                assert!(p.contains("nonexistent.txt"));
                assert!(
                    hint.contains("parent directory contains:"),
                    "hint should mention parent directory, got: {}",
                    hint
                );
            }
            other => panic!("Expected FileNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn read_file_not_found_missing_parent() {
        let path = "/nonexistent_parent/does_not_exist/foo.txt".to_string();

        let result = read_file(ReadFileOptions {
            path: path.clone(),
            offset: 0,
            limit: 0,
        })
        .await;

        match result {
            Err(ToolsError::FileNotFound { hint, .. }) => {
                assert!(
                    hint.contains("does not exist either"),
                    "hint should say 'does not exist either', got: {}",
                    hint
                );
            }
            other => panic!("Expected FileNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn read_file_on_directory() {
        let dir = tempfile::tempdir().unwrap();

        let result = read_file(ReadFileOptions {
            path: dir.path().to_string_lossy().to_string(),
            offset: 0,
            limit: 0,
        })
        .await;

        match result {
            Err(ToolsError::NotAFile(p)) => {
                assert!(p.contains(dir.path().to_str().unwrap_or("")));
            }
            other => panic!("Expected NotAFile, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn read_file_offset_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "a\nb\nc\n").await.unwrap();

        let result = read_file(ReadFileOptions {
            path: path.to_string_lossy().to_string(),
            offset: 10,
            limit: 0,
        })
        .await;

        match result {
            Err(ToolsError::InvalidArgument { name, reason }) => {
                assert_eq!(name, "offset");
                assert!(reason.contains("3 lines"));
            }
            other => panic!("Expected InvalidArgument, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn read_file_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        tokio::fs::write(&path, "").await.unwrap();

        let result = read_file(ReadFileOptions {
            path: path.to_string_lossy().to_string(),
            offset: 0,
            limit: 0,
        })
        .await
        .unwrap();

        assert_eq!(result, "");
    }

    // -----------------------------------------------------------------------
    /// write_file tests
    /// -----------------------------------------------------------------------

    #[tokio::test]
    async fn write_file_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.txt");
        let content = "hello parents";

        write_file(WriteFileOptions {
            path: path.to_string_lossy().to_string(),
            content: content.to_string(),
        })
        .await
        .unwrap();

        let result = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(result, content);
    }

    #[tokio::test]
    async fn write_file_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");

        // First write
        write_file(WriteFileOptions {
            path: path.to_string_lossy().to_string(),
            content: "first".to_string(),
        })
        .await
        .unwrap();

        // Second write
        write_file(WriteFileOptions {
            path: path.to_string_lossy().to_string(),
            content: "second".to_string(),
        })
        .await
        .unwrap();

        let result = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(result, "second");
    }

    #[tokio::test]
    async fn write_file_on_directory() {
        let dir = tempfile::tempdir().unwrap();

        let result = write_file(WriteFileOptions {
            path: dir.path().to_string_lossy().to_string(),
            content: "data".to_string(),
        })
        .await;

        match result {
            Err(ToolsError::NotAFile(p)) => {
                assert!(p.contains(dir.path().to_str().unwrap_or("")));
            }
            other => panic!("Expected NotAFile, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    /// edit_file tests
    /// -----------------------------------------------------------------------

    #[tokio::test]
    async fn edit_file_nominal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "hello world\nfoo bar\n")
            .await
            .unwrap();

        let result = edit_file(EditFileOptions {
            path: path.to_string_lossy().to_string(),
            old_string: "world".into(),
            new_string: "universe".into(),
            replace_all: false,
        })
        .await
        .unwrap();

        assert!(result.contains("1 replacement"));
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("hello universe"));
    }

    #[tokio::test]
    async fn edit_file_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "hello world\nfoo bar\n")
            .await
            .unwrap();

        let result = edit_file(EditFileOptions {
            path: path.to_string_lossy().to_string(),
            old_string: "xyzzy".into(),
            new_string: "qux".into(),
            replace_all: false,
        })
        .await;

        match result {
            Err(ToolsError::EditNoMatch { path: p, hint }) => {
                assert!(p.contains("test.txt"));
                assert!(hint.contains("Closest line in file:"));
            }
            other => panic!("Expected EditNoMatch, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn edit_file_no_match_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        tokio::fs::write(&path, "").await.unwrap();

        let result = edit_file(EditFileOptions {
            path: path.to_string_lossy().to_string(),
            old_string: "xyzzy".into(),
            new_string: "qux".into(),
            replace_all: false,
        })
        .await;

        match result {
            Err(ToolsError::EditNoMatch { hint, .. }) => {
                // Should not panic, hint should be empty
                assert!(
                    hint.is_empty(),
                    "hint should be empty for empty file, got: '{}'",
                    hint
                );
            }
            other => panic!("Expected EditNoMatch, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn edit_file_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "apple\napple\napple\n")
            .await
            .unwrap();

        let result = edit_file(EditFileOptions {
            path: path.to_string_lossy().to_string(),
            old_string: "apple".into(),
            new_string: "orange".into(),
            replace_all: false,
        })
        .await;

        match result {
            Err(ToolsError::EditAmbiguous { path: p, count }) => {
                assert!(p.contains("test.txt"));
                assert_eq!(count, 3);
            }
            other => panic!("Expected EditAmbiguous, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn edit_file_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "apple\napple\napple\n")
            .await
            .unwrap();

        let result = edit_file(EditFileOptions {
            path: path.to_string_lossy().to_string(),
            old_string: "apple".into(),
            new_string: "orange".into(),
            replace_all: true,
        })
        .await
        .unwrap();

        assert!(result.contains("3 replacement"));
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "orange\norange\norange\n");
    }

    #[tokio::test]
    async fn edit_file_empty_old_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "hello\n").await.unwrap();

        let result = edit_file(EditFileOptions {
            path: path.to_string_lossy().to_string(),
            old_string: "".into(),
            new_string: "world".into(),
            replace_all: false,
        })
        .await;

        match result {
            Err(ToolsError::InvalidArgument { name, reason }) => {
                assert_eq!(name, "old_string");
                assert!(reason.contains("must not be empty"));
            }
            other => panic!("Expected InvalidArgument, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn edit_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.txt");

        let result = edit_file(EditFileOptions {
            path: path.to_string_lossy().to_string(),
            old_string: "hello".into(),
            new_string: "world".into(),
            replace_all: false,
        })
        .await;

        match result {
            Err(ToolsError::FileNotFound { path: p, .. }) => {
                assert!(p.contains("nonexistent.txt"));
            }
            other => panic!("Expected FileNotFound, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // list_directory v2 tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_directory_nominal() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("alpha.txt"), "content")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("beta"))
            .await
            .unwrap();

        let result = list_directory(ListDirectoryOptions {
            path: dir.path().to_string_lossy().to_string(),
            depth: 0,
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        // alpha.txt first (alphabetically), then beta/
        assert!(lines[0] == "alpha.txt");
        assert!(lines[1] == "beta/");
    }

    #[tokio::test]
    async fn list_directory_depth_2() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = a.join("b.txt");
        tokio::fs::create_dir_all(&a).await.unwrap();
        tokio::fs::write(&b, "content").await.unwrap();

        let result = list_directory(ListDirectoryOptions {
            path: dir.path().to_string_lossy().to_string(),
            depth: 2,
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        // a/ at depth 0, then "  b.txt" at depth 1 (indented)
        assert!(lines[0] == "a/");
        assert!(lines[1] == "  b.txt");
    }

    #[tokio::test]
    async fn list_directory_depth_default_no_recursion() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = a.join("b.txt");
        tokio::fs::create_dir_all(&a).await.unwrap();
        tokio::fs::write(&b, "content").await.unwrap();

        // depth=0 → effective_depth=1 → only immediate children, no recursion
        let result = list_directory(ListDirectoryOptions {
            path: dir.path().to_string_lossy().to_string(),
            depth: 0,
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0] == "a/");
        // b.txt should NOT be in the output (not descended)
        assert!(!lines.join("").contains("b.txt"));
    }

    #[tokio::test]
    async fn list_directory_empty() {
        let dir = tempfile::tempdir().unwrap();

        let result = list_directory(ListDirectoryOptions {
            path: dir.path().to_string_lossy().to_string(),
            depth: 0,
        })
        .await
        .unwrap();

        assert!(result.contains("is empty"));
    }

    #[tokio::test]
    async fn list_directory_not_found() {
        let result = list_directory(ListDirectoryOptions {
            path: "/nonexistent/path/xyz".into(),
            depth: 0,
        })
        .await;

        match result {
            Err(ToolsError::FileNotFound { path: ref p, .. }) => {
                assert!(p.contains("nonexistent"));
            }
            other => panic!("Expected FileNotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn list_directory_on_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        tokio::fs::write(&file, "data").await.unwrap();

        let result = list_directory(ListDirectoryOptions {
            path: file.to_string_lossy().to_string(),
            depth: 0,
        })
        .await;

        match result {
            Err(ToolsError::NotADirectory(ref p)) => {
                assert!(p.contains("test.txt"));
            }
            other => panic!("Expected NotADirectory, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn list_directory_truncates() {
        let dir = tempfile::tempdir().unwrap();
        // Create 201 files to exceed LIST_MAX_ENTRIES (200)
        for i in 0..201 {
            let f = dir.path().join(format!("file_{:04}.txt", i));
            tokio::fs::write(&f, "x").await.unwrap();
        }

        let result = list_directory(ListDirectoryOptions {
            path: dir.path().to_string_lossy().to_string(),
            depth: 0,
        })
        .await
        .unwrap();

        assert!(result.contains("truncated at 200 entries"));
    }

    // -----------------------------------------------------------------------
    // delete_file tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn delete_file_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("delete_me.txt");
        tokio::fs::write(&file, "temp content").await.unwrap();

        delete_file(DeleteFileOptions {
            path: file.to_string_lossy().to_string(),
        })
        .await
        .unwrap();

        // file is gone — metadata should fail
        let meta = tokio::fs::metadata(&file).await;
        assert!(meta.is_err());
    }

    #[tokio::test]
    async fn delete_file_removes_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let empty_dir = dir.path().join("empty_subdir");
        tokio::fs::create_dir(&empty_dir).await.unwrap();

        assert!(tokio::fs::metadata(&empty_dir).await.is_ok());

        delete_file(DeleteFileOptions {
            path: empty_dir.to_string_lossy().to_string(),
        })
        .await
        .unwrap();

        // directory should be gone
        assert!(tokio::fs::metadata(&empty_dir).await.is_err());
    }

    #[tokio::test]
    async fn delete_file_non_empty_dir_error() {
        let dir = tempfile::tempdir().unwrap();
        let non_empty = dir.path().join("non_empty");
        tokio::fs::create_dir(&non_empty).await.unwrap();
        // Create a file inside to make it non-empty
        tokio::fs::write(non_empty.join("inner.txt"), "data")
            .await
            .unwrap();

        let result = delete_file(DeleteFileOptions {
            path: non_empty.to_string_lossy().to_string(),
        })
        .await;

        match result {
            Err(ToolsError::InvalidArgument { name, reason }) => {
                assert_eq!(name, "path");
                assert!(reason.contains("not empty"));
            }
            other => panic!("Expected InvalidArgument, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn delete_file_not_found() {
        let result = delete_file(DeleteFileOptions {
            path: "/nonexistent/fake/path/file.txt".into(),
        })
        .await;

        match result {
            Err(ToolsError::FileNotFound { path: ref p, .. }) => {
                assert!(p.contains("/nonexistent/fake/path/file.txt"));
            }
            other => panic!("Expected FileNotFound, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    /// R15 — atomic_write tests
    /// -----------------------------------------------------------------------

    #[tokio::test]
    async fn write_file_preserves_permissions_of_existing_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        tokio::fs::write(&path, "old content").await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
            .await
            .unwrap();

        let result = write_file(WriteFileOptions {
            path: path.to_string_lossy().to_string(),
            content: "new content".to_string(),
        })
        .await;

        assert!(result.is_ok());
        let meta = tokio::fs::metadata(&path).await.unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o640);
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "new content"
        );
    }

    #[tokio::test]
    async fn write_file_does_not_leave_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target.txt");

        let result = write_file(WriteFileOptions {
            path: path.to_string_lossy().to_string(),
            content: "content".to_string(),
        })
        .await;
        assert!(result.is_ok());

        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().to_string());
        }
        assert_eq!(names, vec!["target.txt".to_string()]);
    }

    #[tokio::test]
    async fn edit_file_preserves_permissions_of_existing_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        tokio::fs::write(&path, "hello world\n").await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
            .await
            .unwrap();

        let result = edit_file(EditFileOptions {
            path: path.to_string_lossy().to_string(),
            old_string: "hello".to_string(),
            new_string: "goodbye".to_string(),
            replace_all: false,
        })
        .await;

        assert!(result.is_ok());
        let meta = tokio::fs::metadata(&path).await.unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o640);
    }
}
