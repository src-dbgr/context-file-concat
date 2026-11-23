//! This module is responsible for mutating the application state by applying filters.
//!
//! It takes the `AppState` and modifies the `filtered_file_list` based on various
//! criteria like search queries, extension filters, and content search results, and it
//! expands the directories of the matches ([`auto_expand_for_matches`]), except for
//! the ones the user collapsed while filtering (the rule is
//! [`AppState::reveal_directory`]).
//! This cleanly separates the logic of state mutation from the logic of state
//! presentation (`view_model`), which only reads the state.

use crate::app::state::{AppState, Discarded, ScanToken};
use crate::app::tree_index::IndexedList;
use crate::config::AppConfig;
use crate::core::{is_blank, FileItem, FileNameMatcher, SearchEngine};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Applies all current filters to the full file list to generate the visible list.
pub fn apply_filters(state: &mut AppState) {
    let all_dirs_in_full_list: HashSet<PathBuf> = state
        .full_file_list
        .iter()
        .filter(|i| i.is_directory)
        .map(|i| i.path.clone())
        .collect();

    // A directory whose content has not been loaded is not known to be empty.
    let unloaded_dirs: HashSet<PathBuf> = all_dirs_in_full_list
        .difference(&state.loaded_dirs)
        .cloned()
        .collect();

    let filtered = apply_filters_on_data(
        &state.full_file_list,
        &PathBuf::from(&state.current_path),
        &state.config,
        &state.file_name_matcher(),
        state.content_filter_matches(),
        &unloaded_dirs,
        state.is_fully_scanned,
    );
    state.set_filtered_file_list(filtered);
}

/// `true` if the filters, as they are set, let every item of the file list through:
/// the filtered list is then the file list, item by item.
///
/// This restates what [`apply_filters_on_data`] does when none of its steps is
/// active, for the callers that want to know it without running the filters.
fn lets_everything_through(state: &AppState) -> bool {
    !(state.config.remove_empty_directories && state.is_fully_scanned)
        && state.content_filter_matches().is_none()
        && !state.file_name_matcher().is_active()
}

/// `true` if the filtered list is known to be the file list, item by item: the
/// filters let everything through and have been applied to the list as it is.
fn mirrors_file_list(state: &AppState) -> bool {
    lets_everything_through(state) && state.filtered_file_list().len() == state.full_file_list.len()
}

/// Items of a scan's walk on their way into the state, between the two short
/// moments in which the state is locked for them.
///
/// Publishing items means work on the whole list (the filtered list, its hierarchy),
/// which grows with the project, and every command waits while the state is locked.
/// The work is therefore done in three steps, of which only the first and the last
/// hold the lock, and only for what is in proportion to the new items:
///
/// 1. [`stage_scanned_items`] (locked) takes what the work needs out of the state.
/// 2. [`StagedScanItems::prepare`] (not locked) does the work.
/// 3. [`commit_scanned_items`] (locked) puts the result into the state, if the state
///    is still the one the result was prepared for.
///
/// Between 1 and 3 the state is consistent and anything may happen to it: until 3
/// the items are simply not there yet.
pub(crate) struct StagedScanItems(Staged);

enum Staged {
    /// Every item is in the list already.
    Nothing,
    /// The filters let everything through: `new_items` are to be appended to the
    /// file list and to the filtered list, which is `base` at this moment.
    Append {
        base: Arc<Vec<FileItem>>,
        base_revision: u64,
        /// The length of the file list `base` mirrors.
        listed: usize,
        root_path: PathBuf,
        new_items: Vec<FileItem>,
    },
    /// The items have been merged and the filters applied, which takes the file
    /// list and therefore the lock; the hierarchy of the result is still to be built.
    Filtered {
        list: Arc<Vec<FileItem>>,
        revision: u64,
        root_path: PathBuf,
    },
}

/// The outcome of [`StagedScanItems::prepare`].
pub(crate) struct PreparedScanItems(Prepared);

enum Prepared {
    Nothing,
    Append {
        base_revision: u64,
        listed: usize,
        new_items: Vec<FileItem>,
        filtered: IndexedList,
    },
    Filtered {
        revision: u64,
        list: IndexedList,
    },
}

/// Step 1 of publishing `items`, which the walk of the scan identified by `token`
/// has listed (see [`StagedScanItems`]). Call it with the state locked.
///
/// Without an active filter nothing is changed yet but the record of what the walk
/// has seen. With one, the items are merged and the filters applied here: that needs
/// the whole file list, which only exists inside the state.
///
/// Returns `None` without touching anything for a stale token.
pub(crate) fn stage_scanned_items(
    state: &mut AppState,
    token: &ScanToken,
    items: Vec<FileItem>,
) -> Option<StagedScanItems> {
    if items.is_empty() {
        return state
            .is_current_scan(token)
            .then_some(StagedScanItems(Staged::Nothing));
    }
    let root_path = PathBuf::from(&state.current_path);
    if mirrors_file_list(state) {
        let new_items = state.confirm_scanned_items(token, items)?;
        if new_items.is_empty() {
            return Some(StagedScanItems(Staged::Nothing));
        }
        return Some(StagedScanItems(Staged::Append {
            base: state.shared_filtered_file_list(),
            base_revision: state.filtered_revision(),
            listed: state.full_file_list.len(),
            root_path,
            new_items,
        }));
    }

    if !state.merge_scanned_items(token, items) {
        return None;
    }
    apply_filters(state);
    Some(StagedScanItems(Staged::Filtered {
        list: state.shared_filtered_file_list(),
        revision: state.filtered_revision(),
        root_path,
    }))
}

