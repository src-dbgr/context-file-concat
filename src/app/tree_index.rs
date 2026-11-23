//! The structural index of the file tree and its revision-keyed cache.
//!
//! The *structure* of the tree (which item is a child of which directory, and in which
//! order siblings are listed) depends only on the filtered file list and the root
//! path. It is therefore computed once per filtered list and reused for everything that
//! merely changes dynamic properties (selection, expansion, preview, ...).
//!
//! This module is deliberately neutral: it knows the file list, but neither the
//! application state nor the view model. [`AppState`](crate::app::state::AppState) owns
//! the cache next to the list it is derived from; the view model and the commands that
//! work on subtrees are its consumers.
//!
//! Items are referred to by their position in the list the index was built from, so an
//! index must only ever be used together with that exact list. [`TreeIndexCache`]
//! guarantees this by keying the index on the list's revision.

use crate::core::FileItem;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Parent/child relations and display order of a filtered file list.
#[derive(Debug)]
pub(crate) struct TreeIndex {
    /// Display names, parallel to the item list.
    names: Vec<String>,
    /// Sorted children per item, parallel to the item list (empty for leaves).
    children: Vec<Vec<usize>>,
    /// Sorted items located directly below the root path.
    roots: Vec<usize>,
    /// How many nodes the complete (unpruned) tree has.
    counts: TreeCounts,
}

/// The number of file and directory nodes of a tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TreeCounts {
    pub files: usize,
    pub dirs: usize,
}

impl TreeIndex {
    /// Builds the index for `items` relative to `root_path`.
    ///
    /// An item is part of the tree if its parent is the root path or another item of the
    /// list. If a path occurs more than once, the last occurrence represents it.
    pub(crate) fn build(items: &[FileItem], root_path: &Path) -> Self {
        let names: Vec<String> = items.iter().map(display_name).collect();

        let mut by_path: HashMap<&Path, usize> = HashMap::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            by_path.insert(item.path.as_path(), index);
        }

        let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
        let mut roots: Vec<usize> = Vec::new();
        for item in items {
            let Some(parent) = item.path.parent() else {
                continue;
            };
            let index = by_path[item.path.as_path()];
            if let Some(&parent_index) = by_path.get(parent) {
                children[parent_index].push(index);
            }
            if parent == root_path {
                roots.push(index);
            }
        }

        let display_order = |a: &usize, b: &usize| display_order(items, &names, *a, *b);
        for siblings in &mut children {
            siblings.sort_by(display_order);
        }
        roots.sort_by(display_order);

        let counts = count_nodes(items, &children, &roots);

        Self {
            names,
            children,
            roots,
            counts,
        }
    }

    /// The number of nodes of the complete tree, independent of what is expanded.
    ///
    /// Counts exactly the nodes a tree with every directory expanded would contain,
    /// which is not necessarily the length of the item list (orphans are not part of
    /// the tree).
    pub(crate) fn counts(&self) -> TreeCounts {
        self.counts
    }

    /// The number of items the index was built from.
    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }

    /// The items directly below the root path, in display order.
    pub(crate) fn roots(&self) -> &[usize] {
        &self.roots
    }

    /// The children of the given item, in display order.
    pub(crate) fn children(&self, index: usize) -> &[usize] {
        &self.children[index]
    }

    /// The display name of the given item.
    pub(crate) fn name(&self, index: usize) -> &str {
        &self.names[index]
    }

    /// The position of the item that represents `path` in `items`, if it is listed.
    ///
    /// `items` must be the list the index was built from. This is a linear search; the
    /// index does not keep a second copy of every path just for the rare lookups.
    pub(crate) fn position(&self, items: &[FileItem], path: &Path) -> Option<usize> {
        debug_assert_eq!(self.len(), items.len());
        // The last occurrence of a path is the one the tree is built from.
        items.iter().rposition(|item| item.path == path)
    }

    /// The files at or below the given item, in display order.
    ///
    /// For a file this is the file itself. For a directory these are the files of its
    /// subtree, i.e. exactly the files that make up the directory's selection state.
    pub(crate) fn files_in_subtree(&self, items: &[FileItem], index: usize) -> Vec<usize> {
        debug_assert_eq!(self.len(), items.len());
        if !items[index].is_directory {
            return vec![index];
        }

        let mut files = Vec::new();
        let mut pending: Vec<usize> = self.children[index].iter().rev().copied().collect();
        while let Some(current) = pending.pop() {
            if items[current].is_directory {
                pending.extend(self.children[current].iter().rev());
            } else {
                files.push(current);
            }
        }
        files
    }
}

