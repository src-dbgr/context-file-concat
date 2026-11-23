//! The one definition of how the selection of files adds up to the selection of the
//! directories above them.
//!
//! Both the rows sent to the UI (the tri-state checkbox of every row) and the commands
//! that act on a whole directory ("toggle everything below") ask the same question, so
//! both get their answer here, computed on the shared [`TreeIndex`].
//!
//! The rows need the answer for every directory on screen with every snapshot, and a
//! directory near the top of a large project has most of its files below it. The counts
//! are therefore kept ([`SelectionCountsCache`]) and follow single changes of the
//! selection ([`SelectedFiles`]) instead of being counted again.

use crate::app::tree_index::TreeIndex;
use crate::core::FileItem;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Hands out the revisions of all [`SelectedFiles`]. They are unique across the
/// process, so a revision identifies one content of one set.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

/// How many single changes [`SelectedFiles`] remembers. A click changes one file, a
/// click on a small directory a few; whatever changes more than this at once is
/// cheaper to count again than to replay. Of more single changes than this, the
/// oldest are forgotten.
const JOURNAL_CAPACITY: usize = 1024;

/// One file that was selected or deselected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionChange {
    /// The revision of the set after this change.
    revision: u64,
    pub path: PathBuf,
    pub selected: bool,
}

/// The paths of the selected files.
///
/// It reads like the `HashSet` it wraps. Changes go through its own methods, which
/// keep a revision and a short journal of the single changes: that is what lets the
/// selection counts of the directories ([`SelectionCountsCache`]) follow a click in
/// the time of the click instead of counting the whole tree again.
#[derive(Debug)]
pub struct SelectedFiles {
    files: HashSet<PathBuf>,
    /// Identifies the current content, see [`NEXT_REVISION`].
    revision: u64,
    /// The revision the journal starts from.
    journal_base: u64,
    /// The single changes since `journal_base`, oldest first.
    journal: Vec<SelectionChange>,
}

impl SelectedFiles {
    /// Selects `path`. Returns `true` if it was not selected before.
    pub fn insert(&mut self, path: PathBuf) -> bool {
        if self.files.contains(&path) {
            return false;
        }
        self.files.insert(path.clone());
        self.record(path, true);
        true
    }

    /// Deselects `path`. Returns `true` if it was selected.
    pub fn remove(&mut self, path: &Path) -> bool {
        if !self.files.remove(path) {
            return false;
        }
        self.record(path.to_path_buf(), false);
        true
    }

    /// Selects every path of `paths`.
    pub fn extend(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let paths = paths.into_iter();
        if paths.size_hint().0 > JOURNAL_CAPACITY {
            // More than the journal holds: not worth a journal entry each.
            let before = self.files.len();
            self.files.extend(paths);
            if self.files.len() != before {
                self.changed_wholesale();
            }
        } else {
            for path in paths {
                self.insert(path);
            }
        }
    }

    /// Deselects every path of `paths`.
    pub fn remove_all<'a>(&mut self, paths: impl IntoIterator<Item = &'a PathBuf>) {
        let paths = paths.into_iter();
        if paths.size_hint().0 > JOURNAL_CAPACITY {
            let before = self.files.len();
            for path in paths {
                self.files.remove(path);
            }
            if self.files.len() != before {
                self.changed_wholesale();
            }
        } else {
            for path in paths {
                self.remove(path);
            }
        }
    }

    /// Deselects everything.
    pub fn clear(&mut self) {
        if !self.files.is_empty() {
            self.files.clear();
            self.changed_wholesale();
        }
    }

    /// Keeps the paths `keep` returns `true` for.
    pub fn retain(&mut self, keep: impl FnMut(&PathBuf) -> bool) {
        let before = self.files.len();
        self.files.retain(keep);
        if self.files.len() != before {
            self.changed_wholesale();
        }
    }

    /// Identifies the current content of the set.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// The single changes that lead from the content identified by `revision` to the
    /// current one, oldest first; `None` if they are not known (the revision belongs
    /// to another set, or more has changed since than the journal holds).
    pub(crate) fn changes_since(&self, revision: u64) -> Option<&[SelectionChange]> {
        if revision == self.revision {
            return Some(&[]);
        }
        if revision == self.journal_base {
            return Some(&self.journal);
        }
        let position = self
            .journal
            .binary_search_by_key(&revision, |change| change.revision)
            .ok()?;
        Some(&self.journal[position + 1..])
    }

    fn record(&mut self, path: PathBuf, selected: bool) {
        if self.journal.len() >= JOURNAL_CAPACITY {
            // Forget the older half: whoever is that far behind starts over.
            let forgotten = JOURNAL_CAPACITY / 2;
            self.journal_base = self.journal[forgotten - 1].revision;
            self.journal.drain(..forgotten);
        }
        self.revision = next_revision();
        self.journal.push(SelectionChange {
            revision: self.revision,
            path,
            selected,
        });
    }

    /// A change that is not journaled: whoever knew an older revision starts over.
    fn changed_wholesale(&mut self) {
        self.revision = next_revision();
        self.journal_base = self.revision;
        self.journal.clear();
    }
}

