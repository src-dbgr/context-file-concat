//! Builds the UI tree from a cached [`TreeIndex`] and the dynamic view state.
//!
//! Everything that changes without the filtered file list changing (selection,
//! expansion, search matches, preview, lazy-load status) is evaluated here on every
//! update; the structure itself comes from the index.

use super::tree_index::TreeIndex;
use super::{matches_extension, TreeNode};
use crate::core::FileItem;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The dynamic, per-update inputs of the tree.
pub(super) struct TreeView<'a> {
    pub selected: &'a HashSet<PathBuf>,
    pub expanded: &'a HashSet<PathBuf>,
    pub loaded_dirs: &'a HashSet<PathBuf>,
    pub content_search_matches: &'a HashSet<PathBuf>,
    pub filename_query: &'a str,
    pub extension_filter: &'a str,
    pub case_sensitive: bool,
    pub previewed_path: Option<&'a Path>,
}

/// Builds the tree for `items`. `index` must have been built from exactly these items.
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
        name_matcher: NameMatcher::new(view.filename_query, view.case_sensitive),
        selection_counts: vec![None; items.len()],
    };
    index
        .roots()
        .iter()
        .map(|&root| builder.build_node(root))
        .collect()
}

/// Decides whether a file name matches the filename query.
enum NameMatcher<'a> {
    /// No query is set: nothing matches.
    Inactive,
    CaseSensitive(&'a str),
    /// Holds the lowercased query, computed once per build.
    CaseInsensitive(String),
}

impl<'a> NameMatcher<'a> {
    fn new(query: &'a str, case_sensitive: bool) -> Self {
        if query.is_empty() {
            Self::Inactive
        } else if case_sensitive {
            Self::CaseSensitive(query)
        } else {
            Self::CaseInsensitive(query.to_lowercase())
        }
    }

    fn matches(&self, file_name: &str) -> bool {
        match self {
            Self::Inactive => false,
            Self::CaseSensitive(query) => file_name.contains(query),
            Self::CaseInsensitive(query) => file_name.to_lowercase().contains(query.as_str()),
        }
    }
}

/// The number of selected and total files below a directory.
#[derive(Clone, Copy)]
struct SelectionCounts {
    selected: usize,
    total_files: usize,
}

impl SelectionCounts {
    fn state(self) -> &'static str {
        if self.total_files == 0 || self.selected == 0 {
            "none"
        } else if self.selected == self.total_files {
            "full"
        } else {
            "partial"
        }
    }
}

/// One tree build: borrows the structure and memoizes selection counts per directory.
struct TreeBuilder<'a> {
    items: &'a [FileItem],
    index: &'a TreeIndex,
    view: &'a TreeView<'a>,
    name_matcher: NameMatcher<'a>,
    /// Memoized counts, parallel to `items`.
    selection_counts: Vec<Option<SelectionCounts>>,
}

impl<'a> TreeBuilder<'a> {
    fn build_node(&mut self, item_index: usize) -> TreeNode {
        let (items, index, view) = (self.items, self.index, self.view);
        let item = &items[item_index];
        let name = index.name(item_index);

        let name_match = self.name_matcher.matches(name);
        let extension_match = !view.extension_filter.is_empty()
            && matches_extension(&item.path, view.extension_filter);
        let content_match = view.content_search_matches.contains(&item.path);

        let children = if item.is_directory {
            index
                .children(item_index)
                .iter()
                .map(|&child| self.build_node(child))
                .collect()
        } else {
            Vec::new()
        };

        TreeNode {
            name: name.to_string(),
            path: item.path.clone(),
            is_directory: item.is_directory,
            is_binary: item.is_binary,
            size: item.size,
            children,
            selection_state: self.selection_state(item_index).to_string(),
            is_expanded: view.expanded.contains(&item.path),
            is_match: name_match || extension_match || content_match,
            is_previewed: view.previewed_path == Some(item.path.as_path()),
            children_loaded: !item.is_directory || view.loaded_dirs.contains(&item.path),
        }
    }

    fn selection_state(&mut self, item_index: usize) -> &'static str {
        let item = &self.items[item_index];
        if item.is_directory {
            self.selection_counts(item_index).state()
        } else if self.view.selected.contains(&item.path) {
            "full"
        } else {
            "none"
        }
    }

    /// Counts the selected and total files below the given directory.
    fn selection_counts(&mut self, dir_index: usize) -> SelectionCounts {
        if let Some(counts) = self.selection_counts[dir_index] {
            return counts;
        }

        let (items, index, view) = (self.items, self.index, self.view);
        let mut counts = SelectionCounts {
            selected: 0,
            total_files: 0,
        };
        for &child in index.children(dir_index) {
            let child_item = &items[child];
            if child_item.is_directory {
                let child_counts = self.selection_counts(child);
                counts.selected += child_counts.selected;
                counts.total_files += child_counts.total_files;
            } else {
                counts.total_files += 1;
                if view.selected.contains(&child_item.path) {
                    counts.selected += 1;
                }
            }
        }

        self.selection_counts[dir_index] = Some(counts);
        counts
    }
}