/// Counts the nodes reachable from `roots`, descending into directories only.
fn count_nodes(items: &[FileItem], children: &[Vec<usize>], roots: &[usize]) -> TreeCounts {
    let mut counts = TreeCounts::default();
    let mut pending: Vec<usize> = roots.to_vec();
    while let Some(index) = pending.pop() {
        if items[index].is_directory {
            counts.dirs += 1;
            pending.extend_from_slice(&children[index]);
        } else {
            counts.files += 1;
        }
    }
    counts
}

/// The name shown for an item in the tree.
fn display_name(item: &FileItem) -> String {
    item.path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Sibling order of the tree: directories first, then by name.
fn display_order(items: &[FileItem], names: &[String], a: usize, b: usize) -> Ordering {
    match (items[a].is_directory, items[b].is_directory) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => names[a].cmp(&names[b]),
    }
}

/// Remembers the [`TreeIndex`] of the most recent filtered file list.
///
/// The cache is keyed by the revision of the filtered list and the root path; asking for
/// any other key rebuilds the index. Interior mutability keeps it usable through a
/// shared reference to its owner.
#[derive(Debug, Default)]
pub(crate) struct TreeIndexCache {
    slot: Mutex<Option<CachedIndex>>,
    #[cfg(test)]
    builds: std::sync::atomic::AtomicUsize,
}

#[derive(Debug)]
struct CachedIndex {
    revision: u64,
    root_path: PathBuf,
    index: Arc<TreeIndex>,
}

impl TreeIndexCache {
    /// Returns the index for `items`, building it only if `revision` or `root_path`
    /// differ from the cached one.
    ///
    /// `revision` must change whenever `items` changes.
    pub(crate) fn get_or_build(
        &self,
        revision: u64,
        root_path: &Path,
        items: &[FileItem],
    ) -> Arc<TreeIndex> {
        // The slot only ever holds a complete value, so a poisoned lock is still usable.
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(cached) = slot.as_ref() {
            if cached.revision == revision && cached.root_path == root_path {
                debug_assert_eq!(cached.index.len(), items.len());
                return Arc::clone(&cached.index);
            }
        }

        #[cfg(test)]
        self.builds
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let index = Arc::new(TreeIndex::build(items, root_path));
        *slot = Some(CachedIndex {
            revision,
            root_path: root_path.to_path_buf(),
            index: Arc::clone(&index),
        });
        index
    }

    /// How often an index has been built by this cache.
    #[cfg(test)]
    pub(crate) fn build_count(&self) -> usize {
        self.builds.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, is_directory: bool) -> FileItem {
        FileItem {
            path: PathBuf::from(path),
            is_directory,
            ..Default::default()
        }
    }

    fn names(index: &TreeIndex, indices: &[usize]) -> Vec<String> {
        indices.iter().map(|&i| index.name(i).to_string()).collect()
    }

    #[test]
    fn build_sorts_directories_first_then_by_name() {
        let items = vec![
            item("/r/b.txt", false),
            item("/r/zeta", true),
            item("/r/a.txt", false),
            item("/r/alpha", true),
            item("/r/zeta/y.rs", false),
            item("/r/zeta/sub", true),
            item("/r/zeta/x.rs", false),
        ];
        let index = TreeIndex::build(&items, Path::new("/r"));

        assert_eq!(
            names(&index, index.roots()),
            ["alpha", "zeta", "a.txt", "b.txt"]
        );
        assert_eq!(names(&index, index.children(1)), ["sub", "x.rs", "y.rs"]);
        assert!(index.children(0).is_empty());
    }

