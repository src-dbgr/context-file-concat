use super::*;

fn scanned_file(path: &str) -> FileItem {
    create_test_file_item(path, false)
}

fn scanned_dir(path: &str) -> FileItem {
    create_test_file_item(path, true)
}

fn listed_paths(state: &AppState) -> Vec<&str> {
    state
        .full_file_list()
        .iter()
        .map(|item| item.path.to_str().unwrap())
        .collect()
}

/// A state in which a scan of `/p` has published the first level: `/p/a` (a
/// directory) and `/p/f.txt`.
fn state_after_shallow_phase() -> (AppState, ScanToken) {
    let mut state = AppState {
        current_path: "/p".to_string(),
        ..AppState::default()
    };
    let token = state.begin_scan();
    state.begin_scan_walk(&token);
    state.replace_with_shallow_file_list(
        Path::new("/p"),
        vec![scanned_dir("/p/a"), scanned_file("/p/f.txt")],
    );
    (state, token)
}

/// The first half of a merge: the walk's items that are not listed yet are
/// handed back, the others are confirmed, and the list is left alone.
#[test]
fn test_confirming_scanned_items_tells_the_new_ones_without_listing_them() {
    let (mut state, token) = state_after_shallow_phase();

    let new_items = state
        .confirm_scanned_items(
            &token,
            vec![scanned_dir("/p/a"), scanned_file("/p/a/x.txt")],
        )
        .unwrap();
    assert_eq!(new_items.len(), 1);
    assert_eq!(new_items[0].path, Path::new("/p/a/x.txt"));
    assert_eq!(listed_paths(&state), ["/p/a", "/p/f.txt"]);

    // `/p/a` is confirmed by this, `/p/f.txt` is not: the end of the walk
    // removes it.
    assert!(state.complete_scanned_file_list(&token, HashSet::new()));
    assert_eq!(listed_paths(&state), ["/p/a"]);

    let (mut state, token) = state_after_shallow_phase();
    state.cancel_current_scan();
    assert!(state
        .confirm_scanned_items(&token, vec![scanned_file("/p/late.txt")])
        .is_none());
}

/// Every way of changing the file list gives it a new revision, and a filtered
/// list is the file list only for as long as that revision stands.
#[test]
fn test_every_change_of_the_file_list_is_a_new_revision() {
    let (mut state, token) = state_after_shallow_phase();
    let mut revision = state.file_list_revision();
    let mut assert_changed = |state: &AppState, what: &str| {
        assert_ne!(state.file_list_revision(), revision, "{what}");
        assert!(!state.filtered_list_is_file_list(), "{what}");
        revision = state.file_list_revision();
    };
    let mirror = |state: &mut AppState| {
        state.set_shared_filtered_file_list(state.shared_full_file_list());
        assert!(state.filtered_list_is_file_list());
    };

    mirror(&mut state);
    assert!(state.merge_scanned_items(&token, vec![scanned_file("/p/a/x.txt")]));
    assert_changed(&state, "items of the walk");

    mirror(&mut state);
    let lazy_load = state.begin_lazy_load();
    assert!(state.merge_directory_level(
        &lazy_load,
        Path::new("/p/a"),
        vec![scanned_file("/p/a/y.txt")],
        HashSet::new()
    ));
    assert_changed(&state, "a level loaded on demand");

    mirror(&mut state);
    assert!(state.complete_scanned_file_list(&token, HashSet::new()));
    assert_changed(&state, "the completion, which removed an item");

    mirror(&mut state);
    state.config.ignore_patterns = HashSet::from(["y.txt".to_string()]);
    state.apply_ignore_patterns();
    assert_changed(&state, "ignore patterns");

    // A list of the same length, and even an equal one, is another list.
    mirror(&mut state);
    state.edit_full_file_list(|list| list[0] = scanned_dir("/p/b"));
    assert_changed(&state, "an item replaced");
    mirror(&mut state);
    state.set_full_file_list(state.full_file_list().to_vec());
    assert_changed(&state, "the list replaced by an equal one");

    mirror(&mut state);
    state.replace_with_shallow_file_list(Path::new("/p"), vec![scanned_dir("/p/a")]);
    assert_changed(&state, "the first level of a scan");
    mirror(&mut state);
    state.reset_directory_state();
    assert_changed(&state, "the directory closed");

    // A filtered list that was made from the file list is not the file list.
    state.set_full_file_list(vec![scanned_dir("/p/a")]);
    state.set_filtered_file_list(state.full_file_list().to_vec());
    assert!(!state.filtered_list_is_file_list());
}

