use super::*;

#[tokio::test]
async fn proactive_scan_task_verifies_two_phase_events() {
    // Arrange
    let mut harness = TestHarness::new();
    let mut scanner = MockScanner::new();

    let shallow_files = vec![FileItem {
        path: harness.root_path.join("src"),
        is_directory: true,
        ..Default::default()
    }];
    let deep_files = vec![
        FileItem {
            path: harness.root_path.join("src"),
            is_directory: true,
            ..Default::default()
        },
        FileItem {
            path: harness.root_path.join("src/main.rs"),
            is_directory: false,
            ..Default::default()
        },
    ];
    scanner.set_results(shallow_files.clone(), deep_files.clone());

    let token = harness.state.lock().begin_scan();

    // Act
    proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        harness.root_path.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    )
    .await;

    // Assert
    // The success path publishes the final state itself; the guard only acts on
    // panics and cancellations, so exactly two events are expected.
    let events = harness.drain_events();
    assert_eq!(events.len(), 2, "Expected two StateUpdate events");

    // Phase 1: Shallow scan update
    let state1 = match &events[0] {
        UserEvent::StateUpdate(s) => s,
        _ => panic!("Expected StateUpdate"),
    };
    assert_eq!(state1.visible_files_count, 1);
    assert!(
        !state1.is_fully_scanned,
        "Should not be fully scanned after phase 1"
    );
    assert!(state1.is_scanning, "Should still be scanning after phase 1");

    // Phase 2: Deep scan update (this is the final event in the success case)
    let state2 = match &events[1] {
        UserEvent::StateUpdate(s) => s,
        _ => panic!("Expected StateUpdate"),
    };
    assert_eq!(state2.visible_files_count, 2);
    assert!(
        state2.is_fully_scanned,
        "Should be fully scanned after phase 2"
    );
    assert!(
        !state2.is_scanning,
        "Scanning should be complete in final state"
    );
    assert!(state2.status_message.contains("Indexing complete"));
}

#[tokio::test]
async fn proactive_scan_cancellation_during_deep_scan_aborts_task() {
    let harness = TestHarness::new();
    let mut scanner = MockScanner::new();
    scanner.set_results(vec![FileItem::default()], vec![]);
    let (has_started_deep_scan, unblock_deep_scan) = scanner.prepare_for_cancellation();
    let token = harness.state.lock().begin_scan();
    let cancel_flag = token.cancel_flag();

    let proxy_clone = harness.proxy.clone();
    let task_state = harness.state.clone();
    let task_path = harness.root_path.clone();

    let task_handle = tokio::spawn(async move {
        proactive_scan_task(
            proxy_clone,
            task_state,
            task_path,
            scanner,
            MockFileSearcher::default(),
            token,
        )
        .await;
    });
    has_started_deep_scan.await.unwrap();
    cancel_flag.store(true, Ordering::SeqCst);
    unblock_deep_scan.send(()).unwrap();
    let _ = task_handle.await;

    // The task now cleans up after itself via the RAII guard.
    // We can now confidently check the final state.
    let final_app_state = harness.state.lock();
    assert!(
        !final_app_state.is_scanning(),
        "is_scanning should be false"
    );
    assert!(
        !final_app_state.is_fully_scanned,
        "is_fully_scanned should be false"
    );
}

#[tokio::test]
async fn proactive_scan_task_handles_shallow_scan_error() {
    // Arrange
    let mut harness = TestHarness::new();
    let scanner = MockScanner::new();
    let scan_error = CoreError::Io("Permission denied".to_string(), harness.root_path.clone());

    // Configure the mock scanner to fail the shallow scan
    *scanner.shallow_result.lock().unwrap() = Err(scan_error.clone());

    let token = harness.state.lock().begin_scan();

    // Act
    proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        harness.root_path.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    )
    .await;

    // Assert
    // The RAII guard will send a final StateUpdate on drop because of the error.
    let events = harness.drain_events();
    assert_eq!(events.len(), 1, "Expected one final StateUpdate event");

    let final_state = match &events[0] {
        UserEvent::StateUpdate(s) => s,
        _ => panic!("Expected a StateUpdate event."),
    };

    assert!(
        !final_state.is_scanning,
        "Scanning should be stopped on error."
    );
    assert!(
        final_state
            .status_message
            .contains("Scan failed: I/O error"),
        "Status message should reflect the scan failure."
    );
}

