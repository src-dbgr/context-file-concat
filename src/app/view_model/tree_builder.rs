//! Builds the UI tree from a cached [`TreeIndex`] and the dynamic view state.
//!
//! The tree is *pruned to what is visible*: the children of a directory are only built
//! if the directory is expanded. A collapsed directory is sent without children, but
//! with everything the UI needs to render its row (`has_children`, `selection_state`,
//! `children_loaded`). The work and the payload are therefore proportional to the
//! expanded part of the tree, not to the number of files.
//!
//! Everything that changes without the filtered file list changing (selection,
//! expansion, search matches, preview, lazy-load status) is evaluated here on every
//! update; the structure itself comes from the index.

use super::TreeNode;
use crate::app::selection::SelectionSummary;
use crate::app::tree_index::TreeIndex;
use crate::core::{FileItem, FileNameMatcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The dynamic, per-update inputs of the tree.
pub(super) struct TreeView<'a> {
    pub selected: &'a HashSet<PathBuf>,
    pub expanded: &'a HashSet<PathBuf>,
    pub loaded_dirs: &'a HashSet<PathBuf>,
    pub content_search_matches: &'a HashSet<PathBuf>,
    /// Decides which names are pointed out as matches of the query or extension filter.
    pub name_matcher: &'a FileNameMatcher,
    pub previewed_path: Option<&'a Path>,
}

/// Builds the visible part of the tree for `items`.
///
/// `index` must have been built from exactly these items.
pub(super) fn build_tree_nodes(
    items: &[FileItem],
    index: &TreeIndex,
    view: &TreeView,
) -> Vec<TreeNode> {
    debug_assert_eq!(index.len(), items.len());

    let mut builder = TreeBuilder {
        items,
        index,
        view,
        selection: SelectionSummary::new(items, index, view.selected),
    };
    index
        .roots()
        .iter()
        .map(|&root| builder.build_node(root))
        .collect()
}

/// One tree build: borrows the structure and the selection summary of the list.
struct TreeBuilder<'a> {
    items: &'a [FileItem],
    index: &'a TreeIndex,
    view: &'a TreeView<'a>,
    selection: SelectionSummary<'a>,
}

impl<'a> TreeBuilder<'a> {
    fn build_node(&mut self, item_index: usize) -> TreeNode {
        let (items, index, view) = (self.items, self.index, self.view);
        let item = &items[item_index];
        let is_match = view.name_matcher.matches_any(&item.path)
            || view.content_search_matches.contains(&item.path);

        let child_indices: &[usize] = if item.is_directory {
            index.children(item_index)
        } else {
            &[]
        };
        let is_expanded = view.expanded.contains(&item.path);
        // Pruning: the subtree of a collapsed directory is not part of the snapshot.
        let children = if is_expanded {
            child_indices
                .iter()
                .map(|&child| self.build_node(child))
                .collect()
        } else {
            Vec::new()
        };

        TreeNode {
            name: index.name(item_index).to_string(),
            path: item.path.clone(),
            is_directory: item.is_directory,
            is_binary: item.is_binary,
            size: item.size,
            children,
            has_children: !child_indices.is_empty(),
            selection_state: self.selection.state_of(item_index),
            is_expanded,
            is_match,
            is_previewed: view.previewed_path == Some(item.path.as_path()),
            children_loaded: !item.is_directory || view.loaded_dirs.contains(&item.path),
        }
    }
}