/// Lists that were prepared elsewhere are installed in one step, with the
/// hierarchy that came with the filtered list.
#[test]
fn test_installing_prepared_lists_installs_their_hierarchy() {
    let (mut state, _token) = state_after_shallow_phase();
    state.set_shared_filtered_file_list(state.shared_full_file_list());
    let (file_revision, filtered_revision) =
        (state.file_list_revision(), state.filtered_revision());

    let mut longer = state.full_file_list().to_vec();
    longer.push(scanned_file("/p/a/x.txt"));
    let longer = Arc::new(longer);
    drop(state.install_file_lists(FileLists {
        full: Some(Arc::clone(&longer)),
        filtered: Some(IndexedList::build(longer, PathBuf::from("/p"))),
        appended_from: Some(2),
        made_with: Arc::new(FilterInputs::of(&state)),
    }));

    assert_eq!(listed_paths(&state), ["/p/a", "/p/f.txt", "/p/a/x.txt"]);
    assert_eq!(state.filtered_file_list().len(), 3);
    assert_ne!(state.file_list_revision(), file_revision);
    assert_ne!(state.filtered_revision(), filtered_revision);
    assert!(state.filtered_list_is_file_list());
    assert_eq!(state.file_tree().children(0), [2]);
    assert_eq!(state.file_tree_build_count(), 0);

    // Nothing to install: nothing changes.
    let (file_revision, filtered_revision) =
        (state.file_list_revision(), state.filtered_revision());
    drop(state.install_file_lists(FileLists {
        full: None,
        filtered: None,
        appended_from: None,
        made_with: Arc::new(FilterInputs::of(&state)),
    }));
    assert_eq!(state.file_list_revision(), file_revision);
    assert_eq!(state.filtered_revision(), filtered_revision);
}

/// The selection counts of a list that only grew are made from the ones that
/// were there: the tree is not counted again.
#[test]
fn test_installing_a_longer_list_extends_the_selection_counts() {
    let (mut state, _token) = state_after_shallow_phase();
    state.set_shared_filtered_file_list(state.shared_full_file_list());
    state.selected_files.insert(PathBuf::from("/p/a/x.txt"));
    state.directory_selection();
    assert_eq!(state.selection_count_count(), 1);

    let mut longer = state.full_file_list().to_vec();
    longer.extend([scanned_file("/p/a/x.txt"), scanned_file("/p/a/y.txt")]);
    let longer = Arc::new(longer);
    drop(state.install_file_lists(FileLists {
        full: Some(Arc::clone(&longer)),
        filtered: Some(IndexedList::build(longer, PathBuf::from("/p"))),
        appended_from: Some(2),
        made_with: Arc::new(FilterInputs::of(&state)),
    }));

    let counts = state.directory_selection();
    assert_eq!(state.selection_count_count(), 1, "not counted again");
    assert_eq!(
        counts.state_of(
            state.filtered_file_list(),
            &state.file_tree(),
            &state.selected_files,
            0
        ),
        crate::app::selection::SelectionState::Partial
    );
}

#[test]
fn test_batches_of_the_walk_are_merged_without_duplicating_the_first_level() {
    let (mut state, token) = state_after_shallow_phase();

    assert!(state.merge_scanned_items(
        &token,
        vec![scanned_dir("/p/a"), scanned_file("/p/a/x.txt")]
    ));
    assert!(state.merge_scanned_items(
        &token,
        vec![scanned_file("/p/a/y.txt"), scanned_file("/p/f.txt")]
    ));

    assert_eq!(
        listed_paths(&state),
        ["/p/a", "/p/f.txt", "/p/a/x.txt", "/p/a/y.txt"]
    );
    // The walk may not be done with `/p/a`: it does not count as loaded yet.
    assert_eq!(state.loaded_dirs, HashSet::from([PathBuf::from("/p")]));
    assert!(!state.is_fully_scanned);
}