#[tokio::test]
async fn proactive_scan_task_handles_deep_scan_error() {
    // Arrange
    let mut harness = TestHarness::new();
    let scanner = MockScanner::new();
    let scan_error = CoreError::Io("Disk full".to_string(), harness.root_path.clone());

    // Shallow scan succeeds, deep scan fails
    let shallow_files = vec![FileItem {
        path: harness.root_path.join("file.txt"),
        ..Default::default()
    }];
    *scanner.shallow_result.lock().unwrap() = Ok((shallow_files.clone(), HashSet::new()));
    *scanner.deep_result.lock().unwrap() = Err(scan_error.clone());

    let token = harness.state.lock().begin_scan();

    // Act
    proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        harness.root_path.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    )
    .await;

    // Assert
    // Expect two StateUpdates: one after the successful shallow scan, one for the final error state.
    let events = harness.drain_events();
    assert_eq!(events.len(), 2, "Expected two StateUpdate events");

    // Check state after shallow scan
    let state1 = match &events[0] {
        UserEvent::StateUpdate(s) => s,
        _ => panic!("Expected StateUpdate for shallow scan"),
    };
    assert!(
        state1.is_scanning,
        "Should still be scanning after shallow scan."
    );
    assert_eq!(
        state1.visible_files_count, 1,
        "Shallow scan results should be visible."
    );

    // Check final state after deep scan error
    let state2 = match &events[1] {
        UserEvent::StateUpdate(s) => s,
        _ => panic!("Expected StateUpdate for deep scan failure"),
    };
    assert!(
        !state2.is_scanning,
        "Scanning should be stopped after error."
    );
    assert!(
        state2.status_message.contains("Scan failed: I/O error"),
        "Status message should reflect the deep scan failure."
    );
}

#[tokio::test]
async fn proactive_scan_task_cancels_between_phases() {
    // Arrange
    let mut harness = TestHarness::new();
    let mut scanner = MockScanner::new();

    // Use the cancellation helper to pause the task between scan phases
    let (shallow_scan_finished, allow_deep_scan_to_proceed) = scanner.prepare_for_cancellation();

    // Shallow scan succeeds.
    let shallow_files = vec![FileItem {
        path: harness.root_path.join("file.txt"),
        ..Default::default()
    }];
    *scanner.shallow_result.lock().unwrap() = Ok((shallow_files.clone(), HashSet::new()));
    *scanner.deep_result.lock().unwrap() = Ok((vec![], HashSet::new()));

    let token = harness.state.lock().begin_scan();
    let cancel_flag = token.cancel_flag();
    let proxy_clone = harness.proxy.clone();
    let state_clone = harness.state.clone();
    let path_clone = harness.root_path.clone();

    // Act
    let task_handle = tokio::spawn(async move {
        proactive_scan_task(
            proxy_clone,
            state_clone,
            path_clone,
            scanner,
            MockFileSearcher::default(),
            token,
        )
        .await
    });

    // 1. Wait for the signal from the mock that the shallow scan is complete.
    shallow_scan_finished
        .await
        .expect("Mock scanner should signal that shallow scan finished");

    // 2. Now that the task is paused, set the cancellation flag.
    cancel_flag.store(true, Ordering::SeqCst);

    // 3. Unblock the task, which will now immediately see the cancel flag and exit.
    allow_deep_scan_to_proceed.send(()).unwrap();

    // 4. Wait for the task to fully terminate.
    task_handle.await.unwrap();

    // Assert
    // The shallow update came first; the last one is the final state update the
    // ScanGuard fired on drop.
    let events = harness.drain_events();
    assert_eq!(events.len(), 2, "Shallow update and final update expected");
    let final_state_update = match events.into_iter().next_back() {
        Some(UserEvent::StateUpdate(state)) => *state.into_ui_state(),
        other => panic!("ScanGuard should send a final update, got {other:?}"),
    };

    assert!(
        !final_state_update.is_scanning,
        "Scanning should be false in the final state."
    );
    assert!(
        !final_state_update.is_fully_scanned,
        "Scan should not be marked as full."
    );

    // The final AppState should be consistent
    let final_app_state = harness.state.lock();
    assert!(!final_app_state.is_scanning());
    assert!(!final_app_state.has_scan_task());
}

// --- Scan progress ---

