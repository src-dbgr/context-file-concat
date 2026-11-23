use super::*;

#[tokio::test]
async fn lazy_load_task_handles_scan_error() {
    // Arrange
    let mut harness = TestHarness::new();
    let scanner = MockScanner::new();
    let path_to_load = harness.root_path.join("lazy");

    // Configure scanner to fail for the shallow scan, which is what lazy_load uses
    let scan_error = CoreError::Io("Not a directory".to_string(), path_to_load.clone());
    *scanner.shallow_result.lock().unwrap() = Err(scan_error.clone());

    let token = harness.state.lock().begin_lazy_load();

    // Act
    lazy_load_task(
        ScanRequest::directory_level(&harness.root_path, &path_to_load).unwrap(),
        harness.proxy.clone(),
        harness.state.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    )
    .await;

    // Assert
    // Expect a ShowError event.
    let events = harness.drain_events();
    assert_eq!(events.len(), 1, "Expected one ShowError event");

    match &events[0] {
        UserEvent::ShowError(msg) => {
            assert!(
                msg.contains("Failed to load directory"),
                "Error message is incorrect"
            );
            assert!(
                msg.contains("lazy"),
                "Error message should contain the path"
            );
            assert!(
                msg.contains("Not a directory"),
                "Error message should contain the error detail"
            );
        }
        _ => panic!("Expected a ShowError event."),
    }

    // Assert that state was not left inconsistent
    let state = harness.state.lock();
    assert!(
        !state.loaded_dirs.contains(&path_to_load),
        "Failed path should not be marked as loaded."
    );
}

