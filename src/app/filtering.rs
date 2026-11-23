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
use crate::config::AppConfig;
use crate::core::{is_blank, FileItem, FileNameMatcher, SearchEngine};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

/// Expands the parent directories of files that match the current search criteria.
///
/// This function iterates through all filtered files that match the current search,
/// extension, or content filters and expands the directories above them, so that the
/// matches are visible in the file tree. A directory the user collapsed while a filter
/// was active stays collapsed, see [`AppState::reveal_directory`]; that is what makes
/// it safe to call this whenever the filters are applied again.
pub fn auto_expand_for_matches(state: &mut AppState) {
    let root_path = PathBuf::from(&state.current_path);
    let name_matcher = state.file_name_matcher();
    let matches: Vec<PathBuf> = state
        .filtered_file_list()
        .iter()
        .filter(|item| {
            !item.is_directory
                && (name_matcher.matches_any(&item.path)
                    || state.content_search_results.contains(&item.path))
        })
        .map(|item| item.path.clone())
        .collect();

    for path in matches {
        let mut current = path.parent();
        while let Some(parent) = current {
            if parent.starts_with(&root_path) && parent != root_path {
                state.reveal_directory(parent);
            } else {
                break;
            }
            current = parent.parent();
        }
    }
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
        assert_eq!(ui_state.tree.len(), 1); // Only `src` directory at root
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
        assert_eq!(generate_ui_state(&state).tree.len(), 2);
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

        let build = &ui_state.tree[0];
        assert!(build.is_expanded, "The directory of the match is expanded");
        assert_eq!(build.children.len(), 1, "Only the match is listed");
        assert_eq!(build.children[0].name, "Makefile");
        assert!(build.children[0].is_match, "The match is highlighted");
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
}