/// A scanner whose deep phase reports progress and then waits to be released.
struct ReportingScanner {
    reported: ScanProgress,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

impl Scanner for ReportingScanner {
    async fn scan(
        &self,
        request: &ScanRequest,
        report: Box<dyn Fn(ScanProgress) + Send + Sync>,
    ) -> ScanOutcome {
        if request.max_depth().is_none() {
            report(self.reported.clone());
            let release = self.release.lock().unwrap().take();
            if let Some(release) = release {
                release.await.ok();
            }
        }
        Ok((
            vec![file_item(&request.project_root().join("a.rs"), 1)],
            HashSet::new(),
        ))
    }
}

/// The status line used to read "0 files processed" for the whole scan, because
/// the progress only went to the UI as events and never into the state.
#[tokio::test]
async fn progress_of_the_running_scan_is_part_of_the_state_updates() {
    let mut harness = TestHarness::new();
    let (release, released) = oneshot::channel();
    let current_file = harness.root_path.join("src").join("main.rs");
    let scanner = ReportingScanner {
        reported: ScanProgress {
            files_scanned: 40,
            large_files_skipped: 2,
            current_scanning_path: current_file.to_string_lossy().to_string(),
        },
        release: Mutex::new(Some(released)),
    };
    let token = harness.state.lock().begin_scan();
    let scan = tokio::spawn(proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        harness.root_path.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    ));

    // The report reaches the UI as an event of its own (the live display) ...
    loop {
        if let UserEvent::ScanProgress(progress) = harness.next_event().await {
            assert_eq!(progress.files_scanned, 40);
            break;
        }
    }
    // ... and is what any state update sent from now on says.
    let during_scan = generate_ui_state(&harness.state.lock());
    assert!(during_scan.is_scanning);
    assert_eq!(
        during_scan.status_message,
        format!(
            "Scanning... 40 files processed. 2 large files skipped ({})",
            current_file.display()
        )
    );
    assert_eq!(during_scan.scan_progress.files_scanned, 40);
    assert_eq!(during_scan.scan_progress.large_files_skipped, 2);

    release.send(()).unwrap();
    scan.await.unwrap();
    let completed = harness.last_state_update().unwrap();
    assert!(!completed.is_scanning);
    assert_eq!(
        completed.status_message,
        "Indexing complete. Found 1 visible items."
    );
    assert_eq!(completed.scan_progress.files_scanned, 40);
}

#[tokio::test]
async fn progress_of_a_superseded_scan_is_neither_stored_nor_sent() {
    let mut harness = TestHarness::new();
    let superseded = harness.state.lock().begin_scan();
    let report = scan_progress_reporter(&harness.proxy, &harness.state, &superseded);
    let _current = harness.state.lock().begin_scan();

    report(ScanProgress {
        files_scanned: 40,
        large_files_skipped: 2,
        current_scanning_path: "/somewhere/else".to_string(),
    });

    harness.assert_no_events("An outdated scan must stay silent");
    let state = harness.state.lock();
    assert_eq!(state.scan_progress.files_scanned, 0);
    assert!(state.scan_progress.current_scanning_path.is_empty());
}

