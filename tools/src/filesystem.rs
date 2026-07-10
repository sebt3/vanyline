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

// Legacy types — kept for delete_file/create_directory/list_directory which
// remain on FilesystemError in this task.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteFileOptions {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateDirectoryOptions {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListDirectoryOptions {
    pub path: String,
}

// ---------------------------------------------------------------------------
// Legacy error type (kept for delete_file / create_directory / list_directory)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct FilesystemError(std::io::Error);

impl std::fmt::Display for FilesystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IO error: {}", self.0)
    }
}

impl std::error::Error for FilesystemError {}

// ---------------------------------------------------------------------------
// Helper: FileNotFound hint
// ---------------------------------------------------------------------------

/// Build the `hint` suffix for `FileNotFound`:
/// - parent dir is empty or "." → no extra info
/// - parent dir is listable → `", parent directory contains: [a.txt, b.rs]"`
/// - parent dir doesn't exist → `", parent directory does not exist either"`
fn file_not_found_hint(path: &str) -> String {
    let parent = std::path::Path::new(&path)
        .parent()
        .map(|p| p.as_os_str())
        .filter(|p| !p.is_empty());

    match parent {
        None => String::new(),
        Some(parent) if parent.as_encoded_bytes().is_empty()
            || parent == std::ffi::OsStr::new(".") =>
        {
            String::new()
        }
        Some(parent) => {
            match std::fs::read_dir(parent) {
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
            }
        }
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
            Err(e) => return Err(ToolsError::Io { path: path.clone(), source: e }),
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
        let result = output::bound_lines(&numbered, offset, effective_limit, output::READ_MAX_BYTES);

        Ok(result)
    })
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
        match tokio::fs::write(&path, content).await {
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
            Err(e) => return Err(ToolsError::Io { path: path.clone(), source: e }),
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
        match tokio::fs::write(&path, new_content).await {
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
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1)
                .min(curr[j - 1] + 1)
                .min(prev[j - 1] + cost);
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
// Legacy functions (not to be modified in this task)
// ---------------------------------------------------------------------------

pub fn delete_file(opts: DeleteFileOptions) -> BoxedFuture<Result<(), FilesystemError>> {
    let path = opts.path;
    Box::pin(async move {
        tokio::fs::remove_file(&path).await
            .map(|_| ())
            .map_err(FilesystemError)
    })
}

pub fn create_directory(opts: CreateDirectoryOptions) -> BoxedFuture<Result<(), FilesystemError>> {
    let path = opts.path;
    Box::pin(async move {
        tokio::fs::create_dir_all(&path).await
            .map(|_| ())
            .map_err(FilesystemError)
    })
}

pub fn list_directory(opts: ListDirectoryOptions) -> BoxedFuture<Result<Vec<String>, FilesystemError>> {
    let path = opts.path;
    Box::pin(async move {
        let mut entries = Vec::new();
        let mut reader = tokio::fs::read_dir(&path).await
            .map_err(FilesystemError)?;
        while let Some(entry) = reader.next_entry().await
            .map_err(FilesystemError)? {
            entries.push(entry.file_name().to_string_lossy().to_string());
        }
        entries.sort();
        Ok(entries)
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// -----------------------------------------------------------------------
    /// read_file tests
    /// -----------------------------------------------------------------------

    #[tokio::test]
    async fn read_file_nominal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "line1\nline2\nline3\nline4\nline5\n").await.unwrap();

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
        tokio::fs::write(&path, lines.join("\n") + "\n").await.unwrap();

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
        let lines: Vec<String> = (0..300)
            .map(|i| format!("line {}", i))
            .collect();
        tokio::fs::write(&path, lines.join("\n") + "\n").await.unwrap();

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
        tokio::fs::write(&path, "hello world\nfoo bar\n").await.unwrap();

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
        tokio::fs::write(&path, "hello world\nfoo bar\n").await.unwrap();

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
                assert!(hint.is_empty(), "hint should be empty for empty file, got: '{}'", hint);
            }
            other => panic!("Expected EditNoMatch, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn edit_file_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "apple\napple\napple\n").await.unwrap();

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
        tokio::fs::write(&path, "apple\napple\napple\n").await.unwrap();

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
}