#[test]
fn test_batches_of_an_outdated_scan_are_not_merged() {
    let (mut state, token) = state_after_shallow_phase();
    let newer = state.begin_scan();

    assert!(!state.merge_scanned_items(&token, vec![scanned_file("/p/a/x.txt")]));
    assert!(!state.complete_scanned_file_list(&token, HashSet::new()));

    assert_eq!(listed_paths(&state), ["/p/a", "/p/f.txt"]);
    assert!(!state.is_fully_scanned);
    assert!(state.is_current_scan(&newer));
}

#[test]
fn test_a_level_loaded_during_the_walk_is_not_listed_twice() {
    let (mut state, token) = state_after_shallow_phase();
    // The walk has delivered one child of `/p/a` when the user expands it.
    state.merge_scanned_items(&token, vec![scanned_file("/p/a/x.txt")]);
    let lazy = state.begin_lazy_load();

    assert!(state.merge_directory_level(
        &lazy,
        Path::new("/p/a"),
        vec![scanned_file("/p/a/x.txt"), scanned_file("/p/a/y.txt")],
        HashSet::from(["*.log".to_string()]),
    ));
    // The walk goes on and comes to the second child itself.
    state.merge_scanned_items(&token, vec![scanned_file("/p/a/y.txt")]);

    assert_eq!(
        listed_paths(&state),
        ["/p/a", "/p/f.txt", "/p/a/x.txt", "/p/a/y.txt"]
    );
    assert!(state.loaded_dirs.contains(Path::new("/p/a")));
    assert!(state.active_ignore_patterns.contains("*.log"));
}

#[test]
fn test_the_completed_list_is_what_the_walk_saw() {
    let (mut state, token) = state_after_shallow_phase();
    let lazy = state.begin_lazy_load();
    state.merge_directory_level(
        &lazy,
        Path::new("/p/a"),
        vec![scanned_file("/p/a/x.txt"), scanned_file("/p/a/gone.txt")],
        HashSet::from(["lazy".to_string()]),
    );
    state.selected_files = HashSet::from([
        PathBuf::from("/p/a/x.txt"),
        PathBuf::from("/p/a/gone.txt"),
        PathBuf::from("/p/f.txt"),
    ])
    .into();
    // The walk finds neither `/p/f.txt` nor `/p/a/gone.txt` (deleted meanwhile).
    state.merge_scanned_items(
        &token,
        vec![
            scanned_dir("/p/a"),
            scanned_file("/p/a/x.txt"),
            scanned_dir("/p/a/sub"),
        ],
    );

    assert!(state.complete_scanned_file_list(&token, HashSet::from(["walk".to_string()])));

    assert_eq!(listed_paths(&state), ["/p/a", "/p/a/x.txt", "/p/a/sub"]);
    assert_eq!(
        state.selected_files,
        HashSet::from([PathBuf::from("/p/a/x.txt")])
    );
    assert_eq!(
        state.loaded_dirs,
        HashSet::from([PathBuf::from("/p/a"), PathBuf::from("/p/a/sub")])
    );
    assert_eq!(
        state.active_ignore_patterns,
        HashSet::from(["walk".to_string()])
    );
    assert!(state.is_fully_scanned);
}

