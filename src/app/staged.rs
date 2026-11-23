//! Changes of the file list in three steps, of which the expensive one holds no lock.
//!
//! Whatever changes the file list or the filters means work on the whole list: the
//! list itself, the filtered list, its hierarchy. That work grows with the project,
//! and every command (and the walk of a scan, which reports its progress) waits
//! while the state is locked. The events that happen while the user works in a
//! large project are therefore brought into the state in three steps:
//!
//! 1. **Stage** (locked): take what the work needs out of the state. That is the
//!    file list (a shared handle, no copy), its revision and the inputs of the
//!    filters ([`FilterInputs`]), plus what the event brings.
//! 2. **Prepare** (not locked): build the new file list, the filtered list and its
//!    hierarchy ([`Staged::prepare`]).
//! 3. **Commit** (locked): install the result, **if the state is still the one it
//!    was prepared for**. That is the case if the file list has the same revision,
//!    the filtered list has the same revision and the inputs of the filters are the
//!    same (`Base::is_current`). None of this is assumed: every change of the
//!    file list gives it a new revision, and the matches of a content search are a
//!    new set whenever they are replaced. Otherwise what was prepared is dropped
//!    and the event takes the plain way under the lock (the methods of
//!    [`AppState`], then [`apply_filters`]); the result is the same.
//!
//! Between 1 and 3 the state is consistent and anything may happen to it: until 3
//! the event has simply not happened yet. Whether the event may happen at all is
//! decided by its token, in step 1 and again in step 3: a scan that was cancelled
//! or superseded, a lazy load of another file list and a content search that was
//! followed by another one change nothing.
//!
//! The events: the items a scan's walk has listed ([`stage_scanned_items`]), the
//! end of that walk ([`stage_scan_completion`]), a directory level that was loaded
//! on demand ([`stage_directory_level`]) and the matches of a content search
//! ([`stage_content_matches`]).

use crate::app::filtering::{apply_filters, FilterInputs};
use crate::app::selection::{Rejected, SelectionSnapshot};
use crate::app::state::{
    directories_of, unlisted_items, AppState, ContentSearchToken, Discarded, FileLists,
    LazyLoadToken, ScanToken,
};
use crate::app::tree_index::{IndexedList, ListedPaths};
use crate::core::FileItem;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a change was prepared for: the state as far as the result depends on it.
struct Base {
    full: Arc<Vec<FileItem>>,
    file_list_revision: u64,
    filtered_revision: u64,
    /// The filtered list is the file list.
    mirrored: bool,
    inputs: FilterInputs,
}

impl Base {
    fn of(state: &AppState) -> Self {
        Self {
            full: state.shared_full_file_list(),
            file_list_revision: state.file_list_revision(),
            filtered_revision: state.filtered_revision(),
            mirrored: state.filtered_list_is_file_list(),
            inputs: FilterInputs::of(state),
        }
    }

    /// `true` if `state` is still what this was taken from, as far as the lists
    /// and the filters go.
    fn is_current(&self, state: &AppState) -> bool {
        self.file_list_revision == state.file_list_revision()
            && self.filtered_revision == state.filtered_revision()
            && self.mirrored == state.filtered_list_is_file_list()
            && self.inputs.is_current(state)
    }
}

/// An event after step 1: what it brings, and the state it met.
pub(crate) struct Staged<C> {
    base: Base,
    change: C,
}

/// An event after step 2: the lists as they are to be, and what step 3 needs of the
/// event besides them.
pub(crate) struct Prepared<R> {
    base: Base,
    lists: FileLists,
    rest: R,
}

/// What an event does to the file list and to the inputs of the filters. This is
/// the part of an event that runs without the lock.
pub(crate) trait Change {
    /// What is left of the event for step 3.
    type Rest: Send + 'static;

    /// The outcome of the event for the file list `full` and the filter inputs
    /// `inputs`.
    fn apply(self, full: &Arc<Vec<FileItem>>, inputs: &FilterInputs) -> Outcome<Self::Rest>;
}

/// See [`Change::apply`].
pub(crate) struct Outcome<R> {
    /// The new file list; `None` if the event leaves it as it is.
    full: Option<Arc<Vec<FileItem>>>,
    /// The inputs of the filters as they are after the event.
    inputs: FilterInputs,
    /// `true` if the new file list is the old one with items appended.
    appended: bool,
    rest: R,
}

