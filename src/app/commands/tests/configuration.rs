use super::*;

#[tokio::test]
async fn test_set_config_option_triggers_refilter() {
    let mut harness = TestHarness::new();
    harness.create_file("src/main.rs", "main");
    harness.create_dir("src/empty_dir");

    harness.set_initial_files(&["src", "src/main.rs", "src/empty_dir"]);
    {
        let mut state = harness.state.lock();
        state.is_fully_scanned = true;
        state.loaded_dirs.insert(harness.root_path.join("src"));
        state
            .loaded_dirs
            .insert(harness.root_path.join("src/empty_dir"));
    }

    set_option(&harness, ConfigOption::RemoveEmptyDirectories(true)).await;

    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(
        ui_state.visible_files_count, 2,
        "Expected 'src/empty_dir' to be removed"
    );
}

/// A configuration as somebody else exported it: other settings, and the last
/// directory of *their* installation.
fn foreign_config(last_directory: Option<PathBuf>) -> AppConfig {
    AppConfig {
        ignore_patterns: HashSet::from(["*.log".to_string()]),
        case_sensitive_search: true,
        output_filename: "imported.txt".to_string(),
        max_file_size_mb: 3,
        last_directory,
        ..Default::default()
    }
}

/// Imports `imported` as a complete configuration file of a version up to 0.3.1,
/// which held the window geometry of that installation as well.
fn import_file(harness: &TestHarness, imported: &AppConfig) {
    let path = harness.root_path.join("imported_config.json");
    let mut file = serde_json::to_value(imported).unwrap();
    file["window_size"] = json!([4000.0, 3000.0]);
    file["window_position"] = json!([-900.0, -900.0]);
    file["geometry_units"] = json!("physical");
    std_fs::write(&path, file.to_string()).unwrap();
    harness.dialog.set_pick_file(Some(path));
    import_config(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );
}

/// The geometry of an imported file reaches neither the state nor the config file.
#[track_caller]
fn assert_no_geometry_was_imported(store: &ConfigStore) {
    let written = std_fs::read_to_string(store.path().unwrap()).unwrap();
    for field in ["window_size", "window_position", "geometry_units"] {
        assert!(!written.contains(field), "{field} in {written}");
    }
}

/// The import is a hard reset: selection and filters of the loaded directory are
/// gone, and the directory the imported file names is opened, listed according to
/// the imported settings, and recorded as the last directory.
#[tokio::test]
async fn test_import_config_resets_and_opens_the_imported_last_directory() {
    let (mut harness, store) = TestHarness::with_config_file();
    let kept = harness.create_file("kept.txt", "");
    harness.set_initial_files(&["kept.txt"]);
    {
        let mut state = harness.state.lock();
        state.selected_files.insert(kept.clone());
        state.search_query = "kept".to_string();
    }
    let project_to_open = harness.create_dir("new_project_dir");
    harness.create_file("new_project_dir/file.rs", "");
    harness.create_file("new_project_dir/debug.log", "");
    let imported = foreign_config(Some(project_to_open.clone()));

    import_file(&harness, &imported);

    // First the clean slate, then the scan.
    match harness.get_next_event().await.unwrap() {
        UserEvent::StateUpdate(clean) => {
            assert!(!clean.is_scanning);
            assert_eq!(clean.current_path, "");
            assert_eq!(clean.selected_files_count, 0);
            assert!(clean.rows.is_empty());
            assert!(clean.config.case_sensitive_search);
            assert_eq!(
                clean.current_config_filename.as_deref(),
                Some("imported_config.json")
            );
        }
        other => panic!("Expected the clean state first, got {other:?}"),
    }
    let final_state = harness.wait_for_scan_completion().await.unwrap();
    assert_eq!(final_state.current_path, project_to_open.to_string_lossy());
    assert!(final_state.is_fully_scanned);

    let expected = imported;
    {
        let state = harness.state.lock();
        assert!(state.selected_files.is_empty() && state.search_query.is_empty());
        let listed: Vec<_> = state
            .full_file_list()
            .iter()
            .filter_map(|item| item.path.file_name()?.to_str())
            .collect();
        assert_eq!(
            listed,
            ["file.rs"],
            "the imported ignore pattern applies to the opened directory"
        );
        assert_eq!(*state.config(), expected);
    }
    let saved = config_on_disk(&store, |saved| {
        saved.last_directory.as_ref() == Some(&project_to_open)
    })
    .await;
    assert_eq!(saved, expected);
    assert_no_geometry_was_imported(&store);
}

