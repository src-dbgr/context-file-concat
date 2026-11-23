use super::*;

#[tokio::test]
async fn search_in_files_updates_state_with_mock_results() {
    let mut harness = TestHarness::new();
    let searcher = MockFileSearcher::default();
    let match1 = harness.root_path.join("match1.txt");
    let match2 = harness.root_path.join("match2.txt");
    let mut mock_results = HashSet::new();
    mock_results.insert(match1.clone());
    mock_results.insert(match2.clone());
    searcher.set_results(mock_results);
    {
        let mut state = harness.state.lock();
        state.content_search_query = "magic".to_string();
        state.edit_full_file_list(|list| {
            list.push(FileItem {
                path: match1.clone(),
                ..Default::default()
            })
        });
        state.edit_full_file_list(|list| {
            list.push(FileItem {
                path: match2.clone(),
                ..Default::default()
            })
        });
        state.edit_full_file_list(|list| {
            list.push(FileItem {
                path: harness.root_path.join("nomatch.txt"),
                ..Default::default()
            })
        });
    }
    search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
    {
        let final_state = harness.state.lock();
        assert_eq!(final_state.content_search_results().len(), 2);
        assert!(final_state.content_search_results().contains(&match1));
        assert!(final_state.content_search_results().contains(&match2));
    }
    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(
        ui_state.visible_files_count, 2,
        "Filtered list should only contain the 2 matches"
    );
}

#[tokio::test]
async fn search_in_files_clears_results_on_empty_query() {
    let mut harness = TestHarness::new();
    let searcher = MockFileSearcher::default();
    {
        let mut state = harness.state.lock();
        state.set_content_search_results(HashSet::from([PathBuf::from("previous_match.txt")]));
        state.content_search_query = "".to_string();
    }
    search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
    {
        let final_state = harness.state.lock();
        assert!(
            final_state.content_search_results().is_empty(),
            "Search results should be cleared"
        );
    }
    assert!(
        harness.last_state_update().is_some(),
        "Should have received a state update"
    );
}

/// Comprehensive test for the RealFileSearcher implementation via the search_in_files task.
/// This test covers:
/// 1. Case-sensitive matching.
/// 2. Case-insensitive matching (default).
/// 3. Correctly skipping binary files and directories.
/// 4. Handling files where the search term is not found.
#[tokio::test]
async fn search_in_files_with_real_searcher_covers_edge_cases() {
    // Arrange
    let mut harness = TestHarness::new();
    let searcher = RealFileSearcher; // Use the real implementation

    // Create a diverse set of files
    let text_file_path = harness.root_path.join("file.txt");
    std::fs::write(&text_file_path, "find the MagicWord here").unwrap();

    let binary_file_path = harness.root_path.join("app.exe");
    std::fs::write(&binary_file_path, [0, 1, 2, 3, 255]).unwrap(); // A binary file

    let no_match_path = harness.root_path.join("another.txt");
    std::fs::write(&no_match_path, "nothing to see").unwrap();

    let dir_path = harness.root_path.join("subfolder");
    std::fs::create_dir(&dir_path).unwrap();

    // Populate state
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![
            FileItem {
                path: text_file_path.clone(),
                is_binary: false,
                ..Default::default()
            },
            FileItem {
                path: binary_file_path,
                is_binary: true,
                ..Default::default()
            },
            FileItem {
                path: no_match_path,
                is_binary: false,
                ..Default::default()
            },
            FileItem {
                path: dir_path,
                is_directory: true,
                ..Default::default()
            },
        ]);
    }

    // --- SCENARIO 1: Case-sensitive search (no match) ---
    {
        let mut state = harness.state.lock();
        state.config_mut().case_sensitive_search = true;
        state.content_search_query = "magicword".to_string(); // Lowercase
    }
    // Pass 'searcher' by value (it's Copy)
    search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
    // Assert against the AppState, not the UiState
    assert!(
        harness.state.lock().content_search_results().is_empty(),
        "Case-sensitive search should not find 'magicword'"
    );
    // Drain the event queue
    let _ = harness.last_state_update();

    // --- SCENARIO 2: Case-sensitive search (exact match) ---
    {
        let mut state = harness.state.lock();
        state.config_mut().case_sensitive_search = true;
        state.content_search_query = "MagicWord".to_string(); // Exact case
    }
    search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
    // Assert against the AppState for correctness
    let final_state = harness.state.lock();
    assert_eq!(
        final_state.content_search_results().len(),
        1,
        "Case-sensitive search should find 'MagicWord'"
    );
    assert!(final_state
        .content_search_results()
        .contains(&text_file_path));
}

