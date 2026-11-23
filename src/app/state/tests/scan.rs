use super::*;

#[tokio::test]
async fn test_cancel_scan_with_active_task() {
    // Arrange
    let mut state = AppState::default();
    let token = state.begin_scan();
    state.attach_scan_task(&token, create_dummy_task());
    state.scan_progress.current_scanning_path = "In progress...".to_string();

    // Act
    state.cancel_current_scan();
    // Give tokio a moment to process the abort
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Assert
    assert!(
        !state.has_scan_task(),
        "Scan task should be taken from the state"
    );
    assert!(!state.is_scanning(), "is_scanning should be false");
    assert_eq!(
        state.status,
        StatusNotice::ScanCancelled,
        "The cancellation should be noted"
    );
    assert!(
        state.scan_progress.current_scanning_path.is_empty(),
        "The progress of the cancelled scan should be gone"
    );
    assert!(
        token.is_cancelled(),
        "Cancellation flag should be set to true"
    );
}

#[tokio::test]
async fn test_cancel_scan_with_no_task_does_not_panic() {
    // Arrange
    let mut state = AppState::default();
    assert!(!state.is_scanning(), "precondition");
    assert!(!state.has_scan_task(), "precondition");

    // Act & Assert
    // The test passes if this call does not panic.
    state.cancel_current_scan();
    assert!(!state.is_scanning());
}

#[tokio::test]
async fn test_begin_scan_marks_state_as_scanning_with_fresh_flag() {
    let mut state = AppState::default();

    let token = state.begin_scan();

    assert!(state.is_scanning());
    assert!(state.is_current_scan(&token));
    assert!(!token.is_cancelled());
    state.cancel_current_scan();
    assert!(
        token.is_cancelled(),
        "State must control the cancellation flag of the current scan"
    );
}

#[tokio::test]
async fn test_begin_scan_supersedes_previous_scan() {
    let mut state = AppState::default();
    let old_token = state.begin_scan();
    let old_handle = create_dummy_task();
    let old_abort = old_handle.abort_handle();
    state.attach_scan_task(&old_token, old_handle);

    let new_token = state.begin_scan();
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert!(old_token.is_cancelled(), "Old scan must be told to stop");
    assert!(old_abort.is_finished(), "Old scan task must be aborted");
    assert!(!state.is_current_scan(&old_token));
    assert!(state.is_current_scan(&new_token));
    assert!(!new_token.is_cancelled());
    assert!(state.is_scanning());
}

#[tokio::test]
async fn test_finish_scan_with_stale_token_leaves_new_scan_untouched() {
    let mut state = AppState::default();
    let old_token = state.begin_scan();
    let new_token = state.begin_scan();
    state.attach_scan_task(&new_token, create_dummy_task());

    let changed = state.finish_scan(&old_token);

    assert!(!changed);
    assert!(state.is_scanning(), "New scan must still be running");
    assert!(state.has_scan_task(), "New scan must stay cancellable");
    assert!(state.is_current_scan(&new_token));
    state.cancel_current_scan();
}

#[tokio::test]
async fn test_finish_scan_with_current_token_releases_scanning_state() {
    let mut state = AppState::default();
    let token = state.begin_scan();
    state.attach_scan_task(&token, create_dummy_task());

    assert!(state.finish_scan(&token));

    assert!(!state.is_scanning());
    assert!(!state.has_scan_task());
    assert!(!state.is_current_scan(&token));
    assert!(!state.finish_scan(&token), "Finishing twice is a no-op");
}

#[tokio::test]
async fn test_attach_scan_task_with_stale_token_aborts_instead_of_storing() {
    let mut state = AppState::default();
    let old_token = state.begin_scan();
    let new_token = state.begin_scan();
    let new_handle = create_dummy_task();
    let new_abort = new_handle.abort_handle();
    state.attach_scan_task(&new_token, new_handle);

    let stale_handle = create_dummy_task();
    let stale_abort = stale_handle.abort_handle();
    state.attach_scan_task(&old_token, stale_handle);
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert!(stale_abort.is_finished(), "Stale task must be aborted");
    assert!(!new_abort.is_finished(), "Current task must keep running");
    assert!(state.has_scan_task());
    state.cancel_current_scan();
}

#[tokio::test]
async fn test_cancel_scan_before_task_is_attached_is_not_lost() {
    let mut state = AppState::default();
    let token = state.begin_scan();

    state.cancel_current_scan();

    assert!(!state.is_scanning());
    assert!(token.is_cancelled());
    assert!(!state.is_current_scan(&token));
    assert_eq!(state.status, StatusNotice::ScanCancelled);

    // The task handle arriving late must not resurrect the cancelled scan.
    let late_handle = create_dummy_task();
    let late_abort = late_handle.abort_handle();
    state.attach_scan_task(&token, late_handle);
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!state.has_scan_task());
    assert!(late_abort.is_finished());
}

