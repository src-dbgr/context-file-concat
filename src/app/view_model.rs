//! Responsible for transforming the `AppState` into a `UiState` view model.
//!
//! This module acts as a presentation layer, preparing data specifically for consumption
//! by the UI. It builds the file tree structure, applies filters, and computes various
//! display-related properties.

use crate::config::AppConfig;
use crate::core::{FileItem, SearchEngine, SearchFilter};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::state::AppState;

/// A serializable representation of the application state for the UI.
#[derive(Serialize, Clone, Debug)]
pub struct UiState {
    pub config: AppConfig,
    pub current_path: String,
    pub tree: Vec<TreeNode>,
    pub total_files_found: usize,
    pub visible_files_count: usize,
    pub selected_files_count: usize,
    pub is_scanning: bool,
    pub is_generating: bool,
    pub status_message: String,
    pub search_query: String,
    pub extension_filter: String,
    pub content_search_query: String,
    pub current_config_filename: Option<String>,
    pub scan_progress: crate::core::ScanProgress,
    pub active_ignore_patterns: HashSet<String>,
}

/// A serializable representation of a single node in the file tree for the UI.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_binary: bool,
    pub size: u64,
    pub children: Vec<TreeNode>,
    pub selection_state: String,
    pub is_expanded: bool,
    pub is_match: bool,
    pub is_previewed: bool,
}

/// Creates the complete `UiState` from the current `AppState`.
pub fn generate_ui_state(state: &AppState) -> UiState {
    let tree = if state.is_scanning {
        Vec::new()
    } else {
        let args = BuildTreeArgs {
            items: &state.filtered_file_list,
            root_path: &PathBuf::from(&state.current_path),
            selected: &state.selected_files,
            expanded: &state.expanded_dirs,
            content_search_matches: &state.content_search_results,
            filename_query: &state.search_query,
            extension_filter: &state.extension_filter,
            case_sensitive: state.config.case_sensitive_search,
            previewed_path: &state.previewed_file_path,
        };
        build_tree_nodes(args)
    };

    let status_message = if state.is_scanning {
        format!(
            "Scanning... {} files processed. {} large files skipped ({})",
            state.scan_progress.files_scanned,
            state.scan_progress.large_files_skipped,
            state.scan_progress.current_scanning_path
        )
    } else {
        state.scan_progress.current_scanning_path.clone()
    };

    UiState {
        config: state.config.clone(),
        current_path: state.current_path.clone(),
        tree,
        total_files_found: state.full_file_list.len(),
        visible_files_count: state.filtered_file_list.len(),
        selected_files_count: state.selected_files.len(),
        is_scanning: state.is_scanning,
        is_generating: state.is_generating,
        status_message,
        search_query: state.search_query.clone(),
        extension_filter: state.extension_filter.clone(),
        content_search_query: state.content_search_query.clone(),
        current_config_filename: state.current_config_filename.clone(),
        scan_progress: state.scan_progress.clone(),
        active_ignore_patterns: state.active_ignore_patterns.clone(),
    }
}

/// Applies all current filters to the full file list to generate the visible list.
pub fn apply_filters(state: &mut AppState) {
    let filtered_list = apply_filters_on_data(
        &state.full_file_list,
        &PathBuf::from(&state.current_path),
        &state.config,
        &state.search_query,
        &state.extension_filter,
        &state.content_search_query,
        &state.content_search_results,
    );
    state.filtered_file_list = filtered_list;
}

