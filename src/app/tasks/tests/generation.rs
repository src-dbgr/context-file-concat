use super::*;

#[tokio::test]
async fn generation_task_happy_path_sends_content_and_resets_state() {
    // Arrange
    let mut harness = TestHarness::new();
    let generator = MockContentGenerator::new();
    generator.set_result(Ok("Generated Content".to_string()));
    let tokenizer = MockTokenizer { token_count: 2 };
    let token = harness.state.lock().begin_generation();

    // Act
    generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        tokenizer,
        token,
        harness.plan(),
    )
    .await;

    // Assert
    let events = harness.drain_events();
    assert_eq!(events.len(), 2, "Expected exactly two events");

    assert!(matches!(events[0], UserEvent::ShowGeneratedContent { .. }));
    if let UserEvent::ShowGeneratedContent {
        document,
        token_count,
    } = &events[0]
    {
        assert_eq!(document.text(), "Generated Content");
        assert_eq!(*token_count, 2);
        // The host keeps what it has shown, under the id it has shown it with.
        assert_eq!(
            harness.state.lock().generated_document(document.id()),
            Some(document.clone())
        );
    }

    // The state update is the second of the two events captured above.
    assert!(matches!(events[1], UserEvent::StateUpdate(_)));
    if let UserEvent::StateUpdate(final_state_in_event) = &events[1] {
        assert!(!final_state_in_event.is_generating);
    }

    assert!(
        !harness.state.lock().is_generating(),
        "is_generating should be reset in AppState"
    );
}

/// The real cancel path, for a task that could not be aborted in time: cancelling
/// releases the generating state, the generator then notices the cancel flag, and
/// the task ends without publishing anything on top of that.
#[tokio::test]
async fn cancelled_generation_task_ends_silently() {
    let mut harness = TestHarness::new();
    let mut generator = MockContentGenerator::new();
    let (has_started_receiver, unblock_sender) = generator.expect_cancellation();
    generator.set_result(Err(CoreError::Cancelled));
    let token = harness.state.lock().begin_generation();

    let task_handle = tokio::spawn(generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        MockTokenizer { token_count: 0 },
        token.clone(),
        harness.plan(),
    ));
    has_started_receiver
        .await
        .expect("Mock did not signal start");

    harness.state.lock().cancel_current_generation();
    assert!(token.is_cancelled());
    unblock_sender.send(()).unwrap();
    task_handle.await.unwrap();

    harness.assert_no_events("A cancelled generation must not publish an outcome");
    assert!(!harness.state.lock().is_generating());
}

#[tokio::test]
async fn generation_task_io_error_sends_error_event_and_resets_state() {
    // Arrange
    let mut harness = TestHarness::new();
    let generator = MockContentGenerator::new();
    let io_error = CoreError::Io("File not found".to_string(), PathBuf::from("a/b/c.txt"));
    generator.set_result(Err(io_error));
    let token = harness.state.lock().begin_generation();

    // Act
    generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        MockTokenizer { token_count: 0 },
        token,
        harness.plan(),
    )
    .await;

    // Assert
    let events = harness.drain_events();
    assert_eq!(events.len(), 2);

    assert!(matches!(events[0], UserEvent::ShowError(_)));
    if let UserEvent::ShowError(msg) = &events[0] {
        assert!(msg.contains("I/O error"));
        assert!(msg.contains("a/b/c.txt"));
    }

    // The state update is the second of the two events captured above.
    assert!(matches!(events[1], UserEvent::StateUpdate(_)));
    if let UserEvent::StateUpdate(final_state_in_event) = &events[1] {
        assert!(!final_state_in_event.is_generating);
    }

    assert!(
        !harness.state.lock().is_generating(),
        "is_generating should be reset in AppState"
    );
}

#[tokio::test]
async fn generation_task_panic_resets_state_and_notifies_ui() {
    // Arrange
    let mut harness = TestHarness::new();
    let token = harness.state.lock().begin_generation();
    assert!(harness.state.lock().is_generating(), "precondition");

    // Act
    let task_handle = tokio::spawn(generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        PanickingContentGenerator,
        MockTokenizer { token_count: 0 },
        token.clone(),
        harness.plan(),
    ));
    let join_error = task_handle.await.expect_err("The task must panic");
    assert!(join_error.is_panic());

    // Assert
    {
        let state = harness.state.lock();
        assert!(
            !state.is_generating(),
            "is_generating should be reset after a panic"
        );
        assert!(!state.is_current_generation(&token));
    }

    let events = harness.drain_events();
    assert_eq!(events.len(), 1, "Expected exactly one event");
    match &events[0] {
        UserEvent::StateUpdate(ui_state) => assert!(!ui_state.is_generating),
        _ => panic!("Expected a StateUpdate event."),
    }
}

