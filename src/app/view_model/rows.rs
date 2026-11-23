//! The rows of the file tree: what the UI shows, top to bottom.
//!
//! The tree is shown as a flat list of rows: every listed item whose ancestors are all
//! expanded, in display order. [`VisibleRows`] is that list for one filtered file list
//! and one set of expanded directories. It only refers to items (position and depth),
//! so it stays valid while selection, preview and matches change, and it is cached
//! ([`VisibleRowsCache`]) until the structure or the expansion changes.
//!
//! A snapshot carries a *window* of the rows ([`Viewport`]): the part the UI has on
//! screen plus its overscan. The size of a snapshot therefore depends on the size of
//! the window, not on the size of the project or on how much of it is expanded.

use super::lossy_path;
use crate::app::selection::{DirectorySelection, SelectionState};
use crate::app::state::AppState;
use crate::app::tree_index::TreeIndex;
use crate::core::{FileItem, FileNameMatcher};
use serde::Serialize;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One row of the tree as the UI renders it.
///
/// Everything the row needs is part of it: the UI neither knows the rows outside its
/// window nor the children of a directory.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TreeRow {
    pub name: String,
    /// Written lossily, so that a name that is not valid Unicode cannot make the
    /// snapshot unserializable (see [`lossy_path`]).
    #[serde(serialize_with = "lossy_path::serialize")]
    pub path: PathBuf,
    /// The nesting level: 0 for the items directly below the loaded directory.
    pub depth: usize,
    pub is_directory: bool,
    pub is_binary: bool,
    pub size: u64,
    /// Whether the directory has children in the filtered list, expanded or not.
    /// Always `false` for files.
    pub has_children: bool,
    /// For a directory: made of all files below it, wherever they are in the list.
    pub selection_state: SelectionState,
    pub is_expanded: bool,
    pub is_match: bool,
    pub is_previewed: bool,
    /// `false` for a directory whose content has not been scanned yet (lazy loading).
    /// Always `true` for files.
    pub children_loaded: bool,
}

/// The part of the rows the UI asks for: `count` rows from row `start` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    start: usize,
    count: usize,
}

impl Viewport {
    /// The window of a UI that has not reported its viewport yet.
    pub const DEFAULT_ROWS: usize = 100;
    /// The largest window the host sends, whatever the UI asks for. This is what
    /// bounds the size of a snapshot. A viewport of 2160 px shows 78 rows of 28 px;
    /// with the same number above and below as overscan the UI asks for about 240.
    pub const MAX_ROWS: usize = 600;

    /// A viewport of `count` rows (at least 1, at most [`Viewport::MAX_ROWS`]) from
    /// row `start` on.
    pub fn new(start: usize, count: usize) -> Self {
        Self {
            start,
            count: count.clamp(1, Self::MAX_ROWS),
        }
    }

    /// The rows of the window in a list of `total` rows.
    ///
    /// A window that starts behind the end of the list (the list has become shorter
    /// since the UI reported its viewport) is moved up to the end, so the UI is not
    /// left without rows until it has reported again.
    pub fn window(&self, total: usize) -> Range<usize> {
        let count = self.count.min(total);
        let start = self.start.min(total - count);
        start..start + count
    }

    /// The same number of rows from the top of the list.
    fn at_top(self) -> Self {
        Self { start: 0, ..self }
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new(0, Self::DEFAULT_ROWS)
    }
}

/// The answer to the most recent `locateRow` request: which row the UI asked for.
///
/// The host remembers the *path* that was found, not the row: every snapshot states
/// where that path is in its own row list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LocateAnswer {
    request: u32,
    path: Option<PathBuf>,
}

/// The answer to a `locateRow` request as a snapshot states it.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct LocatedRow {
    /// The number the UI gave its request.
    pub request: u32,
    /// The row in `rows_total` of this snapshot; `None` if there is no such row (the
    /// item is not listed or below a collapsed directory, or nothing matched).
    pub row: Option<usize>,
}

/// What the host keeps about the view of the tree: the window the UI looks at, the
/// answer to its last row lookup and the cached row list.
///
/// It is view state, not project state: nothing here says anything about the files.
/// The application state hosts it so that it lives and is locked with the data it is
/// derived from.
#[derive(Debug, Default)]
pub struct TreeViewState {
    viewport: Viewport,
    located: Option<LocateAnswer>,
    rows: VisibleRowsCache,
}

impl TreeViewState {
    /// The window the UI looks at.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Stores the window the UI reported.
    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    /// Stores the answer to the `locateRow` request `request`: the path that was
    /// found, `None` if nothing was.
    pub(crate) fn answer_locate(&mut self, request: u32, path: Option<PathBuf>) {
        self.located = Some(LocateAnswer { request, path });
    }

    /// A new tree is shown (another directory, or none): the window starts at the
    /// top again and the last lookup is void. The size of the window is the UI's.
    pub fn reset(&mut self) {
        self.viewport = self.viewport.at_top();
        self.located = None;
    }
}

