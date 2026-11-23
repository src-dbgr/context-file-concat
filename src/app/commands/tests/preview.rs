use super::*;

#[tokio::test]
async fn test_load_file_preview_sends_search_term() {
    let mut harness = TestHarness::new();
    let file_path = harness.create_file("preview.txt", "content with magic_word");
    harness.set_initial_files(&["preview.txt"]);
    let search_term = "magic_word";
    {
        let mut state = harness.state.lock();
        state.content_search_query = search_term.to_string();
    }
    let payload = json!(file_path);

    load_file_preview(typed(payload), harness.proxy.clone(), harness.state.clone());

    // The command answers with the state (the file is highlighted) right away ...
    let Some(UserEvent::StateUpdate(_)) = harness.event_rx.try_recv().ok() else {
        panic!("Expected the synchronous state update");
    };
    assert_eq!(
        harness.state.lock().previewed_file_path,
        Some(file_path.clone())
    );
    // ... and the content follows once the file has been read in the background.
    match harness.get_next_event().await {
        Some(UserEvent::ShowFilePreview {
            search_term: term,
            content,
            path,
            ..
        }) => {
            assert_eq!(term, Some(search_term.to_string()));
            assert!(content.contains("content with magic_word"));
            assert_eq!(path, file_path);
        }
        other => panic!("Expected the ShowFilePreview event, got {other:?}"),
    }
}

