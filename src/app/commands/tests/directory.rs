use super::*;

#[tokio::test]
async fn test_select_directory_starts_scan_on_ok() {
    let mut harness = TestHarness::new();
    let new_dir = harness.create_dir("new_project");
    harness.dialog.set_pick_folder(Some(new_dir.clone()));

    select_directory(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    let final_state = harness.wait_for_scan_completion().await.unwrap();
    assert!(!final_state.is_scanning);
    assert_eq!(final_state.current_path, new_dir.to_string_lossy());
}

#[tokio::test]
async fn test_select_directory_updates_state_on_cancel() {
    let mut harness = TestHarness::new();
    harness.dialog.set_pick_folder(None);

    select_directory(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    let final_state = harness.last_state_update().unwrap();
    assert!(!final_state.is_scanning);
}

/// Dismissing the dialog is not a request to stop anything: a scan that is running in
/// the background must keep running and keep owning the scanning state.
#[tokio::test]
async fn test_select_directory_cancel_leaves_running_scan_untouched() {
    let mut harness = TestHarness::new();
    harness.dialog.set_pick_folder(None);
    let token = {
        let mut state = harness.state.lock();
        let token = state.begin_scan();
        state.attach_scan_task(&token, tokio::spawn(std::future::pending()));
        token
    };

    select_directory(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    let ui_state = harness.last_state_update().unwrap();
    assert!(ui_state.is_scanning, "UI must still show the running scan");
    let mut state = harness.state.lock();
    assert!(state.is_scanning());
    assert!(state.is_current_scan(&token));
    assert!(state.has_scan_task(), "Scan must stay cancellable");
    assert!(!token.is_cancelled());
    state.cancel_current_scan();
}

#[tokio::test]
async fn test_rescan_directory_on_empty_path_does_nothing() {
    let mut harness = TestHarness::new();
    {
        let mut state = harness.state.lock();
        state.current_path = String::new();
    }

    rescan_directory(harness.proxy.clone(), harness.state.clone());

    harness
        .expect_no_event_within(
            QUIET_PERIOD,
            "Rescan should not trigger any event when path is empty",
        )
        .await;
}

#[tokio::test]
async fn test_clear_directory_resets_state() {
    let mut harness = TestHarness::new();
    let file_path = harness.create_file("file.txt", "content");
    harness.set_initial_files(&["file.txt"]);
    {
        let mut state = harness.state.lock();
        state.selected_files.insert(file_path);
        state.config_mut().last_directory = Some(harness.root_path.clone());
    }

    clear_directory(harness.proxy.clone(), harness.state.clone());

    let ui_state = harness.last_state_update().unwrap();
    assert!(ui_state.current_path.is_empty());
    assert_eq!(ui_state.visible_files_count, 0);
    let state = harness.state.lock();
    assert!(state.current_path.is_empty());
    assert!(state.full_file_list().is_empty());
    assert!(state.selected_files.is_empty());
    assert!(state.config().last_directory.is_none());
}

#[tokio::test]
async fn test_cancel_scan_updates_state() {
    let mut harness = TestHarness::new();
    {
        let mut state = harness.state.lock();
        let token = state.begin_scan();
        let handle = tokio::spawn(async {});
        state.attach_scan_task(&token, handle);
    }

    cancel_scan(harness.proxy.clone(), harness.state.clone());

    let ui_state = harness.last_state_update().unwrap();
    assert!(!ui_state.is_scanning);
    assert_eq!(ui_state.status_message, "Scan cancelled.");
}

#[tokio::test]
async fn test_initialize_sends_initial_state() {
    let mut harness = TestHarness::new();
    initialize(harness.proxy.clone(), harness.state.clone());
    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(ui_state.current_path, harness.root_path.to_string_lossy());
}

#[tokio::test]
async fn test_clear_directory_persists_the_forgotten_last_directory() {
    let (harness, store) = TestHarness::with_config_file();
    {
        let mut state = harness.state.lock();
        state.config_mut().last_directory = Some(harness.root_path.clone());
        state.config_snapshot().save().unwrap();
    }

    clear_directory(harness.proxy.clone(), harness.state.clone());

    let saved = config_on_disk(&store, |saved| saved.last_directory.is_none()).await;
    assert_eq!(saved, *harness.state.lock().config());
}