impl Default for SelectedFiles {
    fn default() -> Self {
        Self::from(HashSet::new())
    }
}

impl From<HashSet<PathBuf>> for SelectedFiles {
    fn from(files: HashSet<PathBuf>) -> Self {
        let revision = next_revision();
        Self {
            files,
            revision,
            journal_base: revision,
            journal: Vec::new(),
        }
    }
}

impl FromIterator<PathBuf> for SelectedFiles {
    fn from_iter<I: IntoIterator<Item = PathBuf>>(paths: I) -> Self {
        Self::from(paths.into_iter().collect::<HashSet<PathBuf>>())
    }
}

impl std::ops::Deref for SelectedFiles {
    type Target = HashSet<PathBuf>;

    fn deref(&self) -> &Self::Target {
        &self.files
    }
}

impl PartialEq<HashSet<PathBuf>> for SelectedFiles {
    fn eq(&self, other: &HashSet<PathBuf>) -> bool {
        &self.files == other
    }
}

/// How much of an item is selected.
///
/// A file is either `Full` or `None`. A directory is `Full` if every file below it is
/// selected, `None` if none is (or if there is no file below it) and `Partial` otherwise.
///
/// Serialized in lowercase (`"none"`, `"partial"`, `"full"`), which is part of the UI
/// contract.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SelectionState {
    None,
    Partial,
    Full,
}

/// The number of selected and total files below a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionCounts {
    pub selected: usize,
    pub total_files: usize,
}

impl SelectionCounts {
    pub(crate) fn state(self) -> SelectionState {
        if self.total_files == 0 || self.selected == 0 {
            SelectionState::None
        } else if self.selected == self.total_files {
            SelectionState::Full
        } else {
            SelectionState::Partial
        }
    }
}

/// Answers selection questions about one file list.
///
/// Directory counts are memoized, so asking for every item of a tree costs O(n) in
/// total. `index` must have been built from exactly `items`.
pub(crate) struct SelectionSummary<'a> {
    items: &'a [FileItem],
    index: &'a TreeIndex,
    selected: &'a HashSet<PathBuf>,
    /// Memoized counts, parallel to `items`.
    counts: Vec<Option<SelectionCounts>>,
}

impl<'a> SelectionSummary<'a> {
    pub(crate) fn new(
        items: &'a [FileItem],
        index: &'a TreeIndex,
        selected: &'a HashSet<PathBuf>,
    ) -> Self {
        debug_assert_eq!(index.len(), items.len());
        Self {
            items,
            index,
            selected,
            counts: vec![None; items.len()],
        }
    }

    /// The selection state of the item at `item_index`.
    #[cfg(test)]
    pub(crate) fn state_of(&mut self, item_index: usize) -> SelectionState {
        let item = &self.items[item_index];
        if item.is_directory {
            self.counts_below(item_index).state()
        } else if self.selected.contains(&item.path) {
            SelectionState::Full
        } else {
            SelectionState::None
        }
    }