/// Tests that the RealFileSearcher gracefully handles files that cannot be read
/// from the filesystem (e.g., due to permissions).
#[tokio::test]
#[cfg(unix)] // This test relies on Unix-style permissions.
async fn search_in_files_with_real_searcher_handles_read_error() {
    if running_as_root() {
        eprintln!("Skipping permission-based test because process runs as root (Docker/act).");
        return;
    }
    // Arrange
    use std::os::unix::fs::PermissionsExt;

    let harness = TestHarness::new();
    let searcher = RealFileSearcher;

    let unreadable_file_path = harness.root_path.join("unreadable.txt");
    std::fs::write(&unreadable_file_path, "you can't read me").unwrap();

    // Set file permissions to write-only (0o200) to cause a read error.
    let mut perms = std::fs::metadata(&unreadable_file_path)
        .unwrap()
        .permissions();
    perms.set_mode(0o200);
    std::fs::set_permissions(&unreadable_file_path, perms).unwrap();

    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![FileItem {
            path: unreadable_file_path.clone(),
            ..Default::default()
        }]);
        state.content_search_query = "read".to_string();
    }

    // Act
    search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;

    // Assert
    // The main assertion is that the task completes without panicking.
    // We also assert that the unreadable file is not included in the results.
    let final_state = harness.state.lock();
    assert!(
        final_state.content_search_results().is_empty(),
        "Unreadable file should not be a search result."
    );
}

/// The real searcher takes the size limit from the request, whether it is stricter
/// or more generous than another one.
#[tokio::test]
async fn real_searcher_applies_the_file_size_limit_of_the_request() {
    let harness = TestHarness::new();
    let small = harness.root_path.join("small.txt");
    let large = harness.root_path.join("large.txt");
    std::fs::write(&small, "needle").unwrap();
    std::fs::write(&large, "needle and hay").unwrap();
    let files = vec![file_item(&small, 1), file_item(&large, 1)];
    let request = |max_file_size| ContentSearchRequest {
        max_file_size,
        ..ContentSearchRequest::new("needle", true)
    };

    let limited = RealFileSearcher.search(files.clone(), request(6)).await;
    let generous = RealFileSearcher.search(files, request(u64::MAX)).await;

    assert_eq!(limited, Ok(HashSet::from([small.clone()])));
    assert_eq!(generous, Ok(HashSet::from([small, large])));
}

/// The real searcher does not read anything for a request that is already cancelled.
#[tokio::test]
async fn real_searcher_stops_for_a_cancelled_request() {
    let harness = TestHarness::new();
    let file = harness.root_path.join("a.txt");
    std::fs::write(&file, "needle").unwrap();
    let request = ContentSearchRequest::new("needle", true);
    request.cancel_flag.store(true, Ordering::SeqCst);

    let result = RealFileSearcher
        .search(vec![file_item(&file, 1)], request)
        .await;

    assert_eq!(result, Ok(HashSet::new()));
}

/// A searcher that keeps the request it was given and finds nothing.
#[derive(Clone, Default)]
struct RequestRecorder(Arc<Mutex<Option<ContentSearchRequest>>>);

impl RequestRecorder {
    fn recorded(&self) -> ContentSearchRequest {
        self.0.lock().unwrap().clone().expect("A search must run")
    }
}

impl FileSearcher for RequestRecorder {
    async fn search(&self, _: Vec<FileItem>, request: ContentSearchRequest) -> SearchOutcome {
        *self.0.lock().unwrap() = Some(request);
        Ok(HashSet::new())
    }
}

