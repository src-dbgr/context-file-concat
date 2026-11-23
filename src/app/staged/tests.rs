//! Tests of the three steps: what they install, what they refuse to install, and
//! that they end where the plain way ends.

use super::*;
use crate::app::filtering::{apply_filter_texts, auto_expand_for_matches};
use crate::app::view_model::generate_ui_state;
use crate::app::view_model::rows::all_rows;

fn item(path: &str, is_directory: bool) -> FileItem {
    FileItem {
        path: PathBuf::from(path),
        is_directory,
        is_binary: false,
        size: if is_directory { 0 } else { 100 },
        depth: path.matches('/').count(),
        parent: Path::new(path).parent().map(Path::to_path_buf),
    }
}

fn paths_of(items: &[FileItem]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item.path.to_str().unwrap())
        .collect()
}

/// A scan of `/project` whose first level (`src`, `docs`) has been published.
fn state_with_a_walking_scan() -> (AppState, ScanToken) {
    let mut state = AppState::default();
    state.config.ignore_patterns.clear();
    state.current_path = "/project".to_string();
    let token = state.begin_scan();
    state.begin_scan_walk(&token);
    state.replace_with_shallow_file_list(
        Path::new("/project"),
        vec![item("/project/src", true), item("/project/docs", true)],
    );
    apply_filters(&mut state);
    state.expand_directories([PathBuf::from("/project/src")]);
    (state, token)
}

/// What the walk lists first: the first level again, and the content of `src`.
fn first_batch() -> Vec<FileItem> {
    vec![
        item("/project/src", true),
        item("/project/src/main.rs", false),
        item("/project/src/notes.txt", false),
        item("/project/docs", true),
    ]
}

/// The three steps of a batch in a row.
fn add_in_steps(state: &mut AppState, token: &ScanToken, items: Vec<FileItem>) -> bool {
    let Some(staged) = stage_scanned_items(state, token, items) else {
        return false;
    };
    commit_scanned_items(state, token, staged.prepare()).0
}

/// Asserts that the lists of `state` are what the plain way makes of its file
/// list: the filtered list is the filters applied to the file list, and the tree
/// (with the selection counts it shows) is the tree of that list.
#[track_caller]
fn assert_filtered_list_is_current(state: &mut AppState) {
    let filtered: Vec<String> = paths_of(state.filtered_file_list())
        .into_iter()
        .map(String::from)
        .collect();
    let rows = all_rows(state);
    let counts = state.file_tree().counts();

    // Whatever was derived is dropped and made anew.
    state.set_filtered_file_list(Vec::new());
    apply_filters(state);

    assert_eq!(paths_of(state.filtered_file_list()), filtered);
    assert_eq!(all_rows(state), rows);
    assert_eq!(state.file_tree().counts(), counts);
}

/// Asserts that asking for the hierarchy now does (`built == 1`) or does not
/// (`built == 0`) build it: whether what was prepared has been installed.
#[track_caller]
fn assert_hierarchy_builds(state: &AppState, before: usize, built: usize, context: &str) {
    state.file_tree();
    assert_eq!(state.file_tree_build_count() - before, built, "{context}");
}

#[test]
fn scanned_items_are_added_with_lists_that_were_built_outside_the_lock() {
    for query in ["", "main"] {
        let (mut state, token) = state_with_a_walking_scan();
        state.search_query = query.to_string();
        apply_filters(&mut state);
        generate_ui_state(&state);
        let built = state.file_tree_build_count();
        let revisions = (state.file_list_revision(), state.filtered_revision());

        let staged = stage_scanned_items(&mut state, &token, first_batch()).unwrap();
        let prepared = staged.prepare();
        // Until the last step the lists are as they were.
        assert_eq!(state.full_file_list().len(), 2);
        assert_eq!(
            (state.file_list_revision(), state.filtered_revision()),
            revisions
        );
        let (is_current, discarded) = commit_scanned_items(&mut state, &token, prepared);
        drop(discarded);

        assert!(is_current);
        assert_eq!(
            paths_of(state.full_file_list()),
            [
                "/project/src",
                "/project/docs",
                "/project/src/main.rs",
                "/project/src/notes.txt"
            ]
        );
        if query.is_empty() {
            assert!(state.filtered_list_is_file_list());
        } else {
            assert_eq!(
                paths_of(state.filtered_file_list()),
                ["/project/src", "/project/src/main.rs"]
            );
        }
        generate_ui_state(&state);
        assert_hierarchy_builds(&state, built, 0, query);
        assert_filtered_list_is_current(&mut state);
    }
}

#[test]
fn items_that_are_all_listed_already_change_nothing() {
    let (mut state, token) = state_with_a_walking_scan();
    let revisions = (state.file_list_revision(), state.filtered_revision());

    assert!(add_in_steps(
        &mut state,
        &token,
        vec![item("/project/src", true)]
    ));
    assert!(add_in_steps(&mut state, &token, Vec::new()));

    assert_eq!(state.full_file_list().len(), 2);
    assert_eq!(
        (state.file_list_revision(), state.filtered_revision()),
        revisions
    );
}

