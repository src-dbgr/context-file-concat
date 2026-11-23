//! This module is responsible for mutating the application state by applying filters.
//!
//! It takes the `AppState` and modifies the `filtered_file_list` based on various
//! criteria like search queries, extension filters, and content search results, and it
//! expands the directories of the matches ([`auto_expand_for_matches`]), except for
//! the ones the user collapsed while filtering (the rule is
//! [`AppState::reveal_directory`]).
//! This cleanly separates the logic of state mutation from the logic of state
//! presentation (`view_model`), which only reads the state.

use crate::app::state::AppState;
use crate::core::{is_blank, FileItem, FileNameMatcher, SearchEngine};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Applies all current filters to the full file list to generate the visible list.
///
/// Where the filters let everything through, the visible list is the file list
/// itself (shared, not copied).
pub fn apply_filters(state: &mut AppState) {
    let inputs = FilterInputs::of(state);
    let filtered = inputs.apply(&state.shared_full_file_list());
    state.set_shared_filtered_file_list(filtered);
    state.note_filters_of_filtered_list(Arc::new(inputs));
}

/// Everything the filters make their result of, apart from the file list: the
/// values as they were in the state at one moment.
///
/// This is what lets the filters be applied without the state, i.e. without the
/// lock on it ([`FilterInputs::apply`]), and what tells afterwards whether the
/// result still belongs to the state ([`FilterInputs::is_current`]). The filters
/// applied to the state itself ([`apply_filters`]) go the same way, so there is one
/// definition of what they do.
#[derive(Debug, Clone)]
pub(crate) struct FilterInputs {
    root_path: PathBuf,
    search_query: String,
    extension_filter: String,
    case_sensitive: bool,
    /// The matches of the content filter, `None` if there is none to apply
    /// ([`AppState::content_filter_matches`]).
    content_matches: Option<Arc<HashSet<PathBuf>>>,
    /// The setting. Empty directories are removed from a complete list only.
    remove_empty_directories: bool,
    is_fully_scanned: bool,
    loaded_dirs: LoadedDirs,
}

/// The directories whose content is in the list: an empty directory that is not
/// one of them is not known to be empty, and stays.
#[derive(Debug, Clone)]
enum LoadedDirs {
    /// Empty directories are not removed, so it does not matter.
    NotAsked,
    These(HashSet<PathBuf>),
    /// Every directory of the list.
    All,
}

impl FilterInputs {
    /// The inputs as they are in `state`.
    pub(crate) fn of(state: &AppState) -> Self {
        let remove_empty_directories = state.config.remove_empty_directories;
        Self {
            root_path: PathBuf::from(&state.current_path),
            search_query: state.search_query.clone(),
            extension_filter: state.extension_filter.clone(),
            case_sensitive: state.config.case_sensitive_search,
            content_matches: state.shared_content_filter_matches(),
            remove_empty_directories,
            is_fully_scanned: state.is_fully_scanned,
            loaded_dirs: if remove_empty_directories && state.is_fully_scanned {
                LoadedDirs::These(state.loaded_dirs.clone())
            } else {
                LoadedDirs::NotAsked
            },
        }
    }