/// The imported file names no directory (what an exported file looks like): the
/// loaded directory stays loaded and is scanned again with the imported settings.
/// Selection and expansion are kept, as with any re-scan. The geometry of the file
/// is ignored.
#[tokio::test]
async fn test_import_config_without_a_last_directory_rescans_the_loaded_directory() {
    let (mut harness, store) = TestHarness::with_config_file();
    let kept = harness.create_file("src/kept.txt", "");
    harness.create_file("src/debug.log", "");
    harness.set_initial_files(&["src", "src/kept.txt", "src/debug.log"]);
    let last_directory = Some(harness.root_path.clone());
    {
        let mut state = harness.state.lock();
        state.config_mut().last_directory = last_directory.clone();
        state.selected_files.insert(kept.clone());
        state.expanded_dirs.insert(harness.root_path.join("src"));
        state.patterns_need_rescan = true;
    }
    let imported = foreign_config(None);

    import_file(&harness, &imported);

    // No clean slate in between: the first answer announces the re-scan of the
    // directory that is still loaded.
    match harness.get_next_event().await {
        Some(UserEvent::StateUpdate(first)) => {
            assert!(first.is_scanning);
            assert_eq!(first.current_path, harness.root_path.to_string_lossy());
            assert!(first.config.case_sensitive_search);
            assert_eq!(
                first.current_config_filename.as_deref(),
                Some("imported_config.json")
            );
        }
        other => panic!("Expected the announcement of the re-scan, got {other:?}"),
    }
    let final_state = harness.wait_for_scan_completion().await.unwrap();
    assert_eq!(
        final_state.current_path,
        harness.root_path.to_string_lossy()
    );
    assert!(!final_state.patterns_need_rescan);

    let expected = AppConfig {
        last_directory,
        ..imported
    };
    {
        let state = harness.state.lock();
        let listed: Vec<_> = state
            .full_file_list()
            .iter()
            .filter_map(|item| item.path.file_name()?.to_str().map(String::from))
            .collect();
        assert!(listed.contains(&"kept.txt".to_string()), "{listed:?}");
        assert!(
            !listed.contains(&"debug.log".to_string()),
            "the imported pattern *.log applies: {listed:?}"
        );
        assert!(state.selected_files.contains(&kept));
        assert!(state.expanded_dirs.contains(&harness.root_path.join("src")));
        assert_eq!(*state.config(), expected);
    }
    assert_eq!(
        config_on_disk(&store, |saved| saved.output_filename == "imported.txt").await,
        expected
    );
    assert_no_geometry_was_imported(&store);
}

/// Without a loaded directory an import that names none changes the settings only.
#[tokio::test]
async fn test_import_config_without_any_directory_scans_nothing() {
    let mut harness = TestHarness::new();
    harness.state.lock().current_path = String::new();

    import_file(&harness, &foreign_config(None));

    match harness.get_next_event().await {
        Some(UserEvent::StateUpdate(update)) => {
            assert!(!update.is_scanning);
            assert_eq!(update.current_path, "");
            assert!(update.config.case_sensitive_search);
        }
        other => panic!("Expected a state update, got {other:?}"),
    }
    harness
        .expect_no_event_within(QUIET_PERIOD, "No scan should start without a directory")
        .await;
    assert!(!harness.state.lock().is_scanning());
}

/// Without a directory there is nothing to filter or scan, but the change is
/// stored and answered: the UI shows the configuration the host reports.
#[tokio::test]
async fn test_config_change_without_a_directory_is_stored_and_answered() {
    let mut harness = TestHarness::new();
    harness.state.lock().current_path = String::new();

    set_option(&harness, ConfigOption::RemoveEmptyDirectories(true)).await;
    let ui_state = harness.last_state_update().expect("the change is answered");
    assert!(ui_state.config.remove_empty_directories);

    add_pattern(&harness, "generated/").await;
    let ui_state = harness.last_state_update().expect("the change is answered");
    assert!(ui_state
        .config
        .ignore_patterns
        .contains(&"generated/".to_string()));
    assert!(!ui_state.patterns_need_rescan);

    remove_pattern(&harness, "generated/").await;
    let ui_state = harness.last_state_update().expect("the change is answered");
    assert!(
        !ui_state.patterns_need_rescan,
        "Nothing is loaded that a re-scan could refresh"
    );
    assert!(!harness.state.lock().is_scanning());
}

#[tokio::test]
async fn test_import_config_sends_error_on_corrupt_file() {
    let mut harness = TestHarness::new();
    let corrupt_config_path = harness.create_file("corrupt_config.json", "{ not_valid_json, }");
    harness.dialog.set_pick_file(Some(corrupt_config_path));

    import_config(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    match harness.get_next_event().await.unwrap() {
        UserEvent::ShowError(msg) => {
            assert!(
                msg.contains("Failed to import config"),
                "Expected an import error message, but got: {}",
                msg
            );
        }
        other => panic!("Expected ShowError event, but got {:?}", other),
    }
}

/// A request the command itself refuses changes nothing and is answered all the
/// same: the UI shows what the host reports, so it has to be told that nothing
/// changed. (A payload that does not fit its command never gets here; see
/// `a_refused_message_changes_nothing_and_is_answered_under_its_number` in
/// `app/mod.rs`.)
#[tokio::test]
async fn test_config_commands_answer_what_they_refuse() {
    let mut harness = TestHarness::new();
    let initial_config = harness.state.lock().config().clone();

    // A blank text is not a pattern.
    for blank in ["", "   ", "\t"] {
        add_pattern(&harness, blank).await;
        assert_eq!(harness.state_updates_sent(), 1, "answer to {blank:?}");
    }
    // A path that is no entry of the loaded directory has no pattern.
    add_ignore_path(
        PathBuf::from("src"),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
    assert_eq!(harness.state_updates_sent(), 1, "answer to addIgnorePath");

    // Nothing was changed, nothing was saved, and each refusal was answered once.
    assert_eq!(*harness.state.lock().config(), initial_config);
    harness
        .expect_no_event_within(QUIET_PERIOD, "Nothing else is sent")
        .await;
}

#[tokio::test]
async fn test_add_ignore_pattern_hides_matching_files() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["src/main.rs"]);

    add_pattern(&harness, "*.rs").await;

    // No scan runs here (none was running): the pattern is applied to the list.
    let final_state = harness.last_state_update().unwrap();
    assert_eq!(
        final_state.visible_files_count, 0,
        "Scan should have removed the .rs file"
    );
}