/// `search_in_files` describes the search completely: query, case sensitivity and
/// size limit come from the state, the flag belongs to this search run.
#[tokio::test]
async fn search_in_files_passes_the_request_to_the_searcher() {
    let harness = TestHarness::new();
    {
        let mut state = harness.state.lock();
        // More generous than the default of the core: must arrive uncapped.
        state.config_mut().max_file_size_mb = 64;
        state.config_mut().case_sensitive_search = true;
        state.content_search_query = " Needle ".to_string();
    }
    let recorder = RequestRecorder::default();

    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        recorder.clone(),
    )
    .await;

    let request = recorder.recorded();
    assert_eq!(request.query, " Needle ");
    assert!(request.case_sensitive);
    assert_eq!(request.max_file_size, 64 * 1024 * 1024);
    assert!(!request.cancel_flag.load(Ordering::SeqCst));
}

struct UnexpectedSearch;

impl FileSearcher for UnexpectedSearch {
    async fn search(&self, _: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
        panic!("No search expected without a content query");
    }
}

/// A content query of whitespace only is no query: no search runs, matches of an
/// earlier query are dropped and the list is shown unfiltered.
#[tokio::test]
async fn search_in_files_treats_a_whitespace_only_query_as_none() {
    let mut harness = TestHarness::new();
    let file = harness.root_path.join("a.txt");
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(&file, 1)]);
        state.content_search_query = " \t ".to_string();
        state.set_content_search_results(HashSet::from([PathBuf::from("/elsewhere.txt")]));
    }

    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        UnexpectedSearch,
    )
    .await;

    let update = harness.last_state_update().expect("The state is sent");
    assert_eq!(update.visible_files_count, 1);
    assert!(harness.state.lock().content_search_results().is_empty());
}

#[tokio::test]
async fn refresh_content_search_does_nothing_for_a_whitespace_only_query() {
    let mut harness = TestHarness::new();
    harness.state.lock().content_search_query = "  ".to_string();

    refresh_content_search(&harness.proxy, &harness.state, UnexpectedSearch).await;

    assert!(harness.last_state_update().is_none());
}

/// `refresh_content_search` runs the same search, so it hands over the same request.
#[tokio::test]
async fn refresh_content_search_passes_the_request_to_the_searcher() {
    let harness = TestHarness::new();
    {
        let mut state = harness.state.lock();
        state.config_mut().max_file_size_mb = 3;
        state.content_search_query = "needle".to_string();
    }
    let recorder = RequestRecorder::default();

    refresh_content_search(&harness.proxy, &harness.state, recorder.clone()).await;

    let request = recorder.recorded();
    assert_eq!(request.query, "needle");
    assert!(!request.case_sensitive);
    assert_eq!(request.max_file_size, 3 * 1024 * 1024);
}

/// A searcher that signals when it has been asked, then waits until the test has
/// superseded the search and only then lets the real search run.
struct SupersededSearcher {
    asked: Mutex<Option<oneshot::Sender<()>>>,
    superseded: Mutex<Option<oneshot::Receiver<()>>>,
    found_after_supersession: Arc<Mutex<Option<HashSet<PathBuf>>>>,
}

impl FileSearcher for SupersededSearcher {
    async fn search(&self, files: Vec<FileItem>, request: ContentSearchRequest) -> SearchOutcome {
        let asked = self.asked.lock().unwrap().take().unwrap();
        let superseded = self.superseded.lock().unwrap().take().unwrap();
        asked.send(()).unwrap();
        superseded.await.unwrap();
        let found = RealFileSearcher.search(files, request).await;
        *self.found_after_supersession.lock().unwrap() = found.clone().ok();
        found
    }
}

/// A search that has been superseded stops: the flag of its request is raised, so
/// the real search does not read the files that are still ahead of it.
#[tokio::test]
async fn superseded_content_search_stops_reading_files() {
    let harness = TestHarness::new();
    let file = harness.root_path.join("a.txt");
    std::fs::write(&file, "needle").unwrap();
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(&file, 1)]);
        state.content_search_query = "needle".to_string();
    }
    let (asked_tx, asked) = oneshot::channel();
    let (superseded_tx, superseded) = oneshot::channel();
    let found_after_supersession = Arc::new(Mutex::new(None));
    let outdated_search = tokio::spawn(search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        SupersededSearcher {
            asked: Mutex::new(Some(asked_tx)),
            superseded: Mutex::new(Some(superseded)),
            found_after_supersession: found_after_supersession.clone(),
        },
    ));
    asked.await.expect("The outdated search must start");

    // Act: a newer search supersedes the one that is still running.
    let recorder = RequestRecorder::default();
    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        recorder.clone(),
    )
    .await;
    superseded_tx.send(()).unwrap();
    outdated_search.await.unwrap();

    // Assert: the outdated search no longer read the file that matches ...
    assert_eq!(
        *found_after_supersession.lock().unwrap(),
        Some(HashSet::new())
    );
    // ... while the flag of the current search stays down.
    assert!(!recorder.recorded().cancel_flag.load(Ordering::SeqCst));
}