/// The rows of one tree, see the [module documentation](self).
#[derive(Debug)]
pub(crate) struct VisibleRows {
    /// Changes whenever the list is built anew; 0 for the empty list of a tree that
    /// is not shown.
    revision: u64,
    rows: Vec<RowRef>,
    /// The row of each item, parallel to the item list; [`NOT_A_ROW`] for items
    /// that are not shown.
    row_of_item: Vec<u32>,
}

/// One row: an item of the list and its nesting level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowRef {
    item: u32,
    depth: u32,
}

const NOT_A_ROW: u32 = u32::MAX;

impl VisibleRows {
    /// The list of a tree that is not shown.
    fn none() -> Self {
        Self {
            revision: 0,
            rows: Vec::new(),
            row_of_item: Vec::new(),
        }
    }

    /// Lists the rows of `items`: preorder, descending into expanded directories only.
    ///
    /// `index` must have been built from exactly `items`.
    fn build(
        items: &[FileItem],
        index: &TreeIndex,
        expanded: &HashSet<PathBuf>,
        revision: u64,
    ) -> Self {
        debug_assert_eq!(index.len(), items.len());

        let mut rows: Vec<RowRef> = Vec::new();
        let mut row_of_item = vec![NOT_A_ROW; items.len()];
        // The siblings still to be listed, per level.
        let mut pending = vec![index.roots().iter()];

        while let Some(siblings) = pending.last_mut() {
            let Some(&item_index) = siblings.next() else {
                pending.pop();
                continue;
            };
            let depth = (pending.len() - 1) as u32;
            row_of_item[item_index] = rows.len() as u32;
            rows.push(RowRef {
                item: item_index as u32,
                depth,
            });

            let item = &items[item_index];
            if item.is_directory && expanded.contains(&item.path) {
                pending.push(index.children(item_index).iter());
            }
        }

        Self {
            revision,
            rows,
            row_of_item,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// The row that shows the item at `item_index`, if it is shown.
    pub(crate) fn row_of(&self, item_index: usize) -> Option<usize> {
        self.row_of_item
            .get(item_index)
            .filter(|&&row| row != NOT_A_ROW)
            .map(|&row| row as usize)
    }

    /// The item the row at `row` shows.
    fn item_at(&self, row: usize) -> usize {
        self.rows[row].item as usize
    }
}

/// Remembers the [`VisibleRows`] of the most recent tree.
///
/// Keyed by what the list depends on: the filtered file list (its revision), the root
/// path and the expanded directories (their revision). Selection, preview and matches
/// are not part of the key; changing them reuses the list.
#[derive(Debug, Default)]
struct VisibleRowsCache {
    slot: Mutex<CachedRows>,
}

#[derive(Debug, Default)]
struct CachedRows {
    /// How often a list has been built: the revision of the next one is this plus one.
    builds: u64,
    current: Option<(RowsKey, Arc<VisibleRows>)>,
}

#[derive(Debug, PartialEq, Eq)]
struct RowsKey {
    list_revision: u64,
    root_path: PathBuf,
    expansion_revision: u64,
}

impl VisibleRowsCache {
    fn get_or_build(&self, state: &AppState) -> Arc<VisibleRows> {
        // The slot only ever holds a complete value, so a poisoned lock is still usable.
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());

        let key = RowsKey {
            list_revision: state.filtered_revision(),
            root_path: PathBuf::from(&state.current_path),
            expansion_revision: state.expanded_dirs.revision(),
        };
        if let Some((cached_key, rows)) = slot.current.as_ref() {
            if *cached_key == key {
                return Arc::clone(rows);
            }
        }

        slot.builds += 1;
        let rows = Arc::new(VisibleRows::build(
            state.filtered_file_list(),
            &state.file_tree(),
            &state.expanded_dirs,
            slot.builds,
        ));
        slot.current = Some((key, Arc::clone(&rows)));
        rows
    }

    #[cfg(test)]
    fn build_count(&self) -> u64 {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).builds
    }
}

/// `true` if the tree is not shown at all, whatever the file list holds.
fn is_tree_suppressed(state: &AppState) -> bool {
    (state.is_scanning() && state.full_file_list.is_empty())
        // A content filter without matches leaves nothing to show. (While the first
        // search for a query is still running there is no content filter yet.)
        || state
            .content_filter_matches()
            .is_some_and(|matches| matches.is_empty())
}

/// The rows of the tree as the UI shows it for `state`.
pub(crate) fn visible_rows(state: &AppState) -> Arc<VisibleRows> {
    if is_tree_suppressed(state) {
        return Arc::new(VisibleRows::none());
    }
    state.tree_view.rows.get_or_build(state)
}

/// How often the row list of `state` has been built.
#[cfg(test)]
pub(crate) fn rows_build_count(state: &AppState) -> u64 {
    state.tree_view.rows.build_count()
}