impl<C: Change> Staged<C> {
    /// Step 2: builds the lists. This is the expensive part; call it without the
    /// lock.
    pub(crate) fn prepare(self) -> Prepared<C::Rest> {
        let Staged { base, change } = self;
        let outcome = change.apply(&base.full, &base.inputs);
        let identity = outcome.inputs.lets_everything_through();

        // The file list stays, and the filtered list is it and remains it: there
        // is nothing to build.
        let lists = if outcome.full.is_none() && base.mirrored && identity {
            FileLists {
                full: None,
                filtered: None,
                appended_from: None,
            }
        } else {
            let filtered = outcome
                .inputs
                .apply(outcome.full.as_ref().unwrap_or(&base.full));
            let root_path = outcome.inputs.root_path().to_path_buf();
            FileLists {
                appended_from: (outcome.appended && base.mirrored && identity)
                    .then_some(base.full.len()),
                filtered: Some(IndexedList::build(filtered, root_path)),
                full: outcome.full,
            }
        };
        Prepared {
            base,
            lists,
            rest: outcome.rest,
        }
    }
}

/// `list` with `items` appended, as a list of its own.
fn appended(list: &[FileItem], items: &[FileItem]) -> Arc<Vec<FileItem>> {
    let mut longer = Vec::with_capacity(list.len() + items.len());
    longer.extend_from_slice(list);
    longer.extend_from_slice(items);
    Arc::new(longer)
}

/// What is dropped with a [`Prepared`] that is not used.
fn unused<R: Send + 'static>(prepared: Prepared<R>) -> Discarded {
    let mut discarded = Discarded::default();
    discarded.add((prepared.base.full, prepared.lists, prepared.rest));
    discarded
}

//================================================================================//
//|                          THE ITEMS OF A SCAN'S WALK                          |//
//================================================================================//

/// The items of a walk that are not in the list yet.
pub(crate) struct ScannedItems(Vec<FileItem>);

impl Change for ScannedItems {
    type Rest = Vec<FileItem>;

    fn apply(self, full: &Arc<Vec<FileItem>>, inputs: &FilterInputs) -> Outcome<Self::Rest> {
        Outcome {
            full: (!self.0.is_empty()).then(|| appended(full, &self.0)),
            inputs: inputs.clone(),
            appended: true,
            rest: self.0,
        }
    }
}

/// Step 1 of publishing `items`, which the walk of the scan identified by `token`
/// has listed. Call it with the state locked. Nothing is changed but the record of
/// what the walk has seen.
///
/// Returns `None` without touching anything for a stale token.
pub(crate) fn stage_scanned_items(
    state: &mut AppState,
    token: &ScanToken,
    items: Vec<FileItem>,
) -> Option<Staged<ScannedItems>> {
    let new_items = state.confirm_scanned_items(token, items)?;
    Some(Staged {
        base: Base::of(state),
        change: ScannedItems(new_items),
    })
}

/// Step 3 of publishing scanned items. Call it with the state locked; afterwards
/// the state holds the items, filtered, and a state update can be sent.
///
/// Returns `false` without touching anything for a stale token: a scan that was
/// cancelled or superseded since step 1 publishes nothing. What the returned
/// [`Discarded`] holds is to be dropped after the lock has been released.
pub(crate) fn commit_scanned_items(
    state: &mut AppState,
    token: &ScanToken,
    prepared: Prepared<Vec<FileItem>>,
) -> (bool, Discarded) {
    if !state.is_current_scan(token) {
        return (false, unused(prepared));
    }
    if prepared.base.is_current(state) {
        let mut discarded = state.install_file_lists(prepared.lists);
        discarded.add((prepared.base.full, prepared.rest));
        return (true, discarded);
    }
    let Prepared { base, lists, rest } = prepared;
    // Some of the items may have been listed meanwhile.
    state.merge_scanned_items(token, rest);
    apply_filters(state);
    let mut discarded = Discarded::default();
    discarded.add((base.full, lists));
    (true, discarded)
}

//================================================================================//
//|                              THE END OF THE WALK                             |//
//================================================================================//

/// The end of a walk: what the walk did not find leaves the list.
pub(crate) struct Completion {
    not_found: HashSet<PathBuf>,
    /// What is selected: selected paths that are not in the complete list are
    /// deselected.
    selection: SelectionSnapshot,
}