/// The request to open a path is a state change of its own. It is complete when
/// `start_scan_on_path` returns: the scan is registered, cancellable and announced,
/// without any task having had the chance to run. The loaded directory is not
/// touched yet: whether the path exists is not known.
#[tokio::test]
async fn start_scan_on_path_registers_the_scan_before_it_returns() {
    let mut harness = TestHarness::new();
    let project = harness.root_path.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let previous = harness.root_path.to_string_lossy().to_string();
    harness.state.lock().selected_files = HashSet::from([harness.root_path.join("a.rs")]).into();

    start_scan_on_path(
        project.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        false,
    );

    // No `.await` since the call: this test runs on a single-threaded runtime, so
    // the spawned task has not run yet.
    let announced = harness
        .last_state_update()
        .expect("The scan must be announced synchronously");
    assert!(announced.is_scanning);
    assert_eq!(announced.current_path, previous);
    assert_eq!(announced.selected_files_count, 1);
    {
        let mut state = harness.state.lock();
        assert!(state.is_scanning() && state.has_scan_task() && state.is_opening());

        // A cancellation that follows immediately therefore hits this scan, and
        // the directory that was loaded is still there.
        state.cancel_current_scan();
        assert!(!state.is_scanning());
        assert_eq!(state.status, StatusNotice::ScanCancelled);
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let state = harness.state.lock();
    assert_eq!(state.current_path, previous);
    assert_eq!(state.selected_files.len(), 1);
    assert_eq!(state.config.last_directory, None);
}

/// Once the path has been found, the directory replaces the loaded one: everything
/// that belonged to the previous directory is reset at that moment.
#[tokio::test]
async fn opening_a_path_replaces_the_loaded_directory_once_it_is_found() {
    let mut harness = TestHarness::new();
    let project = harness.root_path.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("new.rs"), "").unwrap();
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(Path::new("/old/a.rs"), 1)]);
        state.selected_files = HashSet::from([PathBuf::from("/old/a.rs")]).into();
        state.expanded_dirs = HashSet::from([PathBuf::from("/old/src")]).into();
        state.search_query = "a".to_string();
        state.is_fully_scanned = true;
        filtering::apply_filters(&mut state);
    }

    start_scan_on_path(
        project.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        false,
    );

    let project_path = project.to_string_lossy().to_string();
    let entered = harness
        .wait_for_state_update(|s| s.current_path == project_path)
        .await;
    assert!(entered.is_scanning);
    assert_eq!(entered.selected_files_count, 0);
    assert_eq!(entered.search_query, "");
    assert!(entered.rows.is_empty() && !entered.is_fully_scanned);

    let completed = harness.wait_for_state_update(|s| !s.is_scanning).await;
    assert!(completed.is_fully_scanned);
    let state = harness.state.lock();
    assert!(state.selected_files.is_empty() && state.expanded_dirs.is_empty());
    assert_eq!(
        state.config.last_directory.as_deref(),
        Some(project.as_path())
    );
    let listed: Vec<_> = state
        .full_file_list()
        .iter()
        .map(|i| i.path.clone())
        .collect();
    assert_eq!(listed, [project.join("new.rs")]);
}

/// A scan that was cancelled before its task looked at the path stays cancelled:
/// the task must not touch the state any more.
#[tokio::test]
async fn locating_the_scan_root_is_discarded_for_a_cancelled_scan() {
    let mut harness = TestHarness::new();
    let file_path = harness.root_path.join("some_file.txt");
    std::fs::write(&file_path, "content").unwrap();
    let token = harness.state.lock().begin_scan();
    harness.state.lock().cancel_current_scan();
    harness.state.lock().current_path = "untouched".to_string();

    let located = locate_scan_root(file_path, &harness.proxy, &harness.state, &token).await;

    assert!(located.is_none());
    harness.assert_no_events("A superseded scan must stay silent");
    let state = harness.state.lock();
    assert_eq!(state.current_path, "untouched");
    assert_eq!(state.config.last_directory, None);
}

/// A path that does not exist is reported, and the directory that was loaded stays
/// exactly as it was: list, selection, expansion, filters.
#[tokio::test]
async fn opening_a_nonexistent_path_keeps_the_loaded_directory() {
    // Arrange
    let mut harness = TestHarness::new();
    let nonexistent_path = harness.root_path.join("nonexistent");
    let loaded = harness.root_path.to_string_lossy().to_string();
    let file = harness.root_path.join("src").join("a.rs");
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![
            dir_item(&harness.root_path.join("src"), 1),
            file_item(&file, 2),
        ]);
        state.selected_files = HashSet::from([file.clone()]).into();
        state.expanded_dirs = HashSet::from([harness.root_path.join("src")]).into();
        state.loaded_dirs = HashSet::from([harness.root_path.join("src")]);
        state.search_query = "a".to_string();
        state.is_fully_scanned = true;
        state.status = StatusNotice::IndexingComplete { visible_items: 2 };
        filtering::apply_filters(&mut state);
    }

    // Act
    start_scan_on_path(
        nonexistent_path,
        harness.proxy.clone(),
        harness.state.clone(),
        false,
    );

    // Assert: the scan is announced with the loaded directory still in place, then
    // its task finds out that there is nothing to scan, reports the error and ends
    // the scan.
    match harness.next_event().await {
        UserEvent::StateUpdate(s) => {
            assert!(s.is_scanning);
            assert_eq!(s.current_path, loaded);
            assert_eq!(s.root_rows().len(), 1, "the previous list stays visible");
        }
        other => panic!("Expected the announcement of the scan, got {other:?}"),
    }
    match harness.next_event().await {
        UserEvent::ShowError(msg) => assert!(msg.contains("Path does not exist")),
        other => panic!("Expected a ShowError event, got {other:?}"),
    }
    match harness.next_event().await {
        UserEvent::StateUpdate(s) => {
            assert!(!s.is_scanning);
            assert_eq!(s.current_path, loaded);
            assert_eq!(s.selected_files_count, 1);
            assert_eq!(s.search_query, "a");
            assert!(s.is_fully_scanned);
            assert_eq!(s.root_rows().len(), 1);
            let directory = s.root_rows()[0];
            assert!(directory.is_expanded && s.children_of(directory).len() == 1);
            assert_eq!(
                s.status_message,
                "Indexing complete. Found 2 visible items."
            );
        }
        other => panic!("Expected the end of the scan, got {other:?}"),
    }
    harness.assert_no_events("Nothing follows a failed opening");

    let state = harness.state.lock();
    assert!(!state.is_scanning(), "The scan must have ended.");
    assert!(!state.is_opening());
    assert_eq!(state.config.last_directory, None);
    assert_eq!(state.selected_files, HashSet::from([file]));
}

