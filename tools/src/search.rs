use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

use crate::error::ToolsError;
use crate::filesystem::file_not_found_hint;
use crate::output;

/// Future boxed `Send` — type d'effacement des outils asynchrones du crate.
pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

// ---------------------------------------------------------------------------
// find_files
// ---------------------------------------------------------------------------

/// Construit un promeneur d'arbre avec les exclusions fixes (`.git`,
/// `target`, `node_modules`), partagé par `find_files` et `search`.
fn tree_walker(root: &str) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .git_ignore(true)
        .git_global(false)
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| name != ".git" && name != "target" && name != "node_modules")
        })
        .build()
}

/// Options de `find_files`.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct FindFilesOptions {
    /// Pattern glob appliqué aux chemins relatifs à `path`.
    pub pattern: String,
    /// Root directory. Empty = current directory (".").
    #[serde(default)]
    pub path: String,
}

/// Cherche les fichiers dont le chemin relatif correspond au glob
/// `opts.pattern` sous `opts.path`, liste triée et bornée.
pub fn find_files(opts: FindFilesOptions) -> BoxedFuture<Result<String, ToolsError>> {
    let pattern = opts.pattern.clone();
    let effective_path = if opts.path.is_empty() {
        ".".to_string()
    } else {
        opts.path
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
                });
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

        let walker = tree_walker(&effective_path);

        for entry in walker {
            if limit_reached.get() {
                break;
            }

            let Ok(entry) = entry else { continue };

            // Only process regular files
            if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                continue;
            }

            // Match against the glob
            let path = &effective_path;
            let Ok(relative) = entry.path().strip_prefix(path) else {
                continue;
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
                "no files matching '{pattern}' under {effective_path}"
            ))
        } else {
            let mut line = results.join("\n");
            if limit_reached.get() {
                use std::fmt::Write as _;
                // write! sur String ne peut pas échouer.
                let _ = write!(
                    line,
                    "\n[truncated at {} matches — narrow the pattern or path]",
                    output::LIST_MAX_ENTRIES
                );
            }
            Ok(line)
        }
    })
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

/// Options de `search`.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SearchOptions {
    /// Motif regex recherché, par ligne.
    pub pattern: String,
    /// Root directory. Empty = current directory (".").
    #[serde(default)]
    pub path: String,
    /// Glob filter on files walked. Empty = all files.
    #[serde(default)]
    pub glob: String,
}

/// Compile le filtre glob optionnel de `search` : `None` si vide,
/// `InvalidArgument("glob")` si le motif est invalide.
fn compile_glob_filter(glob: &str) -> Result<Option<globset::GlobMatcher>, ToolsError> {
    if glob.is_empty() {
        return Ok(None);
    }
    let g = globset::Glob::new(glob).map_err(|e| ToolsError::InvalidArgument {
        name: "glob".into(),
        reason: e.to_string(),
    })?;
    // compile_matcher() returns GlobMatcher directly
    Ok(Some(g.compile_matcher()))
}

