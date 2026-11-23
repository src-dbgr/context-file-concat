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
use std::hash::{Hash, Hasher};
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
    /// How many nodes the complete tree has, whatever is expanded.
    counts: TreeCounts,
    /// The number of files below each item, parallel to the item list: what
    /// [`TreeIndex::files_in_subtree`] would list for a directory (0 for files).
    files_below: Vec<u32>,
    /// How often each item is listed as a child of its parent, parallel to the item
    /// list, see [`TreeIndex::occurrences`].
    occurrences: Vec<u32>,
    /// Finds an item by its path without a second copy of every path.
    paths: PathLookup,
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
        let (paths, representatives) = PathLookup::build(items);

        let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
        let mut occurrences = vec![0u32; items.len()];
        let mut roots: Vec<usize> = Vec::new();
        for (item, &index) in items.iter().zip(&representatives) {
            let Some(parent) = item.path.parent() else {
                continue;
            };
            if let Some(parent_index) = paths.position(items, parent) {
                children[parent_index].push(index);
                occurrences[index] = occurrences[index].saturating_add(1);
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
        let files_below = count_files_below(items, &children);

        Self {
            names,
            children,
            roots,
            counts,
            files_below,
            occurrences,
            paths,
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
    /// `items` must be the list the index was built from. The lookup is a hash
    /// lookup (constant time on average): a click on one row must not cost a pass
    /// over the whole list.
    pub(crate) fn position(&self, items: &[FileItem], path: &Path) -> Option<usize> {
        debug_assert_eq!(self.len(), items.len());
        self.paths.position(items, path)
    }

    /// The number of files below the directory at `index`, i.e. the number of files
    /// its selection state is made of. 0 for a file.
    pub(crate) fn files_below(&self, index: usize) -> usize {
        self.files_below[index] as usize
    }

    /// The directory the item at `index` is listed in, if that directory is part of
    /// the list.
    ///
    /// Only a *directory* counts as a parent here: a file is never descended into,
    /// so whatever is listed below the path of a file belongs to no subtree.
    pub(crate) fn parent_directory(&self, items: &[FileItem], index: usize) -> Option<usize> {
        let parent = self.position(items, items[index].path.parent()?)?;
        items[parent].is_directory.then_some(parent)
    }

    /// How often the item at `index` is listed among the children of its parent
    /// (all occurrences of a path have the same parent). This is 1 for an item with
    /// a listed parent, except in a list that names a path more than once: every
    /// occurrence is a child of its own, all of them represented by the same item.
    /// 0 for an item whose parent is not listed.
    pub(crate) fn occurrences(&self, index: usize) -> usize {
        self.occurrences[index] as usize
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

/// Counts, for every directory, the files of its subtree (see
/// [`TreeIndex::files_in_subtree`]). Files are not descended into.
fn count_files_below(items: &[FileItem], children: &[Vec<usize>]) -> Vec<u32> {
    /// The count of one directory, computed after the counts of its subdirectories.
    fn count(
        dir: usize,
        items: &[FileItem],
        children: &[Vec<usize>],
        counts: &mut [Option<u32>],
    ) -> u32 {
        if let Some(known) = counts[dir] {
            return known;
        }
        let mut files = 0u32;
        for &child in &children[dir] {
            files = files.saturating_add(if items[child].is_directory {
                count(child, items, children, counts)
            } else {
                1
            });
        }
        counts[dir] = Some(files);
        files
    }

    let mut counts: Vec<Option<u32>> = vec![None; items.len()];
    for (index, item) in items.iter().enumerate() {
        if item.is_directory {
            count(index, items, children, &mut counts);
        }
    }
    counts.into_iter().map(|count| count.unwrap_or(0)).collect()
}

/// Finds the position of a path in an item list.
///
/// A hash table of positions: the paths themselves stay in the item list, so the
/// table costs a few bytes per item instead of a second copy of every path. If a path
/// occurs more than once, the last occurrence represents it.
#[derive(Debug)]
struct PathLookup {
    /// Open addressing with linear probing; the length is a power of two and at
    /// least twice the number of items.
    slots: Vec<Slot>,
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    /// The upper half of the hash of the path, to tell most other paths apart
    /// without looking at them.
    tag: u32,
    /// The position of the item, [`Slot::EMPTY`] for a free slot.
    position: u32,
}

impl Slot {
    const EMPTY: u32 = u32::MAX;
}

impl PathLookup {
    /// Builds the table. Also returns, parallel to `items`, the position of the item
    /// that represents each item's path (its own position, unless the path is listed
    /// again further down).
    fn build(items: &[FileItem]) -> (Self, Vec<usize>) {
        assert!(
            items.len() < Slot::EMPTY as usize,
            "the file list is too long to be indexed"
        );
        let capacity = (items.len() * 2).next_power_of_two().max(8);
        let mut slots = vec![
            Slot {
                tag: 0,
                position: Slot::EMPTY,
            };
            capacity
        ];

        let mut slot_of_item: Vec<usize> = Vec::with_capacity(items.len());
        for (position, item) in items.iter().enumerate() {
            let hash = hash_of(&item.path);
            let tag = tag_of(hash);
            let mut slot = hash as usize & (capacity - 1);
            loop {
                let entry = &mut slots[slot];
                if entry.position == Slot::EMPTY {
                    entry.tag = tag;
                    entry.position = position as u32;
                    break;
                }
                if entry.tag == tag && same_path(&items[entry.position as usize].path, &item.path) {
                    // Listed before: the later occurrence takes over.
                    entry.position = position as u32;
                    break;
                }
                slot = (slot + 1) & (capacity - 1);
            }
            slot_of_item.push(slot);
        }

        let representatives = slot_of_item
            .into_iter()
            .map(|slot| slots[slot].position as usize)
            .collect();
        (Self { slots }, representatives)
    }

    fn position(&self, items: &[FileItem], path: &Path) -> Option<usize> {
        let hash = hash_of(path);
        let tag = tag_of(hash);
        let mask = self.slots.len() - 1;
        let mut slot = hash as usize & mask;
        loop {
            let entry = self.slots[slot];
            if entry.position == Slot::EMPTY {
                return None;
            }
            if entry.tag == tag && same_path(&items[entry.position as usize].path, path) {
                return Some(entry.position as usize);
            }
            slot = (slot + 1) & mask;
        }
    }
}

fn tag_of(hash: u64) -> u32 {
    (hash >> 32) as u32
}

/// `a == b`, decided by the bytes where they are the same (the usual case: the UI
/// sends back the paths the host listed), and by the components otherwise.
fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str() == b.as_os_str() || a == b
}

/// The hash a path is looked up by. Equal paths have equal hashes (`Path` hashes its
/// components, like it compares them), and the hasher has no random state, so the
/// table is the same for the same list.
fn hash_of(path: &Path) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
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

/// A file list together with the hierarchy that was built from it.
///
/// Building the hierarchy of a large list takes time, and nothing about it needs the
/// application state: whoever has the list in a shareable form can build it without
/// holding the lock on the state, and hand both over together. That the index belongs
/// to exactly these items and this root is what this type stands for; there is no
/// other way to make one than [`IndexedList::build`].
#[derive(Debug)]
pub(crate) struct IndexedList {
    items: Arc<Vec<FileItem>>,
    root_path: PathBuf,
    index: Arc<TreeIndex>,
}

impl IndexedList {
    /// Builds the hierarchy of `items` below `root_path`.
    pub(crate) fn build(items: Arc<Vec<FileItem>>, root_path: PathBuf) -> Self {
        let index = Arc::new(TreeIndex::build(&items, &root_path));
        Self {
            items,
            root_path,
            index,
        }
    }

    /// The path the hierarchy was built below.
    pub(crate) fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// The list, the root path and the index, for the owner of a [`TreeIndexCache`].
    pub(crate) fn into_parts(self) -> (Arc<Vec<FileItem>>, PathBuf, Arc<TreeIndex>) {
        (self.items, self.root_path, self.index)
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

    /// `true` if the cache holds the index of the list of `revision` below `root_path`.
    pub(crate) fn holds(&self, revision: u64, root_path: &Path) -> bool {
        let slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_ref()
            .is_some_and(|cached| cached.revision == revision && cached.root_path == root_path)
    }

    /// Makes `index`, which was built elsewhere from the list of `revision` below
    /// `root_path`, the cached one. If the cache already holds the index of that
    /// list, it keeps it.
    ///
    /// Returns the index that is not needed any more (the one that was replaced, or
    /// `index` itself). Dropping the index of a large list is not free, so this is
    /// left to the caller, who may hold a lock at this point.
    pub(crate) fn install(
        &self,
        revision: u64,
        root_path: PathBuf,
        index: Arc<TreeIndex>,
    ) -> Option<Arc<TreeIndex>> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if slot
            .as_ref()
            .is_some_and(|cached| cached.revision == revision && cached.root_path == root_path)
        {
            return Some(index);
        }
        slot.replace(CachedIndex {
            revision,
            root_path,
            index,
        })
        .map(|replaced| replaced.index)
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
        // A path is found by what it means, not by how it is spelled.
        assert_eq!(index.position(&items, Path::new("/r/a/")), Some(2));
        assert_eq!(index.position(&items, Path::new("/r/./a/f.rs")), Some(1));
    }

    #[test]
    fn position_finds_every_item_of_a_long_list() {
        let items: Vec<FileItem> = (0..5_000)
            .map(|i| item(&format!("/r/dir{}/file{i}.rs", i % 70), false))
            .collect();
        let index = TreeIndex::build(&items, Path::new("/r"));

        for (position, listed) in items.iter().enumerate() {
            assert_eq!(index.position(&items, &listed.path), Some(position));
        }
        assert_eq!(index.position(&items, Path::new("/r/dir1/file0.rs")), None);
        assert_eq!(
            TreeIndex::build(&[], Path::new("/r")).position(&[], Path::new("/r/a")),
            None
        );
    }

    #[test]
    fn files_below_counts_what_files_in_subtree_lists() {
        let items = vec![
            item("/r/a", true),
            item("/r/a/z.rs", false),
            item("/r/a/deep", true),
            item("/r/a/deep/f.rs", false),
            item("/r/a/deep/g.rs", false),
            item("/r/a/empty", true),
            item("/r/other.rs", false),
            // A file is not descended into: what is listed below its path counts
            // for no directory.
            item("/r/a/z.rs/inside.rs", false),
        ];
        let index = TreeIndex::build(&items, Path::new("/r"));

        for (position, listed) in items.iter().enumerate() {
            let expected = if listed.is_directory {
                index.files_in_subtree(&items, position).len()
            } else {
                0
            };
            assert_eq!(index.files_below(position), expected, "{:?}", listed.path);
        }
        assert_eq!(index.files_below(0), 3);
    }

    #[test]
    fn parent_directory_is_the_listed_directory_an_item_lies_in() {
        let items = vec![
            item("/r/a", true),
            item("/r/a/f.rs", false),
            item("/r/a/f.rs/inside.rs", false),
            item("/r/missing/orphan.rs", false),
            item("/r/a", true),
        ];
        let index = TreeIndex::build(&items, Path::new("/r"));

        assert_eq!(index.parent_directory(&items, 1), Some(4));
        assert_eq!(index.parent_directory(&items, 4), None, "below the root");
        assert_eq!(index.parent_directory(&items, 3), None, "not listed");
        assert_eq!(
            index.parent_directory(&items, 2),
            None,
            "a file is no parent"
        );
        assert_eq!(index.occurrences(1), 1);
        assert_eq!(index.occurrences(3), 0, "its parent is not listed");
        assert_eq!(index.occurrences(4), 0, "below the root");
    }

    #[test]
    fn occurrences_count_how_often_a_path_is_listed_below_its_parent() {
        let items = vec![
            item("/r/a", true),
            item("/r/a/f.rs", false),
            item("/r/a/g.rs", false),
            item("/r/a/f.rs", false),
        ];
        let index = TreeIndex::build(&items, Path::new("/r"));

        // The last occurrence represents the path, and is a child twice.
        assert_eq!(index.children(0), [3, 3, 2]);
        assert_eq!(index.occurrences(3), 2);
        assert_eq!(index.occurrences(2), 1);
        assert_eq!(index.occurrences(1), 0, "represented by the item at 3");
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

    /// An index that was built elsewhere is cached for the list it names; the cache
    /// hands back whichever index is not needed.
    #[test]
    fn cache_takes_an_index_that_was_built_elsewhere() {
        let items = Arc::new(vec![item("/r/a", true), item("/r/a/f.rs", false)]);
        let cache = TreeIndexCache::default();
        assert!(!cache.holds(1, Path::new("/r")));

        let (_, root_path, built) =
            IndexedList::build(Arc::clone(&items), PathBuf::from("/r")).into_parts();
        assert_eq!(built.roots(), [0]);
        assert!(cache.install(1, root_path, Arc::clone(&built)).is_none());
        assert!(cache.holds(1, Path::new("/r")));
        assert!(!cache.holds(2, Path::new("/r")));
        assert!(!cache.holds(1, Path::new("/r/a")));
        assert!(Arc::ptr_eq(
            &cache.get_or_build(1, Path::new("/r"), &items),
            &built
        ));
        assert_eq!(cache.build_count(), 0);

        // The index of the same list a second time: the cached one stays.
        let (_, root_path, second) =
            IndexedList::build(Arc::clone(&items), PathBuf::from("/r")).into_parts();
        let unused = cache.install(1, root_path, Arc::clone(&second)).unwrap();
        assert!(Arc::ptr_eq(&unused, &second));
        assert!(Arc::ptr_eq(
            &cache.get_or_build(1, Path::new("/r"), &items),
            &built
        ));

        // The index of another list replaces it.
        let replaced = cache
            .install(2, PathBuf::from("/r"), Arc::clone(&second))
            .unwrap();
        assert!(Arc::ptr_eq(&replaced, &built));
        assert!(cache.holds(2, Path::new("/r")));
        assert_eq!(cache.build_count(), 0);
    }
}