    #[test]
    fn build_skips_items_whose_parent_is_unknown() {
        let items = vec![item("/r/a", true), item("/r/missing/orphan.rs", false)];
        let index = TreeIndex::build(&items, Path::new("/r"));

        assert_eq!(index.roots(), [0]);
        assert!(index.children(0).is_empty());
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn counts_cover_the_whole_tree_but_not_orphans() {
        let items = vec![
            item("/r/a", true),
            item("/r/a/deep", true),
            item("/r/a/deep/f.rs", false),
            item("/r/a/g.rs", false),
            item("/r/empty", true),
            item("/r/h.rs", false),
            item("/r/missing/orphan.rs", false),
            item("/elsewhere/x.rs", false),
        ];
        let index = TreeIndex::build(&items, Path::new("/r"));

        assert_eq!(index.counts(), TreeCounts { files: 3, dirs: 3 });
        assert_eq!(
            TreeIndex::build(&[], Path::new("/r")).counts(),
            TreeCounts::default()
        );
    }

    #[test]
    fn position_finds_the_last_occurrence_of_a_listed_path() {
        let items = vec![
            item("/r/a", true),
            item("/r/a/f.rs", false),
            item("/r/a", true),
        ];
        let index = TreeIndex::build(&items, Path::new("/r"));

        assert_eq!(index.position(&items, Path::new("/r/a")), Some(2));
        assert_eq!(index.position(&items, Path::new("/r/a/f.rs")), Some(1));
        assert_eq!(index.position(&items, Path::new("/r")), None);
        assert_eq!(index.position(&items, Path::new("/r/unlisted")), None);
    }

    #[test]
    fn files_in_subtree_lists_all_nested_files_in_display_order() {
        let items = vec![
            item("/r/a", true),
            item("/r/a/z.rs", false),
            item("/r/a/deep", true),
            item("/r/a/deep/f.rs", false),
            item("/r/a/empty", true),
            item("/r/a/b.rs", false),
            item("/r/other.rs", false),
            // Not part of the tree: its parent is not listed.
            item("/r/a/missing/orphan.rs", false),
        ];
        let index = TreeIndex::build(&items, Path::new("/r"));
        let paths = |indices: Vec<usize>| -> Vec<&str> {
            indices
                .into_iter()
                .map(|i| items[i].path.to_str().unwrap())
                .collect()
        };

        assert_eq!(
            paths(index.files_in_subtree(&items, 0)),
            ["/r/a/deep/f.rs", "/r/a/b.rs", "/r/a/z.rs"]
        );
        assert_eq!(paths(index.files_in_subtree(&items, 2)), ["/r/a/deep/f.rs"]);
        assert!(index.files_in_subtree(&items, 4).is_empty());
        // A file is its own subtree.
        assert_eq!(paths(index.files_in_subtree(&items, 6)), ["/r/other.rs"]);
    }

    #[test]
    fn cache_reuses_index_for_same_key_and_rebuilds_otherwise() {
        let items = vec![item("/r/a", true), item("/r/a/f.rs", false)];
        let cache = TreeIndexCache::default();

        let first = cache.get_or_build(1, Path::new("/r"), &items);
        let again = cache.get_or_build(1, Path::new("/r"), &items);
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(cache.build_count(), 1);

        let new_revision = cache.get_or_build(2, Path::new("/r"), &items);
        assert!(!Arc::ptr_eq(&first, &new_revision));
        assert_eq!(cache.build_count(), 2);

        let new_root = cache.get_or_build(2, Path::new("/r/a"), &items);
        assert!(!Arc::ptr_eq(&new_revision, &new_root));
        assert_eq!(new_root.roots(), [1]);
        assert_eq!(cache.build_count(), 3);
    }
}