    /// `true` if these are the inputs of `state` as it is now: whatever these
    /// inputs made of a list, the filters of `state` make of the same list.
    ///
    /// The matches of the content filter are the same if they are the same set (the
    /// same allocation: the state gets another one whenever they are replaced), not
    /// if they are equal.
    ///
    /// Every input is compared, none is left to a revision of the lists, because
    /// none of them is tied to one. The path changes with a re-scan that is started
    /// (`start_scan_on_path`), case sensitivity with the configuration, and whether
    /// the list is complete with the walk that begins
    /// ([`AppState::begin_scan_walk`]): each of them while the lists stay as they
    /// are.
    ///
    /// The loaded directories are the one input that nothing changes on its own
    /// today where they are compared, i.e. while empty directories are removed
    /// from a complete list: a level that is merged gives the file list a new
    /// revision, a level that is recorded for lists prepared outside the lock
    /// comes with a new filtered list unless the directory was loaded already, and
    /// the end of a walk is followed by the filters under the same lock. The
    /// comparison stays all the same: `loaded_dirs` is a public field, and nothing
    /// but these call sequences ties it to a revision. A wrong answer would show
    /// or hide an empty directory wrongly until the filters are applied again; the
    /// comparison costs one look at two sets, and only where it is needed. The
    /// test of the steps against the plain way changes the field directly to hold
    /// it in place (`Command::ForgetLoadedDirectory`).
    pub(crate) fn is_current(&self, state: &AppState) -> bool {
        self.same_content_matches(state.shared_content_filter_matches().as_ref())
            && self.root_path == Path::new(&state.current_path)
            && self.search_query == state.search_query
            && self.extension_filter == state.extension_filter
            && self.case_sensitive == state.config.case_sensitive_search
            && self.remove_empty_directories == state.config.remove_empty_directories
            && self.is_fully_scanned == state.is_fully_scanned
            && match &self.loaded_dirs {
                LoadedDirs::NotAsked | LoadedDirs::All => true,
                LoadedDirs::These(loaded_dirs) => *loaded_dirs == state.loaded_dirs,
            }
    }

    fn same_content_matches(&self, theirs: Option<&Arc<HashSet<PathBuf>>>) -> bool {
        match (&self.content_matches, theirs) {
            (None, None) => true,
            (Some(mine), Some(theirs)) => Arc::ptr_eq(mine, theirs),
            _ => false,
        }
    }

    /// `true` if these inputs make of any list what `other` make of it.
    ///
    /// That is less than being equal: whether the list is complete and which
    /// directories are loaded matters only where empty directories are removed.
    /// Where they are, "every directory" and a set of directories are not compared
    /// (the answer is `false`): that would take the list.
    pub(crate) fn filters_like(&self, other: &FilterInputs) -> bool {
        self.same_content_matches(other.content_matches.as_ref())
            && self.root_path == other.root_path
            && self.search_query == other.search_query
            && self.extension_filter == other.extension_filter
            && self.case_sensitive == other.case_sensitive
            && self.removes_empty_directories() == other.removes_empty_directories()
            && (!self.removes_empty_directories()
                || match (&self.loaded_dirs, &other.loaded_dirs) {
                    (LoadedDirs::All, LoadedDirs::All) => true,
                    (LoadedDirs::These(mine), LoadedDirs::These(theirs)) => mine == theirs,
                    _ => false,
                })
    }

    /// The path the list lies below.
    pub(crate) fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// The inputs with `matches` as the matches of the content filter.
    pub(crate) fn with_content_matches(mut self, matches: Arc<HashSet<PathBuf>>) -> Self {
        self.content_matches = Some(matches);
        self
    }

    /// The inputs with `directory` loaded as well.
    pub(crate) fn with_loaded_directory(mut self, directory: &Path) -> Self {
        if let LoadedDirs::These(loaded_dirs) = &mut self.loaded_dirs {
            loaded_dirs.insert(directory.to_path_buf());
        }
        self
    }

    /// The inputs for the list once it is complete: every directory is loaded.
    pub(crate) fn of_the_complete_list(mut self) -> Self {
        self.is_fully_scanned = true;
        self.loaded_dirs = LoadedDirs::All;
        self
    }

    fn removes_empty_directories(&self) -> bool {
        self.remove_empty_directories && self.is_fully_scanned
    }

    fn name_matcher(&self) -> FileNameMatcher {
        FileNameMatcher::new(
            &self.search_query,
            &self.extension_filter,
            self.case_sensitive,
        )
    }

    /// `true` if the filters let every item of a list through: the filtered list is
    /// then the list, item by item.
    ///
    /// This restates what [`apply_filters_on_data`] does when none of its steps is
    /// active, for the callers that want to know it without running the filters.
    pub(crate) fn lets_everything_through(&self) -> bool {
        !self.removes_empty_directories()
            && self.content_matches.is_none()
            && !self.name_matcher().is_active()
    }

