use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

use crate::error::ToolsError;
use crate::filesystem::file_not_found_hint;
use crate::output;

pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

// ---------------------------------------------------------------------------
// find_files
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct FindFilesOptions {
    pub pattern: String,
    /// Root directory. Empty = current directory (".").
    #[serde(default)]
    pub path: String,
}

pub fn find_files(opts: FindFilesOptions) -> BoxedFuture<Result<String, ToolsError>> {
    let pattern = opts.pattern.clone();
    let effective_path = if opts.path.is_empty() {
        ".".to_string()
    } else {
        opts.path.clone()
    };

    Box::pin(async move {
        // 1. Resolve path
        let meta = match tokio::fs::metadata(&effective_path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolsError::FileNotFound {
                    path: effective_path.clone(),
                    hint: file_not_found_hint(&effective_path),
                });
            }
            Err(e) => {
                return Err(ToolsError::Io {
                    path: effective_path.clone(),
                    source: e,
                })
            }
        };
        if !meta.is_dir() {
            return Err(ToolsError::NotADirectory(effective_path.clone()));
        }

        // 2. Compile glob pattern
        let glob_matcher = match globset::Glob::new(&pattern) {
            Ok(g) => {
                // compile_matcher() returns GlobMatcher directly (not a Result)
                g.compile_matcher()
            }
            Err(e) => {
                return Err(ToolsError::InvalidArgument {
                    name: "pattern".into(),
                    reason: e.to_string(),
                });
            }
        };

        // 3. Walk the tree
        let mut results = Vec::new();
        let max_entries = output::LIST_MAX_ENTRIES;
        let limit_reached = std::cell::Cell::new(false);

        let walker = ignore::WalkBuilder::new(&effective_path)
            .git_ignore(true)
            .git_global(false)
            .filter_entry(|entry| {
                // Always exclude .git, target, node_modules
                entry
                    .file_name()
                    .to_str()
                    .map(|name| name != ".git" && name != "target" && name != "node_modules")
                    .unwrap_or(true)
            })
            .build();

        for entry in walker {
            if limit_reached.get() {
                break;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Only process regular files
            if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                continue;
            }

            // Match against the glob
            let path = &effective_path;
            let relative = match entry.path().strip_prefix(path) {
                Ok(r) => r,
                Err(_) => continue,
            };

            if glob_matcher.is_match(relative) {
                results.push(relative.to_string_lossy().to_string());
                if results.len() >= max_entries {
                    limit_reached.set(true);
                    break;
                }
            }
        }

        results.sort();

        if results.is_empty() {
            Ok(format!(
                "no files matching '{}' under {}",
                pattern, effective_path
            ))
        } else {
            let mut line = results.join("\n");
            if limit_reached.get() {
                line.push_str(&format!(
                    "\n[truncated at {} matches — narrow the pattern or path]",
                    output::LIST_MAX_ENTRIES
                ));
            }
            Ok(line)
        }
    })
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SearchOptions {
    pub pattern: String,
    /// Root directory. Empty = current directory (".").
    #[serde(default)]
    pub path: String,
    /// Glob filter on files walked. Empty = all files.
    #[serde(default)]
    pub glob: String,
}