/// A "pure" function that takes data and returns a filtered list of `FileItem`s.
fn apply_filters_on_data(
    full_file_list: &[FileItem],
    root_path: &Path,
    config: &AppConfig,
    search_query: &str,
    extension_filter: &str,
    content_search_query: &str,
    content_search_results: &HashSet<PathBuf>,
) -> Vec<FileItem> {
    // Check if content search is active but has no results
    if !content_search_query.trim().is_empty() && content_search_results.is_empty() {
        return Vec::new(); // Return empty list when content search finds nothing
    }

    let filter = SearchFilter {
        query: search_query.to_string(),
        extension: extension_filter.to_string(),
        case_sensitive: config.case_sensitive_search,
        ignore_patterns: config.ignore_patterns.clone(),
    };

    let mut filtered = SearchEngine::filter_files(full_file_list, &filter);

    if !content_search_results.is_empty() {
        filtered.retain(|item| content_search_results.contains(&item.path));
    }

    let required_dirs: HashSet<PathBuf> = filtered
        .par_iter()
        .flat_map(|item| {
            let mut parents = Vec::new();
            let mut current = item.path.parent();
            while let Some(parent) = current {
                if parent.starts_with(root_path) {
                    parents.push(parent.to_path_buf());
                } else {
                    break;
                }
                current = parent.parent();
            }
            parents
        })
        .collect();

    let existing_paths_in_filtered: HashSet<PathBuf> =
        filtered.par_iter().map(|item| item.path.clone()).collect();

    for dir_path in required_dirs {
        if !existing_paths_in_filtered.contains(&dir_path) {
            if let Some(dir_item) = full_file_list.iter().find(|i| i.path == dir_path) {
                filtered.push(dir_item.clone());
            }
        }
    }

    if config.remove_empty_directories {
        let (filtered_without_empty, _) = SearchEngine::remove_empty_directories(filtered);
        filtered_without_empty
    } else {
        filtered
    }
}

/// Expands the parent directories of files that match the current search criteria.
pub fn auto_expand_for_matches(state: &mut AppState) {
    let root_path = PathBuf::from(&state.current_path);
    let matches: Vec<PathBuf> = state
        .filtered_file_list
        .iter()
        .filter(|item| {
            let file_name = item.path.file_name().unwrap_or_default().to_string_lossy();
            let name_match = if !state.search_query.is_empty() {
                if state.config.case_sensitive_search {
                    file_name.contains(&state.search_query)
                } else {
                    file_name
                        .to_lowercase()
                        .contains(&state.search_query.to_lowercase())
                }
            } else {
                false
            };
            let content_match = state.content_search_results.contains(&item.path);
            (name_match || content_match) && !item.is_directory
        })
        .map(|item| item.path.clone())
        .collect();

    for path in matches {
        let mut current = path.parent();
        while let Some(parent) = current {
            if parent.starts_with(&root_path) && parent != root_path {
                state.expanded_dirs.insert(parent.to_path_buf());
            } else {
                break;
            }
            current = parent.parent();
        }
    }
}

/// Arguments for the `build_tree_nodes` function.
struct BuildTreeArgs<'a> {
    items: &'a [FileItem],
    root_path: &'a Path,
    selected: &'a HashSet<PathBuf>,
    expanded: &'a HashSet<PathBuf>,
    content_search_matches: &'a HashSet<PathBuf>,
    filename_query: &'a str,
    extension_filter: &'a str,
    case_sensitive: bool,
    previewed_path: &'a Option<PathBuf>,
}

/// Recursively builds the `TreeNode` structure for the UI from a flat list of `FileItem`s.
fn build_tree_nodes(args: BuildTreeArgs) -> Vec<TreeNode> {
    let mut nodes: HashMap<PathBuf, TreeNode> = HashMap::new();
    let mut children_map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    for item in args.items {
        let selection_state = if item.is_directory {
            get_directory_selection_state(&item.path, args.items, args.selected)
        } else if args.selected.contains(&item.path) {
            "full".to_string()
        } else {
            "none".to_string()
        };

        let file_name = item.path.file_name().unwrap_or_default().to_string_lossy();

        // Filename search match
        let name_match = if !args.filename_query.is_empty() {
            if args.case_sensitive {
                file_name.contains(args.filename_query)
            } else {
                file_name
                    .to_lowercase()
                    .contains(&args.filename_query.to_lowercase())
            }
        } else {
            false
        };

        // Extension filter match
        let extension_match = if !args.extension_filter.is_empty() {
            matches_extension(&item.path, args.extension_filter)
        } else {
            false
        };

        // Content search match
        let content_match = args.content_search_matches.contains(&item.path);

        let is_previewed = args.previewed_path.as_ref() == Some(&item.path);

        nodes.insert(
            item.path.clone(),
            TreeNode {
                name: file_name.to_string(),
                path: item.path.clone(),
                is_directory: item.is_directory,
                is_binary: item.is_binary,
                size: item.size,
                children: Vec::new(),
                selection_state,
                is_expanded: args.expanded.contains(&item.path),
                is_match: name_match || extension_match || content_match,
                is_previewed,
            },
        );

        if let Some(parent) = item.path.parent() {
            if parent.starts_with(args.root_path) {
                children_map
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(item.path.clone());
            }
        }
    }

    let mut root_nodes_paths: Vec<PathBuf> = args
        .items
        .iter()
        .filter(|item| item.path.parent() == Some(args.root_path))
        .map(|item| item.path.clone())
        .collect();

    fn build_level(
        paths: &mut Vec<PathBuf>,
        nodes: &mut HashMap<PathBuf, TreeNode>,
        children_map: &HashMap<PathBuf, Vec<PathBuf>>,
    ) -> Vec<TreeNode> {
        paths.sort_by(|a, b| {
            if let (Some(node_a), Some(node_b)) = (nodes.get(a), nodes.get(b)) {
                match (node_a.is_directory, node_b.is_directory) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.file_name().cmp(&b.file_name()),
                }
            } else {
                a.cmp(b) // Fallback sorting if a node is missing (should not happen)
            }
        });

        let mut result = Vec::new();
        for path in paths {
            if let Some(mut node) = nodes.remove(path) {
                if let Some(children_paths) = children_map.get(path) {
                    node.children = build_level(&mut children_paths.clone(), nodes, children_map);
                }
                result.push(node);
            }
        }
        result
    }

    build_level(&mut root_nodes_paths, &mut nodes, &children_map)
}