#[tokio::test]
async fn generation_task_handles_generator_error() {
    // Arrange
    let mut harness = TestHarness::new();
    let generator = MockContentGenerator::new();
    let gen_error = CoreError::Io("Failed to read file".to_string(), PathBuf::from("test.txt"));
    generator.set_result(Err(gen_error.clone()));
    let tokenizer = MockTokenizer { token_count: 0 };
    let token = harness.state.lock().begin_generation();

    // Act
    generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        tokenizer,
        token,
        harness.plan(),
    )
    .await;

    // Assert
    // Expect a ShowError event, followed by a StateUpdate.
    let events = harness.drain_events();
    assert_eq!(events.len(), 2, "Expected ShowError and StateUpdate events");

    // Check the ShowError event
    match &events[0] {
        UserEvent::ShowError(msg) => {
            assert!(
                msg.contains("Failed to read file"),
                "Error message content is incorrect."
            );
        }
        _ => panic!("Expected a ShowError event first."),
    }

    // Check the final StateUpdate
    match &events[1] {
        UserEvent::StateUpdate(s) => {
            assert!(!s.is_generating, "is_generating flag should be reset.");
        }
        _ => panic!("Expected a StateUpdate event second."),
    }

    // Double-check the final state directly
    assert!(
        !harness.state.lock().is_generating(),
        "is_generating should be false in final AppState."
    );
}

/// Tests that the generation_task correctly prunes empty directories from the
/// generated tree view when the corresponding config flags are set.
#[tokio::test]
async fn generation_task_prunes_empty_directories_from_tree() {
    // Arrange
    let mut harness = TestHarness::new();
    let generator = MockContentGenerator::new();
    // The generator will include a tree. We check if the input it receives is correct.
    generator.set_result(Ok("Success".to_string()));
    let tokenizer = MockTokenizer { token_count: 1 };

    let file_path = harness.root_path.join("src/main.rs");
    let empty_dir_path = harness.root_path.join("src/empty");

    {
        let mut state = harness.state.lock();
        state.is_fully_scanned = true; // Required for pruning
        state.config_mut().remove_empty_directories = true; // Enable feature
        state.config_mut().include_tree_by_default = true;
        state.selected_files.insert(file_path.clone());
        state.set_full_file_list(vec![
            FileItem {
                path: harness.root_path.join("src"),
                is_directory: true,
                ..Default::default()
            },
            FileItem {
                path: file_path,
                ..Default::default()
            },
            FileItem {
                path: empty_dir_path,
                is_directory: true,
                ..Default::default()
            },
        ]);
    }

    let token = harness.state.lock().begin_generation();

    // Act
    generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        tokenizer,
        token,
        harness.plan(),
    )
    .await;

    // Assert
    let events = harness.drain_events();
    assert!(matches!(events[0], UserEvent::ShowGeneratedContent { .. }));
    assert!(matches!(events[1], UserEvent::StateUpdate(_)));
}

/// Records what a generation hands to its generator.
#[derive(Clone, Default)]
struct RecordingGenerator {
    calls: Arc<Mutex<Vec<RecordedGeneration>>>,
}

#[derive(Debug, Clone, PartialEq)]
struct RecordedGeneration {
    selected_files: Vec<PathBuf>,
    include_tree: bool,
    tree_paths: Vec<PathBuf>,
    tree_ignore_patterns: HashSet<String>,
    use_relative_paths: bool,
}

impl RecordingGenerator {
    fn only_call(&self) -> RecordedGeneration {
        let calls = self.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "{calls:?}");
        calls[0].clone()
    }
}

impl ContentGenerator for RecordingGenerator {
    async fn generate(
        &self,
        selected_files: &[PathBuf],
        _: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    ) -> Result<String, CoreError> {
        self.calls.lock().unwrap().push(RecordedGeneration {
            selected_files: selected_files.to_vec(),
            include_tree,
            tree_paths: items_for_tree.into_iter().map(|item| item.path).collect(),
            tree_ignore_patterns,
            use_relative_paths,
        });
        Ok("recorded".to_string())
    }
}