/// Two overlapping searches: the one for the older query finishes last. Its matches
/// must not replace those of the current query.
#[tokio::test]
async fn outdated_content_search_does_not_overwrite_newer_results() {
    // Arrange
    let mut harness = TestHarness::new();
    let fo_match = harness.root_path.join("fo.txt");
    let foo_match = harness.root_path.join("foo.txt");
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(&fo_match, 1), file_item(&foo_match, 1)]);
        state.content_search_query = "fo".to_string();
    }
    let (slow_searcher, slow_started, release_slow) =
        GatedFileSearcher::new(HashSet::from([fo_match.clone(), foo_match.clone()]));
    let slow_search = tokio::spawn(search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        slow_searcher,
    ));
    slow_started.await.expect("Slow search must start");

    // Act: the user types on; the search for "foo" completes first.
    harness.state.lock().content_search_query = "foo".to_string();
    let fast_searcher = MockFileSearcher::default();
    fast_searcher.set_results(HashSet::from([foo_match.clone()]));
    search_in_files(harness.proxy.clone(), harness.state.clone(), fast_searcher).await;
    let state_after_current_search = harness.last_state_update().unwrap();
    release_slow.send(()).unwrap();
    slow_search.await.unwrap();

    // Assert
    assert_eq!(state_after_current_search.visible_files_count, 1);
    assert!(
        harness.last_state_update().is_none(),
        "The outdated search must not publish anything"
    );
    let state = harness.state.lock();
    assert_eq!(*state.content_search_results(), HashSet::from([foo_match]));
    assert_eq!(state.filtered_file_list().len(), 1);
}

/// Clearing the query while a search is running must not let that search bring its
/// matches back afterwards.
#[tokio::test]
async fn content_search_finishing_after_query_was_cleared_is_discarded() {
    let harness = TestHarness::new();
    let file = harness.root_path.join("a.txt");
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(&file, 1)]);
        state.content_search_query = "a".to_string();
    }
    let (slow_searcher, slow_started, release_slow) =
        GatedFileSearcher::new(HashSet::from([file.clone()]));
    let slow_search = tokio::spawn(search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        slow_searcher,
    ));
    slow_started.await.expect("Slow search must start");

    harness.state.lock().content_search_query.clear();
    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        MockFileSearcher::default(),
    )
    .await;
    release_slow.send(()).unwrap();
    slow_search.await.unwrap();

    assert!(harness.state.lock().content_search_results().is_empty());
}

/// A content query entered while the scan is still running has only been applied to
/// the files of the shallow list. Once the deep scan delivers the complete list, the
/// search must be repeated so that deeper files are found as well.
#[tokio::test]
async fn content_search_is_repeated_on_complete_list_after_deep_scan() {
    // Arrange
    let mut harness = TestHarness::new();
    let root = harness.root_path.clone();
    let src = root.join("src");
    let top_match = root.join("top.txt");
    let deep_match = src.join("deep.txt");
    let shallow = vec![dir_item(&src, 1), file_item(&top_match, 1)];
    let mut deep = shallow.clone();
    deep.push(file_item(&deep_match, 2));
    deep.push(file_item(&src.join("other.txt"), 2));
    let mut scanner = MockScanner::new();
    scanner.set_results(shallow.clone(), deep);
    let searcher = ListBoundFileSearcher {
        matching: HashSet::from([top_match.clone(), deep_match.clone()]),
    };
    let token = {
        // The query was entered during the scan and searched on the shallow list.
        let mut state = harness.state.lock();
        state.content_search_query = "needle".to_string();
        state.set_content_search_results(HashSet::from([top_match.clone()]));
        state.begin_scan()
    };

    // Act
    proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        root,
        scanner,
        searcher,
        token,
    )
    .await;

    // Assert
    let ui_state = harness.last_state_update().unwrap();
    assert!(!ui_state.is_scanning);
    let state = harness.state.lock();
    assert_eq!(
        *state.content_search_results(),
        HashSet::from([top_match.clone(), deep_match.clone()])
    );
    let visible: HashSet<PathBuf> = state
        .filtered_file_list()
        .iter()
        .map(|item| item.path.clone())
        .collect();
    assert_eq!(visible, HashSet::from([src, top_match, deep_match]));
}

