use super::*;

/// The generated output the host keeps is the one the editor shows. A file
/// preview takes its place there, so the host gives its copy up; a preview that
/// fails leaves the editor, and so the document, as it is.
#[tokio::test]
async fn file_preview_takes_the_place_of_the_generated_document() {
    let mut harness = TestHarness::new();
    let file = harness.root_path.join("a.txt");
    let generate = |harness: &TestHarness| {
        let mut state = harness.state.lock();
        let token = state.begin_generation();
        state
            .complete_generation(&token, Arc::new("output".to_string()))
            .unwrap()
            .id()
    };

    let id = generate(&harness);
    launch_file_preview(
        file.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        |_| Err(CoreError::Cancelled),
    )
    .await
    .unwrap();
    assert!(harness.state.lock().generated_document(id).is_some());

    launch_file_preview(file, harness.proxy.clone(), harness.state.clone(), |_| {
        Ok("file content".to_string())
    })
    .await
    .unwrap();
    assert!(harness.state.lock().generated_document(id).is_none());
    assert!(harness
        .drain_events()
        .iter()
        .any(|event| matches!(event, UserEvent::ShowFilePreview { .. })));
}

// --- File preview ---

/// A way of reading a file that holds its result back until the test releases it.
/// Reads run on the blocking thread pool, hence the std channels.
fn gated_read(
    content: &str,
) -> (
    impl FnOnce(&Path) -> Result<String, CoreError> + Send + 'static,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let content = content.to_string();
    let read = move |_: &Path| {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
        Ok(content)
    };
    (read, started_rx, release_tx)
}

fn shown_previews(events: &[UserEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            UserEvent::ShowFilePreview { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

/// The request is recorded and answered while the file is still being read; the
/// caller (the command lane) does not wait for the disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_preview_is_announced_before_the_file_has_been_read() {
    let mut harness = TestHarness::new();
    let path = harness.root_path.join("big.txt");
    let (read, read_started, release_read) = gated_read("content of big.txt");

    let preview = launch_file_preview(
        path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        read,
    );
    read_started
        .recv_timeout(EVENT_TIMEOUT)
        .expect("The read was not started");

    // The read is blocked, yet the request has been dealt with.
    assert_eq!(harness.state.lock().previewed_file_path, Some(path.clone()));
    let events = harness.drain_events();
    assert!(matches!(events.as_slice(), [UserEvent::StateUpdate(_)]));

    release_read.send(()).unwrap();
    preview.await.unwrap();
    let events = harness.drain_events();
    match events.as_slice() {
        [UserEvent::ShowFilePreview {
            content,
            path: shown,
            search_term,
            ..
        }] => {
            assert_eq!(content, "content of big.txt");
            assert_eq!(shown, &path);
            assert_eq!(search_term, &None);
        }
        other => panic!("Expected exactly the preview, got {other:?}"),
    }
}

/// The read of a file the user has already left finishes last. Its content must
/// not replace the preview the user is looking at.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_preview_does_not_replace_a_newer_one() {
    let mut harness = TestHarness::new();
    let slow_path = harness.root_path.join("slow.txt");
    let fast_path = harness.root_path.join("fast.txt");
    let (slow_read, slow_started, release_slow) = gated_read("slow");

    let slow = launch_file_preview(
        slow_path,
        harness.proxy.clone(),
        harness.state.clone(),
        slow_read,
    );
    slow_started
        .recv_timeout(EVENT_TIMEOUT)
        .expect("The first read was not started");
    launch_file_preview(
        fast_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        |_: &Path| Ok("fast".to_string()),
    )
    .await
    .unwrap();
    release_slow.send(()).unwrap();
    slow.await.unwrap();

    assert_eq!(shown_previews(&harness.drain_events()), ["fast"]);
    assert_eq!(harness.state.lock().previewed_file_path, Some(fast_path));
}

/// Closing the preview (or replacing it by the generated output, or leaving the
/// directory) while the file is being read: nothing is shown afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_closed_while_reading_shows_nothing() {
    let mut harness = TestHarness::new();
    let (read, read_started, release_read) = gated_read("too late");

    let preview = launch_file_preview(
        harness.root_path.join("a.txt"),
        harness.proxy.clone(),
        harness.state.clone(),
        read,
    );
    read_started
        .recv_timeout(EVENT_TIMEOUT)
        .expect("The read was not started");
    harness.state.lock().clear_preview();
    let _ = harness.drain_events();
    release_read.send(()).unwrap();
    preview.await.unwrap();

    harness.assert_no_events("A closed preview must not show anything");
    assert_eq!(harness.state.lock().previewed_file_path, None);
}

/// A read that fails or panics is reported; it does not take the task down silently.
#[tokio::test]
async fn failed_preview_read_is_reported() {
    let mut harness = TestHarness::new();
    let path = harness.root_path.join("gone.txt");

    launch_file_preview(
        path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        |path: &Path| Err(CoreError::Io("denied".to_string(), path.to_path_buf())),
    )
    .await
    .unwrap();
    launch_file_preview(
        path,
        harness.proxy.clone(),
        harness.state.clone(),
        |_: &Path| panic!("read failure injected by the test"),
    )
    .await
    .unwrap();

    let errors: Vec<String> = harness
        .drain_events()
        .into_iter()
        .filter_map(|event| match event {
            UserEvent::ShowError(message) => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].contains("denied"), "{errors:?}");
    assert!(errors[1].contains("gone.txt"), "{errors:?}");
}