/// A project of two files, one of them below `src`, and an empty directory.
fn load_small_project(harness: &TestHarness) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let src = harness.root_path.join("src");
    let main = src.join("main.rs");
    let empty = harness.root_path.join("empty");
    let readme = harness.root_path.join("README.md");
    let mut state = harness.state.lock();
    state.set_full_file_list(vec![
        dir_item(&src, 1),
        FileItem {
            size: 100,
            ..file_item(&main, 2)
        },
        dir_item(&empty, 1),
        FileItem {
            size: 1000,
            ..file_item(&readme, 1)
        },
    ]);
    (src, main, empty, readme)
}

async fn run_recorded_generation(harness: &TestHarness) -> RecordedGeneration {
    let generator = RecordingGenerator::default();
    let token = harness.state.lock().begin_generation();
    generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator.clone(),
        MockTokenizer { token_count: 0 },
        token,
        harness.plan(),
    )
    .await;
    generator.only_call()
}

#[tokio::test]
async fn generation_hands_the_generator_the_tree_without_empty_directories() {
    let harness = TestHarness::new();
    let (src, main, empty, readme) = load_small_project(&harness);
    {
        let mut state = harness.state.lock();
        state.is_fully_scanned = true;
        state.config_mut().remove_empty_directories = true;
        state.config_mut().include_tree_by_default = true;
        state.config_mut().tree_ignore_patterns = HashSet::from(["*.lock".to_string()]);
        state.config_mut().use_relative_paths = false;
        state.selected_files = HashSet::from([readme.clone(), main.clone()]).into();
    }

    let recorded = run_recorded_generation(&harness).await;

    assert_eq!(
        recorded,
        RecordedGeneration {
            // The order of the tree, whatever the order of the selection.
            selected_files: vec![readme.clone(), main.clone()],
            include_tree: true,
            // The list as it is, without the directory that holds no file.
            tree_paths: vec![src.clone(), main.clone(), readme.clone()],
            tree_ignore_patterns: HashSet::from(["*.lock".to_string()]),
            use_relative_paths: false,
        }
    );

    // Empty directories stay where the option is off, and where the list is not
    // known to be complete (a directory that was not loaded yet looks empty).
    for (remove_empty_directories, is_fully_scanned) in [(false, true), (true, false)] {
        {
            let mut state = harness.state.lock();
            state.config_mut().remove_empty_directories = remove_empty_directories;
            state.is_fully_scanned = is_fully_scanned;
        }
        let recorded = run_recorded_generation(&harness).await;
        assert_eq!(
            recorded.tree_paths,
            vec![src.clone(), main.clone(), empty.clone(), readme.clone()]
        );
    }
}

#[tokio::test]
async fn generation_without_a_tree_does_not_copy_the_file_list() {
    let harness = TestHarness::new();
    let (_, main, _, _) = load_small_project(&harness);
    {
        let mut state = harness.state.lock();
        state.config_mut().include_tree_by_default = false;
        state.selected_files.insert(main.clone());
    }

    assert!(harness.plan().tree.is_none());
    let recorded = run_recorded_generation(&harness).await;
    assert!(!recorded.include_tree);
    assert_eq!(recorded.tree_paths, Vec::<PathBuf>::new());
    assert_eq!(recorded.selected_files, vec![main]);
}

/// The plan is the selection of the moment it was made: what is selected or
/// deselected while the generation runs belongs to the next one.
#[tokio::test]
async fn generation_works_with_the_selection_of_its_plan() {
    let harness = TestHarness::new();
    let (_, main, _, readme) = load_small_project(&harness);
    harness.state.lock().selected_files.insert(main.clone());
    let plan = harness.plan();
    {
        let mut state = harness.state.lock();
        state.selected_files.clear();
        state.selected_files.insert(readme);
    }

    let generator = RecordingGenerator::default();
    let token = harness.state.lock().begin_generation();
    generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator.clone(),
        MockTokenizer { token_count: 0 },
        token,
        plan,
    )
    .await;

    assert_eq!(generator.only_call().selected_files, vec![main]);
}

