use super::*;

/// A generation started via the command owns the generating state and can be cancelled.
#[tokio::test]
async fn test_generate_preview_registers_cancellable_generation() {
    let mut harness = TestHarness::new();
    harness.create_file("file.txt", "content");
    harness.set_initial_files(&["file.txt"]);

    generate_preview(harness.proxy.clone(), harness.state.clone());

    {
        let state = harness.state.lock();
        assert!(state.is_generating());
        assert!(state.has_generation_task());
    }
    cancel_generation(harness.proxy.clone(), harness.state.clone());
    let ui_state = harness.last_state_update().unwrap();
    assert!(!ui_state.is_generating);
    let state = harness.state.lock();
    assert!(!state.is_generating());
    assert!(!state.has_generation_task());
}

#[tokio::test]
async fn test_generate_preview_sets_generating_state_and_spawns_task() {
    let mut harness = TestHarness::new();
    harness.create_file("file.txt", "content");
    harness.set_initial_files(&["file.txt"]);

    generate_preview(harness.proxy.clone(), harness.state.clone());

    let event = harness.get_next_event().await.unwrap();
    let ui_state = match event {
        UserEvent::StateUpdate(ui_state) => ui_state,
        _ => panic!("Expected a StateUpdate event first"),
    };
    assert!(ui_state.is_generating);

    // The generation task reports its end with a final state update.
    harness
        .wait_for_state(|ui_state| !ui_state.is_generating)
        .await;
}

#[tokio::test]
async fn test_cancel_generation_resets_generating_state() {
    let mut harness = TestHarness::new();
    generate_preview(harness.proxy.clone(), harness.state.clone());
    let _ = harness.last_state_update();

    cancel_generation(harness.proxy.clone(), harness.state.clone());
    let ui_state = harness.last_state_update().unwrap();
    assert!(!ui_state.is_generating);
    assert_eq!(ui_state.status_message, "Generation cancelled.");
}

#[tokio::test]
async fn test_cancel_generation_without_running_generation_keeps_status() {
    let mut harness = TestHarness::new();
    let status_before = harness.state.lock().status.clone();

    cancel_generation(harness.proxy.clone(), harness.state.clone());

    let ui_state = harness.last_state_update().unwrap();
    assert!(!ui_state.is_generating);
    assert_eq!(harness.state.lock().status, status_before);
}