// --- Opening a path ---

fn state_with_loaded_directory() -> AppState {
    let mut state = AppState {
        current_path: "/old".to_string(),
        full_file_list: Arc::new(vec![create_test_file_item("/old/a.rs", false)]),
        selected_files: HashSet::from([PathBuf::from("/old/a.rs")]).into(),
        is_fully_scanned: true,
        ..AppState::default()
    };
    state.set_filtered_file_list(state.full_file_list().to_vec());
    state
}

#[test]
fn test_begin_open_registers_the_scan_and_leaves_the_loaded_directory_alone() {
    let mut state = state_with_loaded_directory();
    let lazy_load = state.begin_lazy_load();

    let token = state.begin_open(Path::new("/new"));

    assert!(state.is_scanning() && state.is_opening());
    assert!(state.is_current_scan(&token));
    assert_eq!(state.current_path, "/old");
    assert_eq!(state.full_file_list().len(), 1);
    assert_eq!(state.selected_files.len(), 1);
    assert!(state.is_fully_scanned);
    assert!(
        state.is_current_lazy_load(&lazy_load) && !lazy_load.is_cancelled(),
        "the file list is still the current one"
    );
}

#[test]
fn test_entering_the_opened_directory_replaces_the_loaded_one_and_keeps_the_scan() {
    let mut state = state_with_loaded_directory();
    let lazy_load = state.begin_lazy_load();
    let token = state.begin_open(Path::new("/new"));

    assert!(state.enter_opened_directory(&token, Path::new("/new")));

    assert!(state.is_scanning() && !state.is_opening());
    assert!(state.is_current_scan(&token) && !token.is_cancelled());
    assert_eq!(state.current_path, "/new");
    assert!(state.full_file_list().is_empty() && state.filtered_file_list().is_empty());
    assert!(state.selected_files.is_empty());
    assert!(!state.is_fully_scanned);
    assert!(lazy_load.is_cancelled());
    assert!(
        !state.enter_opened_directory(&token, Path::new("/again")),
        "a directory is entered once"
    );
    assert_eq!(state.current_path, "/new");
}

#[test]
fn test_only_the_scan_that_is_opening_enters_its_directory() {
    let mut state = state_with_loaded_directory();
    let superseded = state.begin_open(Path::new("/new"));
    let current = state.begin_open(Path::new("/new"));
    assert!(!state.enter_opened_directory(&superseded, Path::new("/stale")));
    assert_eq!(state.current_path, "/old");
    assert!(state.enter_opened_directory(&current, Path::new("/new")));

    // A re-scan keeps the directory it has.
    let mut state = state_with_loaded_directory();
    let rescan = state.begin_scan();
    assert!(!state.enter_opened_directory(&rescan, Path::new("/new")));
    assert_eq!(state.current_path, "/old");
}

#[test]
fn test_abandoning_an_open_keeps_the_loaded_directory() {
    let mut state = state_with_loaded_directory();
    let token = state.begin_open(Path::new("/nonexistent"));

    let todo = state.abandon_scan(&token);

    assert!(
        matches!(todo, AfterAbandonedScan::Nothing),
        "nothing was running when the opening began, got {todo:?}"
    );
    assert!(!state.is_scanning() && !state.is_opening());
    assert_eq!(state.current_path, "/old");
    assert_eq!(state.selected_files.len(), 1);
    assert!(state.is_fully_scanned);
    assert_eq!(state.status, StatusNotice::Ready);
}

#[test]
fn test_abandoning_an_open_asks_for_the_scan_it_superseded() {
    let mut state = state_with_loaded_directory();
    let _superseded = state.begin_scan();
    let token = state.begin_open(Path::new("/nonexistent"));

    let todo = state.abandon_scan(&token);

    assert!(matches!(todo, AfterAbandonedScan::RescanLoadedDirectory));
    assert!(!state.is_scanning());

    // Without a loaded directory there is nothing to scan again.
    let mut state = AppState::default();
    let _superseded = state.begin_scan();
    let token = state.begin_open(Path::new("/nonexistent"));
    assert!(matches!(
        state.abandon_scan(&token),
        AfterAbandonedScan::Nothing
    ));

    // A re-scan that finds its directory gone superseded nothing worth resuming.
    let mut state = state_with_loaded_directory();
    let rescan = state.begin_scan();
    assert!(matches!(
        state.abandon_scan(&rescan),
        AfterAbandonedScan::Nothing
    ));
    assert!(!state.is_scanning());
}