/// A re-scan is started and the user expands a directory before the first level
/// of the scan is there: the level that was loaded for the new scan survives the
/// replacement of the list, and the walk does not list it twice.
#[test]
fn test_a_level_loaded_before_the_first_level_of_a_rescan_is_kept() {
    let mut state = AppState {
        current_path: "/p".to_string(),
        ..AppState::default()
    };
    state.replace_with_complete_file_list(vec![scanned_dir("/p/a"), scanned_file("/p/a/old.txt")]);
    let before_the_rescan = state.begin_lazy_load();
    let token = state.begin_scan();
    let lazy = state.begin_lazy_load();
    assert!(!state.merge_directory_level(
        &before_the_rescan,
        Path::new("/p/a"),
        vec![scanned_file("/p/a/stale.txt")],
        HashSet::new(),
    ));
    assert!(state.merge_directory_level(
        &lazy,
        Path::new("/p/a"),
        vec![scanned_file("/p/a/x.txt")],
        HashSet::new(),
    ));

    state.replace_with_shallow_file_list(Path::new("/p"), vec![scanned_dir("/p/a")]);

    assert_eq!(listed_paths(&state), ["/p/a", "/p/a/x.txt"]);
    assert!(state.loaded_dirs.contains(Path::new("/p/a")));

    // The walk confirms the level; a level loaded from here on is merged directly.
    state.merge_scanned_items(
        &token,
        vec![scanned_dir("/p/a"), scanned_file("/p/a/x.txt")],
    );
    state.complete_scanned_file_list(&token, HashSet::new());
    assert_eq!(listed_paths(&state), ["/p/a", "/p/a/x.txt"]);
}

/// Only the scan that has begun keeps levels for its list: a cancelled scan, or
/// the opening of another directory, leaves nothing behind for a later scan.
#[test]
fn test_levels_are_not_kept_beyond_the_scan_they_were_loaded_for() {
    let mut state = AppState {
        current_path: "/p".to_string(),
        ..AppState::default()
    };
    let _cancelled = state.begin_scan();
    let lazy = state.begin_lazy_load();
    state.merge_directory_level(
        &lazy,
        Path::new("/p/a"),
        vec![scanned_file("/p/a/x.txt")],
        HashSet::new(),
    );
    state.cancel_current_scan();

    let _next = state.begin_scan();
    state.replace_with_shallow_file_list(Path::new("/p"), vec![scanned_dir("/p/a")]);

    assert_eq!(listed_paths(&state), ["/p/a"]);
    assert!(!state.loaded_dirs.contains(Path::new("/p/a")));
}

#[test]
fn test_a_level_loaded_for_a_replaced_list_is_not_merged() {
    let (mut state, _token) = state_after_shallow_phase();
    let lazy = state.begin_lazy_load();
    let _newer = state.begin_scan();

    assert!(!state.merge_directory_level(
        &lazy,
        Path::new("/p/a"),
        vec![scanned_file("/p/a/x.txt")],
        HashSet::new(),
    ));

    assert_eq!(listed_paths(&state), ["/p/a", "/p/f.txt"]);
    assert!(!state.loaded_dirs.contains(Path::new("/p/a")));
}

#[test]
fn test_a_level_loaded_after_a_cancelled_scan_completes_what_the_walk_left() {
    let (mut state, token) = state_after_shallow_phase();
    state.merge_scanned_items(&token, vec![scanned_file("/p/a/x.txt")]);
    state.cancel_current_scan();
    let lazy = state.begin_lazy_load();

    state.merge_directory_level(
        &lazy,
        Path::new("/p/a"),
        vec![scanned_file("/p/a/x.txt"), scanned_file("/p/a/y.txt")],
        HashSet::new(),
    );

    assert_eq!(
        listed_paths(&state),
        ["/p/a", "/p/f.txt", "/p/a/x.txt", "/p/a/y.txt"]
    );
}

#[tokio::test]
async fn test_shallow_file_list_forgets_previously_loaded_directories() {
    let mut state = AppState::default();
    let root = PathBuf::from("/project");
    state.replace_with_complete_file_list(vec![
        create_test_file_item("/project/src", true),
        create_test_file_item("/project/src/core", true),
        create_test_file_item("/project/src/core/lib.rs", false),
    ]);
    assert_eq!(
        state.loaded_dirs,
        HashSet::from([
            PathBuf::from("/project/src"),
            PathBuf::from("/project/src/core")
        ])
    );

    state.replace_with_shallow_file_list(&root, vec![create_test_file_item("/project/src", true)]);

    assert_eq!(state.loaded_dirs, HashSet::from([root]));
    assert_eq!(state.full_file_list().len(), 1);
}