impl StagedScanItems {
    /// Step 2: builds the filtered list with the new items (where step 1 has not)
    /// and its hierarchy. This is the expensive part; call it without the lock.
    pub(crate) fn prepare(self) -> PreparedScanItems {
        PreparedScanItems(match self.0 {
            Staged::Nothing => Prepared::Nothing,
            Staged::Append {
                base,
                base_revision,
                listed,
                root_path,
                new_items,
            } => {
                let mut filtered = Vec::with_capacity(base.len() + new_items.len());
                filtered.extend_from_slice(&base);
                filtered.extend_from_slice(&new_items);
                // The state may give up the list meanwhile; not holding on to it
                // lets it go there and then.
                drop(base);
                Prepared::Append {
                    base_revision,
                    listed,
                    new_items,
                    filtered: IndexedList::build(Arc::new(filtered), root_path),
                }
            }
            Staged::Filtered {
                list,
                revision,
                root_path,
            } => Prepared::Filtered {
                revision,
                list: IndexedList::build(list, root_path),
            },
        })
    }
}

/// Step 3 of publishing scanned items: puts what was prepared into the state. Call
/// it with the state locked; afterwards the state holds the items, filtered, and a
/// state update can be sent.
///
/// What was prepared is only used if the state it was prepared for is still there
/// (the same filtered list, the same file list, the same filters). If not, it is
/// dropped and the items take the plain way: merged and filtered here, the hierarchy
/// built by whoever asks for it next. The result is the same either way.
///
/// Returns `false` without touching anything for a stale token: a scan that was
/// cancelled or superseded since step 1 publishes nothing. What the returned
/// [`Discarded`] holds is to be dropped after the lock has been released.
pub(crate) fn commit_scanned_items(
    state: &mut AppState,
    token: &ScanToken,
    prepared: PreparedScanItems,
) -> (bool, Discarded) {
    let mut discarded = Discarded::default();
    if !state.is_current_scan(token) {
        discarded.add(prepared);
        return (false, discarded);
    }
    match prepared.0 {
        Prepared::Nothing => {}
        Prepared::Append {
            base_revision,
            listed,
            new_items,
            filtered,
        } => {
            let is_current = state.filtered_revision() == base_revision
                && state.full_file_list.len() == listed
                && filtered.root_path() == Path::new(&state.current_path)
                && lets_everything_through(state);
            if is_current {
                discarded = state.extend_file_lists(new_items, filtered);
            } else {
                discarded.add(filtered);
                // Some of the items may have been listed meanwhile.
                state.merge_scanned_items(token, new_items);
                apply_filters(state);
            }
        }
        Prepared::Filtered { revision, list } => {
            discarded = state.adopt_file_tree(revision, list);
        }
    }
    (true, discarded)
}

/// Declares the file list complete on behalf of the scan identified by `token`
/// ([`AppState::complete_scanned_file_list`]) and applies the filters to it.
///
/// Returns `false` without touching anything for a stale token.
pub(crate) fn complete_scanned_file_list(
    state: &mut AppState,
    token: &ScanToken,
    active_patterns: HashSet<String>,
) -> bool {
    let mirrored = mirrors_file_list(state);
    let listed = state.full_file_list.len();
    if !state.complete_scanned_file_list(token, active_patterns) {
        return false;
    }
    // Completing a list adds nothing to it. If it has removed nothing either and the
    // filters still let everything through (being complete is what the removal of
    // empty directories waits for), the filtered list is still the file list, and so
    // are the hierarchy and the rows made from it.
    let still_mirrored =
        mirrored && state.full_file_list.len() == listed && lets_everything_through(state);
    if !still_mirrored {
        apply_filters(state);
    }
    true
}

/// Applies the filters after the filter texts have changed, and reveals what the
/// file name and extension filters match.
///
/// The matches of a content search are revealed when the search delivers them (see
/// `tasks::search_in_files`), not here: without a file name or extension filter
/// nothing is expanded.
pub fn apply_filter_texts(state: &mut AppState) {
    apply_filters(state);
    if !is_blank(&state.search_query) || !is_blank(&state.extension_filter) {
        auto_expand_for_matches(state);
    }
}

/// The number of directories one application of the filters expands at most.
///
/// A filter that matches broadly (an extension most files have) would otherwise
/// expand every directory of the project: nobody reads tens of thousands of rows,
/// and the expanded set would stay that large after the filter is gone. The first
/// matches in tree order are revealed; every other match is still listed and
/// counted, its directory just stays collapsed until the user opens it or narrows
/// the filter. 500 directories are far more than fit on a screen (a viewport shows
/// some 30 to 80 rows), and revealing them costs one pass over the list instead of
/// a path per match and level: 6 ms instead of 160 ms for 500,000 matching files
/// in 25,000 directories (measured: see `docs/virtualization.md`).
pub const AUTO_EXPAND_LIMIT: usize = 500;

/// Expands the parent directories of files that match the current search criteria.
///
/// The directories above the files that match the search, extension or content filter
/// are expanded, so that the matches are visible in the file tree: the first
/// [`AUTO_EXPAND_LIMIT`] of them in the order of the tree. A directory the user
/// collapsed while a filter was active stays collapsed, see
/// [`AppState::reveal_directory`]; that is what makes it safe to call this whenever
/// the filters are applied again.
pub fn auto_expand_for_matches(state: &mut AppState) {
    for directory in directories_of_matches(state, AUTO_EXPAND_LIMIT) {
        state.reveal_directory(&directory);
    }
}

