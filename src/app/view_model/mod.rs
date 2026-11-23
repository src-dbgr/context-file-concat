//! Responsible for transforming the `AppState` into a `UiState` view model.
//!
//! This module acts as a presentation layer, preparing data specifically for consumption
//! by the UI. It builds the file tree structure and computes various
//! display-related properties. It is purely for data transformation and does not
//! mutate the application state.
//!
//! Every `UiState` is a complete, self-contained snapshot of what the UI shows. The
//! tree is pruned to the visible part: children are only sent for expanded directories
//! (`AppState::expanded_dirs` is the single source of truth for expansion). Totals that
//! would otherwise require the whole tree are sent as counters.

mod status;
mod tree_builder;

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod pruning_tests;
#[cfg(test)]
mod reference;

use crate::app::selection::SelectionState;
use crate::app::state::AppState;
use crate::app::tree_index::TreeCounts;
use crate::config::AppConfig;
use crate::core::{is_blank, FileItem, FileNameMatcher};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tree_builder::{build_tree_nodes, TreeView};

/// A serializable representation of the application state for the UI.
#[derive(Serialize, Clone, Debug)]
pub struct UiState {
    pub config: AppConfig,
    pub current_path: String,
    pub tree: Vec<TreeNode>,
    /// The number of items (files *and* directories) of the unfiltered list.
    pub total_files_found: usize,
    /// The number of items (files *and* directories) of the filtered list.
    pub visible_files_count: usize,
    /// The number of files in the complete filtered tree, including those below
    /// collapsed directories that are not part of `tree`.
    pub tree_files_count: usize,
    /// The number of directories in the complete filtered tree, including those below
    /// collapsed directories that are not part of `tree`.
    pub tree_dirs_count: usize,
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
    pub patterns_need_rescan: bool,
}

/// A serializable representation of a single node in the file tree for the UI.
///
/// The tree is pruned to what is visible: only expanded directories carry their
/// children. Everything else a row needs is part of the node itself.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_binary: bool,
    pub size: u64,
    /// The child nodes. Always empty for files and for collapsed directories.
    pub children: Vec<TreeNode>,
    /// Whether the directory has children in the filtered list, even if they are not
    /// part of `children` because the directory is collapsed. Always `false` for files.
    pub has_children: bool,
    pub selection_state: SelectionState,
    pub is_expanded: bool,
    pub is_match: bool,
    pub is_previewed: bool,
    /// Indicates if the children of this directory have been loaded.
    /// This is used for the lazy-loading UI.
    pub children_loaded: bool,
}

/// The visible part of the tree together with the totals of the complete tree.
struct TreeSnapshot {
    nodes: Vec<TreeNode>,
    counts: TreeCounts,
}

/// Creates the complete `UiState` from the current `AppState`.
pub fn generate_ui_state(state: &AppState) -> UiState {
    let tree = build_tree_snapshot(state);

    UiState {
        config: state.config.clone(),
        current_path: state.current_path.clone(),
        tree: tree.nodes,
        total_files_found: state.full_file_list.len(),
        visible_files_count: state.filtered_file_list().len(),
        tree_files_count: tree.counts.files,
        tree_dirs_count: tree.counts.dirs,
        selected_files_count: state.selected_files.len(),
        is_scanning: state.is_scanning(),
        is_generating: state.is_generating(),
        is_fully_scanned: state.is_fully_scanned,
        status_message: status::status_message(state),
        search_query: state.search_query.clone(),
        extension_filter: state.extension_filter.clone(),
        content_search_query: state.content_search_query.clone(),
        current_config_filename: state.current_config_filename.clone(),
        scan_progress: status::scan_progress_for_ui(state),
        active_ignore_patterns: state.active_ignore_patterns.clone(),
        patterns_need_rescan: state.patterns_need_rescan,
    }
}

