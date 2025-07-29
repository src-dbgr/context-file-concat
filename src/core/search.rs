//! Provides logic for filtering and searching lists of `FileItem`s.

use super::{build_globset_from_patterns, FileItem, SearchFilter};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A utility struct for searching and filtering file lists.
///
/// This struct is stateless and provides methods as associated functions.
pub struct SearchEngine;

impl SearchEngine {
    /// Filters a slice of `FileItem`s based on the provided `SearchFilter`.
    ///
    /// This is the main entry point for applying all filters (name, extension, ignore patterns).
    pub fn filter_files(files: &[FileItem], filter: &SearchFilter) -> Vec<FileItem> {
        let (ignore_glob_set, _) = build_globset_from_patterns(&filter.ignore_patterns);

        files
            .par_iter()
            .filter(|file| Self::matches_filter(file, filter, &ignore_glob_set))
            .cloned()
            .collect()
    }

    /// Checks if a single `FileItem` matches the given filter criteria.
    fn matches_filter(
        file: &FileItem,
        filter: &SearchFilter,
        ignore_glob_set: &globset::GlobSet,
    ) -> bool {
        // This check enables real-time filtering in the UI based on ignore patterns.
        if file.path.components().any(|c| c.as_os_str() == ".git")
            || ignore_glob_set.is_match(&file.path)
        {
            return false;
        }

        if !filter.query.is_empty()
            && !Self::matches_search_query(&file.path, &filter.query, filter.case_sensitive)
        {
            return false;
        }

        if !filter.extension.is_empty() && !Self::matches_extension(&file.path, &filter.extension) {
            return false;
        }

        true
    }

    /// Checks if a path's filename contains the search query.
    fn matches_search_query(path: &Path, query: &str, case_sensitive: bool) -> bool {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");

        if case_sensitive {
            file_name.contains(query)
        } else {
            let query_lower = query.to_lowercase();
            file_name.to_lowercase().contains(&query_lower)
        }
    }