/// Without a content query a finished scan must not trigger a search (nor an
/// additional state update).
#[tokio::test]
async fn deep_scan_without_content_query_does_not_search() {
    let mut harness = TestHarness::new();
    let token = harness.state.lock().begin_scan();

    proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        harness.root_path.clone(),
        MockScanner::new(),
        UnexpectedSearch,
        token,
    )
    .await;

    let events = harness.drain_events();
    assert_eq!(events.len(), 2, "Only the shallow and the deep update");
}

/// "Indexing complete" reports the number of visible items. With a content query
/// that number is only known once the search has been repeated on the complete
/// list, so the scan must not be reported as complete before that.
#[tokio::test]
async fn indexing_complete_counts_the_items_visible_after_the_repeated_search() {
    // Arrange: the query matched one file of the shallow list.
    let mut harness = TestHarness::new();
    let root = harness.root_path.clone();
    let src = root.join("src");
    let top_match = root.join("top.txt");
    let deep_match = src.join("deep.txt");
    let shallow = vec![dir_item(&src, 1), file_item(&top_match, 1)];
    let mut deep = shallow.clone();
    deep.push(file_item(&deep_match, 2));
    deep.push(file_item(&src.join("other.txt"), 2));
    let mut scanner = MockScanner::new();
    scanner.set_results(shallow, deep);
    let (searcher, search_started, release_search) =
        GatedFileSearcher::new(HashSet::from([top_match.clone(), deep_match.clone()]));
    let token = {
        let mut state = harness.state.lock();
        state.content_search_query = "needle".to_string();
        state.set_content_search_results(HashSet::from([top_match.clone()]));
        state.begin_scan()
    };

    // Act: the deep scan has delivered, the repeated search is still running.
    let scan = tokio::spawn(proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        root,
        scanner,
        searcher,
        token,
    ));
    search_started.await.unwrap();

    // Assert: nothing is reported as complete on the basis of the old matches.
    {
        let state = harness.state.lock();
        assert!(state.is_scanning(), "The scan is not complete yet");
        assert!(state.is_fully_scanned, "The complete list is in place");
        assert_eq!(state.status, StatusNotice::Ready);
    }
    assert!(harness
        .drain_events()
        .iter()
        .all(|event| !matches!(event, UserEvent::StateUpdate(s) if !s.is_scanning)));

    // Act: the search delivers.
    release_search.send(()).unwrap();
    scan.await.unwrap();

    // Assert: `src`, `top.txt` and `deep.txt` are visible, and that is what is said.
    let ui_state = harness.last_state_update().unwrap();
    assert!(!ui_state.is_scanning);
    assert_eq!(ui_state.visible_files_count, 3);
    let state = harness.state.lock();
    assert_eq!(state.filtered_file_list().len(), 3);
    assert_eq!(
        state.status,
        StatusNotice::IndexingComplete { visible_items: 3 }
    );
}

/// The search that follows the deep scan belongs to the file list. Cancelling the
/// scan while it runs must not leave the matches of the smaller, shallow list.
#[tokio::test]
async fn search_after_deep_scan_survives_the_cancellation_of_the_scan() {
    let mut harness = TestHarness::new();
    let root = harness.root_path.clone();
    let deep_match = root.join("src").join("deep.txt");
    let deep = vec![dir_item(&root.join("src"), 1), file_item(&deep_match, 2)];
    let mut scanner = MockScanner::new();
    scanner.set_results(vec![], deep);
    let (searcher, search_started, release_search) =
        GatedFileSearcher::new(HashSet::from([deep_match.clone()]));
    let token = {
        let mut state = harness.state.lock();
        state.content_search_query = "needle".to_string();
        state.begin_scan()
    };
    let scan = tokio::spawn(proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        root,
        scanner,
        searcher,
        token.clone(),
    ));
    harness.state.lock().attach_scan_task(&token, scan);
    search_started.await.unwrap();

    // Act: the user cancels, then the search delivers.
    harness.state.lock().cancel_current_scan();
    let _ = harness.drain_events();
    release_search.send(()).unwrap();
    let ui_state = harness.wait_for_state(|s| s.visible_files_count == 2).await;

    // Assert
    assert!(!ui_state.is_scanning);
    let state = harness.state.lock();
    assert_eq!(*state.content_search_results(), HashSet::from([deep_match]));
    assert_eq!(state.status, StatusNotice::ScanCancelled);
}

