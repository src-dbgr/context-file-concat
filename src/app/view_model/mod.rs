//! Responsible for transforming the `AppState` into a `UiState` view model.
//!
//! This module acts as a presentation layer, preparing data specifically for consumption
//! by the UI. It lists the rows of the file tree and computes various
//! display-related properties. It is purely for data transformation and does not
//! mutate the application state: every function here takes the state by shared
//! reference. (State changes that go with filtering, such as expanding the directories
//! of matches, live in `app::filtering`.)
//!
//! Every `UiState` is a complete, self-contained snapshot of what the UI shows. Of the
//! tree it carries a window: the rows the UI has on screen (see [`rows`]), out of the
//! list of all rows that `AppState::expanded_dirs`, the single source of truth for
//! expansion, makes visible. Totals that would otherwise require the whole tree are
//! sent as counters.

pub(crate) mod lossy_path;
pub(crate) mod rows;
mod status;
mod ui_config;

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod pruning_tests;
#[cfg(test)]
mod reference;
#[cfg(test)]
mod windowing_tests;

use crate::app::state::AppState;
use crate::app::tree_index::TreeCounts;
use serde::Serialize;
use std::collections::HashSet;

pub use rows::{LocatedRow, TreeRow, TreeViewState, Viewport};
pub use ui_config::UiConfig;

/// A serializable representation of the application state for the UI.
#[derive(Serialize, Clone, Debug)]
pub struct UiState {
    pub config: UiConfig,
    pub current_path: String,
    /// The rows of the tree inside the window the UI looks at, in display order.
    pub rows: Vec<TreeRow>,
    /// The position of `rows[0]` in the list of all rows.
    pub rows_start: usize,
    /// The number of rows of the tree: every item whose ancestors are all expanded.
    pub rows_total: usize,
    /// Identifies the list of all rows. It changes when the list does (another file
    /// list, a directory expanded or collapsed), and only then: positions in the
    /// list keep their meaning for as long as it stays the same.
    pub rows_revision: u64,
    /// The answer to the most recent `locateRow` request, `None` if there was none
    /// for the tree that is shown.
    pub located: Option<LocatedRow>,
    /// The number of items (files *and* directories) of the unfiltered list.
    pub total_files_found: usize,
    /// The number of items (files *and* directories) of the filtered list.
    pub visible_files_count: usize,
    /// The number of files in the complete filtered tree, including those below
    /// collapsed directories, which are no rows.
    pub tree_files_count: usize,
    /// The number of directories in the complete filtered tree, including those below
    /// collapsed directories, which are no rows.
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

/// Creates the complete `UiState` from the current `AppState`.
pub fn generate_ui_state(state: &AppState) -> UiState {
    let tree = rows::tree_snapshot(state);
    let counts = if tree.is_shown {
        state.file_tree().counts()
    } else {
        TreeCounts::default()
    };

    UiState {
        config: UiConfig::from(&state.config),
        current_path: state.current_path.clone(),
        rows: tree.rows,
        rows_start: tree.rows_start,
        rows_total: tree.rows_total,
        rows_revision: tree.rows_revision,
        located: tree.located,
        total_files_found: state.full_file_list().len(),
        visible_files_count: state.filtered_file_list().len(),
        tree_files_count: counts.files,
        tree_dirs_count: counts.dirs,
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

#[cfg(test)]
impl UiState {
    /// The rows directly below the loaded directory.
    pub(crate) fn root_rows(&self) -> Vec<&TreeRow> {
        self.rows.iter().filter(|row| row.depth == 0).collect()
    }

    /// The first row with the given name.
    pub(crate) fn row_named(&self, name: &str) -> Option<&TreeRow> {
        self.rows.iter().find(|row| row.name == name)
    }

    /// The rows directly below `parent`, which must be a row of this snapshot.
    pub(crate) fn children_of(&self, parent: &TreeRow) -> Vec<&TreeRow> {
        let position = self
            .rows
            .iter()
            .position(|row| std::ptr::eq(row, parent))
            .expect("the row belongs to this snapshot");
        self.rows[position + 1..]
            .iter()
            .take_while(|row| row.depth > parent.depth)
            .filter(|row| row.depth == parent.depth + 1)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::selection::SelectionState;
    use crate::app::state::AppState;
    use crate::config::AppConfig;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Creates a test `FileItem` with the specified path and directory flag.
    fn create_test_file_item(path_str: &str, is_dir: bool) -> crate::core::FileItem {
        crate::core::FileItem {
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

        assert!(ui_state.rows.is_empty());
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

        assert_eq!(ui_state.root_rows().len(), 2); // src dir and Cargo.toml file
        let src_node = ui_state
            .root_rows()
            .into_iter()
            .find(|n| n.name == "src")
            .unwrap();
        assert!(src_node.is_directory);
        assert!(src_node.children_loaded);
        // Collapsed: the children are known but not part of the snapshot.
        assert!(src_node.has_children);
        assert!(ui_state.children_of(src_node).is_empty());
        assert_eq!(ui_state.tree_files_count, 2);
        assert_eq!(ui_state.tree_dirs_count, 1);

        state.expanded_dirs.insert(PathBuf::from("/project/src"));
        let ui_state = generate_ui_state(&state);
        let src_node = ui_state.row_named("src").unwrap();
        let children = ui_state.children_of(src_node);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "main.rs");
        assert_eq!((src_node.depth, children[0].depth), (0, 1));
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
        state.selected_files = HashSet::from([main_rs_path.clone()]).into();
        state.expanded_dirs = HashSet::from([src_path.clone()]).into();
        state.loaded_dirs = HashSet::from([src_path.clone()]);
        state.previewed_file_path = Some(preview_path);

        let ui_state = generate_ui_state(&state);

        assert_eq!(ui_state.selected_files_count, 1);
        let src_node = ui_state
            .root_rows()
            .into_iter()
            .find(|n| n.path == src_path)
            .unwrap();
        assert!(src_node.is_expanded);
        let main_rs_node = ui_state
            .children_of(src_node)
            .into_iter()
            .find(|n| n.path == main_rs_path)
            .unwrap();
        assert!(main_rs_node.is_previewed);
        assert_eq!(main_rs_node.selection_state, SelectionState::Full);
    }
}