/// What step 3 of a completion needs.
pub(crate) struct CompletionRest {
    loaded_dirs: HashSet<PathBuf>,
    /// The selected paths that are not in the complete list.
    not_listed: Rejected,
    /// Looks a path up in the complete list, if that was needed.
    listed: Option<ListedPaths>,
    /// The complete list, for the case that `listed` is needed after all.
    full: Arc<Vec<FileItem>>,
}

impl Change for Completion {
    type Rest = CompletionRest;

    fn apply(self, full: &Arc<Vec<FileItem>>, inputs: &FilterInputs) -> Outcome<Self::Rest> {
        let shorter = (!self.not_found.is_empty()).then(|| {
            let kept = full
                .iter()
                .filter(|item| !self.not_found.contains(&item.path));
            Arc::new(kept.cloned().collect::<Vec<_>>())
        });
        let complete = shorter.as_ref().unwrap_or(full);
        // A lookup of the paths of a large list is not built for nothing.
        let listed = (!self.selection.is_empty()).then(|| ListedPaths::build(Arc::clone(complete)));
        let not_listed = self
            .selection
            .rejected(|path| listed.as_ref().is_some_and(|listed| listed.contains(path)));
        Outcome {
            inputs: inputs.clone().of_the_complete_list(),
            appended: false,
            rest: CompletionRest {
                loaded_dirs: directories_of(complete),
                not_listed,
                listed,
                full: Arc::clone(complete),
            },
            full: shorter,
        }
    }
}

/// Step 1 of declaring the file list complete on behalf of the scan identified by
/// `token`, whose walk has ended and has delivered all its items. Call it with the
/// state locked; nothing is changed.
///
/// Returns `None` for a stale token.
pub(crate) fn stage_scan_completion(
    state: &AppState,
    token: &ScanToken,
) -> Option<Staged<Completion>> {
    let not_found = state.items_the_walk_did_not_find(token)?;
    Some(Staged {
        base: Base::of(state),
        change: Completion {
            not_found,
            selection: state.selected_files.snapshot(),
        },
    })
}

/// Step 3 of declaring the file list complete: afterwards it is
/// ([`AppState::complete_scanned_file_list`]) and the filters are applied to it;
/// `active_patterns` are the ignore patterns that were active in the walk.
///
/// Returns `false` without touching anything for a stale token.
pub(crate) fn commit_scan_completion(
    state: &mut AppState,
    token: &ScanToken,
    prepared: Prepared<CompletionRest>,
    active_patterns: HashSet<String>,
) -> (bool, Discarded) {
    if !state.is_current_scan(token) {
        return (false, unused(prepared));
    }
    if !prepared.base.is_current(state) {
        state.complete_scanned_file_list(token, active_patterns);
        apply_filters(state);
        return (true, unused(prepared));
    }
    let Prepared { base, lists, rest } = prepared;
    // Nothing was selected in step 1 and something is now: the lookup is made here.
    let listed = match rest.listed {
        None if !state.selected_files.is_empty() => Some(ListedPaths::build(rest.full)),
        listed => listed,
    };
    state.complete_walk(
        token,
        active_patterns,
        rest.loaded_dirs,
        rest.not_listed,
        |path| listed.as_ref().is_some_and(|listed| listed.contains(path)),
    );
    let mut discarded = state.install_file_lists(lists);
    discarded.add((base.full, listed));
    (true, discarded)
}

//================================================================================//
//|                        A DIRECTORY LEVEL LOADED ON DEMAND                    |//
//================================================================================//

/// The content of a directory as a lazy load has listed it.
pub(crate) struct DirectoryLevel {
    directory: PathBuf,
    items: Vec<FileItem>,
}

/// What step 3 of a directory level needs.
pub(crate) struct DirectoryLevelRest {
    directory: PathBuf,
    /// The level as it was listed.
    items: Vec<FileItem>,
    /// The items of the level that were not in the list.
    added: Vec<FileItem>,
}

impl Change for DirectoryLevel {
    type Rest = DirectoryLevelRest;

    fn apply(self, full: &Arc<Vec<FileItem>>, inputs: &FilterInputs) -> Outcome<Self::Rest> {
        let added = unlisted_items(full, self.items.clone());
        Outcome {
            full: (!added.is_empty()).then(|| appended(full, &added)),
            inputs: inputs.clone().with_loaded_directory(&self.directory),
            appended: true,
            rest: DirectoryLevelRest {
                directory: self.directory,
                items: self.items,
                added,
            },
        }
    }
}