/// Checks if a path's extension matches the extension filter.
/// This is a copy of the logic from SearchEngine::matches_extension to avoid circular dependencies.
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

/// Determines the selection state of a directory ('none', 'partial', 'full').
pub fn get_directory_selection_state(
    dir_path: &Path,
    all_items: &[FileItem],
    selected_files: &HashSet<PathBuf>,
) -> String {
    let child_files: Vec<_> = all_items
        .iter()
        .filter(|i| !i.is_directory && i.path.starts_with(dir_path))
        .collect();

    if child_files.is_empty() {
        return "none".to_string();
    }

    let selected_count = child_files
        .iter()
        .filter(|f| selected_files.contains(&f.path))
        .count();

    if selected_count == 0 {
        "none".to_string()
    } else if selected_count == child_files.len() {
        "full".to_string()
    } else {
        "partial".to_string()
    }
}

/// Returns a list of the selected file paths in natural tree order.
pub fn get_selected_files_in_tree_order(state: &AppState) -> Vec<PathBuf> {
    let mut selected_file_items: Vec<&FileItem> = state
        .full_file_list
        .iter()
        .filter(|item| !item.is_directory && state.selected_files.contains(&item.path))
        .collect();

    selected_file_items.sort_by_key(|a| a.path.clone());

    selected_file_items
        .into_iter()
        .map(|item| item.path.clone())
        .collect()
}