/// Two paths are opened in a row and the second one does not exist: the first one
/// is opened after all, and it still knows what it had superseded itself.
#[test]
fn test_abandoning_an_open_registers_the_opening_it_superseded_again() {
    let mut state = state_with_loaded_directory();
    let _scan = state.begin_scan();
    let first = state.begin_open(Path::new("/first"));
    let second = state.begin_open(Path::new("/nonexistent"));

    assert!(
        matches!(state.abandon_scan(&first), AfterAbandonedScan::Nothing),
        "a stale token changes nothing"
    );
    assert!(state.is_opening() && state.is_current_scan(&second));

    let AfterAbandonedScan::Reopen { path, token } = state.abandon_scan(&second) else {
        panic!("the superseded opening must be taken up again");
    };
    assert_eq!(path, Path::new("/first"));
    assert!(state.is_scanning() && state.is_opening());
    assert!(state.is_current_scan(&token) && !token.is_cancelled());
    assert!(!state.is_current_scan(&first), "it is a run of its own");

    // The first path does not exist either: back to the scan of the loaded directory.
    assert!(matches!(
        state.abandon_scan(&token),
        AfterAbandonedScan::RescanLoadedDirectory
    ));
    assert!(!state.is_scanning() && !state.is_opening());
}

#[test]
fn test_cancelling_or_resetting_ends_the_opening() {
    let mut state = state_with_loaded_directory();
    let token = state.begin_open(Path::new("/new"));
    state.cancel_current_scan();
    assert!(!state.is_opening() && !state.is_scanning());
    assert!(!state.enter_opened_directory(&token, Path::new("/new")));
    assert_eq!(state.current_path, "/old");

    let token = state.begin_open(Path::new("/new"));
    state.reset_directory_state();
    assert!(!state.is_opening() && !state.is_scanning());
    assert!(!state.enter_opened_directory(&token, Path::new("/new")));
    assert_eq!(state.current_path, "");

    let _open = state.begin_open(Path::new("/new"));
    let _rescan = state.begin_scan();
    assert!(!state.is_opening(), "a re-scan is not an opening");
}

// --- Scan progress ---

fn progress(files_scanned: usize, path: &str) -> ScanProgress {
    ScanProgress {
        files_scanned,
        large_files_skipped: 1,
        current_scanning_path: path.to_string(),
    }
}

#[test]
fn test_progress_of_the_current_scan_is_recorded() {
    let mut state = AppState::default();
    let token = state.begin_scan();

    assert!(state.record_scan_progress(&token, progress(40, "/project/src")));

    assert_eq!(state.scan_progress.files_scanned, 40);
    assert_eq!(state.scan_progress.large_files_skipped, 1);
    assert_eq!(state.scan_progress.current_scanning_path, "/project/src");
}

#[test]
fn test_progress_of_an_outdated_scan_is_ignored() {
    let mut state = AppState::default();
    let superseded = state.begin_scan();
    let current = state.begin_scan();
    assert!(!state.record_scan_progress(&superseded, progress(40, "/old")));
    assert_eq!(state.scan_progress.files_scanned, 0);

    state.cancel_current_scan();
    state.record_scan_progress(&current, progress(50, "/cancelled"));
    assert_eq!(state.scan_progress.files_scanned, 0);
    assert!(state.scan_progress.current_scanning_path.is_empty());
}

#[test]
fn test_a_new_scan_starts_counting_from_zero_and_a_finished_one_keeps_its_counters() {
    let mut state = AppState::default();
    let token = state.begin_scan();
    state.record_scan_progress(&token, progress(40, "/project/src"));

    assert!(state.finish_scan(&token));
    assert_eq!(state.scan_progress.files_scanned, 40);
    assert!(
        state.scan_progress.current_scanning_path.is_empty(),
        "a finished scan works on no path"
    );

    let _next = state.begin_scan();
    assert_eq!(state.scan_progress.files_scanned, 0);
    let token = state.begin_open(Path::new("/new"));
    state.record_scan_progress(&token, progress(7, "/other"));
    let _next = state.begin_open(Path::new("/new"));
    assert_eq!(state.scan_progress.files_scanned, 0);
}

// --- Scan walk ---

#[test]
fn test_begin_scan_walk_marks_the_list_incomplete_for_the_current_scan_only() {
    let mut state = AppState {
        is_fully_scanned: true,
        ..Default::default()
    };
    let outdated = state.begin_scan();
    assert!(
        state.is_fully_scanned,
        "beginning a scan changes no list flag"
    );
    let current = state.begin_scan();

    assert!(!state.begin_scan_walk(&outdated));
    assert!(state.is_fully_scanned);
    assert!(state.begin_scan_walk(&current));
    assert!(!state.is_fully_scanned);
    assert!(!state.begin_scan_walk(&current), "nothing changes twice");
}