/// The directories of the tree that hold a match somewhere below them, in the order
/// of the tree (a directory before its content), at most `limit` of them.
fn directories_of_matches(state: &AppState, limit: usize) -> Vec<PathBuf> {
    let items = state.filtered_file_list();
    let index = state.file_tree();
    let name_matcher = state.file_name_matcher();
    let content_matches = &state.content_search_results;
    if !name_matcher.is_active() && content_matches.is_empty() {
        return Vec::new();
    }
    let is_match = |item: &FileItem| {
        name_matcher.matches_any(&item.path) || content_matches.contains(&item.path)
    };

    // The directories of the tree, each before the directories inside it.
    let mut directories: Vec<usize> = Vec::new();
    let mut pending: Vec<usize> = index.roots().iter().rev().copied().collect();
    while let Some(current) = pending.pop() {
        if items[current].is_directory {
            directories.push(current);
            pending.extend(index.children(current).iter().rev());
        }
    }

    // Backwards, the content of a directory is decided before the directory itself.
    let mut holds_match = vec![false; items.len()];
    for &directory in directories.iter().rev() {
        holds_match[directory] = index.children(directory).iter().any(|&child| {
            if items[child].is_directory {
                holds_match[child]
            } else {
                is_match(&items[child])
            }
        });
    }

    directories
        .into_iter()
        .filter(|&directory| holds_match[directory])
        .take(limit)
        .map(|directory| items[directory].path.clone())
        .collect()
}

/// Collects all parent directories for a given set of file paths.
fn get_required_ancestors(file_paths: &HashSet<PathBuf>, root_path: &Path) -> HashSet<PathBuf> {
    file_paths
        .par_iter()
        .flat_map(|item_path| {
            let mut parents = Vec::new();
            let mut current = item_path.parent();
            while let Some(parent) = current {
                if parent.starts_with(root_path) && parent != root_path {
                    parents.push(parent.to_path_buf());
                } else {
                    break;
                }
                current = parent.parent();
            }
            parents
        })
        .collect()
}