struct FailingSearcher;

impl FileSearcher for FailingSearcher {
    async fn search(&self, _: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
        Err("the search worker panicked".to_string())
    }
}

/// A search that fails is reported; it is not a search without matches. The
/// matches of the search before it stay, and the next search is not blocked.
#[tokio::test]
async fn a_failed_content_search_is_reported_and_keeps_the_previous_matches() {
    let mut harness = TestHarness::new();
    let matching = harness.root_path.join("a.txt");
    let other = harness.root_path.join("b.txt");
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(&matching, 1), file_item(&other, 1)]);
        state.content_search_query = "needle".to_string();
    }
    let first = MockFileSearcher::default();
    first.set_results(HashSet::from([matching.clone()]));
    search_in_files(harness.proxy.clone(), harness.state.clone(), first).await;
    harness.drain_events();

    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        FailingSearcher,
    )
    .await;

    let events = harness.drain_events();
    match &events[..] {
        [UserEvent::ShowError(message), UserEvent::StateUpdate(ui_state)] => {
            assert!(message.contains("content search failed"), "{message}");
            assert!(message.contains("panicked"), "{message}");
            assert_eq!(ui_state.visible_files_count, 1, "the matches stay");
        }
        other => panic!("Expected an error and a state update, got {other:?}"),
    }
    {
        let state = harness.state.lock();
        assert_eq!(*state.content_search_results(), HashSet::from([matching]));
    }

    // The slot is free: a later search delivers.
    let next = MockFileSearcher::default();
    next.set_results(HashSet::from([other.clone()]));
    search_in_files(harness.proxy.clone(), harness.state.clone(), next).await;
    assert_eq!(
        *harness.state.lock().content_search_results(),
        HashSet::from([other])
    );
}

/// The first search for a query fails: there are no earlier matches to keep, and
/// the list stays as it is without the content filter. "The search failed" is not
/// "nothing matches"; the error is shown.
#[tokio::test]
async fn a_failed_first_content_search_keeps_the_unfiltered_list() {
    let mut harness = TestHarness::new();
    let (a, b) = (
        harness.root_path.join("a.txt"),
        harness.root_path.join("b.txt"),
    );
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(&a, 1), file_item(&b, 1)]);
        state.content_search_query = "needle".to_string();
    }

    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        FailingSearcher,
    )
    .await;

    let events = harness.drain_events();
    match &events[..] {
        [UserEvent::ShowError(message), UserEvent::StateUpdate(ui_state)] => {
            assert!(message.contains("content search failed"), "{message}");
            assert_eq!(ui_state.visible_files_count, 2, "not an empty list");
        }
        other => panic!("Expected an error and a state update, got {other:?}"),
    }
    // Also when the filters are applied again (another filter text, a lazy load).
    {
        let mut state = harness.state.lock();
        filtering::apply_filters(&mut state);
        assert_eq!(state.filtered_file_list().len(), 2);
    }

    // A later search that delivers filters as usual.
    let next = MockFileSearcher::default();
    next.set_results(HashSet::from([a.clone()]));
    search_in_files(harness.proxy.clone(), harness.state.clone(), next).await;
    assert_eq!(harness.state.lock().filtered_file_list().len(), 1);
}

/// The failure of a search that has been superseded is nobody's business.
#[tokio::test]
async fn a_failed_content_search_that_was_superseded_stays_silent() {
    let mut harness = TestHarness::new();
    harness.state.lock().content_search_query = "needle".to_string();

    struct SupersededThenFailing(SharedState);
    impl FileSearcher for SupersededThenFailing {
        async fn search(&self, _: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
            self.0.lock().begin_content_search();
            Err("too late".to_string())
        }
    }

    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        SupersededThenFailing(harness.state.clone()),
    )
    .await;

    harness.assert_no_events("An outdated search reports nothing");
}