/// Opening a path supersedes the scan that is running. If the path then turns out
/// not to exist, the loaded directory would be left with the incomplete list of
/// the interrupted scan, so that scan is started again.
#[tokio::test]
async fn a_failed_opening_restarts_the_scan_it_interrupted() {
    let mut harness = TestHarness::new();
    std::fs::create_dir_all(harness.root_path.join("src")).unwrap();
    std::fs::write(harness.root_path.join("src").join("a.rs"), "").unwrap();
    let loaded = harness.root_path.clone();
    harness.state.lock().selected_files = HashSet::from([loaded.join("src").join("a.rs")]).into();

    // A scan of the loaded directory is running (its task has not had the chance
    // to run yet: there is no `.await` before the next call) ...
    start_scan_on_path(
        loaded.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        true,
    );
    // ... when a path that does not exist is dropped.
    start_scan_on_path(
        loaded.join("nonexistent"),
        harness.proxy.clone(),
        harness.state.clone(),
        false,
    );

    let mut error_shown = false;
    let completed = loop {
        match harness.next_event().await {
            UserEvent::ShowError(msg) => {
                assert!(msg.contains("Path does not exist"));
                error_shown = true;
            }
            UserEvent::StateUpdate(s) if error_shown && !s.is_scanning => break s,
            UserEvent::StateUpdate(s) => {
                assert!(s.is_scanning, "the loaded directory is never shown as done");
            }
            _ => {}
        }
    };
    assert!(completed.is_fully_scanned);
    assert_eq!(completed.current_path, loaded.to_string_lossy());
    assert_eq!(completed.selected_files_count, 1, "the selection is kept");

    let state = harness.state.lock();
    assert_eq!(state.full_file_list().len(), 2);
    assert_eq!(
        state.config.last_directory.as_deref(),
        Some(loaded.as_path())
    );
}

#[tokio::test]
async fn start_scan_on_path_handles_path_is_file() {
    // Arrange
    let mut harness = TestHarness::new();
    let file_path = harness.root_path.join("some_file.txt");
    std::fs::write(&file_path, "content").unwrap();

    // Act
    // We expect this to run the scan on the parent directory.
    start_scan_on_path(
        file_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        false,
    );

    // Assert
    // The scan is announced right away; the path has not been looked at yet.
    match harness.next_event().await {
        UserEvent::StateUpdate(s) => assert!(s.is_scanning),
        other => panic!("Expected a StateUpdate event, got {other:?}"),
    }
    // Its task then finds the directory to scan: the parent of the file.
    let root = harness.root_path.to_string_lossy().to_string();
    let located = harness
        .wait_for_state_update(|s| s.current_path == root)
        .await;
    assert!(located.is_scanning);
    let completed = harness.wait_for_state_update(|s| !s.is_scanning).await;
    assert!(completed.is_fully_scanned);

    let state = harness.state.lock();
    assert_eq!(state.current_path, root);
    assert_eq!(
        state.config.last_directory.as_deref(),
        Some(harness.root_path.as_path())
    );
    assert!(state
        .full_file_list()
        .iter()
        .any(|item| item.path == file_path));
}

