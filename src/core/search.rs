//! Provides logic for filtering and searching lists of `FileItem`s.

use super::scanner::MAX_FILE_SIZE;
use super::{FileItem, SearchFilter};
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
    /// This is the main entry point for applying dynamic UI filters (name, extension).
    /// Ignore patterns are handled during the initial scan by the `scanner` module.
    pub fn filter_files(files: &[FileItem], filter: &SearchFilter) -> Vec<FileItem> {
        files
            .par_iter()
            .filter(|file| Self::matches_filter(file, filter))
            .cloned()
            .collect()
    }

    /// Checks if a single `FileItem` matches the given filter criteria.
    pub fn matches_filter(file: &FileItem, filter: &SearchFilter) -> bool {
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
            extension_filter.is_empty() || extension_filter.eq_ignore_ascii_case("no extension")
        }
    }

    /// Recursively removes directories from a list that do not contain any files.
    ///
    /// This implementation is optimized to run in near-linear time O(N).
    /// It now takes a reference to `all_project_files` to correctly determine
    /// which directories are genuinely empty, regardless of any active filters.
    pub fn remove_empty_directories(
        files_to_filter: Vec<FileItem>,
        all_project_files: &[FileItem],
        dirs_to_preserve: &HashSet<PathBuf>,
    ) -> (Vec<FileItem>, HashSet<PathBuf>) {
        if files_to_filter.is_empty() {
            return (files_to_filter, HashSet::new());
        }

        // 1. Build a set of all non-directory paths from the complete file list.
        // This is the crucial reference to determine which directories are essential.
        let all_file_paths: HashSet<_> = all_project_files
            .iter()
            .filter(|item| !item.is_directory)
            .map(|item| item.path.clone())
            .collect();

        // 2. Build a set of all directories that are ancestors of at least one file.
        let mut essential_dirs = HashSet::new();
        for file_path in &all_file_paths {
            let mut current = file_path.parent();
            while let Some(parent) = current {
                // No need to check if parent is in a directory list; any parent of a file is essential.
                if essential_dirs.insert(parent.to_path_buf()) {
                    current = parent.parent();
                } else {
                    // Stop if we've already processed this parent path.
                    break;
                }
            }
        }

        // 3. Filter the original list: keep all files and all essential/preserved directories.
        let mut removed_dirs_set = HashSet::new();
        let final_list: Vec<FileItem> = files_to_filter
            .into_iter()
            .filter(|item| {
                if item.is_directory {
                    // Keep if it's essential (contains files somewhere in its subtree) OR
                    // if it's explicitly preserved (e.g., it's not fully loaded yet).
                    if essential_dirs.contains(&item.path) || dirs_to_preserve.contains(&item.path)
                    {
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

/// Searches the *contents* of files for a query string.
///
/// The query is normalised once on construction, so the same instance can be applied
/// to any number of files. All methods perform blocking file I/O; async callers must
/// run them on a blocking thread.
pub struct ContentSearch {
    /// The query, already lowercased if the search is case-insensitive.
    needle: String,
    case_sensitive: bool,
    max_file_size: u64,
}

impl ContentSearch {
    /// Creates a search for `query`, skipping files larger than the scanner's size limit.
    pub fn new(query: &str, case_sensitive: bool) -> Self {
        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        Self {
            needle,
            case_sensitive,
            max_file_size: MAX_FILE_SIZE,
        }
    }

    /// Overrides the size limit, so tests do not need to create huge files.
    #[cfg(test)]
    fn with_max_file_size(mut self, max_file_size: u64) -> Self {
        self.max_file_size = max_file_size;
        self
    }

    /// Returns the paths of all `files` whose content contains the query.
    ///
    /// Directories, binary files, files above the size limit and files that cannot be
    /// read as UTF-8 text are skipped silently.
    pub fn matching_paths(&self, files: &[FileItem]) -> HashSet<PathBuf> {
        files
            .par_iter()
            .filter(|item| self.matches_file(item))
            .map(|item| item.path.clone())
            .collect()
    }

    fn matches_file(&self, item: &FileItem) -> bool {
        if item.is_directory || item.is_binary || !self.is_within_size_limit(&item.path) {
            return false;
        }
        std::fs::read_to_string(&item.path)
            .map(|content| self.matches_content(&content))
            .unwrap_or(false)
    }

    /// Checks the size on disk rather than `FileItem::size`, which may be outdated.
    fn is_within_size_limit(&self, path: &Path) -> bool {
        std::fs::metadata(path)
            .map(|metadata| metadata.len() <= self.max_file_size)
            .unwrap_or(false)
    }

    fn matches_content(&self, content: &str) -> bool {
        if self.case_sensitive {
            content.contains(&self.needle)
        } else {
            content.to_lowercase().contains(&self.needle)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// Writes `content` to `name` inside `dir` and returns a matching text `FileItem`.
    fn write_text_file(dir: &Path, name: &str, content: &str) -> FileItem {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        FileItem {
            path,
            size: content.len() as u64,
            ..Default::default()
        }
    }

    #[test]
    fn test_content_search_case_sensitive_requires_exact_case() {
        let dir = tempdir().unwrap();
        let exact = write_text_file(dir.path(), "exact.txt", "find the MagicWord here");
        let other_case = write_text_file(dir.path(), "other.txt", "find the magicword here");
        let files = vec![exact.clone(), other_case];

        let result = ContentSearch::new("MagicWord", true).matching_paths(&files);

        assert_eq!(result, HashSet::from([exact.path]));
    }

    #[test]
    fn test_content_search_case_insensitive_ignores_case_of_query_and_content() {
        let dir = tempdir().unwrap();
        let upper = write_text_file(dir.path(), "upper.txt", "find the MAGICWORD here");
        let mixed = write_text_file(dir.path(), "mixed.txt", "find the MagicWord here");
        let miss = write_text_file(dir.path(), "miss.txt", "nothing to see");
        let files = vec![upper.clone(), mixed.clone(), miss];

        let result = ContentSearch::new("mAgIcWoRd", false).matching_paths(&files);

        assert_eq!(result, HashSet::from([upper.path, mixed.path]));
    }

    #[test]
    fn test_content_search_skips_files_above_size_limit() {
        let dir = tempdir().unwrap();
        let at_limit = write_text_file(dir.path(), "at_limit.txt", "needle 890");
        let too_large = write_text_file(dir.path(), "too_large.txt", "needle 8901");
        let files = vec![at_limit.clone(), too_large];

        let result = ContentSearch::new("needle", true)
            .with_max_file_size(10)
            .matching_paths(&files);

        assert_eq!(result, HashSet::from([at_limit.path]));
    }

    #[test]
    fn test_content_search_uses_scanner_size_limit_by_default() {
        let dir = tempdir().unwrap();
        let mut large = write_text_file(dir.path(), "large.txt", "needle");
        // Extend the file sparsely beyond the limit; it still starts with the query.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&large.path)
            .unwrap();
        file.set_len(MAX_FILE_SIZE + 1).unwrap();
        // A stale size from an earlier scan must not bypass the limit.
        large.size = 6;

        let result = ContentSearch::new("needle", true).matching_paths(&[large]);

        assert!(result.is_empty());
    }

    #[test]
    fn test_content_search_skips_binaries_and_directories() {
        let dir = tempdir().unwrap();
        let text = write_text_file(dir.path(), "text.txt", "needle");
        let binary = FileItem {
            is_binary: true,
            ..write_text_file(dir.path(), "data.bin", "needle")
        };
        let directory_path = dir.path().join("needle_dir");
        std::fs::create_dir(&directory_path).unwrap();
        let directory = FileItem {
            path: directory_path,
            is_directory: true,
            ..Default::default()
        };
        let files = vec![text.clone(), binary, directory];

        let result = ContentSearch::new("needle", false).matching_paths(&files);

        assert_eq!(result, HashSet::from([text.path]));
    }

    #[test]
    fn test_content_search_ignores_unreadable_files() {
        let dir = tempdir().unwrap();
        let missing = FileItem {
            path: dir.path().join("deleted_since_scan.txt"),
            ..Default::default()
        };
        let invalid_utf8_path = dir.path().join("latin1.txt");
        std::fs::write(&invalid_utf8_path, [b'n', 0xE9, 0xFF]).unwrap();
        let invalid_utf8 = FileItem {
            path: invalid_utf8_path,
            ..Default::default()
        };

        let result = ContentSearch::new("n", false).matching_paths(&[missing, invalid_utf8]);

        assert!(result.is_empty());
    }

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
        };
        let result = SearchEngine::filter_files(&files, &filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path.to_str(), Some("src/main.rs"));
    }

    #[test]
    fn test_filter_by_no_extension() {
        let files = vec![
            file("src/main.rs"),
            file("Makefile"),
            file(".config"),
            file("archive.tar.gz"),
        ];
        let filter = SearchFilter {
            query: String::new(),
            extension: "no extension".to_string(),
            case_sensitive: false,
        };

        let result = SearchEngine::filter_files(&files, &filter);
        let result_paths: std::collections::HashSet<_> =
            result.iter().map(|f| f.path.to_str().unwrap()).collect();

        assert_eq!(
            result.len(),
            2,
            "Should find the two files without the file ending"
        );
        assert!(result_paths.contains("Makefile"));
        assert!(result_paths.contains(".config"));
    }
}