    /// Counts the selected and total files below the directory at `dir_index`.
    ///
    /// These are exactly the files [`TreeIndex::files_in_subtree`] lists.
    pub(crate) fn counts_below(&mut self, dir_index: usize) -> SelectionCounts {
        if let Some(counts) = self.counts[dir_index] {
            return counts;
        }

        let (items, index, selected) = (self.items, self.index, self.selected);
        let mut counts = SelectionCounts {
            selected: 0,
            total_files: 0,
        };
        for &child in index.children(dir_index) {
            let child_item = &items[child];
            if child_item.is_directory {
                let child_counts = self.counts_below(child);
                counts.selected += child_counts.selected;
                counts.total_files += child_counts.total_files;
            } else {
                counts.total_files += 1;
                if selected.contains(&child_item.path) {
                    counts.selected += 1;
                }
            }
        }

        self.counts[dir_index] = Some(counts);
        counts
    }
}

/// The selected files of `items` in the order of the tree, i.e. sorted by path.
///
/// `items` is meant to be the unfiltered list: what is selected stays selected when a
/// filter hides it, and a generation covers all of it. Selected paths that are not
/// listed (any more) are left out.
pub fn selected_files_in_tree_order(
    items: &[FileItem],
    selected: &HashSet<PathBuf>,
) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = items
        .iter()
        .filter(|item| !item.is_directory && selected.contains(&item.path))
        .map(|item| item.path.clone())
        .collect();
    files.sort();
    files
}

/// The selection of one subtree of the file tree: what a click on its checkbox acts on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SubtreeSelection {
    /// The selection state of the item at the top of the subtree.
    pub state: SelectionState,
    /// The files at or below that item, i.e. the files `state` is made of.
    pub files: Vec<PathBuf>,
}

/// Describes the selection of the subtree at `path`, or `None` if `path` is not listed.
///
/// `index` must have been built from exactly `items`.
pub(crate) fn subtree_selection(
    items: &[FileItem],
    index: &TreeIndex,
    selected: &HashSet<PathBuf>,
    path: &Path,
) -> Option<SubtreeSelection> {
    let top = index.position(items, path)?;
    let files: Vec<PathBuf> = index
        .files_in_subtree(items, top)
        .into_iter()
        .map(|file| items[file].path.clone())
        .collect();
    // Counted on the files of the subtree, so a click on a small directory does not
    // cost a pass over the whole list.
    let state = if items[top].is_directory {
        SelectionCounts {
            selected: files.iter().filter(|file| selected.contains(*file)).count(),
            total_files: files.len(),
        }
        .state()
    } else if selected.contains(path) {
        SelectionState::Full
    } else {
        SelectionState::None
    };
    Some(SubtreeSelection { state, files })
}

/// The number of selected files below every directory of one file list.
///
/// Parallel to the item list the [`TreeIndex`] was built from; the entry of a file is
/// not used. Together with [`TreeIndex::files_below`] it answers the selection state
/// of any directory, expanded or not, in constant time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectorySelection {
    selected_below: Vec<u32>,
}

impl DirectorySelection {
    /// Counts the whole list: one pass over the tree.
    fn count(items: &[FileItem], index: &TreeIndex, selected: &HashSet<PathBuf>) -> Self {
        let mut summary = SelectionSummary::new(items, index, selected);
        let selected_below = items
            .iter()
            .enumerate()
            .map(|(item_index, item)| {
                if item.is_directory {
                    summary.counts_below(item_index).selected as u32
                } else {
                    0
                }
            })
            .collect();
        Self { selected_below }
    }

    /// Follows one file that was selected or deselected: only the directories above
    /// it change.
    fn apply(&mut self, items: &[FileItem], index: &TreeIndex, change: &SelectionChange) {
        let Some(file) = index.position(items, &change.path) else {
            return;
        };
        if items[file].is_directory {
            return;
        }
        // How often the directory at hand counts the file; more than once only in a
        // list that names a path more than once.
        let mut weight = 1usize;
        let mut child = file;
        while let Some(parent) = index.parent_directory(items, child) {
            weight = weight.saturating_mul(index.occurrences_below(parent, child));
            let count = &mut self.selected_below[parent];
            *count = if change.selected {
                count.saturating_add(weight as u32)
            } else {
                count.saturating_sub(weight as u32)
            };
            child = parent;
        }
    }