/// Step 1 of adding `items`, the content of `directory` as the lazy load identified
/// by `token` has listed it. Call it with the state locked; nothing is changed.
///
/// Returns `None` if the file list is no longer the one the lazy load was started
/// for.
pub(crate) fn stage_directory_level(
    state: &AppState,
    token: &LazyLoadToken,
    directory: &Path,
    items: Vec<FileItem>,
) -> Option<Staged<DirectoryLevel>> {
    state.is_current_lazy_load(token).then(|| Staged {
        base: Base::of(state),
        change: DirectoryLevel {
            directory: directory.to_path_buf(),
            items,
        },
    })
}

/// Step 3 of adding a directory level: afterwards the level is in the list
/// ([`AppState::merge_directory_level`]) and the filters are applied;
/// `active_patterns` are the ignore patterns that were active in the level.
///
/// Returns `false` without touching anything for a stale token.
pub(crate) fn commit_directory_level(
    state: &mut AppState,
    token: &LazyLoadToken,
    prepared: Prepared<DirectoryLevelRest>,
    active_patterns: HashSet<String>,
) -> (bool, Discarded) {
    if !state.is_current_lazy_load(token) {
        return (false, unused(prepared));
    }
    let Prepared { base, lists, rest } = prepared;
    let mut discarded = Discarded::default();
    if base.is_current(state) {
        state.record_directory_level(
            token,
            &rest.directory,
            rest.items,
            &rest.added,
            active_patterns,
        );
        discarded = state.install_file_lists(lists);
        discarded.add((base.full, rest.added));
    } else {
        state.merge_directory_level(token, &rest.directory, rest.items, active_patterns);
        apply_filters(state);
        discarded.add((base.full, lists, rest.added));
    }
    (true, discarded)
}

//================================================================================//
//|                         THE MATCHES OF A CONTENT SEARCH                      |//
//================================================================================//

/// The paths a content search has found.
pub(crate) struct ContentMatches(Arc<HashSet<PathBuf>>);

impl Change for ContentMatches {
    type Rest = (Arc<HashSet<PathBuf>>, FilterInputs);

    fn apply(self, _: &Arc<Vec<FileItem>>, inputs: &FilterInputs) -> Outcome<Self::Rest> {
        let inputs = inputs.clone().with_content_matches(Arc::clone(&self.0));
        Outcome {
            full: None,
            appended: false,
            rest: (self.0, inputs.clone()),
            inputs,
        }
    }
}

/// Step 1 of applying `matches`, which the content search identified by `token`
/// has found. Call it with the state locked; nothing is changed.
///
/// Returns `None` if the search is no longer the one whose matches are awaited.
pub(crate) fn stage_content_matches(
    state: &AppState,
    token: &ContentSearchToken,
    matches: HashSet<PathBuf>,
) -> Option<Staged<ContentMatches>> {
    state.is_current_content_search(token).then(|| Staged {
        base: Base::of(state),
        change: ContentMatches(Arc::new(matches)),
    })
}

/// Step 3 of applying the matches of a content search: afterwards they are the
/// matches of the state ([`AppState::finish_content_search`]) and the filters are
/// applied.
///
/// Returns `false` without touching anything for a stale token.
pub(crate) fn commit_content_matches(
    state: &mut AppState,
    token: &ContentSearchToken,
    prepared: Prepared<(Arc<HashSet<PathBuf>>, FilterInputs)>,
) -> (bool, Discarded) {
    if !state.is_current_content_search(token) {
        return (false, unused(prepared));
    }
    let Prepared { base, lists, rest } = prepared;
    let (matches, inputs_with_matches) = rest;
    let was_current = base.is_current(state);
    state.finish_content_search_shared(token, matches);
    let mut discarded = Discarded::default();
    // With the matches in, the filters of the state have to be the ones the lists
    // were made with (they are not if there is no content query to apply them to).
    if was_current && inputs_with_matches.is_current(state) {
        discarded = state.install_file_lists(lists);
        discarded.add(base.full);
    } else {
        apply_filters(state);
        discarded.add((base.full, lists));
    }
    (true, discarded)
}

#[cfg(test)]
mod tests;
