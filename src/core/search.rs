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
    pub fn remove_empty_directories(mut files: Vec<FileItem>) -> (Vec<FileItem>, HashSet<PathBuf>) {
        let mut has_changes = true;
        let mut all_removed_dirs = HashSet::new();

        while has_changes {
            has_changes = false;
            let files_before_len = files.len();

            let mut dirs_to_remove = Vec::new();

            for item in &files {
                if item.is_directory {
                    let has_children = files
                        .iter()
                        .any(|other| other.path != item.path && other.path.starts_with(&item.path));

                    if !has_children {
                        dirs_to_remove.push(item.path.clone());
                    }
                }
            }

            if !dirs_to_remove.is_empty() {
                files.retain(|item| !dirs_to_remove.contains(&item.path));
                if files.len() != files_before_len {
                    has_changes = true;
                    // Add the directories removed in this round to the total set.
                    all_removed_dirs.extend(dirs_to_remove);
                }
            }
        }

        (files, all_removed_dirs)
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