    /// The selection state of the item at `item_index`.
    pub(crate) fn state_of(
        &self,
        items: &[FileItem],
        index: &TreeIndex,
        selected: &HashSet<PathBuf>,
        item_index: usize,
    ) -> SelectionState {
        let item = &items[item_index];
        if item.is_directory {
            SelectionCounts {
                selected: self.selected_below[item_index] as usize,
                total_files: index.files_below(item_index),
            }
            .state()
        } else if selected.contains(&item.path) {
            SelectionState::Full
        } else {
            SelectionState::None
        }
    }
}

/// Remembers the [`DirectorySelection`] of the most recent file list and selection.
///
/// It is keyed like the [`TreeIndexCache`](crate::app::tree_index::TreeIndexCache)
/// (revision of the filtered list, root path) plus the revision of the selection. For
/// the same list it follows the journal of the selection; everything else is counted
/// again.
#[derive(Debug, Default)]
pub(crate) struct SelectionCountsCache {
    slot: Mutex<Option<CachedSelection>>,
    #[cfg(test)]
    full_counts: std::sync::atomic::AtomicUsize,
}

#[derive(Debug)]
struct CachedSelection {
    list_revision: u64,
    root_path: PathBuf,
    selection_revision: u64,
    counts: Arc<DirectorySelection>,
}

impl SelectionCountsCache {
    /// The counts for `items` and `selected`. `index` must have been built from
    /// exactly `items`, and `list_revision` must change whenever `items` changes.
    pub(crate) fn get_or_update(
        &self,
        list_revision: u64,
        root_path: &Path,
        items: &[FileItem],
        index: &TreeIndex,
        selected: &SelectedFiles,
    ) -> Arc<DirectorySelection> {
        // The slot only ever holds a complete value, so a poisoned lock is still usable.
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());

        let reusable = slot.take().filter(|cached| {
            cached.list_revision == list_revision && cached.root_path == root_path
        });
        let counts = match reusable {
            Some(mut cached) => match selected.changes_since(cached.selection_revision) {
                Some([]) => cached.counts,
                Some(changes) => {
                    // Nobody else holds the counts between two snapshots, so this
                    // changes them in place.
                    let counts = Arc::make_mut(&mut cached.counts);
                    for change in changes {
                        counts.apply(items, index, change);
                    }
                    cached.counts
                }
                None => self.count(items, index, selected),
            },
            None => self.count(items, index, selected),
        };