/// Test for the lazy load happy path, using the proper entry point.
#[tokio::test]
async fn start_lazy_load_scan_happy_path_adds_files() {
    // Arrange
    let mut harness = TestHarness::new();
    let dir_to_load = harness.root_path.join("src");
    std::fs::create_dir_all(&dir_to_load).unwrap();
    let new_file_path = dir_to_load.join("main.rs");
    std::fs::write(&new_file_path, "fn main() {}").unwrap();

    {
        let mut state = harness.state.lock();
        state.edit_full_file_list(|list| {
            list.push(FileItem {
                path: dir_to_load.clone(),
                is_directory: true,
                ..Default::default()
            })
        });
        let all_items = state.full_file_list().to_vec();
        state.set_filtered_file_list(all_items);
    }

    // Act: the returned handle completes when the load has been dealt with.
    start_lazy_load_scan(
        dir_to_load.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .expect("The lazy load must be accepted")
    .await
    .expect("The lazy load task must not panic");

    // --- Assert ---
    // The task is done, so its state update has been sent.
    let final_state_update = harness
        .last_state_update()
        .expect("Should receive a state update after lazy load");

    // 3. Perform the final state checks.
    assert_eq!(
        final_state_update.visible_files_count, 2,
        "Should show parent dir and new file"
    );
    let state = harness.state.lock();
    assert!(state.loaded_dirs.contains(&dir_to_load));
    assert!(state.expanded_dirs.contains(&dir_to_load));
    assert!(state
        .full_file_list()
        .iter()
        .any(|item| item.path == new_file_path));
}

/// A directory can be loaded while a scan is walking the project: the level is
/// listed at once, and the scan goes on.
#[tokio::test]
async fn start_lazy_load_scan_during_a_scan_loads_the_level() {
    // Arrange: a scan that has published the first level.
    let mut harness = TestHarness::new();
    let dir_to_load = harness.root_path.join("src");
    std::fs::create_dir(&dir_to_load).unwrap();
    std::fs::write(dir_to_load.join("main.rs"), "fn main() {}").unwrap();
    let token = {
        let mut state = harness.state.lock();
        let token = state.begin_scan();
        let root = harness.root_path.clone();
        state.replace_with_shallow_file_list(&root, vec![dir_item(&dir_to_load, 1)]);
        token
    };

    // Act
    start_lazy_load_scan(
        dir_to_load.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .expect("The load is started")
    .await
    .unwrap();

    // Assert
    let ui_state = harness.last_state_update().unwrap();
    assert!(ui_state.is_scanning);
    assert_eq!(ui_state.visible_files_count, 2);
    let state = harness.state.lock();
    assert!(state.is_current_scan(&token), "The scan is not disturbed");
    assert!(state.expanded_dirs.contains(&dir_to_load));
    assert!(state.loaded_dirs.contains(&dir_to_load));
    assert!(state
        .full_file_list()
        .iter()
        .any(|item| item.path == dir_to_load.join("main.rs")));
}

/// The scanner of a lazy load must stop working once the file list it loads into is
/// replaced by a new scan or dropped by a directory reset.
#[tokio::test]
async fn lazy_load_scanner_is_cancelled_when_file_list_is_replaced() {
    let harness = TestHarness::new();
    let mut state = harness.state.lock();

    let token = state.begin_lazy_load();
    let scanner = lazy_load_scanner(state.config(), &token);
    assert!(!scanner.cancel_flag.load(Ordering::SeqCst));
    let scan = state.begin_scan();
    assert!(
        scanner.cancel_flag.load(Ordering::SeqCst),
        "A new scan must stop the lazy load"
    );
    state.finish_scan(&scan);

    let token = state.begin_lazy_load();
    let scanner = lazy_load_scanner(state.config(), &token);
    assert!(!scanner.cancel_flag.load(Ordering::SeqCst));
    state.reset_directory_state();
    assert!(
        scanner.cancel_flag.load(Ordering::SeqCst),
        "A directory reset must stop the lazy load"
    );
}

/// A lazy load that finishes after the directory was replaced must not merge its
/// items into the new file list.
#[tokio::test]
async fn lazy_load_result_is_discarded_after_file_list_was_replaced() {
    // Arrange
    let mut harness = TestHarness::new();
    let old_dir = harness.root_path.join("old_project/src");
    let mut scanner = MockScanner::new();
    scanner.set_results(
        vec![FileItem {
            path: old_dir.join("main.rs"),
            ..Default::default()
        }],
        vec![],
    );
    let token = harness.state.lock().begin_lazy_load();
    // Act: the user switches directories while the lazy load is in flight.
    harness.state.lock().reset_directory_state();
    lazy_load_task(
        ScanRequest::directory_level(&harness.root_path, &old_dir).unwrap(),
        harness.proxy.clone(),
        harness.state.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    )
    .await;

    // Assert
    harness.assert_no_events("A discarded lazy load must stay silent");
    let state = harness.state.lock();
    assert!(state.full_file_list().is_empty());
    assert!(!state.loaded_dirs.contains(&old_dir));
    assert!(!state.expanded_dirs.contains(&old_dir));
}

/// The result of a lazy load only delivers content. Whether the directory is shown
/// expanded was decided when the load was requested; a directory the user has
/// collapsed in the meantime must not pop open again.
#[tokio::test]
async fn lazy_load_result_does_not_expand_directory_collapsed_meanwhile() {
    // Arrange: the load of `src` is in flight, and `src` is (no longer) expanded.
    let mut harness = TestHarness::new();
    let dir = harness.root_path.join("src");
    let mut scanner = MockScanner::new();
    scanner.set_results(vec![file_item(&dir.join("main.rs"), 2)], vec![]);
    let token = {
        let mut state = harness.state.lock();
        state.edit_full_file_list(|list| list.push(dir_item(&dir, 1)));
        state.begin_lazy_load()
    };

    // Act
    lazy_load_task(
        ScanRequest::directory_level(&harness.root_path, &dir).unwrap(),
        harness.proxy.clone(),
        harness.state.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    )
    .await;

    // Assert
    let ui_state = harness.last_state_update().unwrap();
    let src_node = ui_state
        .root_rows()
        .into_iter()
        .find(|n| n.name == "src")
        .unwrap();
    assert!(src_node.children_loaded && src_node.has_children);
    assert!(!src_node.is_expanded, "Directory must stay collapsed");
    let state = harness.state.lock();
    assert!(state.loaded_dirs.contains(&dir));
    assert!(!state.expanded_dirs.contains(&dir));
}

/// End to end with the real scanner: a lazily loaded level of a subdirectory honours
/// the ignore rules of the project (parent `.gitignore`, user patterns relative to the
/// project root) and its items carry their depth within the project.
#[tokio::test]
async fn lazy_load_of_subdirectory_applies_project_ignore_rules() {
    // Arrange
    let mut harness = TestHarness::new();
    let root = harness.root_path.clone();
    let src = root.join("src");
    std::fs::create_dir_all(src.join("build_out")).unwrap();
    std::fs::create_dir_all(src.join("generated")).unwrap();
    std::fs::write(root.join(".gitignore"), "*.gen\n/src/build_out/\n").unwrap();
    std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(src.join("secret.gen"), "secret").unwrap();
    std::fs::write(src.join("build_out/artifact.o"), "obj").unwrap();
    std::fs::write(src.join("generated/bindings.rs"), "// generated").unwrap();
    {
        // The state after a scan that was cancelled in its deep phase.
        let mut state = harness.state.lock();
        state.config_mut().ignore_patterns = HashSet::from(["/src/generated/".to_string()]);
        state.replace_with_shallow_file_list(&root, vec![dir_item(&src, 1)]);
        filtering::apply_filters(&mut state);
    }

    // Act
    start_lazy_load_scan(src.clone(), harness.proxy.clone(), harness.state.clone())
        .expect("The lazy load must be accepted")
        .await
        .expect("The lazy load task must not panic");

    // Assert
    let ui_state = harness.last_state_update().unwrap();
    let src_node = ui_state.row_named("src").unwrap();
    let child_names: Vec<&str> = ui_state
        .children_of(src_node)
        .into_iter()
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(child_names, vec!["main.rs"]);

    let state = harness.state.lock();
    let loaded: Vec<&FileItem> = state
        .full_file_list()
        .iter()
        .filter(|item| item.path != src)
        .collect();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].path, src.join("main.rs"));
    assert_eq!(
        loaded[0].depth, 2,
        "Depth must be relative to the project root"
    );
    assert!(state.active_ignore_patterns.contains("/src/generated/"));
}

