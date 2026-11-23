use super::*;

#[tokio::test]
async fn test_update_filters_applies_filename_filter_without_content_search() {
    let mut harness = TestHarness::new();
    harness.create_file("src/main.rs", "");
    harness.create_file("src/lib.rs", "");
    harness.create_file("README.md", "");
    harness.set_initial_files(&["src", "src/main.rs", "src/lib.rs", "README.md"]);

    let filters = json!({
        "searchQuery": "main",
        "extensionFilter": "",
        "contentSearchQuery": ""
    });
    update_filters(typed(filters), harness.proxy.clone(), harness.state.clone()).await;

    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(ui_state.visible_files_count, 2);
}

#[tokio::test]
async fn test_update_filters_triggers_content_search() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["file1.txt"]);
    harness.create_file("file1.txt", "hello world");
    {
        let mut state = harness.state.lock();
        state.content_search_query = "initial".to_string();
    }

    let filters = json!({
        "contentSearchQuery": "world"
    });

    update_filters(typed(filters), harness.proxy.clone(), harness.state.clone()).await;

    let final_state = harness.last_state_update().unwrap();
    assert_eq!(final_state.content_search_query, "world");
    assert_eq!(
        final_state.visible_files_count, 1,
        "The matching file should be visible"
    );
}

/// A filter text that is left out is empty.
#[tokio::test]
async fn test_update_filters_payload_is_typed() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["main.rs", "notes.md"]);
    let filter = |payload| update_filters(payload, harness.proxy.clone(), harness.state.clone());

    filter(typed(json!({ "searchQuery": "main" }))).await;
    {
        let state = harness.state.lock();
        assert_eq!(state.search_query, "main");
        assert_eq!(state.extension_filter, "");
        assert_eq!(state.content_search_query, "");
    }
    let _ = harness.last_state_update();

    let all = UpdateFiltersPayload {
        search_query: String::new(),
        extension_filter: "md".to_string(),
        content_search_query: String::new(),
    };
    assert_eq!(
        serde_json::to_value(&all).unwrap(),
        json!({ "searchQuery": "", "extensionFilter": "md", "contentSearchQuery": "" })
    );
    update_filters(
        typed(serde_json::to_value(&all).unwrap()),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(ui_state.search_query, "");
    assert_eq!(ui_state.extension_filter, "md");
    assert_eq!(ui_state.visible_files_count, 1);
}

/// A searcher that reports that it was started and then waits to be released.
struct GatedSearcher {
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Notify>,
    matches: HashSet<PathBuf>,
}

impl tasks::FileSearcher for GatedSearcher {
    async fn search(
        &self,
        _: Vec<FileItem>,
        _: crate::core::ContentSearchRequest,
    ) -> tasks::SearchOutcome {
        let _ = self.started.send(());
        self.release.notified().await;
        Ok(self.matches.clone())
    }
}

/// `updateFilters` with a new content query is answered before the search has
/// come to an end: the filter inputs of the UI wait for their texts to be
/// reported back, and a search can take long. The matches follow.
#[tokio::test]
async fn test_update_filters_answers_before_the_content_search_completes() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["main.rs", "notes.md", "lib.rs"]);
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let searcher = GatedSearcher {
        started: started_tx,
        release: release.clone(),
        matches: HashSet::from([harness.root_path.join("main.rs")]),
    };

    let command = tokio::spawn(update_filters_with(
        typed(
            json!({ "searchQuery": "", "extensionFilter": "rs", "contentSearchQuery": "needle" }),
        ),
        harness.proxy.clone(),
        harness.state.clone(),
        searcher,
    ));
    started.recv().await.expect("The search must start");

    // The search is running: the answer is there, with the new texts.
    let pending = harness
        .last_state_update()
        .expect("updateFilters must be answered while the search is running");
    assert_eq!(pending.content_search_query, "needle");
    assert_eq!(pending.extension_filter, "rs");
    assert_eq!(pending.search_query, "");
    // The extension filter applies; the content filter does not empty the list
    // while its first matches are outstanding.
    assert_eq!(pending.visible_files_count, 2);
    assert!(!command.is_finished());
    harness
        .expect_no_event_within(QUIET_PERIOD, "Nothing more until the search answers")
        .await;

    release.notify_one();
    command.await.unwrap();
    assert_eq!(harness.state_updates_sent(), 1);
    let state = harness.state.lock();
    assert_eq!(state.content_search_query, "needle");
    assert_eq!(state.filtered_file_list().len(), 1);
}

/// The answer to a refined query shows the matches of the query before until the
/// new ones are there, and typing into another filter meanwhile does not end the
/// search.
#[tokio::test]
async fn test_update_filters_during_a_content_search_keeps_it_running() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["main.rs", "notes.md", "lib.rs"]);
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let searcher = GatedSearcher {
        started: started_tx,
        release: release.clone(),
        matches: HashSet::from([
            harness.root_path.join("main.rs"),
            harness.root_path.join("notes.md"),
        ]),
    };
    let command = tokio::spawn(update_filters_with(
        typed(json!({ "contentSearchQuery": "needle" })),
        harness.proxy.clone(),
        harness.state.clone(),
        searcher,
    ));
    started.recv().await.expect("The search must start");
    assert_eq!(harness.state_updates_sent(), 1);

    // Another filter, with the content query as the UI shows it.
    update_filters(
        typed(json!({ "searchQuery": "main", "contentSearchQuery": "needle" })),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
    let meanwhile = harness.last_state_update().unwrap();
    assert_eq!(meanwhile.search_query, "main");
    assert_eq!(meanwhile.content_search_query, "needle");
    assert_eq!(meanwhile.visible_files_count, 1);

    release.notify_one();
    command.await.unwrap();
    let answered = harness
        .last_state_update()
        .expect("The running search must still publish its matches");
    assert_eq!(answered.content_search_query, "needle");
    assert_eq!(answered.visible_files_count, 1);
    assert_eq!(
        *harness.state.lock().content_search_results(),
        HashSet::from([
            harness.root_path.join("main.rs"),
            harness.root_path.join("notes.md"),
        ])
    );
}
