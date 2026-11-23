//! The original, uncached tree building algorithm.
//!
//! Kept verbatim as a test-only oracle (apart from filling in `has_children` and using
//! the `SelectionState` type instead of its string form). It always
//! builds the *complete* tree; the index-based builder must produce exactly this tree
//! with the subtrees of collapsed directories cut off.

use super::{generate_ui_state, rows, TreeRow, UiState};
use crate::app::selection::SelectionState;
use crate::app::state::AppState;
use crate::core::FileItem;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A node of the tree as the original algorithm builds it: the nested form of a row.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_binary: bool,
    pub size: u64,
    pub children: Vec<TreeNode>,
    pub has_children: bool,
    pub selection_state: SelectionState,
    pub is_expanded: bool,
    pub is_match: bool,
    pub is_previewed: bool,
    pub children_loaded: bool,
}

/// Rebuilds the nested tree from its rows: a row belongs to the nearest row above it
/// that is one level less deep. This is the inverse of listing a tree row by row, so
/// rows that nest into the expected tree are the expected rows, depth included.
pub(super) fn nest(rows: &[TreeRow]) -> Vec<TreeNode> {
    fn nest_level(
        rows: &mut std::iter::Peekable<std::slice::Iter<TreeRow>>,
        depth: usize,
    ) -> Vec<TreeNode> {
        let mut nodes = Vec::new();
        while let Some(row) = rows.next_if(|row| row.depth == depth) {
            nodes.push(TreeNode {
                name: row.name.clone(),
                path: row.path.clone(),
                is_directory: row.is_directory,
                is_binary: row.is_binary,
                size: row.size,
                children: nest_level(rows, depth + 1),
                has_children: row.has_children,
                selection_state: row.selection_state,
                is_expanded: row.is_expanded,
                is_match: row.is_match,
                is_previewed: row.is_previewed,
                children_loaded: row.children_loaded,
            });
        }
        nodes
    }

    let mut remaining = rows.iter().peekable();
    let nodes = nest_level(&mut remaining, 0);
    assert!(
        remaining.next().is_none(),
        "a row is deeper than the row above it allows"
    );
    nodes
}

/// The tree the UI can show for `state`: all of its rows, whatever the viewport is,
/// in nested form.
pub(super) fn snapshot_tree(state: &AppState) -> Vec<TreeNode> {
    nest(&rows::all_rows(state))
}

/// A snapshot together with the tree the UI can show for the same state.
pub(super) struct Snapshot {
    /// All rows of the tree, in nested form.
    pub tree: Vec<TreeNode>,
    ui_state: UiState,
}

impl std::ops::Deref for Snapshot {
    type Target = UiState;

    fn deref(&self) -> &UiState {
        &self.ui_state
    }
}

pub(super) fn snapshot(state: &AppState) -> Snapshot {
    Snapshot {
        tree: snapshot_tree(state),
        ui_state: generate_ui_state(state),
    }
}

/// Arguments for the `build_tree_nodes` function.
pub(super) struct BuildTreeArgs<'a> {
    pub items: &'a [FileItem],
    pub root_path: &'a Path,
    pub selected: &'a HashSet<PathBuf>,
    pub expanded: &'a HashSet<PathBuf>,
    pub loaded_dirs: &'a HashSet<PathBuf>,
    pub content_search_matches: &'a HashSet<PathBuf>,
    pub filename_query: &'a str,
    pub extension_filter: &'a str,
    pub case_sensitive: bool,
    pub previewed_path: &'a Option<PathBuf>,
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
            SelectionState::None
        } else if counts.selected == counts.total_files {
            SelectionState::Full
        } else {
            SelectionState::Partial
        }
    } else if args.selected.contains(&item.path) {
        SelectionState::Full
    } else {
        SelectionState::None
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
        has_children: !children_nodes.is_empty(),
        children: children_nodes,
        selection_state,
        is_expanded: args.expanded.contains(&item.path),
        is_match: name_match || extension_match || content_match,
        is_previewed,
        children_loaded: !item.is_directory || args.loaded_dirs.contains(&item.path),
    }
}

/// The original extension rule of the tree builder, kept with the oracle so that it
/// stays independent of the matcher the real builder uses.
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

/// Main entry point for the optimized tree building process.
pub(super) fn build_tree_nodes(args: BuildTreeArgs) -> Vec<TreeNode> {
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