#[test]
fn a_stale_scan_can_neither_stage_nor_commit() {
    // Stale before the first step.
    let (mut state, token) = state_with_a_walking_scan();
    state.cancel_current_scan();
    assert!(stage_scanned_items(&mut state, &token, first_batch()).is_none());
    assert!(stage_scan_completion(&state, &token).is_none());
    assert_eq!(state.full_file_list().len(), 2);

    // Cancelled, or superseded by another scan, between the first and the last
    // step: with and without an active filter, a batch and the completion.
    for query in ["", "main"] {
        for supersede in [false, true] {
            for completion in [false, true] {
                let context = format!("{query:?}, superseded {supersede}, completion {completion}");
                let (mut state, token) = state_with_a_walking_scan();
                state.search_query = query.to_string();
                apply_filters(&mut state);
                let batch = (!completion).then(|| {
                    stage_scanned_items(&mut state, &token, first_batch())
                        .unwrap()
                        .prepare()
                });
                let end =
                    completion.then(|| stage_scan_completion(&state, &token).unwrap().prepare());

                if supersede {
                    let _ = state.begin_scan();
                } else {
                    state.cancel_current_scan();
                }
                let revisions = (state.file_list_revision(), state.filtered_revision());
                let built = state.file_tree_build_count();
                let (is_current, discarded) = match (batch, end) {
                    (Some(batch), _) => commit_scanned_items(&mut state, &token, batch),
                    (_, Some(end)) => {
                        commit_scan_completion(&mut state, &token, end, HashSet::new())
                    }
                    _ => unreachable!(),
                };
                drop(discarded);

                assert!(!is_current, "{context}");
                assert_eq!(
                    (state.file_list_revision(), state.filtered_revision()),
                    revisions,
                    "{context}"
                );
                assert_eq!(state.full_file_list().len(), 2, "{context}");
                assert!(!state.is_fully_scanned, "{context}");
                assert_hierarchy_builds(&state, built, 1, &context);
            }
        }
    }
}