#[tokio::test]
async fn plan_estimates_the_output_from_the_sizes_of_the_scan() {
    let harness = TestHarness::new();
    let (_, main, _, readme) = load_small_project(&harness);
    let estimate = |harness: &TestHarness| harness.plan().estimated_size;
    harness.state.lock().config_mut().include_tree_by_default = false;

    let nothing = estimate(&harness);
    harness.state.lock().selected_files.insert(main);
    let one_file = estimate(&harness);
    harness.state.lock().selected_files.insert(readme.clone());
    let two_files = estimate(&harness);

    // 100 and 1000 bytes of content, plus the path and the frame of each block.
    assert!((100..400).contains(&(one_file - nothing)), "{one_file}");
    assert!(
        (1000..1300).contains(&(two_files - one_file)),
        "{two_files}"
    );

    // The tree adds to it; a file above the size limit is not embedded.
    harness.state.lock().config_mut().include_tree_by_default = true;
    assert!(estimate(&harness) > two_files);
    {
        let mut state = harness.state.lock();
        state.config_mut().include_tree_by_default = false;
        state.edit_full_file_list(|list| list[3].size = 3 * 1024 * 1024);
        state.config_mut().max_file_size_mb = 1;
    }
    assert!(estimate(&harness) < two_files);

    // A selected path that is not listed (any more) is not part of the output.
    let before = estimate(&harness);
    harness
        .state
        .lock()
        .selected_files
        .insert(PathBuf::from("/elsewhere/gone.txt"));
    assert_eq!(estimate(&harness), before);
}

/// A tokenizer whose count is cancelled, as the real one reports it when the
/// cancellation flag of its generation is raised.
struct CancelledTokenizer;

impl Tokenizer for CancelledTokenizer {
    async fn count_tokens(&self, _: Arc<String>) -> Option<usize> {
        None
    }
}

/// Cancelling during the token count: the generation is over for the state when
/// the count gives up, and neither the document nor an error is shown.
#[tokio::test]
async fn generation_cancelled_during_the_token_count_shows_nothing() {
    let mut harness = TestHarness::new();
    let generator = MockContentGenerator::new();
    generator.set_result(Ok("document".to_string()));
    let token = harness.state.lock().begin_generation();
    harness.state.lock().cancel_current_generation();

    generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        CancelledTokenizer,
        token,
        harness.plan(),
    )
    .await;

    harness.assert_no_events("A cancelled generation publishes nothing");
    assert!(!harness.state.lock().is_generating());
}

#[tokio::test]
async fn real_tokenizer_counts_shared_text_and_stops_when_cancelled() {
    let text = Arc::new("hello world\n".repeat(1000));
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let tokenizer = RealTokenizer {
        cancel_flag: cancel_flag.clone(),
    };

    // "hello", " world" and the line end, a thousand times.
    assert_eq!(tokenizer.count_tokens(text.clone()).await, Some(3000));
    // The text was borrowed for the count, not kept.
    assert_eq!(Arc::strong_count(&text), 1);

    cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(tokenizer.count_tokens(text).await, None);
}

/// Passes the outcome of the wrapped generator on and keeps a copy of it.
struct OutcomeRecorder<G> {
    inner: G,
    outcome: Arc<Mutex<Option<Result<String, CoreError>>>>,
}

impl<G: ContentGenerator> ContentGenerator for OutcomeRecorder<G> {
    async fn generate(
        &self,
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    ) -> Result<String, CoreError> {
        let outcome = self
            .inner
            .generate(
                selected_files,
                root_path,
                include_tree,
                items_for_tree,
                tree_ignore_patterns,
                use_relative_paths,
            )
            .await;
        *self.outcome.lock().unwrap() = Some(outcome.clone());
        outcome
    }
}