pub fn search(opts: SearchOptions) -> BoxedFuture<Result<String, ToolsError>> {
    let pattern = opts.pattern.clone();
    let effective_path = if opts.path.is_empty() {
        ".".to_string()
    } else {
        opts.path.clone()
    };
    let glob_filter = opts.glob.clone();

    Box::pin(async move {
        // 1. Resolve path
        let meta = match tokio::fs::metadata(&effective_path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolsError::FileNotFound {
                    path: effective_path.clone(),
                    hint: file_not_found_hint(&effective_path),
                });
            }
            Err(e) => {
                return Err(ToolsError::Io {
                    path: effective_path.clone(),
                    source: e,
                })
            }
        };
        if !meta.is_dir() {
            return Err(ToolsError::NotADirectory(effective_path.clone()));
        }

        // 2. Compile regex pattern
        let regex = match regex::Regex::new(&pattern) {
            Ok(r) => r,
            Err(e) => {
                return Err(ToolsError::InvalidArgument {
                    name: "pattern".into(),
                    reason: e.to_string(),
                });
            }
        };

        // 3. Compile optional glob filter
        let glob_matcher: Option<globset::GlobMatcher> = if !glob_filter.is_empty() {
            let g = match globset::Glob::new(&glob_filter) {
                Ok(g) => g,
                Err(e) => {
                    return Err(ToolsError::InvalidArgument {
                        name: "glob".into(),
                        reason: e.to_string(),
                    });
                }
            };
            // compile_matcher() returns GlobMatcher directly
            Some(g.compile_matcher())
        } else {
            None
        };

        // 4. Walk the tree
        let max_matches = output::SEARCH_MAX_MATCHES;
        let mut results = Vec::new();
        let limit_reached = std::cell::Cell::new(false);

        let walker = ignore::WalkBuilder::new(&effective_path)
            .git_ignore(true)
            .git_global(false)
            .filter_entry(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .map(|name| name != ".git" && name != "target" && name != "node_modules")
                    .unwrap_or(true)
            })
            .build();

        for entry in walker {
            if limit_reached.get() {
                break;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Only process regular files
            if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                continue;
            }

            // Apply glob filter if provided
            if let Some(ref matcher) = glob_matcher {
                let rel = match entry.path().strip_prefix(&effective_path) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if !matcher.is_match(rel) {
                    continue;
                }
            }

            // Read file; skip binary (non-UTF8) silently
            let content = match tokio::fs::read_to_string(entry.path()).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            let rel = entry
                .path()
                .strip_prefix(&effective_path)
                .unwrap_or(entry.path());
            let rel_str = rel.to_string_lossy().to_string();

            // Search line by line
            for (line_number, line) in content.lines().enumerate() {
                if results.len() >= max_matches {
                    limit_reached.set(true);
                    break;
                }
                if regex.is_match(line) {
                    results.push(format!("{}:{}: {}", rel_str, line_number + 1, line));
                }
            }
        }

        if results.is_empty() {
            Ok(format!(
                "no matches for '{}' under {}",
                pattern, effective_path
            ))
        } else {
            let mut line = results.join("\n");
            if limit_reached.get() {
                line.push_str(&format!(
                    "\n[truncated at {} matches — narrow the pattern, path, or glob]",
                    output::SEARCH_MAX_MATCHES
                ));
            }
            Ok(line)
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

    // -----------------------------------------------------------------------
    // find_files tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn find_files_nominal() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        tokio::fs::create_dir_all(&src).await.unwrap();
        tokio::fs::write(src.join("main.rs"), "fn main() {}")
            .await
            .unwrap();
        tokio::fs::write(src.join("lib.rs"), "pub fn lib() {}")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("README.md"), "# Project")
            .await
            .unwrap();

        let result = find_files(FindFilesOptions {
            pattern: "**/*.rs".into(),
            path: dir.path().to_string_lossy().to_string(),
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        // Sorted: lib.rs before main.rs
        assert!(lines[0].ends_with("lib.rs"));
        assert!(lines[1].ends_with("main.rs"));
    }

    #[tokio::test]
    async fn find_files_ignores_git_target() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("main.rs");
        let git_file = dir.path().join(".git").join("config");
        let target_file = dir.path().join("target").join("out.o");
        tokio::fs::create_dir_all(dir.path().join(".git"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(dir.path().join("target"))
            .await
            .unwrap();
        tokio::fs::write(&git_file, "[core]").await.unwrap();
        tokio::fs::write(&target_file, "binary").await.unwrap();
        tokio::fs::write(&rs_file, "fn main() {}").await.unwrap();

        let result = find_files(FindFilesOptions {
            pattern: "**/*.rs".into(),
            path: dir.path().to_string_lossy().to_string(),
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("main.rs"));
    }

    #[tokio::test]
    async fn find_files_no_match() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("README.md"), "# Project")
            .await
            .unwrap();

        let result = find_files(FindFilesOptions {
            pattern: "**/*.rs".into(),
            path: dir.path().to_string_lossy().to_string(),
        })
        .await
        .unwrap();

        assert!(result.contains("no files matching"));
    }

    #[tokio::test]
    async fn find_files_invalid_pattern() {
        let dir = tempfile::tempdir().unwrap();

        let result = find_files(FindFilesOptions {
            pattern: "[".into(),
            path: dir.path().to_string_lossy().to_string(),
        })
        .await;

        match result {
            Err(ToolsError::InvalidArgument { name, .. }) => {
                assert_eq!(name, "pattern");
            }
            other => panic!("Expected InvalidArgument, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn find_files_not_found() {
        let result = find_files(FindFilesOptions {
            pattern: "**/*.rs".into(),
            path: "/nonexistent/path/xyz".into(),
        })
        .await;

        match result {
            Err(ToolsError::FileNotFound { path: ref p, .. }) => {
                assert!(p.contains("nonexistent"));
            }
            other => panic!("Expected FileNotFound, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // search tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn search_nominal() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        tokio::fs::write(&file, "fn foo() {}\nfn bar() {}\nfn baz() {}\n")
            .await
            .unwrap();

        let result = search(SearchOptions {
            pattern: "fn \\w+".into(),
            path: dir.path().to_string_lossy().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.lines().collect();
        assert!(lines.len() >= 2);
        for line in lines {
            assert!(line.starts_with("test.rs:"));
        }
    }

    #[tokio::test]
    async fn search_glob_filter() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("test.rs");
        let md_file = dir.path().join("test.md");
        tokio::fs::write(&rs_file, "fn foo() {}\n").await.unwrap();
        tokio::fs::write(&md_file, "fn foo() {}\n").await.unwrap();

        let result = search(SearchOptions {
            pattern: "fn \\w+".into(),
            path: dir.path().to_string_lossy().to_string(),
            glob: "*.rs".into(),
        })
        .await
        .unwrap();

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("test.rs"));
        assert!(!lines[0].contains("test.md"));
    }

    #[tokio::test]
    async fn search_skips_binary() {
        let dir = tempfile::tempdir().unwrap();
        // Binary file with invalid UTF-8
        std::fs::write(dir.path().join("binary.bin"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();
        // A non-empty text file with no match
        tokio::fs::write(dir.path().join("text.txt"), "hello world\n")
            .await
            .unwrap();

        let result = search(SearchOptions {
            pattern: "fn \\w+".into(),
            path: dir.path().to_string_lossy().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();

        // Should not contain binary, only "no matches" since text.txt has no match
        assert!(!result.contains("binary"));
    }

    #[tokio::test]
    async fn search_no_match() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("test.txt"), "hello world\n")
            .await
            .unwrap();

        let result = search(SearchOptions {
            pattern: "fn \\w+".into(),
            path: dir.path().to_string_lossy().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();

        assert!(result.contains("no matches for"));
    }

    #[tokio::test]
    async fn search_invalid_regex() {
        let dir = tempfile::tempdir().unwrap();

        let result = search(SearchOptions {
            pattern: "(".into(),
            path: dir.path().to_string_lossy().to_string(),
            ..Default::default()
        })
        .await;

        match result {
            Err(ToolsError::InvalidArgument { name, .. }) => {
                assert_eq!(name, "pattern");
            }
            other => panic!("Expected InvalidArgument, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn search_invalid_glob() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("test.txt"), "foo\n")
            .await
            .unwrap();

        let result = search(SearchOptions {
            pattern: "foo".into(),
            path: dir.path().to_string_lossy().to_string(),
            glob: "[".into(),
        })
        .await;

        match result {
            Err(ToolsError::InvalidArgument { name, .. }) => {
                assert_eq!(name, "glob");
            }
            other => panic!("Expected InvalidArgument, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn search_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.txt");
        let content: String = (0..200)
            .map(|i| format!("fn foo{}() {{}}", i))
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(&file, content).await.unwrap();

        let result = search(SearchOptions {
            pattern: "fn foo".into(),
            path: dir.path().to_string_lossy().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();

        assert!(result.contains("truncated at 50 matches"));
    }
}