/// A lazy load for a path that is not part of the loaded project is refused: it
/// loads nothing, and it is answered with the state as it is.
#[tokio::test]
async fn lazy_load_outside_project_is_refused_and_answered() {
    let mut harness = TestHarness::new();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("file.txt"), "content").unwrap();

    let load = start_lazy_load_scan(
        outside.path().to_path_buf(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    assert!(load.is_none(), "No load must have been started");
    let answer = harness.event_rx.try_recv();
    assert!(
        matches!(answer, Ok(UserEvent::StateUpdate(_))),
        "the refusal is answered, got {answer:?}"
    );
    harness.assert_no_events("No load must have been performed");
    let state = harness.state.lock();
    assert!(state.full_file_list().is_empty());
    assert!(state.expanded_dirs.is_empty());
}

/// Files that arrive through a lazy load are subject to an active content query too.
#[tokio::test]
async fn content_search_is_repeated_after_lazy_load() {
    let harness = TestHarness::new();
    let dir = harness.root_path.join("src");
    let new_match = dir.join("match.txt");
    let mut scanner = MockScanner::new();
    scanner.set_results(
        vec![
            file_item(&new_match, 2),
            file_item(&dir.join("other.txt"), 2),
        ],
        vec![],
    );
    let token = {
        let mut state = harness.state.lock();
        state.edit_full_file_list(|list| list.push(dir_item(&dir, 1)));
        state.content_search_query = "needle".to_string();
        state.begin_lazy_load()
    };

    lazy_load_task(
        ScanRequest::directory_level(&harness.root_path, &dir).unwrap(),
        harness.proxy.clone(),
        harness.state.clone(),
        scanner,
        ListBoundFileSearcher {
            matching: HashSet::from([new_match.clone()]),
        },
        token,
    )
    .await;

    let state = harness.state.lock();
    assert_eq!(*state.content_search_results(), HashSet::from([new_match]));
}

/// A lazy load started with the previous ignore patterns must not re-insert entries
/// that the patterns added in the meantime have just removed from the list.
#[tokio::test]
async fn lazy_load_result_is_discarded_after_ignore_patterns_were_applied() {
    // Arrange: a lazy load of `src` is in flight and will report `src/generated`.
    let mut harness = TestHarness::new();
    let dir = harness.root_path.join("src");
    let generated = dir.join("generated");
    let mut scanner = MockScanner::new();
    scanner.set_results(vec![dir_item(&generated, 2)], vec![]);
    let token = {
        let mut state = harness.state.lock();
        state.edit_full_file_list(|list| list.push(dir_item(&dir, 1)));
        state.begin_lazy_load()
    };

    // Act: meanwhile the user adds an ignore pattern, which is applied locally.
    {
        let mut state = harness.state.lock();
        state
            .config_mut()
            .ignore_patterns
            .insert("/src/generated/".to_string());
        state.apply_ignore_patterns();
    }
    lazy_load_task(
        ScanRequest::directory_level(&harness.root_path, &dir).unwrap(),
        harness.proxy.clone(),
        harness.state.clone(),
        scanner,
        MockFileSearcher::default(),
        token,
    )
    .await;

    // Assert
    assert!(harness.last_state_update().is_none());
    let state = harness.state.lock();
    assert!(!state.full_file_list().iter().any(|i| i.path == generated));
    assert!(
        !state.loaded_dirs.contains(&dir),
        "The directory stays loadable with the new patterns"
    );
}