/// A "pure" function that applies filters sequentially to a list of `FileItem`s.
///
/// # When a filter is active
///
/// A filter whose text is [blank](is_blank) (empty or whitespace only) is inactive
/// and ignored. This one rule decides it for the filename query and the extension
/// filter (through [`FileNameMatcher`]) and for the content query. An active text is
/// matched as typed, without trimming.
///
/// `content_matches` are the matches of the content filter, `None` if there is none
/// to apply (see [`AppState::content_filter_matches`]).
fn apply_filters_on_data(
    full_file_list: &[FileItem],
    root_path: &Path,
    config: &AppConfig,
    name_matcher: &FileNameMatcher,
    content_matches: Option<&HashSet<PathBuf>>,
    dirs_to_preserve: &HashSet<PathBuf>,
    is_fully_scanned: bool,
) -> Vec<FileItem> {
    // Step 1: Create the base list. If "remove empty" is on, prune the full list first.
    let mut working_list: Vec<FileItem> = if config.remove_empty_directories && is_fully_scanned {
        SearchEngine::remove_empty_directories(
            full_file_list.to_vec(),
            full_file_list,
            dirs_to_preserve,
        )
        .0
    } else {
        full_file_list.to_vec()
    };

    // Step 2: Apply content search if active.
    if let Some(content_matches) = content_matches {
        if content_matches.is_empty() {
            return Vec::new();
        }
        let required_dirs = get_required_ancestors(content_matches, root_path);
        working_list.retain(|item| {
            content_matches.contains(&item.path) || required_dirs.contains(&item.path)
        });
    }

    // Step 3: Apply filename/extension search if active.
    if name_matcher.is_active() {
        let matching_files: HashSet<_> = working_list
            .iter()
            .filter(|item| !item.is_directory && name_matcher.matches_all(&item.path))
            .map(|item| item.path.clone())
            .collect();

        if matching_files.is_empty() {
            return Vec::new();
        }

        let required_dirs = get_required_ancestors(&matching_files, root_path);
        working_list.retain(|item| {
            matching_files.contains(&item.path) || required_dirs.contains(&item.path)
        });
    }

    working_list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use crate::app::view_model::generate_ui_state;
    use crate::config::AppConfig;
    use crate::core::FileItem;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Creates a test `FileItem` with the specified path and directory flag.
    fn create_test_file_item(path_str: &str, is_dir: bool) -> FileItem {
        FileItem {
            path: PathBuf::from(path_str),
            is_directory: is_dir,
            is_binary: false,
            size: if is_dir { 0 } else { 100 },
            depth: path_str.matches('/').count(),
            parent: PathBuf::from(path_str).parent().map(|p| p.to_path_buf()),
        }
    }

    /// `directories` directories below `/project`, each with one file `match.rs`.
    fn state_with_matches_in(directories: usize) -> AppState {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        for directory in 0..directories {
            let path = format!("/project/dir{directory:04}");
            state
                .full_file_list
                .push(create_test_file_item(&path, true));
            state
                .full_file_list
                .push(create_test_file_item(&format!("{path}/match.rs"), false));
        }
        state.is_fully_scanned = true;
        state
    }

    #[test]
    fn test_auto_expansion_stops_at_the_limit_in_tree_order() {
        let mut state = state_with_matches_in(AUTO_EXPAND_LIMIT + 20);
        state.search_query = "match".to_string();

        apply_filter_texts(&mut state);

        // The first directories of the tree are expanded, the rest stays collapsed.
        assert_eq!(state.expanded_dirs.len(), AUTO_EXPAND_LIMIT);
        let expected: HashSet<PathBuf> = (0..AUTO_EXPAND_LIMIT)
            .map(|directory| PathBuf::from(format!("/project/dir{directory:04}")))
            .collect();
        assert_eq!(state.expanded_dirs, expected);
        // Every match is still listed and counted.
        let ui_state = generate_ui_state(&state);
        assert_eq!(ui_state.tree_files_count, AUTO_EXPAND_LIMIT + 20);
        assert_eq!(ui_state.rows_total, 2 * AUTO_EXPAND_LIMIT + 20);

        // Applying the same filter again expands nothing more.
        auto_expand_for_matches(&mut state);
        assert_eq!(state.expanded_dirs.len(), AUTO_EXPAND_LIMIT);

        // A directory the user opens beyond the limit is theirs.
        let beyond = PathBuf::from(format!("/project/dir{:04}", AUTO_EXPAND_LIMIT + 5));
        state.expand_directories([beyond.clone()]);
        auto_expand_for_matches(&mut state);
        assert_eq!(state.expanded_dirs.len(), AUTO_EXPAND_LIMIT + 1);
        assert!(state.expanded_dirs.contains(&beyond));
    }

    #[test]
    fn test_auto_expansion_below_the_limit_reveals_every_match() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/a", true),
            create_test_file_item("/project/a/deep", true),
            create_test_file_item("/project/a/deep/hit.rs", false),
            create_test_file_item("/project/a/other.txt", false),
            create_test_file_item("/project/b", true),
            create_test_file_item("/project/b/miss.txt", false),
            create_test_file_item("/project/c", true),
            create_test_file_item("/project/c/hit.rs", false),
        ];
        state.is_fully_scanned = true;
        state.extension_filter = "rs".to_string();

        apply_filter_texts(&mut state);

        let expected: HashSet<PathBuf> = ["/project/a", "/project/a/deep", "/project/c"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(state.expanded_dirs, expected);

        // The matches of a content search are revealed the same way, and without a
        // filter nothing is.
        let mut state = state_with_matches_in(3);
        apply_filters(&mut state);
        auto_expand_for_matches(&mut state);
        assert!(state.expanded_dirs.is_empty());
        state.content_search_results = HashSet::from([PathBuf::from("/project/dir0001/match.rs")]);
        auto_expand_for_matches(&mut state);
        assert_eq!(
            state.expanded_dirs,
            HashSet::from([PathBuf::from("/project/dir0001")])
        );
    }

    /// Creates a clean test configuration without any ignore patterns.
    fn create_test_config() -> AppConfig {
        AppConfig {
            ignore_patterns: HashSet::new(),
            case_sensitive_search: false,
            remove_empty_directories: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_apply_filters_with_search_query() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/src/lib.rs", false),
        ];
        state.search_query = "main".to_string();
        apply_filters(&mut state);

        let ui_state = generate_ui_state(&state);

        // The filtered list should contain `src` (parent dir) and `main.rs`.
        assert_eq!(ui_state.visible_files_count, 2);
        assert_eq!(ui_state.root_rows().len(), 1); // Only `src` directory at root
    }

    #[test]
    fn test_apply_filters_with_extension_filter() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/README.md", false),
        ];
        state.extension_filter = "md".to_string();
        apply_filters(&mut state);

        let visible_paths: HashSet<_> = state
            .filtered_file_list()
            .iter()
            .map(|i| i.path.clone())
            .collect();
        assert_eq!(visible_paths.len(), 1);
        assert!(visible_paths.contains(&PathBuf::from("/project/README.md")));
    }

    #[test]
    fn test_apply_filters_content_search_with_no_results() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/README.md", false),
        ];
        state.content_search_query = "nonexistent".to_string();
        state.content_search_results = HashSet::new(); // No matches
        apply_filters(&mut state);

        assert_eq!(state.filtered_file_list().len(), 0);
    }

    fn visible_paths(state: &AppState) -> HashSet<PathBuf> {
        state
            .filtered_file_list()
            .iter()
            .map(|item| item.path.clone())
            .collect()
    }

    /// A state with one file whose name contains a blank and one whose name does not.
    fn state_with_and_without_blank() -> AppState {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/release notes.md", false),
            create_test_file_item("/project/main.rs", false),
        ];
        state
    }

    /// A filename query of whitespace only is inactive: nothing is filtered.
    #[test]
    fn test_whitespace_only_search_query_is_inactive() {
        let mut state = state_with_and_without_blank();
        state.search_query = "  ".to_string();

        apply_filters(&mut state);

        assert_eq!(visible_paths(&state).len(), 2);
    }

    /// A query that is not blank is matched as typed: surrounding blanks belong to it.
    #[test]
    fn test_search_query_is_matched_with_its_surrounding_whitespace() {
        let mut state = state_with_and_without_blank();
        state.search_query = "main ".to_string();
        apply_filters(&mut state);
        assert!(visible_paths(&state).is_empty());

        state.search_query = "release ".to_string();
        apply_filters(&mut state);
        assert_eq!(
            visible_paths(&state),
            HashSet::from([PathBuf::from("/project/release notes.md")])
        );
    }

    /// An extension filter of whitespace only is inactive: nothing is filtered.
    #[test]
    fn test_whitespace_only_extension_filter_is_inactive() {
        let mut state = state_with_and_without_blank();
        state.extension_filter = " ".to_string();

        apply_filters(&mut state);

        assert_eq!(visible_paths(&state).len(), 2);
    }

    /// A content query of whitespace only is inactive: the list is not narrowed down,
    /// neither to (stale) matches nor to nothing because there are no matches.
    #[test]
    fn test_whitespace_only_content_query_is_inactive() {
        let mut state = state_with_and_without_blank();
        state.content_search_query = " \t".to_string();

        state.content_search_results = HashSet::from([PathBuf::from("/project/main.rs")]);
        apply_filters(&mut state);
        assert_eq!(visible_paths(&state).len(), 2);

        state.content_search_results.clear();
        apply_filters(&mut state);
        assert_eq!(visible_paths(&state).len(), 2);
        assert_eq!(generate_ui_state(&state).root_rows().len(), 2);
    }

    /// Filtering, highlighting and auto-expansion share one matching rule, so a filter
    /// spelled in a way the file list accepts also highlights and expands its matches.
    #[test]
    fn test_filter_highlight_and_auto_expand_agree_on_matches() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/build", true),
            create_test_file_item("/project/build/Makefile", false),
            create_test_file_item("/project/build/main.rs", false),
        ];
        state.extension_filter = "No Extension".to_string();

        apply_filters(&mut state);
        auto_expand_for_matches(&mut state);
        let ui_state = generate_ui_state(&state);

        let build = ui_state.root_rows()[0];
        assert!(build.is_expanded, "The directory of the match is expanded");
        let listed = ui_state.children_of(build);
        assert_eq!(listed.len(), 1, "Only the match is listed");
        assert_eq!(listed[0].name, "Makefile");
        assert!(listed[0].is_match, "The match is highlighted");
    }

    #[test]
    fn test_auto_expand_expands_the_ancestors_of_matches_only() {
        let mut state = AppState::default();
        state.config = create_test_config();
        state.current_path = "/project".to_string();
        state.full_file_list = vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/deep", true),
            create_test_file_item("/project/src/deep/needle.rs", false),
            create_test_file_item("/project/docs", true),
            create_test_file_item("/project/docs/guide.md", false),
            create_test_file_item("/project/found_by_content.txt", false),
            create_test_file_item("/project/other", true),
            create_test_file_item("/project/other/hit.txt", false),
        ];
        state.search_query = "needle".to_string();
        apply_filters(&mut state);
        auto_expand_for_matches(&mut state);
        assert_eq!(
            state.expanded_dirs,
            HashSet::from([
                PathBuf::from("/project/src"),
                PathBuf::from("/project/src/deep"),
            ]),
            "the root itself and directories without a match are not expanded"
        );

        // A content match expands its ancestors as well.
        state.search_query.clear();
        state.expanded_dirs.clear();
        state.content_search_query = "hit".to_string();
        state.content_search_results = HashSet::from([PathBuf::from("/project/other/hit.txt")]);
        apply_filters(&mut state);
        auto_expand_for_matches(&mut state);
        assert_eq!(
            state.expanded_dirs,
            HashSet::from([PathBuf::from("/project/other")])
        );
    }

    #[test]
    fn test_empty_filters_keep_the_full_list() {
        let mut state = state_with_and_without_blank();

        apply_filters(&mut state);

        assert_eq!(visible_paths(&state).len(), 2);
    }

    #[test]
    fn test_expanded_empty_dir_is_not_preserved_when_remove_is_on() {
        let mut state = AppState::default();
        let mut config = create_test_config();
        config.remove_empty_directories = true; // Feature is ON
        state.config = config;
        state.current_path = "/project".to_string();

        let empty_dir_path = PathBuf::from("/project/empty_dir");
        let file_path = PathBuf::from("/project/file.txt");

        state.full_file_list = vec![
            create_test_file_item("/project/empty_dir", true),
            create_test_file_item("/project/file.txt", false),
        ];
        state.loaded_dirs.insert(empty_dir_path.clone());
        state.is_fully_scanned = true;
        state.expanded_dirs.insert(empty_dir_path.clone()); // User expands the empty dir

        apply_filters(&mut state); // This should now remove the empty_dir

        let visible_paths: HashSet<_> = state
            .filtered_file_list()
            .iter()
            .map(|item| item.path.clone())
            .collect();

        assert!(
            !visible_paths.contains(&empty_dir_path),
            "Expanded empty directory should be REMOVED when feature is on"
        );
        assert!(
            visible_paths.contains(&file_path),
            "Regular file should still be visible"
        );
    }

    //========================================================================================//
    //|                     SCANNED ITEMS: STAGED, PREPARED, COMMITTED                       |//
    //========================================================================================//

    use crate::app::state::ScanToken;
    use crate::app::view_model::rows::all_rows;

    /// A scan of `/project` whose first level (`src`, `docs`) has been published.
    fn state_with_a_walking_scan() -> (AppState, ScanToken) {
        let mut state = AppState::default();
        state.config = AppConfig {
            remove_empty_directories: false,
            ..create_test_config()
        };
        state.current_path = "/project".to_string();
        let token = state.begin_scan();
        state.begin_scan_walk(&token);
        state.replace_with_shallow_file_list(
            Path::new("/project"),
            vec![
                create_test_file_item("/project/src", true),
                create_test_file_item("/project/docs", true),
            ],
        );
        apply_filters(&mut state);
        state.expand_directories([PathBuf::from("/project/src")]);
        (state, token)
    }

    /// What the walk lists first: the first level again, and the content of `src`.
    fn first_batch() -> Vec<FileItem> {
        vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/src/notes.txt", false),
            create_test_file_item("/project/docs", true),
        ]
    }

    fn paths_of(items: &[FileItem]) -> Vec<&str> {
        items
            .iter()
            .map(|item| item.path.to_str().unwrap())
            .collect()
    }

    /// The three steps in a row, as a publication without anything in between.
    fn add_in_steps(state: &mut AppState, token: &ScanToken, items: Vec<FileItem>) -> bool {
        let Some(staged) = stage_scanned_items(state, token, items) else {
            return false;
        };
        commit_scanned_items(state, token, staged.prepare()).0
    }

    /// The way without steps, which defines the result: merged, then filtered.
    fn add_plainly(state: &mut AppState, token: &ScanToken, items: Vec<FileItem>) -> bool {
        let merged = state.merge_scanned_items(token, items);
        if merged {
            apply_filters(state);
        }
        merged
    }

    /// Asserts that `state` is what the plain way makes of the same file list: the
    /// filtered list is the filters applied to the file list, and the tree shows it.
    #[track_caller]
    fn assert_filtered_list_is_current(state: &mut AppState) {
        let filtered: Vec<FileItem> = state.filtered_file_list().to_vec();
        let rows = all_rows(state);
        let counts = state.file_tree().counts();

        apply_filters(state);

        assert_eq!(paths_of(state.filtered_file_list()), paths_of(&filtered));
        assert_eq!(all_rows(state), rows);
        assert_eq!(state.file_tree().counts(), counts);
    }

    #[test]
    fn scanned_items_are_added_with_a_hierarchy_that_was_built_outside_the_lock() {
        let (mut state, token) = state_with_a_walking_scan();
        generate_ui_state(&state);
        let built = state.file_tree_build_count();

        let staged = stage_scanned_items(&mut state, &token, first_batch()).unwrap();
        // Until the last step the lists are as they were.
        assert_eq!(state.full_file_list.len(), 2);
        assert_eq!(state.filtered_file_list().len(), 2);
        let prepared = staged.prepare();
        let revision = state.filtered_revision();
        let (is_current, discarded) = commit_scanned_items(&mut state, &token, prepared);
        drop(discarded);

        assert!(is_current);
        assert_eq!(
            paths_of(&state.full_file_list),
            [
                "/project/src",
                "/project/docs",
                "/project/src/main.rs",
                "/project/src/notes.txt"
            ]
        );
        assert_eq!(
            paths_of(state.filtered_file_list()),
            paths_of(&state.full_file_list)
        );
        assert_ne!(state.filtered_revision(), revision);

        let ui_state = generate_ui_state(&state);
        let src = ui_state.row_named("src").unwrap();
        assert_eq!(ui_state.children_of(src).len(), 2);
        assert_eq!(ui_state.visible_files_count, 4);
        assert_eq!(
            state.file_tree_build_count(),
            built,
            "The state update must find the hierarchy that came with the list"
        );
        assert_filtered_list_is_current(&mut state);
    }

    #[test]
    fn items_that_are_all_listed_already_change_nothing() {
        let (mut state, token) = state_with_a_walking_scan();
        let revision = state.filtered_revision();

        assert!(add_in_steps(
            &mut state,
            &token,
            vec![create_test_file_item("/project/src", true)]
        ));
        assert!(add_in_steps(&mut state, &token, Vec::new()));

        assert_eq!(state.full_file_list.len(), 2);
        assert_eq!(state.filtered_revision(), revision);
    }

    #[test]
    fn a_stale_scan_can_neither_stage_nor_commit_items() {
        // Stale before the first step.
        let (mut state, token) = state_with_a_walking_scan();
        state.cancel_current_scan();
        assert!(stage_scanned_items(&mut state, &token, first_batch()).is_none());
        assert!(stage_scanned_items(&mut state, &token, Vec::new()).is_none());
        assert_eq!(state.full_file_list.len(), 2);

        // Cancelled, or superseded by another scan, between the first and the last
        // step: with and without an active filter.
        for query in ["", "main"] {
            for supersede in [false, true] {
                let (mut state, token) = state_with_a_walking_scan();
                state.search_query = query.to_string();
                apply_filters(&mut state);
                let staged = stage_scanned_items(&mut state, &token, first_batch()).unwrap();
                let prepared = staged.prepare();
                let listed = paths_of(&state.full_file_list)
                    .into_iter()
                    .map(String::from)
                    .collect::<Vec<_>>();

                if supersede {
                    let _ = state.begin_scan();
                } else {
                    state.cancel_current_scan();
                }
                let revision = state.filtered_revision();
                let built = state.file_tree_build_count();
                let (is_current, discarded) = commit_scanned_items(&mut state, &token, prepared);
                drop(discarded);

                assert!(!is_current, "query {query:?}, superseded: {supersede}");
                assert_eq!(paths_of(&state.full_file_list), listed);
                assert_eq!(state.filtered_revision(), revision);
                state.file_tree();
                assert_eq!(
                    state.file_tree_build_count(),
                    built + 1,
                    "Nothing of a stale scan may be installed"
                );
                assert_filtered_list_is_current(&mut state);
            }
        }
    }

    /// The prepared list belongs to the filtered list it was made from. If that one
    /// has been replaced meanwhile, the prepared one is dropped; the items are
    /// listed all the same.
    #[test]
    fn prepared_items_are_not_installed_over_a_filtered_list_that_has_changed() {
        type Change = fn(&mut AppState);
        let changes: [(&str, Change); 4] = [
            ("the filters were applied again", |state| {
                apply_filters(state)
            }),
            ("a filter was set", |state| {
                state.set_filter_texts("main".to_string(), String::new(), String::new());
                apply_filter_texts(state);
            }),
            ("a level was loaded that the walk lists as well", |state| {
                let lazy_load = state.begin_lazy_load();
                assert!(state.merge_directory_level(
                    &lazy_load,
                    Path::new("/project/src"),
                    vec![
                        create_test_file_item("/project/src/main.rs", false),
                        create_test_file_item("/project/src/only_listed_lazily.rs", false),
                    ],
                    HashSet::new(),
                ));
                apply_filters(state);
            }),
            ("items were removed by an ignore pattern", |state| {
                state.config.ignore_patterns.insert("docs".to_string());
                state.apply_ignore_patterns();
                apply_filters(state);
            }),
        ];
        for (what, change) in changes {
            let (mut state, token) = state_with_a_walking_scan();
            let staged = stage_scanned_items(&mut state, &token, first_batch()).unwrap();
            let prepared = staged.prepare();

            change(&mut state);
            let built = state.file_tree_build_count();
            let (is_current, discarded) = commit_scanned_items(&mut state, &token, prepared);
            drop(discarded);

            assert!(is_current, "{what}");
            let mut listed = paths_of(&state.full_file_list);
            listed.sort_unstable();
            let unique = listed.iter().collect::<HashSet<_>>().len();
            assert_eq!(unique, listed.len(), "{what}: nothing is listed twice");
            for path in ["/project/src/main.rs", "/project/src/notes.txt"] {
                assert!(listed.contains(&path), "{what}: {path} is listed");
            }
            state.file_tree();
            assert_eq!(
                state.file_tree_build_count(),
                built + 1,
                "{what}: the prepared hierarchy must not have been installed"
            );
            assert_filtered_list_is_current(&mut state);
        }
    }

    /// A file list that changed without the filtered list following it (which no
    /// command does) is not what the prepared list was made for either.
    #[test]
    fn prepared_items_are_not_installed_next_to_a_file_list_that_has_changed() {
        let (mut state, token) = state_with_a_walking_scan();
        let staged = stage_scanned_items(&mut state, &token, first_batch()).unwrap();
        let prepared = staged.prepare();

        state
            .full_file_list
            .push(create_test_file_item("/project/added.txt", false));
        let (is_current, discarded) = commit_scanned_items(&mut state, &token, prepared);
        drop(discarded);

        assert!(is_current);
        assert_eq!(state.full_file_list.len(), 5);
        assert_filtered_list_is_current(&mut state);
    }

    /// With an active filter the list is filtered in the first step; what is prepared
    /// is its hierarchy, and that is only taken over for the list it was built from.
    #[test]
    fn with_a_filter_the_hierarchy_is_taken_over_only_for_the_list_it_was_built_from() {
        for changed_meanwhile in [false, true] {
            let (mut state, token) = state_with_a_walking_scan();
            state.search_query = "main".to_string();
            apply_filters(&mut state);

            let staged = stage_scanned_items(&mut state, &token, first_batch()).unwrap();
            // Merged and filtered at once: the state is consistent between the steps.
            assert_eq!(state.full_file_list.len(), 4);
            assert_eq!(
                paths_of(state.filtered_file_list()),
                ["/project/src", "/project/src/main.rs"]
            );
            let prepared = staged.prepare();
            if changed_meanwhile {
                state.search_query = "notes".to_string();
                apply_filters(&mut state);
            }
            let built = state.file_tree_build_count();
            let (is_current, discarded) = commit_scanned_items(&mut state, &token, prepared);
            drop(discarded);

            assert!(is_current);
            state.file_tree();
            assert_eq!(
                state.file_tree_build_count() - built,
                usize::from(changed_meanwhile),
                "changed meanwhile: {changed_meanwhile}"
            );
            assert_filtered_list_is_current(&mut state);
        }
    }

    /// Whatever the filters are and however the walk is cut into batches: the steps
    /// end with the state the plain way ends with.
    #[test]
    fn adding_items_in_steps_ends_like_merging_and_filtering_them() {
        let walk: Vec<FileItem> = [
            ("/project/src", true),
            ("/project/src/main.rs", false),
            ("/project/src/core", true),
            ("/project/src/core/mod.rs", false),
            ("/project/src/core/notes.txt", false),
            ("/project/src/empty", true),
            ("/project/docs", true),
            ("/project/docs/guide.md", false),
            ("/project/readme.md", false),
        ]
        .into_iter()
        .map(|(path, is_dir)| create_test_file_item(path, is_dir))
        .collect();
        let filters: [fn(&mut AppState); 4] = [
            |_| {},
            |state| state.search_query = "mod".to_string(),
            |state| state.extension_filter = "md".to_string(),
            |state| state.config.remove_empty_directories = true,
        ];

        for (filter_index, filter) in filters.into_iter().enumerate() {
            for batch_size in [1, 2, 4, walk.len()] {
                let (mut stepped, stepped_token) = state_with_a_walking_scan();
                let (mut plain, plain_token) = state_with_a_walking_scan();
                for state in [&mut stepped, &mut plain] {
                    filter(state);
                    apply_filter_texts(state);
                    state.expand_directories([PathBuf::from("/project/src/core")]);
                }

                for batch in walk.chunks(batch_size) {
                    assert!(add_in_steps(&mut stepped, &stepped_token, batch.to_vec()));
                    assert!(add_plainly(&mut plain, &plain_token, batch.to_vec()));
                    generate_ui_state(&stepped);
                }
                assert!(complete_scanned_file_list(
                    &mut stepped,
                    &stepped_token,
                    HashSet::new()
                ));
                assert!(plain.complete_scanned_file_list(&plain_token, HashSet::new()));
                apply_filters(&mut plain);

                let context = format!("filter {filter_index}, batches of {batch_size}");
                assert_eq!(
                    paths_of(&stepped.full_file_list),
                    paths_of(&plain.full_file_list),
                    "{context}"
                );
                assert_eq!(
                    paths_of(stepped.filtered_file_list()),
                    paths_of(plain.filtered_file_list()),
                    "{context}"
                );
                assert_eq!(all_rows(&stepped), all_rows(&plain), "{context}");
                assert_eq!(
                    stepped.file_tree().counts(),
                    plain.file_tree().counts(),
                    "{context}"
                );
                assert_filtered_list_is_current(&mut stepped);
            }
        }
    }

    /// The rule the steps rely on to leave the filters out: nothing active, nothing
    /// removed.
    #[test]
    fn without_an_active_filter_the_filtered_list_is_the_file_list() {
        let (mut state, token) = state_with_a_walking_scan();
        assert!(add_plainly(&mut state, &token, first_batch()));
        assert!(lets_everything_through(&state));
        assert!(mirrors_file_list(&state));
        assert_eq!(
            paths_of(state.filtered_file_list()),
            paths_of(&state.full_file_list)
        );

        // A content query whose first search is still running filters nothing yet.
        state.content_search_query = "fn".to_string();
        let _search = state.begin_content_search();
        assert!(lets_everything_through(&state));

        let active: [fn(&mut AppState); 4] = [
            |state| state.search_query = "main".to_string(),
            |state| state.extension_filter = "rs".to_string(),
            |state| {
                state.content_search_query = "fn".to_string();
                let search = state.begin_content_search();
                state.finish_content_search(&search, HashSet::new());
            },
            |state| {
                state.config.remove_empty_directories = true;
                state.is_fully_scanned = true;
            },
        ];
        for (index, activate) in active.into_iter().enumerate() {
            let (mut state, _token) = state_with_a_walking_scan();
            activate(&mut state);
            assert!(!lets_everything_through(&state), "filter {index}");
        }

        // The removal of empty directories waits for the complete list.
        let (mut state, _token) = state_with_a_walking_scan();
        state.config.remove_empty_directories = true;
        assert!(lets_everything_through(&state));
    }

    /// Completing a list that the filters let through as it is leaves the filtered
    /// list (and with it the hierarchy and the rows) alone; anything else is
    /// filtered again.
    #[test]
    fn completing_the_list_filters_it_again_only_if_something_can_have_changed() {
        // Nothing removed, no filter: kept.
        let (mut state, token) = state_with_a_walking_scan();
        assert!(add_in_steps(&mut state, &token, first_batch()));
        let revision = state.filtered_revision();
        assert!(complete_scanned_file_list(
            &mut state,
            &token,
            HashSet::new()
        ));
        assert!(state.is_fully_scanned);
        assert_eq!(state.filtered_revision(), revision);
        assert_filtered_list_is_current(&mut state);

        // An item of the first level that the walk did not find is removed.
        let (mut state, token) = state_with_a_walking_scan();
        let mut batch = first_batch();
        batch.retain(|item| item.path != Path::new("/project/docs"));
        assert!(add_in_steps(&mut state, &token, batch));
        assert!(complete_scanned_file_list(
            &mut state,
            &token,
            HashSet::new()
        ));
        assert_eq!(
            paths_of(state.filtered_file_list()),
            [
                "/project/src",
                "/project/src/main.rs",
                "/project/src/notes.txt"
            ]
        );

        // Empty directories are removed once the list is complete.
        let (mut state, token) = state_with_a_walking_scan();
        state.config.remove_empty_directories = true;
        assert!(add_in_steps(&mut state, &token, first_batch()));
        assert_eq!(state.filtered_file_list().len(), 4);
        assert!(complete_scanned_file_list(
            &mut state,
            &token,
            HashSet::new()
        ));
        assert_eq!(
            paths_of(state.filtered_file_list()),
            [
                "/project/src",
                "/project/src/main.rs",
                "/project/src/notes.txt"
            ]
        );

        // A stale token changes nothing.
        let (mut state, token) = state_with_a_walking_scan();
        state.cancel_current_scan();
        assert!(!complete_scanned_file_list(
            &mut state,
            &token,
            HashSet::new()
        ));
        assert!(!state.is_fully_scanned);
    }
}