        *slot = Some(CachedSelection {
            list_revision,
            root_path: root_path.to_path_buf(),
            selection_revision: selected.revision(),
            counts: Arc::clone(&counts),
        });
        counts
    }

    fn count(
        &self,
        items: &[FileItem],
        index: &TreeIndex,
        selected: &SelectedFiles,
    ) -> Arc<DirectorySelection> {
        #[cfg(test)]
        self.full_counts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Arc::new(DirectorySelection::count(items, index, selected))
    }

    /// How often the whole tree has been counted by this cache.
    #[cfg(test)]
    pub(crate) fn full_count_count(&self) -> usize {
        self.full_counts.load(std::sync::atomic::Ordering::Relaxed)
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

    fn project() -> Vec<FileItem> {
        vec![
            item("/r/src", true),
            item("/r/src/core", true),
            item("/r/src/core/a.rs", false),
            item("/r/src/core/b.rs", false),
            item("/r/src/main.rs", false),
            item("/r/empty", true),
            item("/r/readme.md", false),
        ]
    }

    fn selected(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn selection_states_of_files_and_directories() {
        let items = project();
        let index = TreeIndex::build(&items, Path::new("/r"));
        let position = |path: &str| index.position(&items, Path::new(path)).unwrap();

        // (selected files, item, expected state)
        let cases: &[(&[&str], &str, SelectionState)] = &[
            (&[], "/r/src", SelectionState::None),
            (&[], "/r/readme.md", SelectionState::None),
            (&["/r/readme.md"], "/r/readme.md", SelectionState::Full),
            (&["/r/readme.md"], "/r/src", SelectionState::None),
            (&["/r/src/core/a.rs"], "/r/src", SelectionState::Partial),
            (
                &["/r/src/core/a.rs"],
                "/r/src/core",
                SelectionState::Partial,
            ),
            (
                &["/r/src/core/a.rs", "/r/src/core/b.rs"],
                "/r/src/core",
                SelectionState::Full,
            ),
            (
                &["/r/src/core/a.rs", "/r/src/core/b.rs"],
                "/r/src",
                SelectionState::Partial,
            ),
            (
                &["/r/src/core/a.rs", "/r/src/core/b.rs", "/r/src/main.rs"],
                "/r/src",
                SelectionState::Full,
            ),
            // A directory without files is never selected, not even "fully".
            (&["/r/empty"], "/r/empty", SelectionState::None),
            // Selected paths that are not listed do not count.
            (&["/r/src/unlisted.rs"], "/r/src", SelectionState::None),
        ];

        for (selection, path, expected) in cases {
            let selection = selected(selection);
            let mut summary = SelectionSummary::new(&items, &index, &selection);
            assert_eq!(
                summary.state_of(position(path)),
                *expected,
                "{path} with {selection:?}"
            );
        }
    }

    #[test]
    fn counts_cover_exactly_the_files_of_the_subtree() {
        let items = project();
        let index = TreeIndex::build(&items, Path::new("/r"));
        let selection = selected(&["/r/src/core/a.rs", "/r/readme.md"]);
        let mut summary = SelectionSummary::new(&items, &index, &selection);

        for (dir_index, dir) in items.iter().enumerate().filter(|(_, i)| i.is_directory) {
            let files = index.files_in_subtree(&items, dir_index);
            let counts = summary.counts_below(dir_index);
            assert_eq!(counts.total_files, files.len(), "{:?}", dir.path);
            assert_eq!(
                counts.selected,
                files
                    .iter()
                    .filter(|&&file| selection.contains(&items[file].path))
                    .count(),
                "{:?}",
                dir.path
            );
        }
    }

    #[test]
    fn subtree_selection_reports_state_and_files_of_the_subtree() {
        let items = project();
        let index = TreeIndex::build(&items, Path::new("/r"));
        let selection = selected(&["/r/src/core/a.rs", "/r/src/core/b.rs", "/r/readme.md"]);
        let subtree = |path: &str| subtree_selection(&items, &index, &selection, Path::new(path));
        let files = |paths: &[&str]| paths.iter().map(PathBuf::from).collect::<Vec<_>>();

        assert_eq!(
            subtree("/r/src"),
            Some(SubtreeSelection {
                state: SelectionState::Partial,
                files: files(&["/r/src/core/a.rs", "/r/src/core/b.rs", "/r/src/main.rs"]),
            })
        );
        assert_eq!(
            subtree("/r/src/core"),
            Some(SubtreeSelection {
                state: SelectionState::Full,
                files: files(&["/r/src/core/a.rs", "/r/src/core/b.rs"]),
            })
        );
        assert_eq!(
            subtree("/r/empty"),
            Some(SubtreeSelection {
                state: SelectionState::None,
                files: Vec::new(),
            })
        );
        // A file is a subtree of its own.
        assert_eq!(
            subtree("/r/readme.md"),
            Some(SubtreeSelection {
                state: SelectionState::Full,
                files: files(&["/r/readme.md"]),
            })
        );
        // Paths that are not listed (the root included) have no subtree.
        assert_eq!(subtree("/r"), None);
        assert_eq!(subtree("/r/unlisted"), None);
    }

    fn path(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn selected_files_read_like_a_set_and_report_real_changes() {
        let mut files = SelectedFiles::default();
        let initial = files.revision();

        assert!(files.insert(path("/r/a")));
        assert!(!files.insert(path("/r/a")), "already selected");
        assert!(!files.remove(Path::new("/r/b")), "not selected");
        assert!(files.contains(Path::new("/r/a")));
        assert_eq!(files.len(), 1);
        assert_eq!(files, selected(&["/r/a"]));

        // What changed nothing is not a change.
        let changes = files.changes_since(initial).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            (changes[0].path.as_path(), changes[0].selected),
            (Path::new("/r/a"), true)
        );
        assert_eq!(files.changes_since(files.revision()), Some(&[][..]));

        files.extend([path("/r/b"), path("/r/a"), path("/r/c")]);
        assert!(files.remove(Path::new("/r/a")));
        let order: Vec<(&str, bool)> = files
            .changes_since(initial)
            .unwrap()
            .iter()
            .map(|change| (change.path.to_str().unwrap(), change.selected))
            .collect();
        assert_eq!(
            order,
            [
                ("/r/a", true),
                ("/r/b", true),
                ("/r/c", true),
                ("/r/a", false)
            ]
        );
        // From a revision in between, only what came after it.
        let after_first = changes_revision(&files, 0);
        assert_eq!(files.changes_since(after_first).unwrap().len(), 3);
    }

    /// The revision of the set after the journaled change at `position`.
    fn changes_revision(files: &SelectedFiles, position: usize) -> u64 {
        files.journal[position].revision
    }

    #[test]
    fn wholesale_changes_and_other_sets_have_no_known_changes() {
        let mut files: SelectedFiles = selected(&["/r/a", "/r/b"]).into();
        let before = files.revision();

        files.retain(|_| true);
        assert_eq!(files.revision(), before, "nothing was removed");
        files.retain(|file| file != Path::new("/r/a"));
        assert_eq!(files.changes_since(before), None);
        assert_eq!(files, selected(&["/r/b"]));

        let before = files.revision();
        files.clear();
        assert_eq!(files.changes_since(before), None);
        let empty = files.revision();
        files.clear();
        assert_eq!(files.revision(), empty, "nothing to clear");

        // More changes at once than the journal holds are a wholesale change, in
        // both directions.
        let many: Vec<PathBuf> = (0..=JOURNAL_CAPACITY)
            .map(|i| path(&format!("/r/{i}")))
            .collect();
        let before = files.revision();
        files.extend(many.clone());
        assert_eq!(files.len(), JOURNAL_CAPACITY + 1);
        assert_eq!(files.changes_since(before), None);
        assert!(files.changes_since(files.revision()).unwrap().is_empty());
        let unchanged = files.revision();
        files.extend(many.clone());
        assert_eq!(files.revision(), unchanged, "all of them were selected");

        let before = files.revision();
        files.remove_all(&many);
        assert!(files.is_empty());
        assert_eq!(files.changes_since(before), None);
        // A few at once are single changes.
        files.extend(many[..3].to_vec());
        let before = files.revision();
        files.remove_all(&many[1..5]);
        assert_eq!(files.changes_since(before).unwrap().len(), 2);
        assert_eq!(files, selected(&["/r/0"]));

        // The revision of another set says nothing about this one.
        let other: SelectedFiles = selected(&["/r/a"]).into();
        assert_eq!(files.changes_since(other.revision()), None);
        assert_ne!(
            SelectedFiles::default().revision(),
            SelectedFiles::default().revision()
        );
    }

    #[test]
    fn the_journal_forgets_its_oldest_changes_only() {
        let mut files = SelectedFiles::default();
        let start = files.revision();
        let mut revisions = Vec::new();
        for i in 0..JOURNAL_CAPACITY * 3 {
            files.insert(path(&format!("/r/{i}")));
            revisions.push(files.revision());
        }

        // Whoever is far behind starts over; whoever follows closely does not.
        assert_eq!(files.changes_since(start), None);
        assert_eq!(files.changes_since(revisions[10]), None);
        let recent = revisions[revisions.len() - 4];
        let changes = files.changes_since(recent).unwrap();
        assert_eq!(changes.len(), 3);
        assert_eq!(
            changes[0].path,
            path(&format!("/r/{}", JOURNAL_CAPACITY * 3 - 3))
        );
        assert!(files.journal.len() <= JOURNAL_CAPACITY);
        assert_eq!(files.len(), JOURNAL_CAPACITY * 3);
    }

    #[test]
    fn directory_selection_follows_the_journal_and_matches_a_fresh_count() {
        let items = project();
        let index = TreeIndex::build(&items, Path::new("/r"));
        let cache = SelectionCountsCache::default();
        let mut files = SelectedFiles::default();
        let state_of = |counts: &DirectorySelection, files: &SelectedFiles, path: &str| {
            let position = index.position(&items, Path::new(path)).unwrap();
            counts.state_of(&items, &index, files, position)
        };

        let counts = cache.get_or_update(1, Path::new("/r"), &items, &index, &files);
        assert_eq!(state_of(&counts, &files, "/r/src"), SelectionState::None);
        drop(counts);
        assert_eq!(cache.full_count_count(), 1);

        let clicks = [
            "/r/src/core/a.rs",
            "/r/readme.md",
            "/r/src/core/b.rs",
            "/r/src/unlisted.rs",
            "/r/src/main.rs",
            "/r/src/core/a.rs",
            "/r/empty",
        ];
        for click in clicks {
            if !files.remove(Path::new(click)) {
                files.insert(path(click));
            }
            let counts = cache.get_or_update(1, Path::new("/r"), &items, &index, &files);
            assert_eq!(
                *counts,
                DirectorySelection::count(&items, &index, &files),
                "after {click}"
            );
        }
        assert_eq!(cache.full_count_count(), 1, "single changes are followed");

        let counts = cache.get_or_update(1, Path::new("/r"), &items, &index, &files);
        assert_eq!(state_of(&counts, &files, "/r/src"), SelectionState::Partial);
        assert_eq!(
            state_of(&counts, &files, "/r/src/core"),
            SelectionState::Partial
        );
        assert_eq!(
            state_of(&counts, &files, "/r/src/main.rs"),
            SelectionState::Full
        );
        assert_eq!(state_of(&counts, &files, "/r/empty"), SelectionState::None);
        drop(counts);

        // Another list, another root or a wholesale change: counted again.
        cache.get_or_update(2, Path::new("/r"), &items, &index, &files);
        assert_eq!(cache.full_count_count(), 2);
        cache.get_or_update(2, Path::new("/other"), &items, &index, &files);
        assert_eq!(cache.full_count_count(), 3);
        files.clear();
        let counts = cache.get_or_update(2, Path::new("/other"), &items, &index, &files);
        assert_eq!(cache.full_count_count(), 4);
        assert_eq!(state_of(&counts, &files, "/r/src"), SelectionState::None);
    }

    #[test]
    fn serializes_in_lowercase() {
        let json = |state: SelectionState| serde_json::to_string(&state).unwrap();
        assert_eq!(json(SelectionState::None), r#""none""#);
        assert_eq!(json(SelectionState::Partial), r#""partial""#);
        assert_eq!(json(SelectionState::Full), r#""full""#);
    }

    #[test]
    fn selected_files_are_listed_in_tree_order() {
        let item = |path: &str, is_directory| FileItem {
            path: PathBuf::from(path),
            is_directory,
            ..Default::default()
        };
        // Scan order is not tree order.
        let items = vec![
            item("/p/z.rs", false),
            item("/p/src", true),
            item("/p/src/b.rs", false),
            item("/p/a.rs", false),
            item("/p/src/a.rs", false),
        ];
        let selected = HashSet::from([
            PathBuf::from("/p/z.rs"),
            PathBuf::from("/p/src/b.rs"),
            PathBuf::from("/p/src/a.rs"),
            // A selected directory is not a file; an unlisted path is gone.
            PathBuf::from("/p/src"),
            PathBuf::from("/p/removed.rs"),
        ]);

        assert_eq!(
            selected_files_in_tree_order(&items, &selected),
            [
                PathBuf::from("/p/src/a.rs"),
                PathBuf::from("/p/src/b.rs"),
                PathBuf::from("/p/z.rs"),
            ]
        );
        assert!(selected_files_in_tree_order(&items, &HashSet::new()).is_empty());
    }
}