/// Builds the pruned tree for the current state.
fn build_tree_snapshot(state: &AppState) -> TreeSnapshot {
    if (state.is_scanning() && state.full_file_list.is_empty())
        // If content search is active but has no results, the tree should be empty
        || (!is_blank(&state.content_search_query) && state.content_search_results.is_empty())
    {
        return TreeSnapshot {
            nodes: Vec::new(),
            counts: TreeCounts::default(),
        };
    }

    let items = state.filtered_file_list();
    let index = state.file_tree();
    let view = TreeView {
        selected: &state.selected_files,
        expanded: &state.expanded_dirs,
        loaded_dirs: &state.loaded_dirs,
        content_search_matches: &state.content_search_results,
        name_matcher: &name_matcher(state),
        previewed_path: state.previewed_file_path.as_deref(),
    };
    TreeSnapshot {
        nodes: build_tree_nodes(items, &index, &view),
        counts: index.counts(),
    }
}

/// The matcher for the filename query and the extension filter of the state.
fn name_matcher(state: &AppState) -> FileNameMatcher {
    FileNameMatcher::new(
        &state.search_query,
        &state.extension_filter,
        state.config.case_sensitive_search,
    )
}

/// Expands the parent directories of files that match the current search criteria.
///
/// This function iterates through all filtered files that match the current search,
/// extension, or content filters and adds their parent directories to the `expanded_dirs` set,
/// ensuring that search results are visible in the file tree.
pub fn auto_expand_for_matches(state: &mut AppState) {
    let root_path = PathBuf::from(&state.current_path);
    let name_matcher = name_matcher(state);
    let matches: Vec<PathBuf> = state
        .filtered_file_list()
        .iter()
        .filter(|item| {
            !item.is_directory
                && (name_matcher.matches_any(&item.path)
                    || state.content_search_results.contains(&item.path))
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
        state.set_filtered_file_list(vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/Cargo.toml", false),
        ]);
        state.loaded_dirs.insert(PathBuf::from("/project/src"));

        let ui_state = generate_ui_state(&state);

        assert_eq!(ui_state.tree.len(), 2); // src dir and Cargo.toml file
        let src_node = ui_state.tree.iter().find(|n| n.name == "src").unwrap();
        assert!(src_node.is_directory);
        assert!(src_node.children_loaded);
        // Collapsed: the children are known but not part of the snapshot.
        assert!(src_node.has_children);
        assert!(src_node.children.is_empty());
        assert_eq!(ui_state.tree_files_count, 2);
        assert_eq!(ui_state.tree_dirs_count, 1);

        state.expanded_dirs.insert(PathBuf::from("/project/src"));
        let ui_state = generate_ui_state(&state);
        let src_node = ui_state.tree.iter().find(|n| n.name == "src").unwrap();
        assert_eq!(src_node.children.len(), 1);
        assert_eq!(src_node.children[0].name, "main.rs");
    }

    /// Filtering, highlighting and auto-expansion share one matching rule, so a filter
    /// spelled in a way the file list accepts also highlights and expands its matches.
    #[test]
    fn test_filter_highlight_and_auto_expand_agree_on_matches() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/build", true),
            create_test_file_item("/project/build/Makefile", false),
            create_test_file_item("/project/build/main.rs", false),
        ];
        state.extension_filter = "No Extension".to_string();

        crate::app::filtering::apply_filters(&mut state);
        auto_expand_for_matches(&mut state);
        let ui_state = generate_ui_state(&state);

        let build = &ui_state.tree[0];
        assert!(build.is_expanded, "The directory of the match is expanded");
        assert_eq!(build.children.len(), 1, "Only the match is listed");
        assert_eq!(build.children[0].name, "Makefile");
        assert!(build.children[0].is_match, "The match is highlighted");
    }

    #[test]
    fn test_stats_and_node_properties() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        let src_path = PathBuf::from("/project/src");
        let main_rs_path = PathBuf::from("/project/src/main.rs");
        let preview_path = main_rs_path.clone();

        state.set_filtered_file_list(vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
        ]);
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
        assert_eq!(main_rs_node.selection_state, SelectionState::Full);
    }
}
