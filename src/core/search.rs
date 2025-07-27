use super::{build_globset_from_patterns, FileItem, SearchFilter};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf}; // MODIFIED

pub struct SearchEngine;

impl SearchEngine {
    // Stellt die Logik zum Filtern nach Ignore-Patterns wieder her.
    pub fn filter_files(files: &[FileItem], filter: &SearchFilter) -> Vec<FileItem> {
        let (ignore_glob_set, _) = build_globset_from_patterns(&filter.ignore_patterns);

        files
            .par_iter()
            .filter(|file| Self::matches_filter(file, filter, &ignore_glob_set))
            .cloned()
            .collect()
    }

    // Berücksichtigt wieder das ignore_glob_set.
    fn matches_filter(
        file: &FileItem,
        filter: &SearchFilter,
        ignore_glob_set: &globset::GlobSet,
    ) -> bool {
        // Die Prüfung ist zurück, um die UI-Filterung in Echtzeit zu ermöglichen.
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

    // Die restlichen Methoden bleiben unverändert.
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

    fn matches_extension(path: &Path, extension_filter: &str) -> bool {
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            let filter = if extension_filter.starts_with('.') {
                &extension_filter[1..]
            } else {
                extension_filter
            };

            ext.eq_ignore_ascii_case(filter)
        } else {
            extension_filter.is_empty() || extension_filter == "no extension"
        }
    }

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
                    // Füge die in dieser Runde entfernten Verzeichnisse zum Gesamtset hinzu.
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
        assert!(!paths.contains(&"target")); // Das Verzeichnis selbst sollte auch weg sein
        assert!(paths.contains(&"src/main.rs"));

        // KORREKTE ERWARTUNG: 6 Elemente bleiben übrig (inkl. dem 'docs' Verzeichnis)
        assert_eq!(result.len(), 6, "Expected 6 items to remain: src, src/main.rs, src/lib.rs, src/module, src/module/component.rs, and docs");
    }
}