/// The tree part of a snapshot.
pub(super) struct TreeSnapshot {
    /// The rows of the window.
    pub rows: Vec<TreeRow>,
    /// The row the window starts at.
    pub rows_start: usize,
    /// The number of rows of the whole list.
    pub rows_total: usize,
    pub rows_revision: u64,
    pub located: Option<LocatedRow>,
    /// `true` if the tree is shown (see [`is_tree_suppressed`]); its counters are
    /// zero otherwise.
    pub is_shown: bool,
}

/// Builds the rows of the current window, and states where the located path is.
pub(super) fn tree_snapshot(state: &AppState) -> TreeSnapshot {
    let rows = visible_rows(state);
    let window = state.tree_view.viewport.window(rows.len());
    let items = state.filtered_file_list();
    let index = state.file_tree();

    let located = state.tree_view.located.as_ref().map(|answer| LocatedRow {
        request: answer.request,
        row: answer
            .path
            .as_deref()
            .and_then(|path| row_of_path(items, &index, &rows, path)),
    });

    TreeSnapshot {
        rows: rows_in(state, &rows, window.clone()),
        rows_start: window.start,
        rows_total: rows.len(),
        rows_revision: rows.revision(),
        located,
        is_shown: !is_tree_suppressed(state),
    }
}

/// Builds the rows `window` of the list.
fn rows_in(state: &AppState, rows: &VisibleRows, window: Range<usize>) -> Vec<TreeRow> {
    if window.is_empty() {
        return Vec::new();
    }
    let view = RowView {
        items: state.filtered_file_list(),
        index: &state.file_tree(),
        selected: &state.selected_files,
        directory_selection: &state.directory_selection(),
        expanded: &state.expanded_dirs,
        loaded_dirs: &state.loaded_dirs,
        content_search_matches: &state.content_search_results,
        name_matcher: &state.file_name_matcher(),
        previewed_path: state.previewed_file_path.as_deref(),
    };
    rows.rows[window].iter().map(|row| view.row(*row)).collect()
}

/// Every row of the tree, whatever the viewport is.
#[cfg(test)]
pub(crate) fn all_rows(state: &AppState) -> Vec<TreeRow> {
    let rows = visible_rows(state);
    rows_in(state, &rows, 0..rows.len())
}

fn row_of_path(
    items: &[FileItem],
    index: &TreeIndex,
    rows: &VisibleRows,
    path: &Path,
) -> Option<usize> {
    rows.row_of(index.position(items, path)?)
}

/// Everything that decides what a row shows, borrowed for one snapshot.
struct RowView<'a> {
    items: &'a [FileItem],
    index: &'a TreeIndex,
    selected: &'a HashSet<PathBuf>,
    directory_selection: &'a DirectorySelection,
    expanded: &'a HashSet<PathBuf>,
    loaded_dirs: &'a HashSet<PathBuf>,
    content_search_matches: &'a HashSet<PathBuf>,
    /// Decides which names are pointed out as matches of the query or extension filter.
    name_matcher: &'a FileNameMatcher,
    previewed_path: Option<&'a Path>,
}

impl RowView<'_> {
    fn row(&self, row: RowRef) -> TreeRow {
        let item_index = row.item as usize;
        let item = &self.items[item_index];
        TreeRow {
            name: self.index.name(item_index).to_string(),
            path: item.path.clone(),
            depth: row.depth as usize,
            is_directory: item.is_directory,
            is_binary: item.is_binary,
            size: item.size,
            has_children: item.is_directory && !self.index.children(item_index).is_empty(),
            selection_state: self.directory_selection.state_of(
                self.items,
                self.index,
                self.selected,
                item_index,
            ),
            is_expanded: self.expanded.contains(&item.path),
            is_match: self.name_matcher.matches_any(&item.path)
                || self.content_search_matches.contains(&item.path),
            is_previewed: self.previewed_path == Some(item.path.as_path()),
            children_loaded: !item.is_directory || self.loaded_dirs.contains(&item.path),
        }
    }
}

/// The path of the first row from row `from` on (wrapping around at the end) whose
/// name starts with `prefix`, ignoring case; `None` if there is none or the prefix is
/// empty. This is the type-ahead of the tree: it has to look at rows the UI does not
/// have.
pub(crate) fn typeahead_match(state: &AppState, prefix: &str, from: usize) -> Option<PathBuf> {
    let prefix = prefix.to_lowercase();
    if prefix.is_empty() {
        return None;
    }
    let rows = visible_rows(state);
    let index = state.file_tree();
    let from = from.min(rows.len());

    (from..rows.len())
        .chain(0..from)
        .map(|row| rows.item_at(row))
        .find(|&item| starts_with_ignoring_case(index.name(item), &prefix))
        .map(|item| state.filtered_file_list()[item].path.clone())
}

/// `true` if `name`, in lowercase, starts with `lowercase_prefix`.
fn starts_with_ignoring_case(name: &str, lowercase_prefix: &str) -> bool {
    let mut name = name.chars().flat_map(char::to_lowercase);
    lowercase_prefix
        .chars()
        .all(|expected| name.next() == Some(expected))
}
