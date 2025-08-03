//! Responsible for transforming the `AppState` into a `UiState` view model.
//!
//! This module acts as a presentation layer, preparing data specifically for consumption
//! by the UI. It builds the file tree structure and computes various
//! display-related properties. It is purely for data transformation and does not
//! mutate the application state.

use crate::app::state::AppState;
use crate::config::AppConfig;
use crate::core::FileItem;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
    pub is_fully_scanned: bool,
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
    /// Indicates if the children of this directory have been loaded.
    /// This is used for the lazy-loading UI.
    pub children_loaded: bool,
}

/// Creates the complete `UiState` from the current `AppState`.
pub fn generate_ui_state(state: &AppState) -> UiState {
    let tree = if (state.is_scanning && state.full_file_list.is_empty())
        // If content search is active but has no results, the tree should be empty
        || (!state.content_search_query.is_empty() && state.content_search_results.is_empty())
    {
        Vec::new()
    } else {
        let args = BuildTreeArgs {
            items: &state.filtered_file_list,
            root_path: &PathBuf::from(&state.current_path),
            selected: &state.selected_files,
            expanded: &state.expanded_dirs,
            loaded_dirs: &state.loaded_dirs,
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
        is_fully_scanned: state.is_fully_scanned,
        status_message,
        search_query: state.search_query.clone(),
        extension_filter: state.extension_filter.clone(),
        content_search_query: state.content_search_query.clone(),
        current_config_filename: state.current_config_filename.clone(),
        scan_progress: state.scan_progress.clone(),
        active_ignore_patterns: state.active_ignore_patterns.clone(),
    }
}

/// Expands the parent directories of files that match the current search criteria.
///
/// This function iterates through all filtered files that match the current search,
/// extension, or content filters and adds their parent directories to the `expanded_dirs` set,
/// ensuring that search results are visible in the file tree.
pub fn auto_expand_for_matches(state: &mut AppState) {
    let root_path = PathBuf::from(&state.current_path);
    let matches: Vec<PathBuf> = state
        .filtered_file_list
        .iter()
        .filter(|item| {
            if item.is_directory {
                return false;
            }

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

            let extension_match = if !state.extension_filter.is_empty() {
                matches_extension(&item.path, &state.extension_filter)
            } else {
                false
            };

            let content_match = state.content_search_results.contains(&item.path);

            name_match || extension_match || content_match
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
    loaded_dirs: &'a HashSet<PathBuf>,
    content_search_matches: &'a HashSet<PathBuf>,
    filename_query: &'a str,
    extension_filter: &'a str,
    case_sensitive: bool,
    previewed_path: &'a Option<PathBuf>,
}

/// A transient struct used during tree construction for memoizing selection counts.
#[derive(Clone, Copy)]
struct SelectionCounts {
    selected: usize,
    total_files: usize,
}

/// Recursively calculates the number of selected and total files within a directory tree.
/// It uses a cache (memoization) to avoid re-calculating for the same directory.
fn get_recursive_selection_counts(
    path: &Path,
    children_map: &HashMap<PathBuf, Vec<PathBuf>>,
    item_map: &HashMap<PathBuf, &FileItem>,
    selected_files: &HashSet<PathBuf>,
    cache: &mut HashMap<PathBuf, SelectionCounts>,
) -> SelectionCounts {
    if let Some(&counts) = cache.get(path) {
        return counts;
    }

    let mut counts = SelectionCounts {
        selected: 0,
        total_files: 0,
    };

    if let Some(children) = children_map.get(path) {
        for child_path in children {
            if let Some(child_item) = item_map.get(child_path) {
                if child_item.is_directory {
                    let child_counts = get_recursive_selection_counts(
                        child_path,
                        children_map,
                        item_map,
                        selected_files,
                        cache,
                    );
                    counts.selected += child_counts.selected;
                    counts.total_files += child_counts.total_files;
                } else {
                    counts.total_files += 1;
                    if selected_files.contains(child_path) {
                        counts.selected += 1;
                    }
                }
            }
        }
    }

    cache.insert(path.to_path_buf(), counts);
    counts
}

/// Sorts a list of TreeNodes: directories first, then alphabetically.
fn sort_tree_nodes(nodes: &mut [TreeNode]) {
    nodes.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
}

/// Recursively builds the `TreeNode` structure for the UI from pre-processed maps.
/// This avoids costly lookups in the flat list during recursion.
fn build_node_recursive(
    path: &Path,
    args: &BuildTreeArgs,
    children_map: &HashMap<PathBuf, Vec<PathBuf>>,
    item_map: &HashMap<PathBuf, &FileItem>,
    selection_cache: &mut HashMap<PathBuf, SelectionCounts>,
) -> TreeNode {
    let item = item_map[path];
    let file_name_str = item.path.file_name().unwrap_or_default().to_string_lossy();

    let selection_state = if item.is_directory {
        let counts = get_recursive_selection_counts(
            &item.path,
            children_map,
            item_map,
            args.selected,
            selection_cache,
        );
        if counts.total_files == 0 || counts.selected == 0 {
            "none".to_string()
        } else if counts.selected == counts.total_files {
            "full".to_string()
        } else {
            "partial".to_string()
        }
    } else if args.selected.contains(&item.path) {
        "full".to_string()
    } else {
        "none".to_string()
    };

    let name_match = if !args.filename_query.is_empty() {
        if args.case_sensitive {
            file_name_str.contains(args.filename_query)
        } else {
            file_name_str
                .to_lowercase()
                .contains(&args.filename_query.to_lowercase())
        }
    } else {
        false
    };

    let extension_match = if !args.extension_filter.is_empty() {
        matches_extension(&item.path, args.extension_filter)
    } else {
        false
    };

    let content_match = args.content_search_matches.contains(&item.path);
    let is_previewed = args.previewed_path.as_ref() == Some(&item.path);

    let mut children_nodes = Vec::new();
    if item.is_directory {
        if let Some(child_paths) = children_map.get(path) {
            children_nodes = child_paths
                .iter()
                .map(|child_path| {
                    build_node_recursive(child_path, args, children_map, item_map, selection_cache)
                })
                .collect();

            sort_tree_nodes(&mut children_nodes);
        }
    }

    TreeNode {
        name: file_name_str.to_string(),
        path: item.path.clone(),
        is_directory: item.is_directory,
        is_binary: item.is_binary,
        size: item.size,
        children: children_nodes,
        selection_state,
        is_expanded: args.expanded.contains(&item.path),
        is_match: name_match || extension_match || content_match,
        is_previewed,
        children_loaded: !item.is_directory || args.loaded_dirs.contains(&item.path),
    }
}

/// Main entry point for the optimized tree building process.
fn build_tree_nodes(args: BuildTreeArgs) -> Vec<TreeNode> {
    if args.items.is_empty() {
        return Vec::new();
    }

    // Step 1: Create lookup maps in a single O(N) pass.
    let item_map: HashMap<PathBuf, &FileItem> = args
        .items
        .iter()
        .map(|item| (item.path.clone(), item))
        .collect();
    let mut children_map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for item in args.items {
        if let Some(parent) = item.path.parent() {
            if parent != args.root_path && !item_map.contains_key(parent) {
                continue;
            }
            children_map
                .entry(parent.to_path_buf())
                .or_default()
                .push(item.path.clone());
        }
    }

    // Step 2: Get root nodes.
    let root_paths = children_map
        .get(args.root_path)
        .cloned()
        .unwrap_or_default();
    let mut selection_cache = HashMap::new();

    // Step 3: Recursively build the tree from the root.
    let mut root_nodes: Vec<TreeNode> = root_paths
        .iter()
        .map(|path| {
            build_node_recursive(path, &args, &children_map, &item_map, &mut selection_cache)
        })
        .collect();

    // Step 4: Sort the final root nodes.
    sort_tree_nodes(&mut root_nodes);

    root_nodes
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
    // Use full_file_list to ensure all selected files are included,
    // regardless of the current search filter. This list already respects ignore patterns.
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
    use crate::config::AppConfig;
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

    /// Creates a clean test configuration.
    fn create_test_config() -> AppConfig {
        AppConfig {
            ignore_patterns: HashSet::new(),
            ..Default::default()
        }
    }

    #[test]
    fn test_generate_ui_state_initial_empty() {
        let state = AppState::default();
        let ui_state = generate_ui_state(&state);

        assert!(ui_state.tree.is_empty());
        assert_eq!(ui_state.total_files_found, 0);
        assert_eq!(ui_state.status_message, "Ready.");
    }

    #[test]
    fn test_generate_ui_state_after_scan() {
        let mut state = AppState::default();
        state.current_path = "/project".to_string();
        state.filtered_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/Cargo.toml", false),
        ];
        state.loaded_dirs.insert(PathBuf::from("/project/src"));

        let ui_state = generate_ui_state(&state);

        assert_eq!(ui_state.tree.len(), 2); // src dir and Cargo.toml file
        let src_node = ui_state.tree.iter().find(|n| n.name == "src").unwrap();
        assert!(src_node.is_directory);
        assert!(src_node.children_loaded);
        assert_eq!(src_node.children.len(), 1);
        assert_eq!(src_node.children[0].name, "main.rs");
    }

    #[test]
    fn test_stats_and_node_properties() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        let src_path = PathBuf::from("/project/src");
        let main_rs_path = PathBuf::from("/project/src/main.rs");
        let preview_path = main_rs_path.clone();

        state.filtered_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
        ];
        state.selected_files = HashSet::from([main_rs_path.clone()]);
        state.expanded_dirs = HashSet::from([src_path.clone()]);
        state.loaded_dirs = HashSet::from([src_path.clone()]);
        state.previewed_file_path = Some(preview_path);

        let ui_state = generate_ui_state(&state);

        assert_eq!(ui_state.selected_files_count, 1);
        let src_node = ui_state.tree.iter().find(|n| n.path == src_path).unwrap();
        assert!(src_node.is_expanded);
        let main_rs_node = src_node
            .children
            .iter()
            .find(|n| n.path == main_rs_path)
            .unwrap();
        assert!(main_rs_node.is_previewed);
        assert_eq!(main_rs_node.selection_state, "full");
    }
}
