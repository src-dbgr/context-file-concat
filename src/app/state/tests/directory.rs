use super::*;

#[tokio::test]
async fn test_reset_directory_state_clears_all_relevant_fields() {
    // Arrange
    let mut state = AppState {
        current_path: "/test/project".to_string(),
        full_file_list: Arc::new(vec![create_test_file_item("/test/project/file.txt", false)]),
        selected_files: HashSet::from([PathBuf::from("/test/project/file.txt")]).into(),
        expanded_dirs: HashSet::from([PathBuf::from("/test/project")]).into(),
        loaded_dirs: HashSet::from([PathBuf::from("/test/project")]),
        search_query: "test".to_string(),
        is_fully_scanned: true,
        ..Default::default()
    };
    state.set_filtered_file_list(state.full_file_list().to_vec());
    let scan = state.begin_scan();
    state.attach_scan_task(&scan, create_dummy_task());
    let generation = state.begin_generation();
    state.attach_generation_task(&generation, create_dummy_task());
    state.patterns_need_rescan = true;

    // Act
    state.reset_directory_state();

    // Assert
    assert!(state.current_path.is_empty());
    assert!(state.full_file_list().is_empty());
    assert!(state.filtered_file_list().is_empty());
    assert!(state.selected_files.is_empty());
    assert!(state.expanded_dirs.is_empty());
    assert!(state.loaded_dirs.is_empty());
    assert!(state.search_query.is_empty());
    assert!(!state.is_fully_scanned);
    assert!(state.previewed_file_path.is_none());
    assert!(!state.is_generating());
    assert_eq!(state.status, StatusNotice::Ready);

    // Assert that the tasks were cancelled and removed
    assert!(!state.has_scan_task());
    assert!(!state.has_generation_task());
    assert!(!state.patterns_need_rescan);
}

#[tokio::test]
async fn test_apply_ignore_patterns_removes_files_and_selections() {
    // Arrange
    let mut state = AppState::default();
    // NOTE: Ensure a clean state, independent of any config file on disk.
    state.config.ignore_patterns.clear();
    state.current_path = "/project".to_string();

    let file_to_keep = create_test_file_item("/project/src/main.rs", false);
    let dir_to_ignore = create_test_file_item("/project/node_modules", true);
    let file_to_ignore = create_test_file_item("/project/node_modules/dep.js", false);

    state.set_full_file_list(vec![
        file_to_keep.clone(),
        dir_to_ignore.clone(),
        file_to_ignore.clone(),
    ]);
    state.selected_files =
        HashSet::from([file_to_keep.path.clone(), file_to_ignore.path.clone()]).into();

    // Set the patterns for this specific test case.
    state.config.ignore_patterns = HashSet::from(["node_modules/".to_string()]);

    // Act
    state.apply_ignore_patterns();

    // Assert
    assert_eq!(
        state.full_file_list().len(),
        1,
        "Only one item should remain in the full list"
    );
    assert_eq!(
        state.full_file_list()[0].path,
        file_to_keep.path,
        "The remaining file should be main.rs"
    );
    assert_eq!(
        state.selected_files.len(),
        1,
        "Only one item should remain selected"
    );
    assert!(
        state.selected_files.contains(&file_to_keep.path),
        "main.rs should still be selected"
    );
}

#[tokio::test]
async fn test_apply_ignore_patterns_with_empty_set_does_nothing() {
    // Arrange
    let mut state = AppState::default();
    // NOTE: Ensure a clean state, independent of any config file on disk.
    state.config.ignore_patterns.clear();

    let initial_file_list = vec![create_test_file_item("/project/file.txt", false)];
    state.set_full_file_list(initial_file_list.clone());

    // Act
    state.apply_ignore_patterns();

    // Assert
    assert_eq!(state.full_file_list().len(), initial_file_list.len());
    assert_eq!(state.full_file_list()[0].path, initial_file_list[0].path);
}