/// Cherche le motif regex `opts.pattern` dans les fichiers sous `opts.path`
/// (filtrés par `opts.glob`) et retourne les lignes `fichier:ligne: contenu`,
/// bornées à `output::SEARCH_MAX_MATCHES`.
pub fn search(opts: SearchOptions) -> BoxedFuture<Result<String, ToolsError>> {
    let pattern = opts.pattern.clone();
    let effective_path = if opts.path.is_empty() {
        ".".to_string()
    } else {
        opts.path.clone()
    };
    let glob_filter = opts.glob;

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
                });
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
        let glob_matcher = compile_glob_filter(&glob_filter)?;

        // 4. Walk the tree
        let max_matches = output::SEARCH_MAX_MATCHES;
        let mut results = Vec::new();
        let limit_reached = std::cell::Cell::new(false);

        let walker = tree_walker(&effective_path);

        for entry in walker {
            if limit_reached.get() {
                break;
            }

            let Ok(entry) = entry else { continue };

            // Only process regular files
            if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                continue;
            }

            // Apply glob filter if provided
            if let Some(ref matcher) = glob_matcher {
                let Ok(rel) = entry.path().strip_prefix(&effective_path) else {
                    continue;
                };
                if !matcher.is_match(rel) {
                    continue;
                }
            }

            // Read file; skip binary (non-UTF8) silently
            let Ok(content) = tokio::fs::read_to_string(entry.path()).await else {
                continue;
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
                    results.push(format!(
                        "{}:{}: {}",
                        rel_str,
                        line_number.saturating_add(1),
                        line
                    ));
                }
            }
        }

        if results.is_empty() {
            Ok(format!("no matches for '{pattern}' under {effective_path}"))
        } else {
            let mut line = results.join("\n");
            if limit_reached.get() {
                use std::fmt::Write as _;
                // write! sur String ne peut pas échouer.
                let _ = write!(
                    line,
                    "\n[truncated at {} matches — narrow the pattern, path, or glob]",
                    output::SEARCH_MAX_MATCHES
                );
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
            other => panic!("Expected InvalidArgument, got: {other:?}"),
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
            other => panic!("Expected FileNotFound, got: {other:?}"),
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
            other => panic!("Expected InvalidArgument, got: {other:?}"),
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
            other => panic!("Expected InvalidArgument, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.txt");
        let content: String = (0..200)
            .map(|i| format!("fn foo{i}() {{}}"))
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

    // -----------------------------------------------------------------------
    // Scenarios de search.sdd (un test par titre de Scenario)
    // -----------------------------------------------------------------------

    use crate::command::CWD_LOCK;

    fn dir_str(dir: &tempfile::TempDir) -> String {
        dir.path().to_string_lossy().to_string()
    }

    fn find_opts(pattern: &str, path: String) -> FindFilesOptions {
        FindFilesOptions {
            pattern: pattern.into(),
            path,
        }
    }

    fn search_opts(pattern: &str, path: String, glob: &str) -> SearchOptions {
        SearchOptions {
            pattern: pattern.into(),
            path,
            glob: glob.into(),
        }
    }

    #[tokio::test]
    async fn find_files_chemins_relatifs_tries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();

        let result = find_files(find_opts("**/*.rs", dir_str(&dir)))
            .await
            .unwrap();
        assert_eq!(result, "src/lib.rs\nsrc/main.rs");
    }

    #[tokio::test]
    async fn find_files_path_vide_signifie_repertoire_courant() {
        let _guard = CWD_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("seulement.rs"), "").unwrap();

        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = find_files(find_opts("*.rs", String::new())).await;
        std::env::set_current_dir(&previous).unwrap();

        assert_eq!(result.unwrap(), "seulement.rs");
    }

    #[tokio::test]
    async fn find_files_aucun_resultat_message_exact() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        let path = dir_str(&dir);

        let result = find_files(find_opts("**/*.rs", path.clone()))
            .await
            .unwrap();
        assert_eq!(result, format!("no files matching '**/*.rs' under {path}"));
    }

    #[tokio::test]
    async fn find_files_exclut_git_target_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        std::fs::write(dir.path().join(".git/config"), "").unwrap();
        std::fs::write(dir.path().join("target/out.o"), "").unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/index.js"), "").unwrap();

        let result = find_files(find_opts("**/*", dir_str(&dir))).await.unwrap();
        assert_eq!(result, "main.rs");
    }

    #[tokio::test]
    async fn find_files_gitignore_respecte_dans_depot_git() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignoree.txt\n").unwrap();
        std::fs::write(dir.path().join("ignoree.txt"), "").unwrap();
        std::fs::write(dir.path().join("gardee.txt"), "").unwrap();

        let result = find_files(find_opts("*.txt", dir_str(&dir))).await.unwrap();
        assert_eq!(result, "gardee.txt");
    }

    #[tokio::test]
    async fn find_files_hors_depot_git_le_gitignore_ne_filtre_rien() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignoree.txt\n").unwrap();
        std::fs::write(dir.path().join("ignoree.txt"), "").unwrap();
        std::fs::write(dir.path().join("gardee.txt"), "").unwrap();

        let result = find_files(find_opts("*.txt", dir_str(&dir))).await.unwrap();
        assert_eq!(result, "gardee.txt\nignoree.txt");
    }

    #[tokio::test]
    async fn find_files_patron_glob_invalide() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_files(find_opts("[", dir_str(&dir))).await.unwrap_err();
        match err {
            ToolsError::InvalidArgument { name, reason } => {
                assert_eq!(name, "pattern");
                assert!(!reason.is_empty());
            }
            other => panic!("InvalidArgument attendu, obtenu : {other:?}"),
        }
    }

    #[tokio::test]
    async fn find_files_chemin_inexistant_avec_hinte() {
        let path = "/nonexistent/path/xyz".to_string();
        let err = find_files(find_opts("**/*.rs", path.clone()))
            .await
            .unwrap_err();
        match err {
            ToolsError::FileNotFound { path: p, hint } => {
                assert_eq!(p, path);
                assert!(hint.contains("parent directory does not exist either"));
            }
            other => panic!("FileNotFound attendu, obtenu : {other:?}"),
        }
    }

    #[tokio::test]
    async fn find_files_chemin_non_repertoire() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("versus.txt");
        std::fs::write(&file, "").unwrap();
        let path = file.to_string_lossy().to_string();

        let err = find_files(find_opts("**/*", path.clone()))
            .await
            .unwrap_err();
        match err {
            ToolsError::NotADirectory(p) => assert_eq!(p, path),
            other => panic!("NotADirectory attendu, obtenu : {other:?}"),
        }
    }

    #[tokio::test]
    async fn find_files_chemin_invalide_prioritaire_sur_patron_invalide() {
        let err = find_files(find_opts("[", "/nonexistent/path/xyz".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolsError::FileNotFound { .. }));
    }

    #[tokio::test]
    async fn find_files_tronque_a_200_entrees() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=250 {
            std::fs::write(dir.path().join(format!("fichier-{i:03}.txt")), "").unwrap();
        }

        let result = find_files(find_opts("*.txt", dir_str(&dir))).await.unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 201);
        assert_eq!(
            lines[200],
            "[truncated at 200 matches — narrow the pattern or path]"
        );
        let kept = &lines[..200];
        let mut sorted = kept.to_vec();
        sorted.sort_unstable();
        assert_eq!(kept, sorted.as_slice());
    }

    #[tokio::test]
    async fn find_files_symlink_non_retenu() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("reel.rs"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("reel.rs"), dir.path().join("lien.rs")).unwrap();

        let result = find_files(find_opts("*.rs", dir_str(&dir))).await.unwrap();
        assert_eq!(result, "reel.rs");
    }

    #[tokio::test]
    async fn search_format_de_ligne_exact() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();

        let result = search(search_opts("fn \\w+", dir_str(&dir), ""))
            .await
            .unwrap();
        assert_eq!(result, "test.rs:1: fn foo() {}\ntest.rs:2: fn bar() {}");
    }

    #[tokio::test]
    async fn search_numerotation_une_based() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "alpha\nbravo\naiguille\n").unwrap();

        let result = search(search_opts("aiguille", dir_str(&dir), ""))
            .await
            .unwrap();
        assert_eq!(result, "notes.txt:3: aiguille");
    }

    #[tokio::test]
    async fn search_aucune_correspondance_message_exact() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();
        let path = dir_str(&dir);

        let result = search(search_opts("fn \\w+", path.clone(), ""))
            .await
            .unwrap();
        assert_eq!(result, format!("no matches for 'fn \\w+' under {path}"));
    }

    #[tokio::test]
    async fn search_regex_invalide_nomme_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let err = search(search_opts("(", dir_str(&dir), ""))
            .await
            .unwrap_err();
        match err {
            ToolsError::InvalidArgument { name, reason } => {
                assert_eq!(name, "pattern");
                assert!(!reason.is_empty());
            }
            other => panic!("InvalidArgument attendu, obtenu : {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_glob_invalide_nomme_glob() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "").unwrap();
        let err = search(search_opts("foo", dir_str(&dir), "["))
            .await
            .unwrap_err();
        match err {
            ToolsError::InvalidArgument { name, reason } => {
                assert_eq!(name, "glob");
                assert!(!reason.is_empty());
            }
            other => panic!("InvalidArgument attendu, obtenu : {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_regex_invalide_prioritaire_sur_glob_invalide() {
        let dir = tempfile::tempdir().unwrap();
        let err = search(search_opts("(", dir_str(&dir), "["))
            .await
            .unwrap_err();
        match err {
            ToolsError::InvalidArgument { name, .. } => assert_eq!(name, "pattern"),
            other => panic!("InvalidArgument attendu, obtenu : {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_chemin_invalide_prioritaire_sur_regex_invalide() {
        let err = search(search_opts("(", "/nonexistent/path/xyz".into(), ""))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolsError::FileNotFound { .. }));
    }

    #[tokio::test]
    async fn search_binaire_non_utf8_ignore_silencieusement() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("binaire.bin"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();
        std::fs::write(dir.path().join("texte.txt"), "hello world").unwrap();
        let path = dir_str(&dir);

        let result = search(search_opts("fn \\w+", path.clone(), ""))
            .await
            .unwrap();
        assert_eq!(result, format!("no matches for 'fn \\w+' under {path}"));
        assert!(!result.contains("binaire.bin"));
    }

    #[tokio::test]
    async fn search_sensible_a_la_casse() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("maj.txt"), "AIGUILLE\n").unwrap();
        let path = dir_str(&dir);

        let result = search(search_opts("aiguille", path.clone(), ""))
            .await
            .unwrap();
        assert_eq!(result, format!("no matches for 'aiguille' under {path}"));
    }

    #[tokio::test]
    async fn search_ne_traverse_pas_les_fin_de_ligne() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("coupe.txt"), "debut\nfin\n").unwrap();
        let path = dir_str(&dir);

        let result = search(search_opts("debut\\s+fin", path.clone(), ""))
            .await
            .unwrap();
        assert_eq!(
            result,
            format!("no matches for 'debut\\s+fin' under {path}")
        );
    }

    #[tokio::test]
    async fn search_tronque_a_50_correspondances() {
        let dir = tempfile::tempdir().unwrap();
        let content: String = (0..200)
            .map(|i| format!("fn foo{i}() {{}}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.path().join("big.txt"), content).unwrap();

        let result = search(search_opts("fn foo", dir_str(&dir), ""))
            .await
            .unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 51);
        assert_eq!(
            lines[50],
            "[truncated at 50 matches — narrow the pattern, path, or glob]"
        );
    }

    #[tokio::test]
    async fn search_chemin_non_repertoire() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("versus.txt");
        std::fs::write(&file, "x").unwrap();
        let path = file.to_string_lossy().to_string();

        let err = search(search_opts("x", path.clone(), ""))
            .await
            .unwrap_err();
        match err {
            ToolsError::NotADirectory(p) => assert_eq!(p, path),
            other => panic!("NotADirectory attendu, obtenu : {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_exclut_git_target_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/x")).unwrap();
        std::fs::write(dir.path().join("racine.txt"), "aiguille\n").unwrap();
        std::fs::write(dir.path().join(".git/config"), "aiguille\n").unwrap();
        std::fs::write(dir.path().join("target/out.txt"), "aiguille\n").unwrap();
        std::fs::write(dir.path().join("node_modules/x/index.js"), "aiguille\n").unwrap();

        let result = search(search_opts("aiguille", dir_str(&dir), ""))
            .await
            .unwrap();
        assert_eq!(result, "racine.txt:1: aiguille");
    }
}