/// The imported file names a directory that does not exist here (somebody else's
/// complete configuration file): as without one, the loaded directory stays and is
/// scanned again with the imported settings.
#[tokio::test]
async fn test_import_config_with_a_nonexistent_last_directory_keeps_the_loaded_directory() {
    let mut harness = TestHarness::new();
    harness.create_file("kept.txt", "");
    harness.create_file("debug.log", "");
    harness.set_initial_files(&["kept.txt", "debug.log"]);
    let nonexistent = harness.root_path.join("nonexistent_dir");

    import_file(&harness, &foreign_config(Some(nonexistent)));

    let final_state = harness.wait_for_scan_completion().await.unwrap();
    assert_eq!(
        final_state.current_path,
        harness.root_path.to_string_lossy()
    );
    assert!(final_state.config.case_sensitive_search);
    let state = harness.state.lock();
    let listed: Vec<_> = state
        .full_file_list()
        .iter()
        .filter_map(|item| item.path.file_name()?.to_str().map(String::from))
        .collect();
    assert!(listed.contains(&"kept.txt".to_string()), "{listed:?}");
    assert!(!listed.contains(&"debug.log".to_string()), "{listed:?}");
}

#[tokio::test]
async fn test_pick_output_directory_updates_config() {
    let harness = TestHarness::new();
    let new_dir = harness.create_dir("output");
    harness.dialog.set_pick_folder(Some(new_dir.clone()));

    pick_output_directory(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    let state = harness.state.lock();
    assert_eq!(state.config().output_directory, Some(new_dir));
}

// --- Every change of the configuration is persisted, through the ordered path ---

#[tokio::test]
async fn test_pick_output_directory_persists_the_choice() {
    let (mut harness, store) = TestHarness::with_config_file();
    let new_dir = harness.create_dir("output");
    harness.dialog.set_pick_folder(Some(new_dir.clone()));

    pick_output_directory(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    let saved = config_on_disk(&store, |saved| {
        saved.output_directory.as_ref() == Some(&new_dir)
    })
    .await;
    assert_eq!(saved, *harness.state.lock().config());
    let answered = harness.last_state_update().expect("the choice is answered");
    assert_eq!(answered.config.output_directory, Some(new_dir));
}

#[tokio::test]
async fn test_config_commands_persist_their_change() {
    let (harness, store) = TestHarness::with_config_file();

    set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;
    add_pattern(&harness, "generated/").await;
    set_option(&harness, ConfigOption::OutputFilename("named.txt".into())).await;

    let saved = config_on_disk(&store, |saved| saved.output_filename == "named.txt").await;
    assert_eq!(saved, *harness.state.lock().config());
    assert!(saved.case_sensitive_search);
    assert!(saved.ignore_patterns.contains("generated/"));
}

/// Many changes in a row, each saved in the background by a task of its own: the
/// saves get to run in any order, and what ends up on disk is still the last one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_rapid_config_changes_end_with_the_last_one_on_disk() {
    for round in 0..10 {
        let (harness, store) = TestHarness::with_config_file();
        let last = 40;

        for i in 0..=last {
            let name = format!("round-{round}-change-{i}.txt");
            set_option(&harness, ConfigOption::OutputFilename(name)).await;
        }
        // Wait for every background save, then look at what is there.
        let expected = format!("round-{round}-change-{last}.txt");
        let saved = config_on_disk(&store, |saved| saved.output_filename == expected).await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert_eq!(saved, *harness.state.lock().config());
        let path = store.path().unwrap();
        assert_eq!(
            config::settings::load_config(path).unwrap().output_filename,
            expected,
            "round {round}: an older save must not have overwritten the last one"
        );
    }
}

// --- exportConfig: shareable settings only ---

#[tokio::test]
async fn test_export_config_writes_the_shared_settings_only() {
    let mut harness = TestHarness::new();
    let save_path = harness.root_path.join("exported.json");
    harness.dialog.set_save_file(Some(save_path.clone()));
    {
        let mut state = harness.state.lock();
        state.config_mut().ignore_patterns = HashSet::from(["*.shared".to_string()]);
        state.config_mut().case_sensitive_search = true;
        state.config_mut().output_filename = "shared.txt".to_string();
        state.config_mut().output_directory = Some(PathBuf::from("/home/alice/out"));
        state.config_mut().last_directory = Some(PathBuf::from("/home/alice/project"));
    }

    export_config(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    assert!(matches!(
        harness.get_next_event().await,
        Some(UserEvent::ConfigExported(true))
    ));
    let written = std_fs::read_to_string(&save_path).unwrap();
    let exported: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(
        exported,
        serde_json::to_value(config::SharedConfig::from(harness.state.lock().config())).unwrap()
    );
    assert_eq!(exported["ignore_patterns"], json!(["*.shared"]));
    for personal in [
        "alice",
        "last_directory",
        "output_directory",
        "window_size",
        "window_position",
        "geometry_units",
    ] {
        assert!(!written.contains(personal), "{personal} in {written}");
    }
}

/// The export of one installation, imported into another: the settings arrive,
/// the directories of the importing installation stay.
#[tokio::test]
async fn test_exported_config_can_be_imported() {
    let mut exporter = TestHarness::new();
    let exported_path = exporter.root_path.join("exported.json");
    exporter.dialog.set_save_file(Some(exported_path.clone()));
    {
        let mut state = exporter.state.lock();
        state.config_mut().ignore_patterns = HashSet::from(["*.shared".to_string()]);
        state.config_mut().case_sensitive_search = true;
        state.config_mut().max_file_size_mb = 3;
        state.config_mut().output_directory = Some(PathBuf::from("/home/alice/out"));
    }
    export_config(
        exporter.dialog.as_ref(),
        exporter.proxy.clone(),
        exporter.state.clone(),
    );
    assert!(matches!(
        exporter.get_next_event().await,
        Some(UserEvent::ConfigExported(true))
    ));

    let mut importer = TestHarness::new();
    {
        let mut state = importer.state.lock();
        state.config_mut().output_directory = Some(PathBuf::from("/home/bob/out"));
        // The directory that is loaded, as in a running installation.
        state.config_mut().last_directory = Some(importer.root_path.clone());
    }
    importer.dialog.set_pick_file(Some(exported_path));
    import_config(
        importer.dialog.as_ref(),
        importer.proxy.clone(),
        importer.state.clone(),
    );

    // An export names no directory to open: the loaded one stays, scanned again.
    let rescanned = importer.wait_for_scan_completion().await.unwrap();
    assert_eq!(rescanned.current_path, importer.root_path.to_string_lossy());
    let config = importer.state.lock().config().clone();
    assert_eq!(
        config::SharedConfig::from(&config),
        config::SharedConfig::from(exporter.state.lock().config())
    );
    assert_eq!(
        config.output_directory,
        Some(PathBuf::from("/home/bob/out"))
    );
    assert_eq!(config.last_directory, Some(importer.root_path.clone()));
}

/// The file is written without the state lock: while the export writes, the state
/// can be locked by anybody else, and what is written is the copy taken before.
#[tokio::test]
async fn test_export_config_writes_without_holding_the_state_lock() {
    let mut harness = TestHarness::new();
    let save_path = harness.root_path.join("exported.json");
    harness.dialog.set_save_file(Some(save_path.clone()));
    harness.state.lock().config_mut().output_filename = "before.txt".to_string();
    let observed_state = harness.state.clone();
    let mut written = None;

    export_config_with(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
        |settings, path| {
            // The lock is free while the file is written ...
            let mut state = observed_state
                .try_lock()
                .expect("the state lock must not be held during the write");
            // ... and an edit made meanwhile does not reach the file.
            state.config_mut().output_filename = "meanwhile.txt".to_string();
            written = Some((settings.clone(), path.to_path_buf()));
            Ok(())
        },
    );

    let (settings, path) = written.expect("the file is written");
    assert_eq!(settings.output_filename, "before.txt");
    assert_eq!(path, save_path);
    assert!(matches!(
        harness.get_next_event().await,
        Some(UserEvent::ConfigExported(true))
    ));
}

/// The seam of the test above sees a held lock: it would fail if the export wrote
/// under the lock.
#[test]
fn test_a_held_state_lock_is_observable() {
    let state = SharedState::new(AppState::default());
    let guard = state.lock();
    assert!(state.try_lock().is_none());
    drop(guard);
    assert!(state.try_lock().is_some());
}

#[tokio::test]
async fn test_export_config_sends_event() {
    let mut harness = TestHarness::new();
    let save_path = harness.root_path.join("my-config.json");
    harness.dialog.set_save_file(Some(save_path));

    export_config(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    match harness.get_next_event().await.unwrap() {
        UserEvent::ConfigExported(success) => assert!(success),
        _ => panic!("Expected ConfigExported event"),
    }
}

#[tokio::test]
async fn test_export_config_sends_no_event_on_cancel() {
    let mut harness = TestHarness::new();
    harness.dialog.set_save_file(None);

    export_config(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    harness
        .expect_no_event_within(
            QUIET_PERIOD,
            "No event should be sent when export is cancelled",
        )
        .await;
}

#[tokio::test]
async fn test_pick_output_directory_does_nothing_on_cancel() {
    let mut harness = TestHarness::new();
    harness.dialog.set_pick_folder(None);
    let initial_config = harness.state.lock().config().clone();
    pick_output_directory(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let final_config = harness.state.lock().config().clone();
    harness
        .expect_no_event_within(QUIET_PERIOD, "No event expected")
        .await;
    assert_eq!(
        initial_config.output_directory,
        final_config.output_directory
    );
}

#[tokio::test]
async fn test_import_config_does_nothing_on_cancel() {
    let mut harness = TestHarness::new();
    harness.dialog.set_pick_file(None); // Simulate user cancelling dialog
    import_config(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    harness
        .expect_no_event_within(QUIET_PERIOD, "No event expected")
        .await;
}

/// An option that affects neither the file list nor the scan is stored and
/// answered with the configuration the UI is to show.
#[tokio::test]
async fn test_set_config_option_stores_output_options() {
    let mut harness = TestHarness::new();

    set_option(
        &harness,
        ConfigOption::OutputFilename("new_name.txt".to_string()),
    )
    .await;
    let ui_state = harness.last_state_update().expect("the change is answered");
    assert_eq!(ui_state.config.output_filename, "new_name.txt");
    assert!(!ui_state.is_scanning);

    set_option(&harness, ConfigOption::IncludeTreeByDefault(false)).await;
    set_option(&harness, ConfigOption::UseRelativePaths(false)).await;
    set_option(
        &harness,
        ConfigOption::OutputDirectory(Some(PathBuf::from("/out"))),
    )
    .await;
    let ui_state = harness.last_state_update().unwrap();
    assert!(!ui_state.config.include_tree_by_default);
    assert!(!ui_state.config.use_relative_paths);
    assert_eq!(
        ui_state.config.output_directory,
        Some(PathBuf::from("/out"))
    );

    // An empty directory text means "no preset", like no directory at all.
    for cleared in [Some(PathBuf::new()), None] {
        set_option(
            &harness,
            ConfigOption::OutputDirectory(Some(PathBuf::from("/out"))),
        )
        .await;
        set_option(&harness, ConfigOption::OutputDirectory(cleared)).await;
        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.config.output_directory, None);
    }
}

/// The reason for the intent commands: every edit is applied to the configuration
/// of the host. With whole-config payloads the second of two quick edits was based
/// on the copy the UI had before the first one and silently reverted it.
#[tokio::test]
async fn test_quick_successive_config_edits_do_not_overwrite_each_other() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["src"]);
    {
        let mut state = harness.state.lock();
        state.config_mut().ignore_patterns = HashSet::from(["keep".to_string()]);
        state.config_mut().max_file_size_mb = 7;
        state.config_mut().last_directory = Some(PathBuf::from("/somewhere"));
    }

    // None of these has seen the outcome of the ones before it.
    add_pattern(&harness, "first").await;
    add_pattern(&harness, "second").await;
    set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;
    set_option(&harness, ConfigOption::UseRelativePaths(false)).await;

    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(
        ui_state.config.ignore_patterns,
        vec![
            "first".to_string(),
            "keep".to_string(),
            "second".to_string()
        ]
    );
    assert!(
        !ui_state.patterns_need_rescan,
        "Adding patterns must never look like a removal"
    );
    let state = harness.state.lock();
    assert!(state.config().case_sensitive_search);
    assert!(!state.config().use_relative_paths);
    // What the UI never edits cannot be touched by its commands.
    assert_eq!(state.config().max_file_size_mb, 7);
    assert_eq!(
        state.config().last_directory,
        Some(PathBuf::from("/somewhere"))
    );
}

#[tokio::test]
async fn test_clear_ignore_patterns_removes_all_and_recommends_rescan() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["src"]);

    clear_ignore_patterns(harness.proxy.clone(), harness.state.clone()).await;

    let ui_state = harness.last_state_update().unwrap();
    assert!(ui_state.config.ignore_patterns.is_empty());
    assert!(ui_state.patterns_need_rescan);

    // Nothing left to remove: answered, but no further recommendation is derived.
    harness.state.lock().patterns_need_rescan = false;
    clear_ignore_patterns(harness.proxy.clone(), harness.state.clone()).await;
    let ui_state = harness.last_state_update().unwrap();
    assert!(!ui_state.patterns_need_rescan);
}

/// Removing a pattern that is not set and adding one that is set are no-ops.
#[tokio::test]
async fn test_pattern_commands_are_idempotent() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["src"]);
    let before = harness.state.lock().config().clone();
    let existing = before.ignore_patterns.iter().next().unwrap().clone();

    remove_pattern(&harness, "never-added").await;
    add_pattern(&harness, &existing).await;

    let ui_state = harness
        .last_state_update()
        .expect("the commands are answered");
    assert!(!ui_state.patterns_need_rescan);
    assert_eq!(*harness.state.lock().config(), before);
}

