//! The original, uncached tree building algorithm.
//!
//! Kept verbatim as a test-only oracle (apart from filling in `has_children`). It always
//! builds the *complete* tree; the index-based builder must produce exactly this tree
//! with the subtrees of collapsed directories cut off.

use super::{matches_extension, TreeNode};
use crate::core::FileItem;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
        has_children: !children_nodes.is_empty(),
        children: children_nodes,
        selection_state,
        is_expanded: args.expanded.contains(&item.path),
        is_match: name_match || extension_match || content_match,
        is_previewed,
        children_loaded: !item.is_directory || args.loaded_dirs.contains(&item.path),
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