    /// What the filters make of `list`. Where they let everything through, that is
    /// `list` itself: no item is copied.
    pub(crate) fn apply(&self, list: &Arc<Vec<FileItem>>) -> Arc<Vec<FileItem>> {
        if self.lets_everything_through() {
            return Arc::clone(list);
        }
        // A directory whose content has not been loaded is not known to be empty.
        let unloaded_dirs: HashSet<PathBuf> = match &self.loaded_dirs {
            LoadedDirs::These(loaded_dirs) if self.removes_empty_directories() => list
                .iter()
                .filter(|item| item.is_directory && !loaded_dirs.contains(&item.path))
                .map(|item| item.path.clone())
                .collect(),
            _ => HashSet::new(),
        };
        Arc::new(apply_filters_on_data(
            list,
            &self.root_path,
            self.removes_empty_directories(),
            &self.name_matcher(),
            self.content_matches.as_deref(),
            &unloaded_dirs,
        ))
    }
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
    let content_matches = state.content_search_results();
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
    remove_empty_directories: bool,
    name_matcher: &FileNameMatcher,
    content_matches: Option<&HashSet<PathBuf>>,
    dirs_to_preserve: &HashSet<PathBuf>,
) -> Vec<FileItem> {
    // Step 1: Create the base list. If "remove empty" is on, prune the full list first.
    let mut working_list: Vec<FileItem> = if remove_empty_directories {
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
            state.edit_full_file_list(|list| {
                list.push(create_test_file_item(&path, true));
                list.push(create_test_file_item(&format!("{path}/match.rs"), false));
            });
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
        state.set_full_file_list(vec![
            create_test_file_item("/project/a", true),
            create_test_file_item("/project/a/deep", true),
            create_test_file_item("/project/a/deep/hit.rs", false),
            create_test_file_item("/project/a/other.txt", false),
            create_test_file_item("/project/b", true),
            create_test_file_item("/project/b/miss.txt", false),
            create_test_file_item("/project/c", true),
            create_test_file_item("/project/c/hit.rs", false),
        ]);
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
        state.set_content_search_results(HashSet::from([PathBuf::from(
            "/project/dir0001/match.rs",
        )]));
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
        state.set_full_file_list(vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/src/lib.rs", false),
        ]);
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
        state.set_full_file_list(vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/README.md", false),
        ]);
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
        state.set_full_file_list(vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/main.rs", false),
            create_test_file_item("/project/README.md", false),
        ]);
        state.content_search_query = "nonexistent".to_string();
        state.set_content_search_results(HashSet::new()); // No matches
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
        state.set_full_file_list(vec![
            create_test_file_item("/project/release notes.md", false),
            create_test_file_item("/project/main.rs", false),
        ]);
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

        state.set_content_search_results(HashSet::from([PathBuf::from("/project/main.rs")]));
        apply_filters(&mut state);
        assert_eq!(visible_paths(&state).len(), 2);

        state.set_content_search_results(HashSet::new());
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
        state.set_full_file_list(vec![
            create_test_file_item("/project/build", true),
            create_test_file_item("/project/build/Makefile", false),
            create_test_file_item("/project/build/main.rs", false),
        ]);
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
        state.set_full_file_list(vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/deep", true),
            create_test_file_item("/project/src/deep/needle.rs", false),
            create_test_file_item("/project/docs", true),
            create_test_file_item("/project/docs/guide.md", false),
            create_test_file_item("/project/found_by_content.txt", false),
            create_test_file_item("/project/other", true),
            create_test_file_item("/project/other/hit.txt", false),
        ]);
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
        state.set_content_search_results(HashSet::from([PathBuf::from("/project/other/hit.txt")]));
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

        state.set_full_file_list(vec![
            create_test_file_item("/project/empty_dir", true),
            create_test_file_item("/project/file.txt", false),
        ]);
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
}