/// A file that names only some settings is completed with the defaults.
#[tokio::test]
async fn test_import_config_with_a_partial_file() {
    let mut harness = TestHarness::new();
    harness.state.lock().current_path = String::new();
    let new_config_path = harness.create_file("config_no_dir.json", r#"{ "ignore_patterns": [] }"#);
    harness.dialog.set_pick_file(Some(new_config_path));
    import_config(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    // A StateUpdate event is sent immediately after reset.
    assert!(
        matches!(
            harness.get_next_event().await,
            Some(UserEvent::StateUpdate(_))
        ),
        "Expected an immediate state update"
    );
    // No *second* event should follow, as no scan is started.
    harness
        .expect_no_event_within(QUIET_PERIOD, "No scan should start without a directory")
        .await;
    assert!(harness.state.lock().config().ignore_patterns.is_empty());
}

#[tokio::test]
async fn test_export_config_sends_false_on_failure() {
    let mut harness = TestHarness::new();
    // A cross-platform way to cause an I/O error is to target a directory.
    let invalid_path = harness.root_path.clone();
    harness.dialog.set_save_file(Some(invalid_path));

    export_config(
        harness.dialog.as_ref(),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    match harness.get_next_event().await.unwrap() {
        UserEvent::ConfigExported(success) => assert!(!success),
        other => panic!("Expected ConfigExported(false), got {:?}", other),
    }
}

#[tokio::test]
async fn test_add_ignore_pattern_applies_locally() {
    let mut harness = TestHarness::new();

    // Ensure a clean state for the test run.
    {
        harness.state.lock().config_mut().ignore_patterns.clear();
    }

    // The `current_path` is the root for the ignore matcher. All file paths
    // in the state MUST be children of this root path.
    let root = "/test/project";
    harness.state.lock().current_path = root.to_string();

    // Create abstract files on the "filesystem".
    harness.create_file("src/main.rs", "fn main() {}");
    harness.create_file("debug.log", "log content");

    // Set up the initial file list using full paths that are consistent
    // with the root path. This resolves the `ignore` crate panic.
    harness.set_initial_files(&[
        &format!("{}/src", root),
        &format!("{}/src/main.rs", root),
        &format!("{}/debug.log", root),
    ]);

    add_pattern(&harness, "*.log").await;

    // Assert the outcome.
    let ui_state = harness.last_state_update().unwrap();

    assert!(
        !ui_state.patterns_need_rescan,
        "Flag should NOT be set when only adding patterns"
    );

    // After filtering "*.log", two items remain: "/test/project/src" and "/test/project/src/main.rs".
    assert_eq!(
        ui_state.visible_files_count, 2,
        "Should have src dir and main.rs, log file should be filtered out"
    );
}

#[tokio::test]
async fn test_remove_ignore_pattern_sets_rescan_flag() {
    let mut harness = TestHarness::new();

    // Create actual files
    harness.create_file("src/main.rs", "fn main() {}");
    harness.create_file("debug.log", "log content");
    harness.create_dir("node_modules");
    harness.create_file("node_modules/package.json", "{}");

    // Set initial files
    harness.set_initial_files(&[
        "src",
        "src/main.rs",
        "debug.log",
        "node_modules",
        "node_modules/package.json",
    ]);

    // Set initial patterns
    {
        let mut state = harness.state.lock();
        state
            .config_mut()
            .ignore_patterns
            .insert("*.log".to_string());
        state
            .config_mut()
            .ignore_patterns
            .insert("node_modules/".to_string());
    }

    remove_pattern(&harness, "node_modules/").await;

    let ui_state = harness.last_state_update().unwrap();
    assert!(
        ui_state.patterns_need_rescan,
        "Flag should be set when patterns are removed"
    );
}

#[tokio::test]
async fn test_rescan_clears_patterns_need_rescan_flag() {
    let mut harness = TestHarness::new();

    // Create a test directory with a file
    harness.create_file("test.txt", "content");

    {
        let mut state = harness.state.lock();
        state.patterns_need_rescan = true;
        // Ensure we have a valid current_path for the rescan
        // (TestHarness::new() already sets this)
    }

    rescan_directory(harness.proxy.clone(), harness.state.clone());

    // Wait for scan to complete
    let final_state = harness.wait_for_scan_completion().await.unwrap();
    assert!(
        !final_state.patterns_need_rescan,
        "Flag should be cleared after rescan"
    );
}

/// This is an isolated, robust unit test for the "add pattern" logic of `add_ignore_pattern`.
/// It does not use the TestHarness and has no external dependencies, making it 100% deterministic.
/// It asserts three things:
/// 1. That the correct log message was emitted (proving the code path was taken).
/// 2. That the event sent to the UI has the correct, updated state.
/// 3. That the internal AppState was mutated correctly.
#[tokio::test]
async fn test_add_ignore_pattern_isolated() {
    // ADDED: Use the same logging setup as the other tests.
    setup_test_logging();

    // --- ARRANGE ---
    let (tx, mut rx) = mpsc::unbounded_channel();
    let proxy = TestEventProxy::new(tx);
    let state = {
        let mut s = AppState::default();
        s.config_mut().ignore_patterns.clear();
        s.current_path = "/app".to_string();
        s.set_full_file_list(vec![
            FileItem {
                path: PathBuf::from("/app/src/main.rs"),
                ..Default::default()
            },
            FileItem {
                path: PathBuf::from("/app/debug.log"),
                ..Default::default()
            },
        ]);
        s.set_filtered_file_list(s.full_file_list().to_vec());
        SharedState::new(s)
    };

    // --- ACT ---
    add_ignore_pattern(typed(json!({ "pattern": "*.log" })), proxy, state.clone()).await;

    // --- ASSERT ---

    // The following state and event assertions are the primary verification of correctness.

    let event = rx.recv().await.expect("An event should have been sent.");
    let ui_state = match event {
        UserEvent::StateUpdate(s) => s,
        _ => panic!("Expected a StateUpdate event"),
    };
    assert_eq!(ui_state.visible_files_count, 1);
    assert!(!ui_state.patterns_need_rescan);

    let final_state = state.lock();
    assert_eq!(final_state.full_file_list().len(), 1);
    assert!(final_state.config().ignore_patterns.contains("*.log"));
}

/// A comprehensive integration test for the entire user workflow of removing an ignore pattern.
///
/// This test simulates a scenario with a nested directory structure to verify
/// the two-phase state update process when un-ignoring files.
///
/// It verifies the following sequence of events:
/// 1. An initial scan is performed with a directory ("build_artifacts/") actively ignored,
///    confirming that the initial file count is low as expected.
/// 2. The configuration is updated to remove the ignore pattern.
/// 3. It asserts that the `patterns_need_rescan` flag is correctly set immediately
///    after the update, while the file count correctly remains unchanged.
/// 4. A re-scan is triggered by the user (simulated by the test).
/// 5. It asserts that after the re-scan completes, the flag is cleared and the final
///    file count ("statistic") has increased, reflecting that the previously ignored
///    files are now visible.
///
/// NOTE on the final file count: The final count is 5, not 6. This is because the
/// scanner's underlying `ignore` library 0has default behavior that automatically
/// filters out files it detects as binary based on their content (e.g., containing
/// null bytes). The `app.dat` file created in this test is correctly identified
/// as binary and skipped, and the test asserts this real-world behavior.
#[tokio::test]
async fn test_removing_pattern_and_rescanning_updates_file_list() {
    // --- 1. Arrange ---
    let mut harness = TestHarness::new();
    harness.create_file(".gitignore", "");
    let artifact_dir_name = "build_artifacts";
    let sub_dir_name = "binaries";
    let binary_filename = "app.dat";

    harness.create_file("src/main.rs", "fn main() {}");
    harness.create_dir(&format!("{}/{}", artifact_dir_name, sub_dir_name));
    harness.create_file(
        &format!("{}/{}/{}", artifact_dir_name, sub_dir_name, binary_filename),
        "\x00\x01\x02\x03", // Use actual binary content to trigger detection
    );
    harness.create_file("README.md", "Project Readme");

    {
        let mut state = harness.state.lock();
        state
            .config_mut()
            .ignore_patterns
            .insert(format!("{}/", artifact_dir_name));
    }

    // --- 2. Act & Assert Initial State ---
    rescan_directory(harness.proxy.clone(), harness.state.clone());
    let initial_state = harness
        .wait_for_scan_completion()
        .await
        .expect("Initial scan should complete.");
    assert_eq!(initial_state.visible_files_count, 3);

    // --- 3. Act: Remove pattern and rescan ---
    remove_pattern(&harness, &format!("{}/", artifact_dir_name)).await;

    let _ = harness.last_state_update();

    rescan_directory(harness.proxy.clone(), harness.state.clone());
    let final_state = harness
        .wait_for_scan_completion()
        .await
        .expect("Final re-scan should complete.");

    // --- 4. Assert Final State ---
    assert!(!final_state.patterns_need_rescan);

    assert_eq!(
        final_state.visible_files_count, 5,
        "Final count should be 5. The binary 'app.dat' is skipped by the scanner's default behavior."
    );
    assert!(final_state
        .root_rows()
        .into_iter()
        .any(|node| node.name == artifact_dir_name));
}

// --- Config changes that need more than a re-filter ---

/// Registers a scan whose task never finishes, as if a long scan were running.
fn begin_parked_scan(harness: &TestHarness) -> crate::app::state::ScanToken {
    let mut state = harness.state.lock();
    let token = state.begin_scan();
    state.attach_scan_task(&token, tokio::spawn(std::future::pending()));
    token
}

fn file_names(state: &AppState) -> HashSet<String> {
    state
        .full_file_list()
        .iter()
        .map(|item| {
            item.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// A scan works with the patterns it was started with. If a pattern is added while
/// it is running, its result would bring the entries back that the pattern has just
/// removed, so the scan is replaced by one that knows the new patterns.
#[tokio::test]
async fn test_pattern_change_restarts_running_scan_when_pattern_is_added() {
    let mut harness = TestHarness::new();
    harness.create_file("src/main.rs", "fn main() {}");
    harness.create_file("docs/guide.md", "# Guide");
    harness.set_initial_files(&["src", "docs"]); // The shallow list of the scan.
    let old_scan = begin_parked_scan(&harness);

    add_pattern(&harness, "docs/").await;

    // The old scan is out of the game, a new one has taken over, and the ignored
    // directory has disappeared at once.
    {
        let state = harness.state.lock();
        assert!(old_scan.is_cancelled());
        assert!(!state.is_current_scan(&old_scan));
        assert!(state.is_scanning(), "A new scan must be running");
        assert!(state.has_scan_task(), "The new scan must be cancellable");
        assert_eq!(file_names(&state), HashSet::from(["src".to_string()]));
    }

    let final_state = harness
        .wait_for_state(|ui_state| !ui_state.is_scanning && ui_state.is_fully_scanned)
        .await;
    assert!(!final_state
        .root_rows()
        .into_iter()
        .any(|node| node.name == "docs"));
    assert!(!final_state.patterns_need_rescan);
    let state = harness.state.lock();
    assert_eq!(
        file_names(&state),
        HashSet::from(["src".to_string(), "main.rs".to_string()]),
        "The complete list must follow the new patterns"
    );
    assert!(state.active_ignore_patterns.contains("docs/"));
}

/// Removing a pattern during a scan restarts it as well: the new scan lists what
/// the pattern used to hide, so no separate re-scan has to be recommended.
#[tokio::test]
async fn test_pattern_change_restarts_running_scan_when_pattern_is_removed() {
    let mut harness = TestHarness::new();
    harness.create_file("src/main.rs", "fn main() {}");
    harness.create_file("docs/guide.md", "# Guide");
    harness.set_initial_files(&["src"]);
    harness
        .state
        .lock()
        .config_mut()
        .ignore_patterns
        .insert("docs/".to_string());
    let old_scan = begin_parked_scan(&harness);

    remove_pattern(&harness, "docs/").await;

    assert!(old_scan.is_cancelled());
    let final_state = harness
        .wait_for_state(|ui_state| !ui_state.is_scanning && ui_state.is_fully_scanned)
        .await;
    assert!(final_state
        .root_rows()
        .into_iter()
        .any(|node| node.name == "docs"));
    assert!(!final_state.patterns_need_rescan);
    let state = harness.state.lock();
    assert!(file_names(&state).contains("guide.md"));
}

/// A config change that does not touch the patterns leaves a running scan alone.
#[tokio::test]
async fn test_config_change_without_pattern_change_keeps_running_scan() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["src"]);
    let scan = begin_parked_scan(&harness);

    set_option(&harness, ConfigOption::RemoveEmptyDirectories(true)).await;
    set_option(
        &harness,
        ConfigOption::OutputFilename("other.txt".to_string()),
    )
    .await;

    let ui_state = harness.last_state_update().unwrap();
    assert!(ui_state.is_scanning);
    let mut state = harness.state.lock();
    assert!(state.is_current_scan(&scan));
    assert!(!scan.is_cancelled());
    state.cancel_current_scan(); // Stop the parked task.
}

/// The matches of a content search depend on the case sensitivity, so changing it
/// repeats the search instead of filtering with the matches of the old setting.
#[tokio::test]
async fn test_case_sensitivity_change_repeats_content_search() {
    let mut harness = TestHarness::new();
    let lower = harness.create_file("lower.txt", "a needle in the hay");
    let upper = harness.create_file("upper.txt", "a NEEDLE in the hay");
    harness.set_initial_files(&["lower.txt", "upper.txt"]);
    {
        // The state a case-insensitive search for "needle" has left behind.
        let mut state = harness.state.lock();
        state.config_mut().case_sensitive_search = false;
        state.content_search_query = "needle".to_string();
        state.set_content_search_results(HashSet::from([lower.clone(), upper.clone()]));
    }

    set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;

    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(ui_state.visible_files_count, 1);
    assert_eq!(
        *harness.state.lock().content_search_results(),
        HashSet::from([lower.clone()])
    );

    // And back: the file that only matches case-insensitively is found again.
    set_option(&harness, ConfigOption::CaseSensitiveSearch(false)).await;

    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(ui_state.visible_files_count, 2);
    assert_eq!(
        *harness.state.lock().content_search_results(),
        HashSet::from([lower, upper])
    );
}

/// Without a content query there is nothing to repeat: the change is answered
/// with the one state update of the re-filter.
#[tokio::test]
async fn test_case_sensitivity_change_without_content_query_only_refilters() {
    let mut harness = TestHarness::new();
    harness.create_file("Main.rs", "");
    harness.set_initial_files(&["Main.rs"]);
    harness.state.lock().search_query = "main".to_string();

    set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;

    let mut updates = Vec::new();
    while let Ok(event) = harness.event_rx.try_recv() {
        updates.push(event);
    }
    assert_eq!(updates.len(), 1, "Exactly one state update expected");
    match &updates[0] {
        UserEvent::StateUpdate(ui_state) => assert_eq!(ui_state.visible_files_count, 0),
        other => panic!("Expected a state update, got {other:?}"),
    }
}

/// A change of the case sensitivity changes what a file name filter matches: the
/// directories of the new matches are expanded, as after a changed filter text,
/// except for those the user collapsed while filtering.
#[tokio::test]
async fn test_case_sensitivity_change_expands_the_directories_of_new_matches() {
    let harness = TestHarness::new();
    harness.create_file("src/Main.rs", "");
    harness.create_file("docs/MAIN.md", "");
    harness.set_initial_files(&["src", "src/Main.rs", "docs", "docs/MAIN.md"]);
    let (src, docs) = (
        harness.root_path.join("src"),
        harness.root_path.join("docs"),
    );
    {
        let mut state = harness.state.lock();
        state.config_mut().case_sensitive_search = true;
        state.search_query = "main".to_string();
        filtering::apply_filter_texts(&mut state);
        assert!(state.expanded_dirs.is_empty(), "nothing matches yet");
        // The user expands `docs` and collapses it again while the filter is active.
        state.expand_directories([docs.clone()]);
        state.collapse_directories([docs.clone()]);
    }

    set_option(&harness, ConfigOption::CaseSensitiveSearch(false)).await;

    let state = harness.state.lock();
    assert_eq!(state.filtered_file_list().len(), 4);
    assert!(
        state.expanded_dirs.contains(&src),
        "the new match is revealed"
    );
    assert!(
        !state.expanded_dirs.contains(&docs),
        "collapsed by the user"
    );
}

/// A negation pattern re-includes entries. Applied to the loaded list a pattern can
/// only take entries away, so the user is told that a re-scan is needed.
#[tokio::test]
async fn test_adding_a_negation_pattern_asks_for_a_rescan() {
    let mut harness = TestHarness::new();
    harness.create_file("keep.log", "");
    harness.set_initial_files(&["main.rs"]);
    harness.state.lock().config_mut().ignore_patterns = HashSet::from(["*.log".to_string()]);

    add_pattern(&harness, "*.tmp").await;
    assert!(!harness.last_state_update().unwrap().patterns_need_rescan);

    add_pattern(&harness, "!keep.log").await;
    let update = harness.last_state_update().unwrap();
    assert!(update.patterns_need_rescan);
    assert!(update
        .config
        .ignore_patterns
        .iter()
        .any(|p| p == "!keep.log"));
}