/// What was prepared belongs to the state it was prepared for. If the file list,
/// the filtered list or the filters are others by the last step, it is dropped;
/// the items are listed all the same, and the lists are right.
#[test]
fn prepared_lists_are_not_installed_into_a_state_that_has_changed() {
    type Change = fn(&mut AppState);
    let changes: [(&str, Change); 9] = [
        ("a filter was set", |state| {
            state.set_filter_texts("main".to_string(), String::new(), String::new());
            apply_filter_texts(state);
        }),
        (
            "a filter text was changed, the filters not applied yet",
            |state| {
                state.extension_filter = "txt".to_string();
            },
        ),
        ("a level was loaded that the walk lists as well", |state| {
            let lazy_load = state.begin_lazy_load();
            assert!(state.merge_directory_level(
                &lazy_load,
                Path::new("/project/src"),
                vec![
                    item("/project/src/main.rs", false),
                    item("/project/src/only_listed_lazily.rs", false),
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
        (
            "an item was replaced, the list as long as before",
            |state| {
                state.edit_full_file_list(|list| list[1] = item("/project/other", true));
            },
        ),
        ("the list was replaced by an equal one", |state| {
            state.set_full_file_list(state.full_file_list().to_vec());
        }),
        ("the filtered list was replaced", |state| {
            state.set_filtered_file_list(state.full_file_list().to_vec());
        }),
        ("empty directories are removed now", |state| {
            state.config.remove_empty_directories = true;
            state.is_fully_scanned = true;
        }),
        (
            "the content filter has other matches, equal ones",
            |state| {
                let matches = state.content_search_results().clone();
                state.set_content_search_results(matches);
            },
        ),
    ];
    for (what, change) in changes {
        for with_content_filter in [false, true] {
            let context = format!("{what} (content filter: {with_content_filter})");
            let (mut state, token) = state_with_a_walking_scan();
            if with_content_filter {
                state.content_search_query = "fn".to_string();
                let search = state.begin_content_search();
                state.finish_content_search(
                    &search,
                    HashSet::from([PathBuf::from("/project/src/main.rs")]),
                );
                apply_filters(&mut state);
            } else if what.contains("content filter") {
                continue;
            }
            let staged = stage_scanned_items(&mut state, &token, first_batch()).unwrap();
            let prepared = staged.prepare();

            change(&mut state);
            let built = state.file_tree_build_count();
            let (is_current, discarded) = commit_scanned_items(&mut state, &token, prepared);
            drop(discarded);

            assert!(is_current, "{context}");
            let mut listed = paths_of(state.full_file_list());
            listed.sort_unstable();
            let unique = listed.iter().collect::<HashSet<_>>().len();
            assert_eq!(unique, listed.len(), "{context}: nothing is listed twice");
            for path in ["/project/src/main.rs", "/project/src/notes.txt"] {
                assert!(listed.contains(&path), "{context}: {path} is listed");
            }
            assert_hierarchy_builds(&state, built, 1, &context);
            assert_filtered_list_is_current(&mut state);
        }
    }
}

/// Changes that leave the lists and the filters alone do not stand in the way.
#[test]
fn prepared_lists_are_installed_whatever_else_has_changed() {
    let (mut state, token) = state_with_a_walking_scan();
    generate_ui_state(&state);
    let staged = stage_scanned_items(&mut state, &token, first_batch()).unwrap();
    let prepared = staged.prepare();

    state
        .selected_files
        .insert(PathBuf::from("/project/src/main.rs"));
    state.expand_directories([PathBuf::from("/project/docs")]);
    state.previewed_file_path = Some(PathBuf::from("/project/src/main.rs"));
    // Applying the filters again leaves a filtered list that is the file list.
    apply_filters(&mut state);
    let built = state.file_tree_build_count();
    let (is_current, discarded) = commit_scanned_items(&mut state, &token, prepared);
    drop(discarded);

    assert!(is_current);
    let ui_state = generate_ui_state(&state);
    assert_hierarchy_builds(&state, built, 0, "installed");
    assert_eq!(ui_state.selected_files_count, 1);
    assert_eq!(
        ui_state.row_named("src").unwrap().selection_state,
        crate::app::selection::SelectionState::Partial
    );
    assert_filtered_list_is_current(&mut state);
}

/// The selection counts of a list that grew by a batch follow from the ones before:
/// the tree is not counted again, whatever is selected.
#[test]
fn a_batch_extends_the_selection_counts_instead_of_counting_again() {
    let (mut state, token) = state_with_a_walking_scan();
    state.selected_files.extend(vec![
        PathBuf::from("/project/src/main.rs"),
        PathBuf::from("/project/src/core/mod.rs"),
        PathBuf::from("/project/not_listed.rs"),
    ]);
    generate_ui_state(&state);
    let counted = state.selection_count_count();

    assert!(add_in_steps(&mut state, &token, first_batch()));
    assert!(add_in_steps(
        &mut state,
        &token,
        vec![
            item("/project/src/core", true),
            item("/project/src/core/mod.rs", false),
            item("/project/src/core/lib.rs", false),
        ]
    ));
    state.expand_directories([PathBuf::from("/project/src/core")]);
    let ui_state = generate_ui_state(&state);

    assert_eq!(state.selection_count_count(), counted, "not counted again");
    let state_of = |name: &str| ui_state.row_named(name).unwrap().selection_state;
    use crate::app::selection::SelectionState::{Full, None as Unselected, Partial};
    assert_eq!(state_of("src"), Partial);
    assert_eq!(state_of("core"), Partial);
    assert_eq!(state_of("main.rs"), Full);
    assert_eq!(state_of("notes.txt"), Unselected);
    assert_eq!(state_of("docs"), Unselected);
    assert_filtered_list_is_current(&mut state);
}

#[test]
fn a_level_loaded_on_demand_is_added_in_steps() {
    for query in ["", "main"] {
        let (mut state, _token) = state_with_a_walking_scan();
        state.search_query = query.to_string();
        apply_filters(&mut state);
        generate_ui_state(&state);
        let built = state.file_tree_build_count();
        let lazy_load = state.begin_lazy_load();
        let level = vec![
            item("/project/src/main.rs", false),
            item("/project/src/lib.rs", false),
        ];

        let staged =
            stage_directory_level(&state, &lazy_load, Path::new("/project/src"), level).unwrap();
        let prepared = staged.prepare();
        assert_eq!(state.full_file_list().len(), 2);
        let patterns = HashSet::from(["target".to_string()]);
        let (is_current, discarded) =
            commit_directory_level(&mut state, &lazy_load, prepared, patterns.clone());
        drop(discarded);

        assert!(is_current);
        assert_eq!(state.full_file_list().len(), 4);
        assert!(state.loaded_dirs.contains(Path::new("/project/src")));
        assert_eq!(state.active_ignore_patterns, patterns);
        let ui_state = generate_ui_state(&state);
        assert!(ui_state.row_named("src").unwrap().children_loaded);
        assert_hierarchy_builds(&state, built, 0, query);
        assert_filtered_list_is_current(&mut state);

        // The walk comes by: nothing is listed twice.
        let token = state.begin_scan();
        state.replace_with_shallow_file_list(
            Path::new("/project"),
            vec![item("/project/src", true)],
        );
        apply_filters(&mut state);
        let lazy_load = state.begin_lazy_load();
        let level = vec![item("/project/src/main.rs", false)];
        let staged =
            stage_directory_level(&state, &lazy_load, Path::new("/project/src"), level).unwrap();
        assert!(commit_directory_level(&mut state, &lazy_load, staged.prepare(), HashSet::new()).0);
        assert!(add_in_steps(&mut state, &token, first_batch()));
        let mut listed = paths_of(state.full_file_list());
        listed.sort_unstable();
        assert_eq!(
            listed,
            [
                "/project/docs",
                "/project/src",
                "/project/src/main.rs",
                "/project/src/notes.txt"
            ]
        );
    }
}

#[test]
fn a_level_of_another_file_list_is_not_added() {
    let (mut state, _token) = state_with_a_walking_scan();
    let lazy_load = state.begin_lazy_load();
    let level = || vec![item("/project/src/main.rs", false)];
    let staged =
        stage_directory_level(&state, &lazy_load, Path::new("/project/src"), level()).unwrap();
    let prepared = staged.prepare();

    // New ignore patterns: the level was listed by the old ones.
    state.config.ignore_patterns.insert("nothing".to_string());
    state.apply_ignore_patterns();
    let revisions = (state.file_list_revision(), state.filtered_revision());
    let (is_current, discarded) =
        commit_directory_level(&mut state, &lazy_load, prepared, HashSet::new());
    drop(discarded);

    assert!(!is_current);
    assert_eq!(
        (state.file_list_revision(), state.filtered_revision()),
        revisions
    );
    assert!(!state.loaded_dirs.contains(Path::new("/project/src")));
    assert!(
        stage_directory_level(&state, &lazy_load, Path::new("/project/src"), level()).is_none()
    );
}

#[test]
fn the_list_is_completed_in_steps() {
    // Nothing to remove, no filter: the lists and what is derived from them stay.
    let (mut state, token) = state_with_a_walking_scan();
    assert!(add_in_steps(&mut state, &token, first_batch()));
    generate_ui_state(&state);
    let revisions = (state.file_list_revision(), state.filtered_revision());
    let built = state.file_tree_build_count();
    let staged = stage_scan_completion(&state, &token).unwrap();
    let patterns = HashSet::from(["target".to_string()]);
    assert!(commit_scan_completion(&mut state, &token, staged.prepare(), patterns.clone()).0);
    assert!(state.is_fully_scanned);
    assert_eq!(state.active_ignore_patterns, patterns);
    assert_eq!(
        state.loaded_dirs,
        HashSet::from([
            PathBuf::from("/project/src"),
            PathBuf::from("/project/docs")
        ])
    );
    assert_eq!(
        (state.file_list_revision(), state.filtered_revision()),
        revisions
    );
    assert_hierarchy_builds(&state, built, 0, "nothing changed");
    assert_filtered_list_is_current(&mut state);

    // An item of the first level that the walk did not find leaves the list, and
    // the selection with it. A file that was selected between the steps counts.
    for selected_between in [false, true] {
        let (mut state, token) = state_with_a_walking_scan();
        let mut batch = first_batch();
        batch.retain(|item| item.path != Path::new("/project/docs"));
        assert!(add_in_steps(&mut state, &token, batch));
        let select = |state: &mut AppState| {
            state.selected_files.extend(vec![
                PathBuf::from("/project/src/main.rs"),
                PathBuf::from("/project/docs"),
                PathBuf::from("/project/gone.rs"),
            ]);
        };
        if !selected_between {
            select(&mut state);
        }
        generate_ui_state(&state);
        let built = state.file_tree_build_count();
        let prepared = stage_scan_completion(&state, &token).unwrap().prepare();
        if selected_between {
            select(&mut state);
        }
        assert!(commit_scan_completion(&mut state, &token, prepared, HashSet::new()).0);

        assert_eq!(
            paths_of(state.full_file_list()),
            [
                "/project/src",
                "/project/src/main.rs",
                "/project/src/notes.txt"
            ]
        );
        assert_eq!(
            state.selected_files,
            HashSet::from([PathBuf::from("/project/src/main.rs")])
        );
        assert_hierarchy_builds(&state, built, 0, "an item removed");
        assert_filtered_list_is_current(&mut state);
    }

    // Empty directories are removed once the list is complete.
    let (mut state, token) = state_with_a_walking_scan();
    state.config.remove_empty_directories = true;
    assert!(add_in_steps(&mut state, &token, first_batch()));
    assert_eq!(state.filtered_file_list().len(), 4);
    generate_ui_state(&state);
    let built = state.file_tree_build_count();
    let prepared = stage_scan_completion(&state, &token).unwrap().prepare();
    assert!(commit_scan_completion(&mut state, &token, prepared, HashSet::new()).0);
    assert_eq!(
        paths_of(state.filtered_file_list()),
        [
            "/project/src",
            "/project/src/main.rs",
            "/project/src/notes.txt"
        ]
    );
    assert_eq!(state.full_file_list().len(), 4);
    assert_hierarchy_builds(&state, built, 0, "empty directories removed");
    assert_filtered_list_is_current(&mut state);
}

#[test]
fn the_matches_of_a_content_search_are_applied_in_steps() {
    let (mut state, token) = state_with_a_walking_scan();
    assert!(add_in_steps(&mut state, &token, first_batch()));
    state.content_search_query = "fn".to_string();
    let search = state.begin_content_search();
    apply_filters(&mut state);
    generate_ui_state(&state);
    let built = state.file_tree_build_count();
    let matches = HashSet::from([PathBuf::from("/project/src/main.rs")]);

    let staged = stage_content_matches(&state, &search, matches.clone()).unwrap();
    let prepared = staged.prepare();
    assert_eq!(state.filtered_file_list().len(), 4);
    let (is_current, discarded) = commit_content_matches(&mut state, &search, prepared);
    drop(discarded);

    assert!(is_current);
    assert_eq!(*state.content_search_results(), matches);
    assert_eq!(
        paths_of(state.filtered_file_list()),
        ["/project/src", "/project/src/main.rs"]
    );
    assert_hierarchy_builds(&state, built, 0, "matches");
    assert_filtered_list_is_current(&mut state);

    // A search that was followed by another one applies nothing.
    let search = state.begin_content_search();
    let prepared = stage_content_matches(&state, &search, HashSet::new())
        .unwrap()
        .prepare();
    let newer = state.begin_content_search();
    let revision = state.filtered_revision();
    assert!(!commit_content_matches(&mut state, &search, prepared).0);
    assert_eq!(*state.content_search_results(), matches);
    assert_eq!(state.filtered_revision(), revision);
    assert!(stage_content_matches(&state, &search, HashSet::new()).is_none());

    // The list changed while the matches were prepared: they are applied to the
    // list as it is.
    let prepared = stage_content_matches(&state, &newer, matches.clone())
        .unwrap()
        .prepare();
    assert!(add_in_steps(
        &mut state,
        &token,
        vec![item("/project/src/more.rs", false)]
    ));
    let built = state.file_tree_build_count();
    assert!(commit_content_matches(&mut state, &newer, prepared).0);
    assert_eq!(state.full_file_list().len(), 5);
    assert_hierarchy_builds(&state, built, 1, "the list changed");
    assert_filtered_list_is_current(&mut state);
}

/// Matches are a content filter only while there is a content query. If the query
/// is gone by the last step (and the search, against the rule, is not), the
/// matches are stored and the list is filtered without them.
#[test]
fn matches_are_not_applied_as_a_filter_that_is_not_there() {
    let (mut state, token) = state_with_a_walking_scan();
    assert!(add_in_steps(&mut state, &token, first_batch()));
    state.content_search_query = "fn".to_string();
    let search = state.begin_content_search();
    let matches = HashSet::from([PathBuf::from("/project/src/main.rs")]);
    let prepared = stage_content_matches(&state, &search, matches)
        .unwrap()
        .prepare();

    state.content_search_query.clear();
    assert!(commit_content_matches(&mut state, &search, prepared).0);

    assert_eq!(state.filtered_file_list().len(), 4);
    assert_filtered_list_is_current(&mut state);
}

//================================================================================//
//|                       THE STEPS AGAINST THE PLAIN WAY                        |//
//================================================================================//

/// A small generator of pseudo-random numbers (xorshift), so that a failing
/// sequence can be repeated by its seed.
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn pick<'a, T>(&mut self, of: &'a [T]) -> &'a T {
        &of[self.below(of.len())]
    }

    fn some_of<T: Clone>(&mut self, of: &[T]) -> Vec<T> {
        let mask = self.next();
        let chosen = of.iter().enumerate();
        chosen
            .filter(|(position, _)| mask >> (position % 64) & 1 == 1)
            .map(|(_, value)| value.clone())
            .collect()
    }
}

/// The project the sequences play in, in the order a walk lists it.
fn project() -> Vec<FileItem> {
    let mut items = Vec::new();
    for directory in ["a", "b", "c"] {
        items.push(item(&format!("/p/{directory}"), true));
        for file in ["f0.rs", "f1.md"] {
            items.push(item(&format!("/p/{directory}/{file}"), false));
        }
        for nested in ["x", "empty"] {
            let nested_path = format!("/p/{directory}/{nested}");
            items.push(item(&nested_path, true));
            if nested == "x" {
                for file in ["f1.rs", "f2.txt"] {
                    items.push(item(&format!("{nested_path}/{file}"), false));
                }
            }
        }
    }
    items.push(item("/p/f0.md", false));
    items
}

/// One state and the tokens of what is running in it.
struct Side {
    state: AppState,
    scan: Option<ScanToken>,
    search: Option<ContentSearchToken>,
}

impl Side {
    fn new() -> Self {
        let mut state = AppState::default();
        state.config.ignore_patterns.clear();
        state.current_path = "/p".to_string();
        Self {
            state,
            scan: None,
            search: None,
        }
    }
}

/// What a command does, synchronously and under one lock, as the commands of the
/// application do. The filters are applied where the command of the application
/// applies them.
#[derive(Debug, Clone)]
enum Command {
    SetFilters {
        query: String,
        extension: String,
    },
    ToggleSelection(PathBuf),
    Expand(PathBuf),
    Collapse(PathBuf),
    AddIgnorePattern(String),
    ToggleRemoveEmptyDirectories,
    LoadLevel {
        directory: PathBuf,
        items: Vec<FileItem>,
    },
    OpenContentSearch,
    AnswerContentSearch(Vec<PathBuf>),
    ClearContentSearch,
    CancelScan,
    Rescan {
        first_level: Vec<FileItem>,
    },
    /// The file list changed behind the back of the filters, keeping its length.
    ReplaceAnItem(usize),
    /// The filter text changed and the filters have not been applied.
    SetQueryOnly(String),
}

fn run(side: &mut Side, command: &Command) {
    let state = &mut side.state;
    match command.clone() {
        Command::SetFilters { query, extension } => {
            let content = state.content_search_query.clone();
            state.set_filter_texts(query, extension, content);
            apply_filter_texts(state);
        }
        Command::ToggleSelection(path) => {
            if !state.selected_files.remove(&path) {
                state.selected_files.insert(path);
            }
        }
        Command::Expand(path) => state.expand_directories([path]),
        Command::Collapse(path) => state.collapse_directories([path]),
        Command::AddIgnorePattern(pattern) => {
            state.config.ignore_patterns.insert(pattern);
            state.apply_ignore_patterns();
            apply_filters(state);
        }
        Command::ToggleRemoveEmptyDirectories => {
            state.config.remove_empty_directories ^= true;
            apply_filters(state);
        }
        Command::LoadLevel { directory, items } => {
            let lazy_load = state.begin_lazy_load();
            if state.merge_directory_level(&lazy_load, &directory, items, HashSet::new()) {
                apply_filters(state);
            }
        }
        Command::OpenContentSearch => {
            state.content_search_query = "fn".to_string();
            side.search = Some(state.begin_content_search());
            apply_filter_texts(state);
        }
        Command::AnswerContentSearch(matches) => {
            if let Some(search) = side.search.take() {
                if state.finish_content_search(&search, matches.into_iter().collect()) {
                    apply_filters(state);
                    auto_expand_for_matches(state);
                }
            }
        }
        Command::ClearContentSearch => {
            state.content_search_query.clear();
            state.clear_content_search();
            side.search = None;
            apply_filter_texts(state);
        }
        Command::CancelScan => {
            state.cancel_current_scan();
            side.scan = None;
        }
        Command::Rescan { first_level } => {
            let token = state.begin_scan();
            state.begin_scan_walk(&token);
            state.replace_with_shallow_file_list(Path::new("/p"), first_level);
            apply_filters(state);
            side.scan = Some(token);
        }
        Command::ReplaceAnItem(position) => {
            if !state.full_file_list().is_empty() {
                let position = position % state.full_file_list().len();
                state.edit_full_file_list(|list| list[position].size += 1);
            }
        }
        Command::SetQueryOnly(query) => state.search_query = query,
    }
}

fn random_command(random: &mut Random, project: &[FileItem]) -> Command {
    let directories: Vec<&FileItem> = project.iter().filter(|item| item.is_directory).collect();
    let files: Vec<&FileItem> = project.iter().filter(|item| !item.is_directory).collect();
    let queries = ["", "", "f1", "a", "zzz", "F0"];
    match random.below(16) {
        0 | 1 => Command::SetFilters {
            query: random.pick(&queries).to_string(),
            extension: random.pick(&["", "", "rs", "md"]).to_string(),
        },
        2 | 3 => Command::ToggleSelection(random.pick(&files).path.clone()),
        4 => Command::Expand(random.pick(&directories).path.clone()),
        5 => Command::Collapse(random.pick(&directories).path.clone()),
        6 => Command::AddIgnorePattern(
            random
                .pick(&["f2.txt", "empty/", "/b/", "*.md"])
                .to_string(),
        ),
        7 => Command::ToggleRemoveEmptyDirectories,
        8 | 9 => {
            let (directory, items) = random_level(random, project);
            Command::LoadLevel { directory, items }
        }
        10 => Command::OpenContentSearch,
        11 => Command::AnswerContentSearch(random_matches(random, project)),
        12 => Command::ClearContentSearch,
        13 => Command::CancelScan,
        14 => Command::Rescan {
            first_level: random_first_level(random, project),
        },
        _ => {
            if random.below(2) == 0 {
                Command::ReplaceAnItem(random.below(64))
            } else {
                Command::SetQueryOnly(random.pick(&queries).to_string())
            }
        }
    }
}

/// A directory of the project (or the project itself) and some of its children.
fn random_level(random: &mut Random, project: &[FileItem]) -> (PathBuf, Vec<FileItem>) {
    let mut directories: Vec<PathBuf> = vec![PathBuf::from("/p")];
    let listed = project.iter().filter(|item| item.is_directory);
    directories.extend(listed.map(|item| item.path.clone()));
    let directory = random.pick(&directories).clone();
    let children: Vec<FileItem> = project
        .iter()
        .filter(|item| item.path.parent() == Some(directory.as_path()))
        .cloned()
        .collect();
    let items = random.some_of(&children);
    (directory, items)
}

fn random_first_level(random: &mut Random, project: &[FileItem]) -> Vec<FileItem> {
    let first_level: Vec<FileItem> = project
        .iter()
        .filter(|item| item.path.parent() == Some(Path::new("/p")))
        .cloned()
        .collect();
    let mut items = random.some_of(&first_level);
    if random.below(4) == 0 {
        // Something the walk will not find.
        items.push(item("/p/deleted_meanwhile.rs", false));
    }
    items
}

fn random_matches(random: &mut Random, project: &[FileItem]) -> Vec<PathBuf> {
    let files: Vec<PathBuf> = project
        .iter()
        .filter(|item| !item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    match random.below(4) {
        0 => Vec::new(),
        _ => random.some_of(&files),
    }
}

/// What happens to the file list in three steps on one side, and plainly on the
/// other.
#[derive(Debug, Clone)]
enum Event {
    Batch(Vec<FileItem>),
    Completion,
    Level {
        directory: PathBuf,
        items: Vec<FileItem>,
    },
    Matches(Vec<PathBuf>),
}

/// The event in three steps on `stepped`, with `between` happening after the first
/// step, and under one lock on `plain`, where `between` has happened before.
fn happen(stepped: &mut Side, plain: &mut Side, event: Event, between: &[Command]) {
    let meanwhile = |stepped: &mut Side, plain: &mut Side| {
        for command in between {
            run(stepped, command);
            run(plain, command);
        }
    };
    let patterns = || HashSet::from(["seen_by_the_walk".to_string()]);
    match event {
        Event::Batch(items) => {
            let tokens = (stepped.scan.clone(), plain.scan.clone());
            let staged = tokens.0.as_ref().and_then(|token| {
                stage_scanned_items(&mut stepped.state, token, items.clone()).map(Staged::prepare)
            });
            meanwhile(stepped, plain);
            if let (Some(token), Some(prepared)) = (&tokens.0, staged) {
                drop(commit_scanned_items(&mut stepped.state, token, prepared));
            }
            if let Some(token) = &tokens.1 {
                if plain.state.merge_scanned_items(token, items) {
                    apply_filters(&mut plain.state);
                }
            }
        }
        Event::Completion => {
            let tokens = (stepped.scan.clone(), plain.scan.clone());
            let staged = tokens.0.as_ref().and_then(|token| {
                stage_scan_completion(&stepped.state, token).map(Staged::prepare)
            });
            meanwhile(stepped, plain);
            if let (Some(token), Some(prepared)) = (&tokens.0, staged) {
                let (done, discarded) =
                    commit_scan_completion(&mut stepped.state, token, prepared, patterns());
                drop(discarded);
                if done && stepped.state.finish_scan(token) {
                    stepped.scan = None;
                }
            }
            if let Some(token) = &tokens.1 {
                if plain.state.complete_scanned_file_list(token, patterns()) {
                    apply_filters(&mut plain.state);
                    if plain.state.finish_scan(token) {
                        plain.scan = None;
                    }
                }
            }
        }
        Event::Level { directory, items } => {
            let tokens = (
                stepped.state.begin_lazy_load(),
                plain.state.begin_lazy_load(),
            );
            let staged =
                stage_directory_level(&stepped.state, &tokens.0, &directory, items.clone())
                    .map(Staged::prepare);
            meanwhile(stepped, plain);
            if let Some(prepared) = staged {
                drop(commit_directory_level(
                    &mut stepped.state,
                    &tokens.0,
                    prepared,
                    patterns(),
                ));
            }
            if plain
                .state
                .merge_directory_level(&tokens.1, &directory, items, patterns())
            {
                apply_filters(&mut plain.state);
            }
        }
        Event::Matches(matches) => {
            let tokens = (stepped.search.take(), plain.search.take());
            let matches: HashSet<PathBuf> = matches.into_iter().collect();
            let staged = tokens.0.as_ref().and_then(|token| {
                stage_content_matches(&stepped.state, token, matches.clone()).map(Staged::prepare)
            });
            meanwhile(stepped, plain);
            if let (Some(token), Some(prepared)) = (&tokens.0, staged) {
                let (applied, discarded) =
                    commit_content_matches(&mut stepped.state, token, prepared);
                drop(discarded);
                if applied {
                    auto_expand_for_matches(&mut stepped.state);
                }
            }
            if let Some(token) = &tokens.1 {
                if plain.state.finish_content_search(token, matches) {
                    apply_filters(&mut plain.state);
                    auto_expand_for_matches(&mut plain.state);
                }
            }
        }
    }
}

/// Asserts that the two sides show and hold the same.
#[track_caller]
fn assert_same(stepped: &Side, plain: &Side, context: &str) {
    let (a, b) = (&stepped.state, &plain.state);
    assert_eq!(
        paths_of(a.full_file_list()),
        paths_of(b.full_file_list()),
        "file list, {context}"
    );
    assert_eq!(
        paths_of(a.filtered_file_list()),
        paths_of(b.filtered_file_list()),
        "filtered list, {context}"
    );
    assert_eq!(all_rows(a), all_rows(b), "rows, {context}");
    assert_eq!(a.file_tree().counts(), b.file_tree().counts(), "{context}");
    assert_eq!(*a.selected_files, *b.selected_files, "selection, {context}");
    assert_eq!(*a.expanded_dirs, *b.expanded_dirs, "expansion, {context}");
    assert_eq!(a.loaded_dirs, b.loaded_dirs, "loaded, {context}");
    assert_eq!(a.is_fully_scanned, b.is_fully_scanned, "{context}");
    assert_eq!(a.is_scanning(), b.is_scanning(), "{context}");
    assert_eq!(
        a.active_ignore_patterns, b.active_ignore_patterns,
        "patterns, {context}"
    );
    assert_eq!(
        a.content_search_results(),
        b.content_search_results(),
        "matches, {context}"
    );
    let (ui_a, ui_b) = (generate_ui_state(a), generate_ui_state(b));
    assert_eq!(ui_a.rows, ui_b.rows, "window, {context}");
    assert_eq!(
        (
            ui_a.rows_total,
            ui_a.total_files_found,
            ui_a.visible_files_count
        ),
        (
            ui_b.rows_total,
            ui_b.total_files_found,
            ui_b.visible_files_count
        ),
        "{context}"
    );
    assert_eq!(
        (
            ui_a.tree_files_count,
            ui_a.tree_dirs_count,
            ui_a.selected_files_count
        ),
        (
            ui_b.tree_files_count,
            ui_b.tree_dirs_count,
            ui_b.selected_files_count
        ),
        "{context}"
    );
}

/// One sequence: a scan (and, now and then, a re-scan) of the project, with
/// commands before, between and after the steps of its events.
fn play(seed: u64) {
    let project = project();
    let mut random = Random::new(seed);
    let (mut stepped, mut plain) = (Side::new(), Side::new());
    let begin = Command::Rescan {
        first_level: random_first_level(&mut random, &project),
    };
    run(&mut stepped, &begin);
    run(&mut plain, &begin);
    let mut walked = 0;

    for step in 0..12 {
        let context = format!("seed {seed}, step {step}");
        if random.below(3) == 0 {
            let command = random_command(&mut random, &project);
            if matches!(command, Command::Rescan { .. }) {
                walked = 0;
            }
            run(&mut stepped, &command);
            run(&mut plain, &command);
            assert_same(&stepped, &plain, &format!("{context}, after {command:?}"));
            continue;
        }

        let event = match random.below(8) {
            0..=3 if walked < project.len() => {
                let until = (walked + 1 + random.below(6)).min(project.len());
                let batch = project[walked..until].to_vec();
                walked = until;
                Event::Batch(batch)
            }
            0..=3 => Event::Completion,
            4 | 5 => {
                let (directory, items) = random_level(&mut random, &project);
                Event::Level { directory, items }
            }
            6 => Event::Matches(random_matches(&mut random, &project)),
            _ => Event::Completion,
        };
        let between: Vec<Command> = (0..random.below(3))
            .map(|_| random_command(&mut random, &project))
            .collect();
        if between.iter().any(|c| matches!(c, Command::Rescan { .. })) {
            walked = 0;
        }
        happen(&mut stepped, &mut plain, event.clone(), &between);
        assert_same(
            &stepped,
            &plain,
            &format!("{context}, after {event:?} with {between:?} between its steps"),
        );
    }
}

/// Whatever happens between the steps of an event (filters set and cleared, ignore
/// patterns, levels that overlap with the walk, selection, expansion, a content
/// search opened, answered or cleared, the removal of empty directories, a
/// cancellation, a re-scan, changes behind the back of the filters): the state ends
/// as it ends when the event happens under one lock after all of that.
#[test]
fn events_in_steps_end_like_events_under_one_lock() {
    use rayon::prelude::*;
    (0..20_000u64).into_par_iter().for_each(play);
}