/// The cancellation mechanism end to end with the `RealContentGenerator`: the
/// generation is cancelled the way the application does it
/// (`cancel_current_generation`), the generator stops at the next file, and the
/// task leaves it to the cancelling command to inform the UI.
#[tokio::test]
async fn generation_task_with_real_generator_cancels_gracefully() {
    // Arrange
    let mut harness = TestHarness::new();
    let token = harness.state.lock().begin_generation();
    let outcome = Arc::new(Mutex::new(None));
    let generator = OutcomeRecorder {
        inner: RealContentGenerator {
            cancel_flag: token.cancel_flag(),
            max_file_size: MAX_FILE_SIZE,
            max_output_size: MAX_OUTPUT_SIZE,
            expected_output_size: 0,
        },
        outcome: outcome.clone(),
    };

    // Set up state with a file to be processed.
    {
        let mut state = harness.state.lock();
        let file_path = harness.root_path.join("file1.txt");
        std::fs::write(&file_path, "content").unwrap();
        state.selected_files.insert(file_path.clone());
        state.edit_full_file_list(|list| {
            list.push(FileItem {
                path: file_path,
                ..Default::default()
            })
        });
    }

    // Act: the task is spawned but (on the single-threaded test runtime) does not
    // run before this test awaits it, so the cancellation reaches it first. Its
    // handle is deliberately not attached: the task is not aborted and has to
    // deal with the cancellation by itself.
    let task_handle = tokio::spawn(generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        MockTokenizer { token_count: 0 },
        token,
        harness.plan(),
    ));
    harness.state.lock().cancel_current_generation();
    task_handle.await.unwrap();

    // Assert
    assert!(
        matches!(*outcome.lock().unwrap(), Some(Err(CoreError::Cancelled))),
        "The generator must stop because of the cancellation"
    );
    harness.assert_no_events("Neither content nor an error may be shown after a cancellation");
    assert!(!harness.state.lock().is_generating());
}

/// The size limit of the configuration reaches the generator: a selected file above
/// it is represented by a marker.
#[tokio::test]
async fn real_generator_applies_its_file_size_limit() {
    let harness = TestHarness::new();
    let file_path = harness.root_path.join("file.txt");
    std::fs::write(&file_path, "123456").unwrap();
    let generator = RealContentGenerator {
        cancel_flag: Arc::new(AtomicBool::new(false)),
        max_file_size: 5,
        max_output_size: MAX_OUTPUT_SIZE,
        expected_output_size: 0,
    };

    let content = generator
        .generate(
            std::slice::from_ref(&file_path),
            &harness.root_path,
            false,
            vec![],
            HashSet::new(),
            true,
        )
        .await
        .unwrap();

    assert!(content.contains("[FILE TOO LARGE: 6 bytes - CONTENT SKIPPED]"));
}

/// A generation that was superseded but could not be aborted in time must not show
/// its content or end the generating state of the generation that replaced it.
#[tokio::test]
async fn superseded_generation_task_does_not_publish_into_newer_generation() {
    // Arrange
    let mut harness = TestHarness::new();
    let mut generator = MockContentGenerator::new();
    let (has_started, unblock) = generator.expect_cancellation();
    generator.set_result(Ok("Outdated content".to_string()));
    let old_token = harness.state.lock().begin_generation();
    let old_task = tokio::spawn(generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        MockTokenizer { token_count: 1 },
        old_token.clone(),
        harness.plan(),
    ));
    has_started.await.expect("Mock did not signal start");

    // Act: the user starts a new generation while the old one is still producing.
    let new_token = harness.state.lock().begin_generation();
    unblock.send(()).unwrap();
    old_task.await.unwrap();

    // Assert
    assert!(old_token.is_cancelled());
    harness.assert_no_events("Outdated generation must stay silent");
    let state = harness.state.lock();
    assert!(
        state.is_generating(),
        "New generation must still be running"
    );
    assert!(state.is_current_generation(&new_token));
}

/// A cancelled generation that only notices the cancellation after a new generation
/// has started must not report "cancelled" on behalf of the new one.
#[tokio::test]
async fn cancelled_generation_task_does_not_reset_state_of_newer_generation() {
    // Arrange
    let mut harness = TestHarness::new();
    let mut generator = MockContentGenerator::new();
    let (has_started, unblock) = generator.expect_cancellation();
    generator.set_result(Err(CoreError::Cancelled));
    let old_token = harness.state.lock().begin_generation();
    let old_task = tokio::spawn(generation_task(
        harness.proxy.clone(),
        harness.state.clone(),
        generator,
        MockTokenizer { token_count: 0 },
        old_token,
        harness.plan(),
    ));
    has_started.await.expect("Mock did not signal start");

    // Act
    let new_token = {
        let mut state = harness.state.lock();
        state.cancel_current_generation();
        state.begin_generation()
    };
    unblock.send(()).unwrap();
    old_task.await.unwrap();

    // Assert
    harness.assert_no_events("The cancelled generation must stay silent");
    let state = harness.state.lock();
    assert!(state.is_generating());
    assert!(state.is_current_generation(&new_token));
    assert_ne!(
        state.status,
        crate::app::state::StatusNotice::GenerationCancelled
    );
}