#[tokio::test]
async fn start_scan_on_path_preserves_state() {
    // Arrange
    let mut harness = TestHarness::new();
    let dir_to_expand = harness.root_path.join("src");
    let new_scan_path = harness.root_path.join("new_project");
    std::fs::create_dir_all(&new_scan_path).unwrap();

    // Set initial state
    {
        let mut state = harness.state.lock();
        state.expanded_dirs.insert(dir_to_expand.clone());
    }

    // Act
    // Call with preserve_state = true
    start_scan_on_path(
        new_scan_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        true,
    );

    // Assert
    // Wait for the initial StateUpdate event which confirms the task has started and set the initial state.
    // This resolves the race condition.
    let initial_update = harness.next_event().await;
    assert!(matches!(initial_update, UserEvent::StateUpdate(_)));

    // Now it's safe to check the state.
    {
        let state = harness.state.lock();
        assert!(state.is_scanning(), "Scanning should have started.");
        assert_eq!(state.current_path, new_scan_path.to_string_lossy());
        assert!(
            state.expanded_dirs.contains(&dir_to_expand),
            "expanded_dirs should have been preserved."
        );
    }

    // The scan runs to its end and reports the complete (empty) directory.
    let final_update = harness
        .wait_for_state_update(|ui_state| !ui_state.is_scanning)
        .await;
    assert!(final_update.is_fully_scanned);
}

/// Tests the edge case where handle_scan_error is called when no scan is active.
#[tokio::test]
async fn handle_scan_error_does_nothing_if_not_scanning() {
    // Arrange
    let mut harness = TestHarness::new();
    // The scan has already finished, so is_scanning is false.
    let token = harness.state.lock().begin_scan();
    harness.state.lock().finish_scan(&token);
    let dummy_error = CoreError::Cancelled;

    // Act
    handle_scan_error(dummy_error, &harness.state, &harness.proxy, &token);

    // Assert
    // The function should return early without sending any events.
    harness.assert_no_events("No events should be sent if not scanning");
}

/// Starts a scan task that parks inside its deep scan phase.
///
/// Returns the task handle, the scan's token and the sender that lets the deep scan
/// return. The shallow phase has already been applied when this function returns.
async fn spawn_scan_parked_in_deep_phase(
    harness: &TestHarness,
    deep_files: Vec<FileItem>,
) -> (tokio::task::JoinHandle<()>, ScanToken, oneshot::Sender<()>) {
    let mut scanner = MockScanner::new();
    scanner.set_results(vec![], deep_files);
    let (shallow_scan_started, unblock_deep_scan) = scanner.prepare_for_cancellation();

    let token = harness.state.lock().begin_scan();
    let handle = tokio::spawn(proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        harness.root_path.clone(),
        scanner,
        MockFileSearcher::default(),
        token.clone(),
    ));
    shallow_scan_started.await.unwrap();
    // On the single-threaded test runtime the task only yields once it is blocked,
    // i.e. it is now waiting inside the deep scan.
    (handle, token, unblock_deep_scan)
}

/// The original race: the old task is aborted and its future (including the
/// `ScanGuard`) is dropped only *after* the new scan has registered itself.
#[tokio::test]
async fn aborted_scan_task_does_not_reset_state_of_newer_scan() {
    // Arrange
    let mut harness = TestHarness::new();
    let (old_task, _old_token, _unblock) = spawn_scan_parked_in_deep_phase(&harness, vec![]).await;
    let _ = harness.last_state_update(); // Drain the shallow update.

    // Act: the new scan starts first, then the old future gets dropped.
    let new_token = begin_replacement_scan(&harness);
    old_task.abort();
    let join_error = old_task.await.expect_err("Old task should be aborted");
    assert!(join_error.is_cancelled());

    // Assert
    assert!(
        harness.last_state_update().is_none(),
        "The stale guard must not announce the end of a scan it does not own"
    );
    assert_replacement_scan_is_intact(&harness, &new_token);
}

/// The old task is not aborted in time and resumes after the new scan has started.
/// It must notice its own cancellation instead of consulting the new scan's flag,
/// and must not publish its results.
#[tokio::test]
async fn superseded_scan_task_does_not_publish_results_into_newer_scan() {
    // Arrange
    let mut harness = TestHarness::new();
    let stale_file = FileItem {
        path: harness.root_path.join("stale.txt"),
        ..Default::default()
    };
    let (old_task, old_token, unblock_deep_scan) =
        spawn_scan_parked_in_deep_phase(&harness, vec![stale_file]).await;
    let _ = harness.last_state_update(); // Drain the shallow update.

    // Act: the new scan starts, then the old deep scan returns successfully.
    let new_token = begin_replacement_scan(&harness);
    unblock_deep_scan.send(()).unwrap();
    old_task.await.expect("Old task should end on its own");

    // Assert
    assert!(old_token.is_cancelled());
    assert!(
        harness.last_state_update().is_none(),
        "A superseded scan must not send state updates"
    );
    assert!(
        harness.state.lock().full_file_list().is_empty(),
        "Results of the superseded scan must be discarded"
    );
    assert_replacement_scan_is_intact(&harness, &new_token);
}