/// A preview that is closed before its file has been read shows nothing.
#[tokio::test]
async fn test_clear_preview_state_discards_preview_being_loaded() {
    let mut harness = TestHarness::new();
    let file_path = harness.create_file("preview.txt", "content");
    harness.set_initial_files(&["preview.txt"]);

    load_file_preview(
        typed(json!(file_path)),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    // On the single-threaded test runtime the read has not been started yet.
    clear_preview_state(harness.proxy.clone(), harness.state.clone());
    let _ = harness.last_state_update();

    harness
        .expect_no_event_within(QUIET_PERIOD, "A closed preview must not show content")
        .await;
    assert_eq!(harness.state.lock().previewed_file_path, None);
}

#[tokio::test]
async fn test_clear_preview_state() {
    let harness = TestHarness::new();
    let file_path = harness.create_file("file.txt", "content");
    {
        let mut state = harness.state.lock();
        state.previewed_file_path = Some(file_path);
    }
    clear_preview_state(harness.proxy.clone(), harness.state.clone());
    let state = harness.state.lock();
    assert!(state.previewed_file_path.is_none());
}

// --- loadFilePreview: only entries of the loaded list ---

/// Asks for a preview of `path` and returns the error it is refused with. Fails
/// the test if the request is not refused: an error, the unchanged state, nothing
/// else, and no file read.
async fn refused_preview(harness: &mut TestHarness, path: &Path) -> String {
    let previewed_before = harness.state.lock().previewed_file_path.clone();

    load_file_preview(
        typed(json!(path)),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    let Some(UserEvent::ShowError(message)) = harness.event_rx.try_recv().ok() else {
        panic!("Expected the preview of {path:?} to be refused with an error");
    };
    let Some(UserEvent::StateUpdate(_)) = harness.event_rx.try_recv().ok() else {
        panic!("Expected a snapshot after the refusal of {path:?}");
    };
    harness
        .expect_no_event_within(QUIET_PERIOD, "A refused preview must not show content")
        .await;
    assert_eq!(
        harness.state.lock().previewed_file_path,
        previewed_before,
        "{path:?}"
    );
    message
}

#[tokio::test]
async fn test_load_file_preview_refuses_a_path_outside_the_loaded_directory() {
    let mut harness = TestHarness::new();
    harness.create_file("listed.txt", "listed");
    harness.set_initial_files(&["listed.txt"]);
    let outside_dir = tempfile::tempdir().unwrap();
    let outside = outside_dir.path().join("secret.txt");
    std_fs::write(&outside, "top secret").unwrap();

    let message = refused_preview(&mut harness, &outside).await;

    assert!(message.contains("not part of the loaded directory"));
    assert!(message.contains("secret.txt"));
    assert!(!message.contains("top secret"));
}

/// Inside the directory is not enough: the file has to be one the scan listed
/// (it may be ignored, too large, or created since).
#[tokio::test]
async fn test_load_file_preview_refuses_an_unlisted_file_of_the_loaded_directory() {
    let mut harness = TestHarness::new();
    harness.create_file("listed.txt", "listed");
    let unlisted = harness.create_file(".env", "API_KEY=secret");
    harness.set_initial_files(&["listed.txt"]);

    refused_preview(&mut harness, &unlisted).await;
}

/// The requested path is compared with the list, not resolved: neither `..` nor a
/// symbolic link leads from a listed directory to something else.
#[tokio::test]
async fn test_load_file_preview_does_not_resolve_the_requested_path() {
    let mut harness = TestHarness::new();
    harness.create_dir("sub");
    let listed = harness.create_file("sub/listed.txt", "listed");
    harness.set_initial_files(&["sub", "sub/listed.txt"]);
    let outside_dir = tempfile::tempdir().unwrap();
    let outside = outside_dir.path().join("secret.txt");
    std_fs::write(&outside, "top secret").unwrap();

    // Out of the directory by `..`, also behind a listed path.
    let outside_name = outside_dir.path().file_name().unwrap();
    let by_parent = harness
        .root_path
        .join("sub/../..")
        .join(outside_name)
        .join("secret.txt");
    refused_preview(&mut harness, &by_parent).await;
    refused_preview(&mut harness, &listed.join("../../../etc/hosts")).await;
    // The listed file by another spelling is not the listed path either.
    let respelled = harness.root_path.join("sub/../sub/listed.txt");
    refused_preview(&mut harness, &respelled).await;

    // A link inside the directory that the scan has not listed.
    #[cfg(unix)]
    {
        let link = harness.root_path.join("sub/link.txt");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        refused_preview(&mut harness, &link).await;
    }
}

#[tokio::test]
async fn test_load_file_preview_refuses_everything_when_no_directory_is_loaded() {
    let mut harness = TestHarness::new();
    let file = harness.create_file("file.txt", "content");
    harness.state.lock().reset_directory_state();
    let _ = harness.last_state_update();

    refused_preview(&mut harness, &file).await;
    refused_preview(&mut harness, Path::new("")).await;
    refused_preview(&mut harness, Path::new("/etc/hosts")).await;
}

/// A refused request leaves the preview that is shown alone.
#[tokio::test]
async fn test_refused_preview_keeps_the_current_preview() {
    let mut harness = TestHarness::new();
    let listed = harness.create_file("listed.txt", "listed content");
    let unlisted = harness.create_file("unlisted.txt", "unlisted content");
    harness.set_initial_files(&["listed.txt"]);
    load_file_preview(
        typed(json!(listed)),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let _ = harness.event_rx.try_recv();
    assert!(matches!(
        harness.get_next_event().await,
        Some(UserEvent::ShowFilePreview { .. })
    ));

    refused_preview(&mut harness, &unlisted).await;

    assert_eq!(harness.state.lock().previewed_file_path, Some(listed));
}

#[test]
fn test_is_listed_knows_exactly_the_entries_of_the_list() {
    let root = PathBuf::from("/project");
    let mut state = AppState::default();
    state.current_path = "/project".to_string();
    state.set_full_file_list(vec![
        file_item(root.join("src"), true),
        file_item(root.join("src/main.rs"), false),
    ]);

    for (path, expected) in [
        ("/project/src/main.rs", true),
        ("/project/src", true),
        ("/project", false),
        ("/project/src/other.rs", false),
        ("/project/src/main.rs/..", false),
        ("/project/src/../src/main.rs", false),
        ("/project/src/main.rs.bak", false),
        ("src/main.rs", false),
        ("/etc/passwd", false),
        ("", false),
    ] {
        assert_eq!(is_listed(&state, Path::new(path)), expected, "{path:?}");
    }
}

#[tokio::test]
async fn test_load_file_preview_sends_error_on_io_failure() {
    let mut harness = TestHarness::new();
    // Listed by the scan, deleted since.
    let non_existent_path = harness.root_path.join("non_existent_file.txt");
    harness.set_initial_files(&["non_existent_file.txt"]);
    let payload = json!(non_existent_path);
    load_file_preview(typed(payload), harness.proxy.clone(), harness.state.clone());

    let mut error_event_found = false;
    // The command sends two events: an error and a state update. We check for the error.
    for _ in 0..2 {
        if let Some(UserEvent::ShowError(msg)) = harness.get_next_event().await {
            assert!(msg.contains("I/O error"));
            error_event_found = true;
        }
    }
    assert!(error_event_found, "Expected a ShowError event");
}
