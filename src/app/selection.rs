//! The one definition of how the selection of files adds up to the selection of the
//! directories above them.
//!
//! Both the tree sent to the UI (the tri-state checkbox of every row) and the commands
//! that act on a whole directory ("toggle everything below") ask the same question, so
//! both get their answer here, computed on the shared [`TreeIndex`].

use crate::app::tree_index::TreeIndex;
use crate::core::FileItem;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
    Some(SubtreeSelection {
        state: SelectionSummary::new(items, index, selected).state_of(top),
        files: index
            .files_in_subtree(items, top)
            .into_iter()
            .map(|file| items[file].path.clone())
            .collect(),
    })
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