/// Even if a stale task gets as far as publishing (its cancellation flag was not
/// observed in time), the state must reject the write.
#[tokio::test]
async fn stale_scan_results_are_rejected_by_state() {
    // Arrange
    let mut harness = TestHarness::new();
    let old_token = harness.state.lock().begin_scan();
    let new_token = begin_replacement_scan(&harness);
    let stale_files = vec![FileItem {
        path: harness.root_path.join("stale.txt"),
        ..Default::default()
    }];

    // Act
    let applied = apply_shallow_scan_result(
        &harness.state,
        &harness.proxy,
        &old_token,
        &harness.root_path,
        stale_files.clone(),
        HashSet::new(),
    );
    let applied_deep =
        apply_deep_scan_result(&harness.state, &old_token, stale_files, HashSet::new());
    complete_scan(&harness.state, &harness.proxy, &old_token);

    // Assert
    assert!(!applied && !applied_deep);
    assert!(harness.last_state_update().is_none());
    {
        let state = harness.state.lock();
        assert!(state.full_file_list().is_empty());
        assert!(state.loaded_dirs.is_empty());
    }
    assert_replacement_scan_is_intact(&harness, &new_token);
}

#[tokio::test]
async fn handle_scan_error_from_stale_scan_does_not_fail_newer_scan() {
    // Arrange
    let mut harness = TestHarness::new();
    let old_token = harness.state.lock().begin_scan();
    let new_token = begin_replacement_scan(&harness);

    // Act
    handle_scan_error(
        CoreError::Cancelled,
        &harness.state,
        &harness.proxy,
        &old_token,
    );

    // Assert
    assert!(harness.last_state_update().is_none());
    assert!(!matches!(
        harness.state.lock().status,
        crate::app::state::StatusNotice::ScanFailed { .. }
    ));
    assert_replacement_scan_is_intact(&harness, &new_token);
}

/// Rescan of a fully scanned project, cancelled in the deep phase: the list holds a
/// single level again, so the subdirectories must be offered for lazy loading instead
/// of counting as loaded (and empty).
#[tokio::test]
async fn rescan_cancelled_in_deep_phase_leaves_subdirectories_unloaded() {
    // Arrange: the state a completed scan leaves behind.
    let mut harness = TestHarness::new();
    let root = harness.root_path.clone();
    let src = root.join("src");
    let core = src.join("core");
    {
        let mut state = harness.state.lock();
        state.replace_with_complete_file_list(vec![
            dir_item(&src, 1),
            dir_item(&core, 2),
            file_item(&core.join("lib.rs"), 3),
        ]);
        state.loaded_dirs.insert(root.clone());
        state.is_fully_scanned = true;
    }
    let mut scanner = MockScanner::new();
    scanner.set_results(vec![dir_item(&src, 1)], vec![]);
    let (_shallow_started, unblock_deep_scan) = scanner.prepare_for_cancellation();
    let token = {
        let mut state = harness.state.lock();
        state.is_fully_scanned = false;
        state.begin_scan()
    };

    // Act: rescan, and cancel once the shallow result has been published.
    let task = tokio::spawn(proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        root.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    ));
    let shallow_event = harness.next_event().await;
    assert!(matches!(shallow_event, UserEvent::StateUpdate(_)));
    harness.state.lock().cancel_current_scan();
    unblock_deep_scan.send(()).ok();
    let _ = task.await;

    // Assert
    let state = harness.state.lock();
    assert_eq!(state.loaded_dirs, HashSet::from([root]));
    let ui_state = generate_ui_state(&state);
    let src_node = ui_state
        .root_rows()
        .into_iter()
        .find(|n| n.name == "src")
        .unwrap();
    assert!(
        !src_node.children_loaded,
        "`src` must be lazily loadable after the cancelled rescan"
    );
}

/// A complete list with one file below `src`, as a finished scan leaves it.
fn load_complete_list(harness: &TestHarness) -> PathBuf {
    let file = harness.root_path.join("src").join("a.rs");
    let mut state = harness.state.lock();
    state.replace_with_complete_file_list(vec![
        dir_item(&harness.root_path.join("src"), 1),
        dir_item(&harness.root_path.join("empty"), 1),
        file_item(&file, 2),
    ]);
    state.is_fully_scanned = true;
    state.config.remove_empty_directories = true;
    filtering::apply_filters(&mut state);
    file
}