    /// Checks if a path's extension matches the extension filter.
    fn matches_extension(path: &Path, extension_filter: &str) -> bool {
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            let filter = extension_filter
                .strip_prefix('.')
                .unwrap_or(extension_filter);
            ext.eq_ignore_ascii_case(filter)
        } else {
            extension_filter.is_empty() || extension_filter == "no extension"
        }
    }

    /// Recursively removes directories from a list that do not contain any files.
    ///
    /// Returns the pruned list of `FileItem`s and a set of the paths that were removed.
    /// Recursively removes directories from a list that do not contain any files.
    ///
    /// This implementation is optimized to run in near-linear time O(N) by using HashSets
    /// to avoid nested loops, which would result in O(N^2) complexity.
    pub fn remove_empty_directories(files: Vec<FileItem>) -> (Vec<FileItem>, HashSet<PathBuf>) {
        if files.is_empty() {
            return (files, HashSet::new());
        }

        // 1. Collect all directories and files into separate sets for efficient lookup.
        let mut directories = HashSet::new();
        let mut file_paths = HashSet::new();
        for item in &files {
            if item.is_directory {
                directories.insert(item.path.clone());
            } else {
                file_paths.insert(item.path.clone());
            }
        }

        // 2. Build a set of all directories that are ancestors of at least one file.
        let mut essential_dirs = HashSet::new();
        for file_path in &file_paths {
            let mut current = file_path.parent();
            while let Some(parent) = current {
                if directories.contains(parent) {
                    essential_dirs.insert(parent.to_path_buf());
                    current = parent.parent();
                } else {
                    // Stop if we go above the known directory structure
                    break;
                }
            }
        }

        // 3. Filter the original list: keep all files and all essential directories.
        let mut removed_dirs_set = HashSet::new();
        let final_list: Vec<FileItem> = files
            .into_iter()
            .filter(|item| {
                if item.is_directory {
                    if essential_dirs.contains(&item.path) {
                        true // Keep this directory
                    } else {
                        removed_dirs_set.insert(item.path.clone());
                        false // Remove this directory
                    }
                } else {
                    true // Always keep files
                }
            })
            .collect();

        (final_list, removed_dirs_set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn file(path: &str) -> FileItem {
        FileItem {
            path: PathBuf::from(path),
            is_directory: false,
            is_binary: false,
            size: 100,
            depth: path.split('/').count(),
            parent: PathBuf::from(path).parent().map(|p| p.to_path_buf()),
        }
    }

    fn dir(path: &str) -> FileItem {
        FileItem {
            path: PathBuf::from(path),
            is_directory: true,
            is_binary: false,
            size: 0,
            depth: path.split('/').count(),
            parent: PathBuf::from(path).parent().map(|p| p.to_path_buf()),
        }
    }

    fn create_test_files() -> Vec<FileItem> {
        vec![
            dir("src"),
            file("src/main.rs"),
            file("src/lib.rs"),
            dir("src/module"),
            file("src/module/component.rs"),
            dir("docs"),
            file("docs/README.md"),
            file("README.md"),
            dir("target"),
            file("target/app.exe"),
        ]
    }

    #[test]
    fn test_filter_by_name_case_sensitive() {
        let files = create_test_files();
        let filter = SearchFilter {
            query: "README".to_string(),
            extension: String::new(),
            case_sensitive: true,
            ignore_patterns: HashSet::new(),
        };
        let result = SearchEngine::filter_files(&files, &filter);
        assert_eq!(result.len(), 2);
        assert!(result
            .iter()
            .any(|f| f.path.to_str() == Some("docs/README.md")));
        assert!(result.iter().any(|f| f.path.to_str() == Some("README.md")));
    }

    #[test]
    fn test_filter_by_name_case_insensitive() {
        let files = create_test_files();
        let filter = SearchFilter {
            query: "readme".to_string(),
            extension: String::new(),
            case_sensitive: false,
            ignore_patterns: HashSet::new(),
        };
        let result = SearchEngine::filter_files(&files, &filter);
        assert_eq!(result.len(), 2);
        assert!(result
            .iter()
            .any(|f| f.path.to_str() == Some("docs/README.md")));
        assert!(result.iter().any(|f| f.path.to_str() == Some("README.md")));
    }

    #[test]
    fn test_filter_by_extension() {
        let files = create_test_files();
        let filter = SearchFilter {
            query: String::new(),
            extension: "rs".to_string(),
            case_sensitive: false,
            ignore_patterns: HashSet::new(),
        };
        let result = SearchEngine::filter_files(&files, &filter);
        assert_eq!(result.len(), 3);
        assert!(result
            .iter()
            .any(|f| f.path.to_str() == Some("src/main.rs")));
        assert!(result.iter().any(|f| f.path.to_str() == Some("src/lib.rs")));
        assert!(result
            .iter()
            .any(|f| f.path.to_str() == Some("src/module/component.rs")));
    }

    #[test]
    fn test_filter_by_name_and_extension() {
        let files = create_test_files();
        let filter = SearchFilter {
            query: "main".to_string(),
            extension: "rs".to_string(),
            case_sensitive: false,
            ignore_patterns: HashSet::new(),
        };
        let result = SearchEngine::filter_files(&files, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path.to_str(), Some("src/main.rs"));
    }

    #[test]
    fn test_filter_with_ignore_patterns() {
        let files = create_test_files();
        let mut ignore_patterns = HashSet::new();
        ignore_patterns.insert("target/".to_string());
        ignore_patterns.insert("*.md".to_string());

        let filter = SearchFilter {
            query: String::new(),
            extension: String::new(),
            case_sensitive: false,
            ignore_patterns,
        };

        let result = SearchEngine::filter_files(&files, &filter);
        let paths: Vec<_> = result.iter().map(|f| f.path.to_str().unwrap()).collect();

        assert!(!paths.contains(&"docs/README.md"));
        assert!(!paths.contains(&"README.md"));
        assert!(!paths.contains(&"target/app.exe"));
        assert!(!paths.contains(&"target"));
        assert!(paths.contains(&"src/main.rs"));

        assert_eq!(result.len(), 6, "Expected 6 items to remain: src, src/main.rs, src/lib.rs, src/module, src/module/component.rs, and docs");
    }
}