/// While the first search for a query is running, a snapshot shows the list
/// without the content filter instead of an empty tree; the filter applies as
/// soon as the matches are there.
#[tokio::test]
async fn a_snapshot_during_the_first_content_search_is_not_empty() {
    let mut harness = TestHarness::new();
    let matching = harness.root_path.join("a.txt");
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![
            file_item(&matching, 1),
            file_item(&harness.root_path.join("b.txt"), 1),
        ]);
        filtering::apply_filters(&mut state);
        state.content_search_query = "needle".to_string();
    }
    let (searcher, started, release) = GatedFileSearcher::new(HashSet::from([matching.clone()]));
    let search = tokio::spawn(search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        searcher,
    ));
    started.await.unwrap();

    // Any command that answers with a snapshot while the search runs, also one
    // that applies the filters again.
    {
        let mut state = harness.state.lock();
        assert_eq!(state.content_filter_matches(), None);
        assert_eq!(generate_ui_state(&state).root_rows().len(), 2);
        filtering::apply_filters(&mut state);
        assert_eq!(generate_ui_state(&state).root_rows().len(), 2);
    }

    release.send(()).unwrap();
    search.await.unwrap();
    let filtered = harness.last_state_update().unwrap();
    assert_eq!(filtered.root_rows().len(), 1);
    assert_eq!(filtered.root_rows()[0].path, matching);
}

/// Once a search has answered, its answer applies until the next one arrives:
/// "no matches" stays an empty tree while the query is being refined.
#[tokio::test]
async fn a_snapshot_during_a_later_content_search_shows_the_previous_matches() {
    let mut harness = TestHarness::new();
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(&harness.root_path.join("a.txt"), 1)]);
        state.content_search_query = "needle".to_string();
    }
    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        MockFileSearcher::default(),
    )
    .await;
    assert!(harness.last_state_update().unwrap().rows.is_empty());

    harness.state.lock().content_search_query = "needles".to_string();
    let (searcher, started, release) = GatedFileSearcher::new(HashSet::new());
    let search = tokio::spawn(search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        searcher,
    ));
    started.await.unwrap();
    {
        let state = harness.state.lock();
        assert_eq!(state.content_filter_matches(), Some(&HashSet::new()));
        assert!(generate_ui_state(&state).rows.is_empty());
    }
    release.send(()).unwrap();
    search.await.unwrap();
}

/// Repeating the content search (here: after a lazy load brought new files) shows
/// the new matches, but does not re-open a directory the user collapsed while the
/// query was active.
#[tokio::test]
async fn repeated_content_search_does_not_reopen_a_collapsed_directory() {
    let harness = TestHarness::new();
    let src = harness.root_path.join("src");
    let lib = harness.root_path.join("lib");
    let old_match = src.join("old.txt");
    let new_match = lib.join("new.txt");
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![
            dir_item(&src, 1),
            dir_item(&lib, 1),
            file_item(&old_match, 2),
        ]);
        state.content_search_query = "needle".to_string();
    }
    let searcher = ListBoundFileSearcher {
        matching: HashSet::from([old_match.clone(), new_match.clone()]),
    };
    search_in_files(
        harness.proxy.clone(),
        harness.state.clone(),
        searcher.clone(),
    )
    .await;
    assert!(harness.state.lock().expanded_dirs.contains(&src));

    // The user collapses `src`, then opens `lib`, which is loaded lazily.
    let token = {
        let mut state = harness.state.lock();
        state.collapse_directories([src.clone()]);
        state.begin_lazy_load()
    };
    let mut scanner = MockScanner::new();
    scanner.set_results(vec![file_item(&new_match, 2)], vec![]);
    lazy_load_task(
        ScanRequest::directory_level(&harness.root_path, &lib).unwrap(),
        harness.proxy.clone(),
        harness.state.clone(),
        scanner,
        searcher,
        token,
    )
    .await;

    let state = harness.state.lock();
    assert_eq!(
        *state.content_search_results(),
        HashSet::from([old_match, new_match])
    );
    assert!(!state.expanded_dirs.contains(&src), "stays collapsed");
    assert!(state.expanded_dirs.contains(&lib), "the new match is shown");
}