/// Re-scanning a directory that has vanished ends with an error and leaves the
/// list as it was, which includes that it is still the complete list: "remove
/// empty directories" keeps working on it.
#[tokio::test]
async fn a_failed_rescan_leaves_the_list_fully_scanned() {
    let mut harness = TestHarness::new();
    load_complete_list(&harness);
    let vanished = harness.root_path.join("vanished");
    harness.state.lock().current_path = vanished.to_string_lossy().to_string();

    start_scan_on_path(vanished, harness.proxy.clone(), harness.state.clone(), true);

    let ended = harness.wait_for_state_update(|s| !s.is_scanning).await;
    assert!(ended.is_fully_scanned);
    let mut state = harness.state.lock();
    assert!(state.is_fully_scanned);
    assert_eq!(state.full_file_list().len(), 3);
    // The empty directory is still removed from what is shown.
    filtering::apply_filters(&mut state);
    assert_eq!(state.filtered_file_list().len(), 2);
}

/// A re-scan that finds its directory reports the list as incomplete from that
/// moment on (the announcement itself changes nothing but the scanning state).
#[tokio::test]
async fn a_rescan_marks_the_list_incomplete_once_the_directory_is_found() {
    let mut harness = TestHarness::new();
    load_complete_list(&harness);

    start_scan_on_path(
        harness.root_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        true,
    );

    match harness.next_event().await {
        UserEvent::StateUpdate(announced) => {
            assert!(announced.is_scanning && announced.is_fully_scanned);
        }
        other => panic!("Expected the announcement of the scan, got {other:?}"),
    }
    let walking = harness.wait_for_state_update(|s| !s.is_fully_scanned).await;
    assert!(walking.is_scanning);
    let completed = harness.wait_for_state_update(|s| !s.is_scanning).await;
    assert!(completed.is_fully_scanned);
}

/// A proxy whose `n`-th event makes the sender panic (once), to stage a panic in
/// the middle of a task. The other events are passed on.
#[derive(Clone)]
struct PanicOnEvent {
    inner: TestEventProxy,
    countdown: Arc<Mutex<Option<usize>>>,
}

impl PanicOnEvent {
    fn nth(inner: TestEventProxy, n: usize) -> Self {
        Self {
            inner,
            countdown: Arc::new(Mutex::new(Some(n))),
        }
    }
}

impl EventProxy for PanicOnEvent {
    fn send_event(&self, event: UserEvent) {
        let fire = {
            let mut countdown = self.countdown.lock().unwrap();
            match countdown.as_mut() {
                Some(0) => countdown.take().is_some(),
                Some(remaining) => {
                    *remaining -= 1;
                    false
                }
                None => false,
            }
        };
        if fire {
            panic!("failure injected by the test while the scan root is located");
        }
        self.inner.send_event(event);
    }
}

/// The guard of a scan covers its first step too: a task that panics while it
/// looks at its path (here: under the state lock, when it reports the directory
/// it has entered) releases the scanning state and tells the UI.
#[tokio::test]
async fn a_panic_while_the_scan_root_is_located_releases_the_scan() {
    let mut harness = TestHarness::new();
    let target = tempdir().unwrap();
    // Event 0 is the announcement, event 1 the report of the entered directory.
    let proxy = PanicOnEvent::nth(harness.proxy.clone(), 1);

    start_scan_on_path(
        target.path().to_path_buf(),
        proxy,
        harness.state.clone(),
        false,
    );

    match harness.next_event().await {
        UserEvent::StateUpdate(announced) => assert!(announced.is_scanning),
        other => panic!("Expected the announcement of the scan, got {other:?}"),
    }
    let released = harness.wait_for_state_update(|s| !s.is_scanning).await;
    assert!(!released.is_scanning);
    {
        let state = harness.state.lock();
        assert!(!state.is_scanning(), "the scan slot must be free again");
        assert!(!state.is_opening());
    }

    // The next scan is not held up by the one that panicked.
    start_scan_on_path(
        target.path().to_path_buf(),
        harness.proxy.clone(),
        harness.state.clone(),
        false,
    );
    let completed = harness
        .wait_for_state_update(|s| !s.is_scanning && s.is_fully_scanned)
        .await;
    assert_eq!(
        completed.current_path,
        target.path().to_string_lossy().to_string()
    );
}
