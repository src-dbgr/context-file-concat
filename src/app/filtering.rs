//! This module is responsible for mutating the application state by applying filters.
//!
//! It takes the `AppState` and modifies the `filtered_file_list` based on various
//! criteria like search queries, extension filters, and content search results.
//! This cleanly separates the logic of state mutation from the logic of state presentation.

use crate::app::state::AppState;
use crate::config::AppConfig;
use crate::core::{FileItem, SearchEngine, SearchFilter};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Applies all current filters to the full file list to generate the visible list.
pub fn apply_filters(state: &mut AppState) {
    let all_dirs_in_full_list: HashSet<PathBuf> = state
        .full_file_list
        .iter()
        .filter(|i| i.is_directory)
        .map(|i| i.path.clone())
        .collect();

    let unloaded_dirs: HashSet<PathBuf> = all_dirs_in_full_list
        .difference(&state.loaded_dirs)
        .cloned()
        .collect();

    let mut dirs_to_preserve = unloaded_dirs;
    if !state.config.remove_empty_directories {
        dirs_to_preserve.extend(state.expanded_dirs.clone());
    }

    state.filtered_file_list = apply_filters_on_data(
        &state.full_file_list,
        &PathBuf::from(&state.current_path),
        &state.config,
        &state.search_query,
        &state.extension_filter,
        &state.content_search_query,
        &state.content_search_results,
        &dirs_to_preserve,
        state.is_fully_scanned,
    );
}

/// Collects all parent directories for a given set of file paths.
fn get_required_ancestors(file_paths: &HashSet<PathBuf>, root_path: &Path) -> HashSet<PathBuf> {
    file_paths
        .par_iter()
        .flat_map(|item_path| {
            let mut parents = Vec::new();
            let mut current = item_path.parent();
            while let Some(parent) = current {
                if parent.starts_with(root_path) && parent != root_path {
                    parents.push(parent.to_path_buf());
                } else {
                    break;
                }
                current = parent.parent();
            }
            parents
        })
        .collect()
}

/// A "pure" function that applies filters sequentially to a list of `FileItem`s.
#[allow(clippy::too_many_arguments)] // Allowed for this function as all params are cohesive for filtering
fn apply_filters_on_data(
    full_file_list: &[FileItem],
    root_path: &Path,
    config: &AppConfig,
    search_query: &str,
    extension_filter: &str,
    content_search_query: &str,
    content_search_results: &HashSet<PathBuf>,
    dirs_to_preserve: &HashSet<PathBuf>,
    is_fully_scanned: bool,
) -> Vec<FileItem> {
    // Step 1: Create the base list. If "remove empty" is on, prune the full list first.
    let mut working_list: Vec<FileItem> = if config.remove_empty_directories && is_fully_scanned {
        SearchEngine::remove_empty_directories(
            full_file_list.to_vec(),
            full_file_list,
            dirs_to_preserve,
        )
        .0
    } else {
        full_file_list.to_vec()
    };

    // Step 2: Apply content search if active.
    let has_content_filter = !content_search_query.trim().is_empty();
    if has_content_filter {
        if content_search_results.is_empty() {
            return Vec::new();
        }
        let required_dirs = get_required_ancestors(content_search_results, root_path);
        working_list.retain(|item| {
            content_search_results.contains(&item.path) || required_dirs.contains(&item.path)
        });
    }

    // Step 3: Apply filename/extension search if active.
    let has_filename_filter = !search_query.trim().is_empty();
    let has_extension_filter = !extension_filter.trim().is_empty();

    if has_filename_filter || has_extension_filter {
        let filter = SearchFilter {
            query: search_query.to_string(),
            extension: extension_filter.to_string(),
            case_sensitive: config.case_sensitive_search,
        };

        let matching_files: HashSet<_> = working_list
            .iter()
            .filter(|item| !item.is_directory && SearchEngine::matches_filter(item, &filter))
            .map(|item| item.path.clone())
            .collect();

        if matching_files.is_empty() {
            return Vec::new();
        }

        let required_dirs = get_required_ancestors(&matching_files, root_path);
        working_list.retain(|item| {
            matching_files.contains(&item.path) || required_dirs.contains(&item.path)
        });
    }

    working_list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use crate::app::view_model::generate_ui_state;
    use crate::config::AppConfig;
    use crate::core::FileItem;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Creates a test `FileItem` with the specified path and directory flag.
    fn create_test_file_item(path_str: &str, is_dir: bool) -> FileItem {
        FileItem {
            path: PathBuf::from(path_str),
            is_directory: is_dir,
            is_binary: false,
            size: if is_dir { 0 } else { 100 },
            depth: path_str.matches('/').count(),
            parent: PathBuf::from(path_str).parent().map(|p| p.to_path_buf()),
        }
    }

    /// Creates a clean test configuration without any ignore patterns.
    fn create_test_config() -> AppConfig {
        AppConfig {
            ignore_patterns: HashSet::new(),
            case_sensitive_search: false,
            remove_empty_directories: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_apply_filters_with_search_query() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/src/lib.rs", false),
        ];
        state.search_query = "main".to_string();
        apply_filters(&mut state);

        let ui_state = generate_ui_state(&state);

        // The filtered list should contain `src` (parent dir) and `main.rs`.
        assert_eq!(ui_state.visible_files_count, 2);
        assert_eq!(ui_state.tree.len(), 1); // Only `src` directory at root
    }

    #[test]
    fn test_apply_filters_with_extension_filter() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/README.md", false),
        ];
        state.extension_filter = "md".to_string();
        apply_filters(&mut state);

        let visible_paths: HashSet<_> = state
            .filtered_file_list
            .iter()
            .map(|i| i.path.clone())
            .collect();
        assert_eq!(visible_paths.len(), 1);
        assert!(visible_paths.contains(&PathBuf::from("/project/README.md")));
    }

    #[test]
    fn test_apply_filters_content_search_with_no_results() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/README.md", false),
        ];
        state.content_search_query = "nonexistent".to_string();
        state.content_search_results = HashSet::new(); // No matches
        apply_filters(&mut state);

        assert_eq!(state.filtered_file_list.len(), 0);
    }

    #[test]
    fn test_expanded_empty_dir_is_not_preserved_when_remove_is_on() {
        let mut state = AppState::default();
        let mut config = create_test_config();
        config.remove_empty_directories = true; // Feature is ON
        state.config = config;
        state.current_path = "/project".to_string();

        let empty_dir_path = PathBuf::from("/project/empty_dir");
        let file_path = PathBuf::from("/project/file.txt");

        state.full_file_list = vec![
            create_test_file_item("/project/empty_dir", true),
            create_test_file_item("/project/file.txt", false),
        ];
        state.loaded_dirs.insert(empty_dir_path.clone());
        state.is_fully_scanned = true;
        state.expanded_dirs.insert(empty_dir_path.clone()); // User expands the empty dir

        apply_filters(&mut state); // This should now remove the empty_dir

        let visible_paths: HashSet<_> = state
            .filtered_file_list
            .iter()
            .map(|item| item.path.clone())
            .collect();

        assert!(
            !visible_paths.contains(&empty_dir_path),
            "Expanded empty directory should be REMOVED when feature is on"
        );
        assert!(
            visible_paths.contains(&file_path),
            "Regular file should still be visible"
        );
    }
}