/// Determines the programming language from a file path for syntax highlighting.
pub fn get_language_from_path(path: &Path) -> String {
    match path.extension().and_then(|s| s.to_str()) {
        Some("rs") => "rust",
        Some("js") | Some("mjs") | Some("cjs") => "javascript",
        Some("ts") | Some("tsx") => "typescript",
        Some("py") => "python",
        Some("html") | Some("htm") => "html",
        Some("css") => "css",
        Some("json") => "json",
        Some("md") => "markdown",
        Some("toml") => "toml",
        Some("yaml") | Some("yml") => "yaml",
        Some("sh") => "shell",
        Some("go") => "go",
        Some("java") => "java",
        Some("c") | Some("h") => "c",
        Some("cpp") | Some("hpp") | Some("cxx") | Some("hxx") => "cpp",
        _ => "plaintext",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
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
    ///
    /// This ensures that test files are not filtered out by production ignore patterns
    /// that might exclude common test file extensions or directories.
    fn create_test_config() -> AppConfig {
        AppConfig {
            ignore_patterns: HashSet::new(),
            case_sensitive_search: false,
            remove_empty_directories: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_generate_ui_state_initial_empty() {
        let state = AppState::default();
        let ui_state = generate_ui_state(&state);

        assert!(ui_state.tree.is_empty());
        assert_eq!(ui_state.total_files_found, 0);
        assert_eq!(ui_state.visible_files_count, 0);
        assert_eq!(ui_state.selected_files_count, 0);
        assert_eq!(ui_state.status_message, "Ready.");
    }

    #[test]
    fn test_generate_ui_state_after_scan() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/Cargo.toml", false),
        ];
        state.filtered_file_list = state.full_file_list.clone();

        let ui_state = generate_ui_state(&state);

        assert_eq!(ui_state.total_files_found, 3);
        assert_eq!(ui_state.visible_files_count, 3);
        assert_eq!(ui_state.tree.len(), 2); // src dir and Cargo.toml file

        let cargo_toml_node = ui_state
            .tree
            .iter()
            .find(|n| n.name == "Cargo.toml")
            .unwrap();
        assert!(!cargo_toml_node.is_directory);

        let src_node = ui_state.tree.iter().find(|n| n.name == "src").unwrap();
        assert!(src_node.is_directory);
        assert_eq!(src_node.children.len(), 1);
        assert_eq!(src_node.children[0].name, "main.rs");
    }

    #[test]
    fn test_generate_ui_state_with_search_query() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/src/lib.rs", false),
        ];
        state.filtered_file_list = state.full_file_list.clone();
        state.search_query = "main".to_string();
        apply_filters(&mut state);

        let ui_state = generate_ui_state(&state);

        // The filtered list should contain `src` (parent dir) and `main.rs`.
        assert_eq!(ui_state.visible_files_count, 2);
        assert_eq!(ui_state.tree.len(), 1); // Only `src` directory at root

        let src_node = &ui_state.tree[0];
        assert_eq!(src_node.name, "src");
        assert_eq!(src_node.children.len(), 1);

        let main_rs_node = &src_node.children[0];
        assert_eq!(main_rs_node.name, "main.rs");
        assert!(main_rs_node.is_match); // Check if match flag is set
    }

    #[test]
    fn test_generate_ui_state_with_extension_filter() {
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

        let ui_state = generate_ui_state(&state);

        assert_eq!(ui_state.visible_files_count, 1);
        assert_eq!(ui_state.tree.len(), 1);
        assert_eq!(ui_state.tree[0].name, "README.md");
        assert!(ui_state.tree[0].is_match); // Extension match should be highlighted
    }

    #[test]
    fn test_content_search_with_no_results() {
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

        let ui_state = generate_ui_state(&state);

        // Should return empty tree when content search is active but finds nothing
        assert_eq!(ui_state.visible_files_count, 0);
        assert_eq!(ui_state.tree.len(), 0);
    }

    #[test]
    fn test_stats_and_node_properties() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        let src_path = PathBuf::from("/project/src");
        let main_rs_path = PathBuf::from("/project/src/main.rs");
        let lib_rs_path = PathBuf::from("/project/src/lib.rs");
        let preview_path = main_rs_path.clone();

        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/src/lib.rs", false),
        ];
        state.filtered_file_list = state.full_file_list.clone();
        state.selected_files = HashSet::from([main_rs_path.clone()]);
        state.expanded_dirs = HashSet::from([src_path.clone()]);
        state.previewed_file_path = Some(preview_path);

        let ui_state = generate_ui_state(&state);

        assert_eq!(ui_state.selected_files_count, 1);

        let src_node = ui_state.tree.iter().find(|n| n.path == src_path).unwrap();
        assert!(src_node.is_expanded);
        assert_eq!(src_node.selection_state, "partial");

        let main_rs_node = src_node
            .children
            .iter()
            .find(|n| n.path == main_rs_path)
            .unwrap();
        assert!(main_rs_node.is_previewed);
        assert_eq!(main_rs_node.selection_state, "full");

        let lib_rs_node = src_node
            .children
            .iter()
            .find(|n| n.path == lib_rs_path)
            .unwrap();
        assert!(!lib_rs_node.is_previewed);
        assert_eq!(lib_rs_node.selection_state, "none");
    }

    #[test]
    fn test_ignore_patterns_functionality() {
        let mut state = AppState::default();

        // Set up configuration with specific ignore patterns for this test
        let mut config = AppConfig::default();
        config.ignore_patterns.insert("*.md".to_string());
        config.ignore_patterns.insert("src/".to_string());
        state.config = config;

        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/README.md", false),
            create_test_file_item("/project/Cargo.toml", false),
        ];

        apply_filters(&mut state);

        let ui_state = generate_ui_state(&state);

        // Only Cargo.toml should remain after applying ignore patterns
        assert_eq!(ui_state.visible_files_count, 1);
        assert_eq!(ui_state.tree.len(), 1);
        assert_eq!(ui_state.tree[0].name, "Cargo.toml");
    }
}