#[tokio::test]
async fn test_save_file_writes_to_disk_on_ok() {
    let mut harness = TestHarness::new();
    let save_path = harness.root_path.join("output.txt");
    let content_to_save = "Hello, World!";
    harness.dialog.set_save_file(Some(save_path.clone()));

    save_file(
        harness.dialog.as_ref(),
        typed(json!({ "kind": "edited", "content": content_to_save })),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    assert_eq!(
        harness.next_save_result().await,
        SaveResult::Saved {
            path: save_path.to_string_lossy().into_owned()
        }
    );
    let written_content = std_fs::read_to_string(save_path).unwrap();
    assert_eq!(written_content, content_to_save);
}

/// The document of a finished generation, as the host keeps it.
fn generated_document(harness: &TestHarness, text: &str) -> GeneratedDocument {
    let mut state = harness.state.lock();
    let token = state.begin_generation();
    state
        .complete_generation(&token, Arc::new(text.to_string()))
        .expect("the generation is the current one")
}

fn save_generated(harness: &TestHarness, document_id: u64) {
    save_file(
        harness.dialog.as_ref(),
        typed(json!({ "kind": "generated", "documentId": document_id })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
}

/// The UI does not send the generated output back: it names the document, and the
/// host writes the copy it has kept, byte for byte.
#[tokio::test]
async fn test_save_file_writes_the_generated_document_the_host_keeps() {
    let mut harness = TestHarness::new();
    let save_path = harness.root_path.join("output.txt");
    harness.dialog.set_save_file(Some(save_path.clone()));
    let text = "# CFC Output\n\nsrc/a.rs\n===FILE-START===\nfn a() {}\r\n\u{fffd}ä\n";
    let document = generated_document(&harness, text);

    save_generated(&harness, document.id());

    assert_eq!(
        harness.next_save_result().await,
        SaveResult::Saved {
            path: save_path.to_string_lossy().into_owned()
        }
    );
    assert_eq!(std_fs::read(&save_path).unwrap(), text.as_bytes());
    // Saving does not use the document up: it can be saved again.
    std_fs::remove_file(&save_path).unwrap();
    save_generated(&harness, document.id());
    assert!(matches!(
        harness.next_save_result().await,
        SaveResult::Saved { .. }
    ));
    assert_eq!(std_fs::read(&save_path).unwrap(), text.as_bytes());
}

/// An edited output is what the UI sends, whatever the host has generated.
#[tokio::test]
async fn test_save_file_writes_edited_content_instead_of_the_generated_document() {
    let mut harness = TestHarness::new();
    let save_path = harness.root_path.join("output.txt");
    harness.dialog.set_save_file(Some(save_path.clone()));
    generated_document(&harness, "generated");

    save_file(
        harness.dialog.as_ref(),
        typed(json!({ "kind": "edited", "content": "generated, then edited\n" })),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    assert!(matches!(
        harness.next_save_result().await,
        SaveResult::Saved { .. }
    ));
    assert_eq!(
        std_fs::read_to_string(&save_path).unwrap(),
        "generated, then edited\n"
    );
}

/// A document the host does not hold is a failed save, reported as such; the user
/// is not asked for a file first.
#[tokio::test]
async fn test_save_file_fails_for_a_document_the_host_does_not_hold() {
    let mut harness = TestHarness::new();
    let save_path = harness.root_path.join("output.txt");
    harness.dialog.set_save_file(Some(save_path.clone()));
    let expect_unavailable = |result: SaveResult| match result {
        SaveResult::Failed { error } => {
            assert!(error.contains("no longer available"), "{error}")
        }
        other => panic!("Expected a failed save, got {other:?}"),
    };

    // Nothing was generated yet.
    save_generated(&harness, 1);
    expect_unavailable(harness.next_save_result().await);

    // The document was replaced by the one of a later generation.
    let first = generated_document(&harness, "first");
    let second = generated_document(&harness, "second");
    save_generated(&harness, first.id());
    expect_unavailable(harness.next_save_result().await);
    assert!(!save_path.exists());

    // The preview was cleared, which empties the editor.
    clear_preview_state(harness.proxy.clone(), harness.state.clone());
    harness.last_state_update().expect("a snapshot");
    save_generated(&harness, second.id());
    expect_unavailable(harness.next_save_result().await);
    assert!(!save_path.exists());
}

/// The output is still on screen after another directory has been opened (the UI
/// clears the editor only when no directory is loaded any more), so it can still
/// be saved: by a drop or the directory dialog, by reopening the same directory,
/// by an import.
#[tokio::test]
async fn test_generated_output_can_be_saved_after_the_directory_was_changed() {
    let mut harness = TestHarness::new();
    let file = harness.create_file("a.txt", "content of a");
    harness.set_initial_files(&["a.txt"]);
    harness.state.lock().selected_files.insert(file);
    let save_path = harness.root_path.join("saved.txt");
    harness.dialog.set_save_file(Some(save_path.clone()));

    generate_preview(harness.proxy.clone(), harness.state.clone());
    let document = loop {
        match harness.get_next_event().await {
            Some(UserEvent::ShowGeneratedContent { document, .. }) => break document,
            Some(_) => {}
            None => panic!("Timed out waiting for the generated content"),
        }
    };

    let other = harness.create_dir("other_project");
    let same = harness.root_path.clone();
    for directory in [other, same] {
        open_dropped_path(
            directory.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        harness
            .wait_for_state(|ui_state| {
                !ui_state.is_scanning && Path::new(&ui_state.current_path) == directory
            })
            .await;

        save_generated(&harness, document.id());
        loop {
            match harness.get_next_event().await {
                Some(UserEvent::SaveResult(result)) => {
                    assert!(
                        matches!(result, SaveResult::Saved { .. }),
                        "after opening {directory:?}: {result:?}"
                    );
                    break;
                }
                Some(UserEvent::StateUpdate(_) | UserEvent::ScanProgress(_)) => {}
                other => panic!("Expected the result of the save, got {other:?}"),
            }
        }
        assert_eq!(std_fs::read_to_string(&save_path).unwrap(), document.text());
        std_fs::remove_file(&save_path).unwrap();
    }

    // A hard reset of the directory (what an import does) keeps it as well.
    harness.state.lock().reset_directory_state();
    save_generated(&harness, document.id());
    assert!(matches!(
        harness.next_save_result().await,
        SaveResult::Saved { .. }
    ));

    // Closing the directory makes the UI clear its editor, which it reports.
    clear_directory(harness.proxy.clone(), harness.state.clone());
    assert!(harness
        .state
        .lock()
        .generated_document(document.id())
        .is_some());
    clear_preview_state(harness.proxy.clone(), harness.state.clone());
    assert!(harness
        .state
        .lock()
        .generated_document(document.id())
        .is_none());
}

/// What is saved is decided when the command starts: the document the UI named
/// is written even if a newer one arrives while the dialog is open.
#[tokio::test]
async fn test_save_file_writes_the_document_it_was_asked_for_when_the_dialog_opened() {
    struct GeneratingDialog {
        state: SharedState,
        path: PathBuf,
    }
    impl DialogService for GeneratingDialog {
        fn pick_directory(&self) -> Option<PathBuf> {
            None
        }
        fn pick_config_to_import(&self) -> Option<PathBuf> {
            None
        }
        fn export_config_path(&self) -> Option<PathBuf> {
            None
        }
        /// While the user is choosing the file, a generation comes to its end.
        fn save_output_file_path(&self, _config: &AppConfig) -> Option<PathBuf> {
            let mut state = self.state.lock();
            let token = state.begin_generation();
            state
                .complete_generation(&token, Arc::new("newer".to_string()))
                .unwrap();
            Some(self.path.clone())
        }
    }

    let mut harness = TestHarness::new();
    let save_path = harness.root_path.join("output.txt");
    let shown = generated_document(&harness, "shown when save was pressed");
    let dialog = GeneratingDialog {
        state: harness.state.clone(),
        path: save_path.clone(),
    };

    save_file(
        &dialog,
        typed(json!({ "kind": "generated", "documentId": shown.id() })),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    assert!(matches!(
        harness.next_save_result().await,
        SaveResult::Saved { .. }
    ));
    assert_eq!(
        std_fs::read_to_string(&save_path).unwrap(),
        "shown when save was pressed"
    );
}

#[test]
fn test_save_file_payload_is_strict() {
    let parse = |value| serde_json::from_value::<SaveFilePayload>(value);

    assert_eq!(
        parse(json!({ "kind": "generated", "documentId": 3 })).unwrap(),
        SaveFilePayload::Generated { document_id: 3 }
    );
    assert_eq!(
        parse(json!({ "kind": "edited", "content": "x" })).unwrap(),
        SaveFilePayload::Edited {
            content: "x".to_string()
        }
    );
    for refused in [
        // The payload of earlier versions: the content itself.
        json!("content"),
        json!({ "kind": "generated" }),
        json!({ "kind": "generated", "documentId": "3" }),
        json!({ "kind": "generated", "documentId": -1 }),
        json!({ "kind": "generated", "documentId": 3, "content": "x" }),
        json!({ "kind": "edited" }),
        json!({ "kind": "edited", "content": "x", "documentId": 3 }),
        json!({ "kind": "unknown", "content": "x" }),
        json!({ "content": "x" }),
        json!(null),
    ] {
        assert!(parse(refused.clone()).is_err(), "{refused}");
    }
}

/// The timestamped default name a generation assigns is a change like any other.
#[tokio::test]
async fn test_generate_preview_persists_the_filename_it_assigns() {
    let (harness, store) = TestHarness::with_config_file();
    harness.state.lock().config.output_filename = "cfc_output_19990101_000000.txt".into();

    generate_preview(harness.proxy.clone(), harness.state.clone());

    let assigned = harness.state.lock().config.output_filename.clone();
    assert_ne!(assigned, "cfc_output_19990101_000000.txt");
    let saved = config_on_disk(&store, |saved| saved.output_filename == assigned).await;
    assert_eq!(saved.output_filename, assigned);
}

#[tokio::test]
async fn test_save_file_sends_error_on_io_failure() {
    let mut harness = TestHarness::new();
    // A cross-platform way to cause an I/O error is to try to write to a directory path.
    let invalid_save_path = harness.root_path.clone();
    harness.dialog.set_save_file(Some(invalid_save_path));

    save_file(
        harness.dialog.as_ref(),
        typed(json!({ "kind": "edited", "content": "some content" })),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    match harness.next_save_result().await {
        // The reason is the error of the operating system, in a field of its own.
        SaveResult::Failed { error } => {
            // Check for common error messages on different platforms.
            assert!(
                error.contains("Is a directory") // Linux, macOS
                    || error.contains("Access is denied") // Windows
                    || error.contains("Permission denied"),
                "{error}"
            );
        }
        other => panic!("Expected a failed save, got {other:?}"),
    }
}

#[tokio::test]
async fn test_generate_preview_preserves_custom_filename() {
    let mut harness = TestHarness::new();
    let custom_filename = "my_special_context.md".to_string();

    // 1. Set a custom filename in the state.
    {
        let mut state = harness.state.lock();
        state.config.output_filename = custom_filename.clone();
    }

    // 2. Run the generate_preview command.
    generate_preview(harness.proxy.clone(), harness.state.clone());

    // 3. Verify the filename in the state has NOT been changed.
    let final_filename = harness.state.lock().config.output_filename.clone();
    assert_eq!(final_filename, custom_filename);

    // Consume events to avoid panics on drop
    let _ = harness.get_next_event().await;
    let _ = harness.get_next_event().await;
}

#[tokio::test]
async fn test_save_file_sends_cancelled_on_dialog_cancel() {
    let mut harness = TestHarness::new();
    harness.dialog.set_save_file(None);
    save_file(
        harness.dialog.as_ref(),
        typed(json!({ "kind": "edited", "content": "content" })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    assert_eq!(harness.next_save_result().await, SaveResult::Cancelled);

    // The same for the host's own document.
    let document = generated_document(&harness, "generated");
    save_generated(&harness, document.id());
    assert_eq!(harness.next_save_result().await, SaveResult::Cancelled);
}

#[tokio::test]
async fn test_generate_preview_creates_timestamped_filename_from_default() {
    let mut harness = TestHarness::new();
    // Use a fixed, old timestamp to make the test deterministic.
    // This ensures the generated filename will always be different.
    let old_default_filename = "cfc_output_20000101_120000.txt".to_string();

    {
        let mut state = harness.state.lock();
        state.config.output_filename = old_default_filename.clone();
    }

    generate_preview(harness.proxy.clone(), harness.state.clone());

    let final_filename = harness.state.lock().config.output_filename.clone();

    assert_ne!(
        final_filename, old_default_filename,
        "A new timestamped filename should have been generated"
    );
    assert!(final_filename.starts_with("cfc_output_"));

    // Clean up events to prevent the test harness from panicking on drop
    let _ = harness.get_next_event().await;
    // The generation task also sends a final StateUpdate when it completes.
    let _ = harness.get_next_event().await;
}

/// Starts a generation with `filename` configured and returns the name that is
/// configured afterwards, which is also the one the UI is told.
async fn output_filename_after_generating(filename: &str) -> String {
    let mut harness = TestHarness::new();
    harness.state.lock().config.output_filename = filename.to_string();

    generate_preview(harness.proxy.clone(), harness.state.clone());

    let configured = harness.state.lock().config.output_filename.clone();
    match harness.get_next_event().await {
        Some(UserEvent::StateUpdate(ui_state)) => {
            assert_eq!(ui_state.config.output_filename, configured)
        }
        other => panic!("Expected a state update, got {other:?}"),
    }
    let _ = harness.get_next_event().await;
    configured
}

/// The static name of a fresh configuration is not a timestamped one: it stays
/// until the user changes it.
#[tokio::test]
async fn test_generate_preview_keeps_the_filename_of_a_fresh_config() {
    let fresh = AppConfig::default().output_filename;
    assert_eq!(fresh, "cfc_output.txt");
    assert_eq!(output_filename_after_generating(&fresh).await, fresh);
}

#[tokio::test]
async fn test_generate_preview_gives_a_timestamped_filename_a_new_timestamp() {
    let old = "cfc_output_20240101_000000.txt";
    let assigned = output_filename_after_generating(old).await;
    assert_ne!(assigned, old);
    let stamp = assigned
        .strip_prefix("cfc_output_")
        .and_then(|rest| rest.strip_suffix(".txt"))
        .unwrap_or_else(|| panic!("Not a timestamped name: {assigned}"));
    assert!(
        chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d_%H%M%S").is_ok(),
        "{assigned}"
    );
}

#[tokio::test]
async fn test_generate_preview_keeps_a_custom_filename() {
    for custom in ["context.txt", "cfc_output.md", "my_cfc_output_1.txt"] {
        assert_eq!(output_filename_after_generating(custom).await, custom);
    }
}

#[test]
fn test_timestamped_output_filenames_are_recognized() {
    assert!(is_timestamped_output_filename(
        "cfc_output_20240101_120000.txt"
    ));
    for other in [
        "cfc_output.txt",
        "context.txt",
        "cfc_output.md",
        "my_cfc_output.txt",
        "",
    ] {
        assert!(!is_timestamped_output_filename(other), "{other}");
    }
}

/// The file size limit of the configuration is the one the generation works with.
#[tokio::test]
async fn test_generate_preview_applies_configured_file_size_limit() {
    let mut harness = TestHarness::new();
    let file = harness.create_file("file.txt", "some content");
    harness.set_initial_files(&["file.txt"]);
    {
        let mut state = harness.state.lock();
        state.selected_files.insert(file);
        state.config.max_file_size_mb = 0;
    }

    generate_preview(harness.proxy.clone(), harness.state.clone());

    let content = loop {
        match harness.get_next_event().await {
            Some(UserEvent::ShowGeneratedContent { document, .. }) => break document,
            Some(_) => {}
            None => panic!("Timed out waiting for the generated content"),
        }
    };
    assert!(content
        .text()
        .contains("[FILE TOO LARGE: 12 bytes - CONTENT SKIPPED]"));
}

/// A project whose listed files weigh `sizes` bytes (as the scan recorded them;
/// nothing of that size is on disk), all of them selected.
fn select_files_of_sizes(harness: &TestHarness, sizes: &[u64]) {
    let mut state = harness.state.lock();
    // Large enough for every file of these tests to count with its full size.
    state.config.max_file_size_mb = 1024;
    let items: Vec<FileItem> = sizes
        .iter()
        .enumerate()
        .map(|(i, size)| FileItem {
            size: *size,
            ..file_item(harness.root_path.join(format!("file{i}.txt")), false)
        })
        .collect();
    state
        .selected_files
        .extend(items.iter().map(|item| item.path.clone()).collect());
    state.set_full_file_list(items.clone());
    state.set_filtered_file_list(items);
}

const MIB: u64 = 1024 * 1024;

/// An output above the limit is refused before anything is read, and refusing
/// changes nothing.
#[tokio::test]
async fn test_generate_preview_refuses_an_output_above_the_limit() {
    let mut harness = TestHarness::new();
    // Each file is within the limit for a single file; together they are not.
    select_files_of_sizes(&harness, &[90 * MIB, 90 * MIB, 30 * MIB]);
    let filename = "cfc_output_20200101_000000.txt".to_string();
    let shown = generated_document(&harness, "the output that is shown");
    harness.state.lock().config.output_filename = filename.clone();

    generate_preview(harness.proxy.clone(), harness.state.clone());

    match harness.get_next_event().await {
        Some(UserEvent::OutputSize(OutputSizeNotice::TooLarge {
            estimated_bytes,
            limit_bytes,
        })) => {
            assert_eq!(limit_bytes, tasks::MAX_OUTPUT_SIZE);
            assert!(
                (210 * MIB..211 * MIB).contains(&estimated_bytes),
                "{estimated_bytes}"
            );
        }
        other => panic!("Expected the refusal, got {other:?}"),
    }
    // The command is answered with a snapshot, which shows no generation.
    match harness.get_next_event().await {
        Some(UserEvent::StateUpdate(ui_state)) => assert!(!ui_state.is_generating),
        other => panic!("Expected a snapshot, got {other:?}"),
    }
    harness
        .expect_no_event_within(QUIET_PERIOD, "A refused generation does nothing else")
        .await;

    let state = harness.state.lock();
    assert!(!state.is_generating());
    assert_eq!(state.config.output_filename, filename);
    assert_eq!(state.generated_document(shown.id()), Some(shown));
}

/// Refusing does not stop a generation that is running.
#[tokio::test]
async fn test_refused_generation_leaves_a_running_generation_alone() {
    let mut harness = TestHarness::new();
    let running = harness.state.lock().begin_generation();
    select_files_of_sizes(&harness, &[201 * MIB]);

    generate_preview(harness.proxy.clone(), harness.state.clone());

    assert!(matches!(
        harness.get_next_event().await,
        Some(UserEvent::OutputSize(OutputSizeNotice::TooLarge { .. }))
    ));
    let state = harness.state.lock();
    assert!(state.is_current_generation(&running));
    assert!(!running.is_cancelled());
}

/// A large output is generated, and announced as large right after the snapshot
/// that shows the generation.
#[tokio::test]
async fn test_generate_preview_announces_a_large_output() {
    let mut harness = TestHarness::new();
    select_files_of_sizes(&harness, &[15 * MIB, 15 * MIB]);

    generate_preview(harness.proxy.clone(), harness.state.clone());

    match harness.get_next_event().await {
        Some(UserEvent::StateUpdate(ui_state)) => assert!(ui_state.is_generating),
        other => panic!("Expected the snapshot of the generation, got {other:?}"),
    }
    match harness.get_next_event().await {
        Some(UserEvent::OutputSize(OutputSizeNotice::Large {
            estimated_bytes,
            limit_bytes,
        })) => {
            assert_eq!(limit_bytes, tasks::LARGE_OUTPUT_SIZE);
            assert!(
                (30 * MIB..31 * MIB).contains(&estimated_bytes),
                "{estimated_bytes}"
            );
        }
        other => panic!("Expected the notice, got {other:?}"),
    }
    // The files do not exist, so each block holds a marker; the generation ends.
    loop {
        match harness.get_next_event().await {
            Some(UserEvent::ShowGeneratedContent { document, .. }) => {
                assert!(document.text().contains("[FILE UNREADABLE"));
                break;
            }
            Some(UserEvent::StateUpdate(_)) => {}
            other => panic!("Expected the generated content, got {other:?}"),
        }
    }
}

/// An output of ordinary size is not commented on, at the thresholds included.
#[tokio::test]
async fn test_generate_preview_says_nothing_about_an_output_of_ordinary_size() {
    let mut harness = TestHarness::new();
    select_files_of_sizes(&harness, &[19 * MIB]);

    generate_preview(harness.proxy.clone(), harness.state.clone());

    loop {
        match harness.get_next_event().await {
            Some(UserEvent::ShowGeneratedContent { .. }) => break,
            Some(UserEvent::StateUpdate(_)) => {}
            other => panic!("Expected only snapshots and the content, got {other:?}"),
        }
    }
}

/// The whole way of an output: generated by the command, shown with an id, saved
/// by naming that id.
#[tokio::test]
async fn test_generated_output_is_saved_as_it_was_shown() {
    let mut harness = TestHarness::new();
    let file = harness.create_file("src/a.txt", "line one\nline two");
    harness.set_initial_files(&["src/a.txt"]);
    harness.state.lock().selected_files.insert(file);
    let save_path = harness.root_path.join("saved.txt");
    harness.dialog.set_save_file(Some(save_path.clone()));

    generate_preview(harness.proxy.clone(), harness.state.clone());
    let (document, token_count) = loop {
        match harness.get_next_event().await {
            Some(UserEvent::ShowGeneratedContent {
                document,
                token_count,
            }) => break (document, token_count),
            Some(_) => {}
            None => panic!("Timed out waiting for the generated content"),
        }
    };
    assert!(token_count > 0);
    assert!(document
        .text()
        .ends_with("a.txt\n===FILE-START===\nline one\nline two\n---FILE-END-----\n\n"));

    save_generated(&harness, document.id());
    loop {
        match harness.get_next_event().await {
            Some(UserEvent::SaveResult(result)) => {
                assert!(matches!(result, SaveResult::Saved { .. }), "{result:?}");
                break;
            }
            Some(UserEvent::StateUpdate(_)) => {}
            other => panic!("Expected the result of the save, got {other:?}"),
        }
    }
    assert_eq!(std_fs::read_to_string(&save_path).unwrap(), document.text());
}

/// Source-like text of about `bytes` bytes, the same for the same `seed`.
fn synthetic_source(seed: usize, bytes: usize) -> String {
    const WORDS: [&str; 16] = [
        "let",
        "value",
        "fn",
        "compute",
        "result",
        "self",
        "index",
        "return",
        "match",
        "Some(item)",
        "buffer.push_str(&line);",
        "// TODO: größer",
        "if",
        "else",
        "0x1f",
        "=>",
    ];
    let mut text = String::with_capacity(bytes + 128);
    let mut n = seed;
    while text.len() < bytes {
        let indent = (n % 4) * 4;
        text.extend(std::iter::repeat_n(' ', indent));
        for _ in 0..(3 + n % 9) {
            n = n
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            text.push_str(WORDS[(n >> 33) % WORDS.len()]);
            text.push(' ');
        }
        text.push_str(&format!("{};\n", n % 1000));
        if n.is_multiple_of(7) {
            text.push('\n');
        }
    }
    text
}

/// The way of a large output through the host, timed: generation and token count,
/// the script for the WebView, the save. Not a test of anything; it prints what
/// `docs/ipc.md` quotes. Run it on a release build, one phase at a time to see
/// the peak memory of each:
///
/// ```sh
/// cargo test --release --lib large_output_benchmark --no-run
/// BENCH_MB=50 BENCH_UNTIL=save /usr/bin/time -l target/release/deps/context_file_concat-<hash> \
///     large_output_benchmark --ignored --nocapture
/// ```
///
/// `BENCH_MB` is the size of the selection (default 50), `BENCH_UNTIL` the last
/// phase to run: `generate`, `show` or `save` (default).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a benchmark, run on request"]
async fn large_output_benchmark() {
    let megabytes: usize = std::env::var("BENCH_MB")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(50);
    let until = std::env::var("BENCH_UNTIL").unwrap_or_else(|_| "save".to_string());
    let mut harness = TestHarness::new();
    let file_size = 20 * 1024;
    let mut items = Vec::new();
    for i in 0..megabytes * 1024 * 1024 / file_size {
        let relative = format!("dir{:03}/file{:05}.rs", i / 100, i);
        let path = harness.create_file(&relative, &synthetic_source(i, file_size));
        if i.is_multiple_of(100) {
            let directory = path.parent().expect("a file has a directory");
            items.push(file_item(directory.to_path_buf(), true));
        }
        items.push(FileItem {
            size: std_fs::metadata(&path).unwrap().len(),
            ..file_item(path, false)
        });
    }
    {
        let mut state = harness.state.lock();
        state
            .selected_files
            .extend(items.iter().map(|item| item.path.clone()).collect());
        state.set_full_file_list(items.clone());
        state.set_filtered_file_list(items);
        state.is_fully_scanned = true;
    }
    let save_path = harness.root_path.join("output.txt");
    harness.dialog.set_save_file(Some(save_path.clone()));

    let started = std::time::Instant::now();
    generate_preview(harness.proxy.clone(), harness.state.clone());
    let event = loop {
        match harness.event_rx.recv().await.expect("the generation ends") {
            event @ UserEvent::ShowGeneratedContent { .. } => break event,
            UserEvent::ShowError(error) => panic!("{error}"),
            _ => {}
        }
    };
    let generated = started.elapsed();
    eprintln!("BENCH generated and counted: {generated:?}");
    if until == "generate" {
        return;
    }

    let script = crate::app::script_for(&event).unwrap();
    let shown = started.elapsed();
    let UserEvent::ShowGeneratedContent {
        document,
        token_count,
    } = event
    else {
        unreachable!()
    };
    eprintln!(
        "BENCH script built: +{:?} ({} bytes) => host time to show {shown:?}",
        shown - generated,
        script.len()
    );
    eprintln!(
        "BENCH document: {} bytes, {token_count} tokens",
        document.text().len()
    );
    drop(script);
    if until == "show" {
        return;
    }

    // What the UI posts for an output it has not changed, as it arrives.
    let message = serde_json::to_string(&json!({
        "seq": 1,
        "command": "saveFile",
        "payload": { "kind": "generated", "documentId": document.id() },
    }))
    .unwrap();
    let bytes = document.text().len() as u64;
    drop(document);
    let saving = std::time::Instant::now();
    let crate::app::ipc::IpcMessage::Command {
        command: crate::app::ipc::Command::SaveFile(payload),
        ..
    } = crate::app::ipc::IpcMessage::parse(&message)
    else {
        panic!("the message is a saveFile command");
    };
    save_file(
        harness.dialog.as_ref(),
        payload,
        harness.proxy.clone(),
        harness.state.clone(),
    );
    eprintln!(
        "BENCH save: {:?} (message {} bytes, written {} bytes)",
        saving.elapsed(),
        message.len(),
        std_fs::metadata(&save_path).unwrap().len()
    );
    assert_eq!(std_fs::metadata(&save_path).unwrap().len(), bytes);
}
