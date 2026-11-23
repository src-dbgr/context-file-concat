use super::*;

/// A scanner whose deep scan delivers the batches the test feeds it, when it feeds
/// them, and ends when the feed is dropped. The shallow scan returns `shallow`.
struct StreamingScanner {
    shallow: Vec<FileItem>,
    feed: tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<FileItem>>>,
}

impl StreamingScanner {
    fn new(shallow: Vec<FileItem>) -> (Self, mpsc::UnboundedSender<Vec<FileItem>>) {
        let (feed_tx, feed_rx) = mpsc::unbounded_channel();
        let scanner = Self {
            shallow,
            feed: tokio::sync::Mutex::new(feed_rx),
        };
        (scanner, feed_tx)
    }
}

impl Scanner for StreamingScanner {
    async fn scan(
        &self,
        _: &ScanRequest,
        _: Box<dyn Fn(ScanProgress) + Send + Sync>,
    ) -> ScanOutcome {
        Ok((self.shallow.clone(), HashSet::new()))
    }

    async fn scan_in_batches(
        &self,
        _: &ScanRequest,
        _: Box<dyn Fn(ScanProgress) + Send + Sync>,
        on_batch: BatchSink,
    ) -> StreamedScanOutcome {
        let mut feed = self.feed.lock().await;
        while let Some(batch) = feed.recv().await {
            on_batch(batch);
        }
        Ok(HashSet::from(["deep".to_string()]))
    }
}

/// Starts a scan of the root of `harness` with a [`StreamingScanner`] and waits
/// until its first level (`shallow`) has been published.
async fn spawn_streaming_scan(
    harness: &mut TestHarness,
    shallow: Vec<FileItem>,
) -> (
    tokio::task::JoinHandle<()>,
    ScanToken,
    mpsc::UnboundedSender<Vec<FileItem>>,
) {
    let (scanner, feed) = StreamingScanner::new(shallow);
    let token = harness.state.lock().begin_scan();
    let task = tokio::spawn(proactive_scan_task(
        harness.proxy.clone(),
        harness.state.clone(),
        harness.root_path.clone(),
        scanner,
        MockFileSearcher::default(),
        token.clone(),
    ));
    let shallow_event = harness.next_event().await;
    assert!(matches!(shallow_event, UserEvent::StateUpdate(_)));
    (task, token, feed)
}

fn listed(harness: &TestHarness) -> Vec<PathBuf> {
    let state = harness.state.lock();
    state
        .full_file_list()
        .iter()
        .map(|item| item.path.clone())
        .collect()
}

/// What the walk finds is shown while the scan is still running, batch after
/// batch, and without listing the first level a second time. The end of the scan
/// completes the list.
#[tokio::test]
async fn deep_scan_publishes_its_batches_while_it_runs() {
    let mut harness = TestHarness::new();
    let src = harness.root_path.join("src");
    let (task, _token, feed) = spawn_streaming_scan(&mut harness, vec![dir_item(&src, 1)]).await;

    // First batch: the walk comes by the first level again and goes below it.
    feed.send(vec![dir_item(&src, 1), file_item(&src.join("a.rs"), 2)])
        .unwrap();
    let first = harness
        .wait_for_state(|ui| ui.visible_files_count == 2)
        .await;
    assert!(first.is_scanning);
    assert!(!first.is_fully_scanned);

    // Second batch.
    feed.send(vec![file_item(&src.join("b.rs"), 2)]).unwrap();
    let second = harness
        .wait_for_state(|ui| ui.visible_files_count == 3)
        .await;
    assert!(second.is_scanning);
    assert!(
        !harness.state.lock().loaded_dirs.contains(&src),
        "The walk may not be done with `src` yet"
    );

    // A last batch right before the end is part of the final state.
    feed.send(vec![file_item(&src.join("c.rs"), 2)]).unwrap();
    drop(feed);
    task.await.unwrap();

    let last = harness.last_state_update().unwrap();
    assert!(!last.is_scanning);
    assert!(last.is_fully_scanned);
    assert_eq!(last.visible_files_count, 4);
    assert!(last.status_message.contains("Indexing complete"));
    assert_eq!(
        listed(&harness),
        [
            src.clone(),
            src.join("a.rs"),
            src.join("b.rs"),
            src.join("c.rs")
        ]
    );
    let state = harness.state.lock();
    assert!(state.loaded_dirs.contains(&src));
    assert_eq!(
        state.active_ignore_patterns,
        HashSet::from(["deep".to_string()])
    );
}

/// The user expands a directory the walk has not come to: its level is loaded and
/// shown at once. When the walk gets there, nothing is listed twice.
#[tokio::test]
async fn a_directory_expanded_during_the_deep_scan_is_loaded_at_once() {
    let mut harness = TestHarness::new();
    let src = harness.root_path.join("src");
    std::fs::create_dir(&src).unwrap();
    std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
    let (task, token, feed) = spawn_streaming_scan(&mut harness, vec![dir_item(&src, 1)]).await;

    // Act: the lazy load, while the deep scan is under way.
    start_lazy_load_scan(src.clone(), harness.proxy.clone(), harness.state.clone())
        .expect("A lazy load is possible during a scan")
        .await
        .unwrap();

    let ui_state = harness.last_state_update().unwrap();
    assert!(ui_state.is_scanning);
    let src_node = ui_state.row_named("src").unwrap();
    assert!(src_node.is_expanded && src_node.children_loaded);
    assert_eq!(ui_state.children_of(src_node).len(), 1);
    assert!(harness.state.lock().is_current_scan(&token));

    // The walk arrives at the directory and finds one more file.
    feed.send(vec![
        dir_item(&src, 1),
        file_item(&src.join("main.rs"), 2),
        file_item(&src.join("new.rs"), 2),
    ])
    .unwrap();
    drop(feed);
    task.await.unwrap();

    let mut paths = listed(&harness);
    paths.sort();
    assert_eq!(
        paths,
        [src.clone(), src.join("main.rs"), src.join("new.rs")]
    );
    assert!(harness.state.lock().is_fully_scanned);
}

/// A scan that was superseded while its walk is delivering publishes nothing more
/// and ends by itself.
#[tokio::test]
async fn batches_of_a_superseded_scan_are_not_published() {
    let mut harness = TestHarness::new();
    let (old_task, old_token, feed) = spawn_streaming_scan(&mut harness, vec![]).await;

    let new_token = begin_replacement_scan(&harness);
    feed.send(vec![file_item(&harness.root_path.join("stale.txt"), 1)])
        .unwrap();
    // The feed stays open: the task has to end because of the stale batch.
    tokio::time::timeout(EVENT_TIMEOUT, old_task)
        .await
        .expect("The superseded task ends when it tries to publish")
        .unwrap();

    assert!(old_token.is_cancelled());
    harness.assert_no_events("A superseded scan must not send state updates");
    assert!(listed(&harness).is_empty());
    assert_replacement_scan_is_intact(&harness, &new_token);
    drop(feed);
}

/// Cancelling a scan keeps what it has published: the list is partial, not
/// complete, and its directories can be loaded lazily.
#[tokio::test]
async fn a_cancelled_deep_scan_keeps_what_it_has_published() {
    let mut harness = TestHarness::new();
    let root = harness.root_path.clone();
    let src = root.join("src");
    let (task, _token, feed) = spawn_streaming_scan(&mut harness, vec![dir_item(&src, 1)]).await;
    feed.send(vec![file_item(&src.join("a.rs"), 2)]).unwrap();
    harness
        .wait_for_state(|ui| ui.visible_files_count == 2)
        .await;

    harness.state.lock().cancel_current_scan();
    feed.send(vec![file_item(&src.join("late.rs"), 2)]).unwrap();
    drop(feed);
    task.await.unwrap();

    assert_eq!(listed(&harness), [src.clone(), src.join("a.rs")]);
    let state = harness.state.lock();
    assert!(!state.is_scanning());
    assert!(!state.is_fully_scanned);
    assert_eq!(state.loaded_dirs, HashSet::from([root]));
}

/// The state is not locked while the items of a publication are prepared, and
/// the state update that ends the publication is made of the state as it is
/// then, changes of the meantime included.
#[tokio::test]
async fn a_publication_shows_the_state_as_it_is_when_the_items_are_in() {
    let mut harness = TestHarness::new();
    let src = harness.root_path.join("src");
    let (task, token, feed) = spawn_streaming_scan(&mut harness, vec![dir_item(&src, 1)]).await;

    let staged = staged::stage_scanned_items(
        &mut harness.state.lock(),
        &token,
        vec![file_item(&src.join("a.rs"), 2)],
    )
    .unwrap();
    let prepared = staged.prepare();
    // What a command does meanwhile: the user selects a file and expands `src`.
    {
        let mut state = harness.state.lock();
        state.selected_files.insert(src.join("a.rs"));
        state.expand_directories([src.clone()]);
    }
    let ui_state = {
        let mut state = harness.state.lock();
        let committed = staged::commit_scanned_items(&mut state, &token, prepared);
        assert!(matches!(committed, staged::Committed::Done(true, _)));
        generate_ui_state(&state)
    };

    assert_eq!(ui_state.visible_files_count, 2);
    assert_eq!(ui_state.selected_files_count, 1);
    let src_row = ui_state.row_named("src").unwrap();
    assert!(src_row.is_expanded);
    assert_eq!(ui_state.children_of(src_row).len(), 1);

    drop(feed);
    task.await.unwrap();
}

/// An event that finds the state changed when its lists are ready does not
/// make them under the lock: it is prepared a second time, outside of it, for
/// the state as it is. Only if the state has changed again by then, the lists
/// are made under the lock. Either way the event happens once.
#[test]
fn an_event_that_meets_a_changed_state_is_prepared_again_without_the_lock() {
    let root = PathBuf::from("/root");
    let src = root.join("src");
    for changes in 0..=3usize {
        let mut state = AppState::default();
        state.config.ignore_patterns.clear();
        state.current_path = root.to_string_lossy().to_string();
        let token = state.begin_scan();
        state.begin_scan_walk(&token);
        state.replace_with_shallow_file_list(&root, vec![dir_item(&src, 1)]);
        filtering::apply_filters(&mut state);
        let state = SharedState::new(state);
        let built = state.lock().file_tree_build_count();

        // What a command does between the steps: the file list is another one.
        let (mut commits, mut updates) = (0, 0);
        let cost = in_three_steps(
            &state,
            |s| staged::stage_scanned_items(s, &token, vec![file_item(&src.join("a.rs"), 2)]),
            |s, prepared| {
                if commits < changes {
                    let other = dir_item(&root.join(format!("other_{commits}")), 1);
                    s.edit_full_file_list(|list| list.push(other));
                    filtering::apply_filters(s);
                }
                commits += 1;
                staged::commit_scanned_items(s, &token, prepared)
            },
            |s| {
                updates += 1;
                assert_eq!(s.full_file_list().len(), 2 + changes.min(2));
            },
        );

        assert!(cost.is_some(), "{changes} changes");
        assert_eq!(commits, 1 + changes.min(1), "{changes} changes");
        assert_eq!(updates, 1, "{changes} changes");
        let state = state.lock();
        let listed = state.full_file_list().iter();
        assert_eq!(
            listed.filter(|item| item.path == src.join("a.rs")).count(),
            1,
            "{changes} changes"
        );
        // The hierarchy came with the lists, unless they were made under the
        // lock (and the command that changed the list built none).
        state.file_tree();
        assert_eq!(
            state.file_tree_build_count() - built,
            usize::from(changes >= 2),
            "{changes} changes"
        );
    }
}

/// The end of the walk leaves the complete list, filtered, and its hierarchy
/// behind, so the state update that reports the scan as finished does not build
/// them with the state locked: with a filter and without.
#[tokio::test]
async fn the_end_of_the_walk_prepares_the_hierarchy_for_the_last_state_update() {
    for query in ["", "a"] {
        let mut harness = TestHarness::new();
        let src = harness.root_path.join("src");
        let token = {
            let mut state = harness.state.lock();
            state.search_query = query.to_string();
            let token = state.begin_scan();
            state.begin_scan_walk(&token);
            token
        };
        assert!(apply_shallow_scan_result(
            &harness.state,
            &harness.proxy,
            &token,
            &harness.root_path.clone(),
            vec![dir_item(&src, 1)],
            HashSet::new(),
        ));

        assert!(apply_deep_scan_result(
            &harness.state,
            &token,
            vec![dir_item(&src, 1), file_item(&src.join("a.rs"), 2)],
            HashSet::new(),
        ));
        let built = harness.state.lock().file_tree_build_count();
        complete_scan(&harness.state, &harness.proxy, &token);

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.is_fully_scanned && !ui_state.is_scanning);
        assert_eq!(ui_state.visible_files_count, 2, "query {query:?}");
        assert_eq!(
            harness.state.lock().file_tree_build_count(),
            built,
            "query {query:?}"
        );
    }
}

/// The pause between two publications grows with what a publication costs, so
/// publishing takes a bounded share of the time however large the list is.
#[test]
fn the_pause_after_a_publication_is_in_proportion_to_its_cost() {
    assert_eq!(
        pause_after_publication(Duration::from_millis(1)),
        MIN_PUBLISH_INTERVAL
    );
    assert_eq!(
        pause_after_publication(Duration::from_millis(400)),
        Duration::from_millis(400) * PUBLISH_PAUSE_FACTOR
    );
}

/// Measurement, not a test: `CFC_PERF_TREE=<dir> cargo test --release --lib
/// perf_scan -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn perf_scan_of_a_large_tree() {
    /// The scan as it was: everything at the end.
    struct AtTheEnd(RealScanner);
    impl Scanner for AtTheEnd {
        async fn scan(
            &self,
            request: &ScanRequest,
            progress: Box<dyn Fn(ScanProgress) + Send + Sync>,
        ) -> ScanOutcome {
            self.0.scan(request, progress).await
        }
    }

    async fn run<S: Scanner + 'static>(
        label: &str,
        root: &Path,
        make: impl Fn(Arc<AtomicBool>) -> S,
    ) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let proxy = TestEventProxy::new(tx);
        let mut app_state = AppState::default();
        app_state.current_path = root.to_string_lossy().to_string();
        let state = SharedState::new(app_state);
        let token = state.lock().begin_scan();
        let scanner = make(token.cancel_flag());
        let started = std::time::Instant::now();
        let task = tokio::spawn(proactive_scan_task(
            proxy,
            state.clone(),
            root.to_path_buf(),
            scanner,
            RealFileSearcher,
            token,
        ));
        let (mut first_level, mut below, mut updates, mut first_count) = (None, None, 0, 0);
        let mut total = None;
        while let Some(event) = rx.recv().await {
            let UserEvent::StateUpdate(ui) = event else {
                continue;
            };
            updates += 1;
            if first_level.is_none() && ui.visible_files_count > 0 {
                first_level = Some(started.elapsed());
                first_count = ui.visible_files_count;
            } else if below.is_none() && ui.visible_files_count > first_count {
                below = Some(started.elapsed());
            }
            if ui.is_fully_scanned && !ui.is_scanning {
                total = Some((started.elapsed(), ui.visible_files_count));
                break;
            }
        }
        task.await.unwrap();
        println!(
            "{label}: first level {first_level:?}, first items below {below:?}, complete {total:?}, {updates} state updates"
        );
    }

    let root = PathBuf::from(std::env::var("CFC_PERF_TREE").expect("CFC_PERF_TREE"));
    let real = |cancel_flag| RealScanner {
        ignore_patterns: HashSet::new(),
        max_file_size: MAX_FILE_SIZE,
        cancel_flag,
    };
    for round in 0..3 {
        run(&format!("[{round}] at the end "), &root, |flag| {
            AtTheEnd(real(flag))
        })
        .await;
        run(&format!("[{round}] incremental"), &root, real).await;
    }
}

/// The items a walk lists for a project of about `count` entries: packages of
/// modules of 25 files each, a directory before its content.
fn synthetic_walk(count: usize) -> Vec<FileItem> {
    const FILES_PER_MODULE: usize = 25;
    const MODULES_PER_PACKAGE: usize = 100;
    let root = Path::new("/root");
    let mut items = Vec::with_capacity(count + count / FILES_PER_MODULE + 64);
    let mut package = 0;
    while items.len() < count {
        let package_path = root.join(format!("package_{package:04}"));
        items.push(dir_item(&package_path, 1));
        for module in 0..MODULES_PER_PACKAGE {
            if items.len() >= count {
                break;
            }
            let module_path = package_path.join(format!("module_{module:03}"));
            items.push(dir_item(&module_path, 2));
            for file in 0..FILES_PER_MODULE {
                items.push(file_item(
                    &module_path.join(format!("source_file_{file:02}.rs")),
                    3,
                ));
            }
        }
        package += 1;
    }
    items
}

/// Measurement, not a test: `cargo test --release --lib perf_lock_hold --
/// --ignored --nocapture`. How long the state is locked for the events that
/// change the file list of a large project (a publication of scanned items, a
/// level loaded on demand, the matches of a content search, the completion of
/// the list), each with its state update, next to the plain way: the same
/// change, the filters and the state update under one lock, which is what
/// these events used to be.
#[test]
#[ignore = "a benchmark, run on request"]
fn perf_lock_hold_of_list_events() {
    #[derive(Clone, Copy, PartialEq)]
    enum Scenario {
        Plain,
        /// A file name filter that matches every fifth file.
        NameFilter,
        /// As after a re-scan of a project the user has worked in: every
        /// directory expanded, every file selected.
        WorkedIn,
        /// Empty directories are removed (from the complete list).
        RemoveEmpty,
    }

    /// A scan of a project of which `listed` entries have been published, and
    /// the items the walk lists after them.
    fn scanning_state(
        listed: usize,
        later: usize,
        scenario: Scenario,
    ) -> (AppState, ScanToken, Vec<FileItem>) {
        let mut walk = synthetic_walk(listed + later);
        let first_level: Vec<FileItem> = walk
            .iter()
            .filter(|item| item.depth == 1)
            .cloned()
            .collect();
        let mut state = AppState::default();
        state.current_path = "/root".to_string();
        match scenario {
            Scenario::Plain => {}
            Scenario::NameFilter => state.search_query = "file_0".to_string(),
            Scenario::RemoveEmpty => state.config.remove_empty_directories = true,
            Scenario::WorkedIn => {
                let (dirs, files): (Vec<_>, Vec<_>) =
                    walk.iter().partition(|item| item.is_directory);
                state.expand_directories(dirs.into_iter().map(|dir| dir.path.clone()));
                state
                    .selected_files
                    .extend(files.into_iter().map(|file| file.path.clone()).collect());
            }
        }
        let later = walk.split_off(listed);
        let token = state.begin_scan();
        state.begin_scan_walk(&token);
        state.replace_with_shallow_file_list(Path::new("/root"), first_level);
        assert!(state.merge_scanned_items(&token, walk));
        filtering::apply_filters(&mut state);
        generate_ui_state(&state);
        (state, token, later)
    }

    /// The content of a directory the walk has not come to.
    fn level() -> (PathBuf, Vec<FileItem>) {
        let directory = PathBuf::from("/root/package_0000/module_000/loaded");
        let items = (0..25)
            .map(|file| file_item(&directory.join(format!("file_{file:02}.rs")), 4))
            .collect();
        (directory, items)
    }

    /// Every tenth file of the list.
    fn matches(state: &AppState) -> HashSet<PathBuf> {
        let files = state
            .full_file_list()
            .iter()
            .filter(|item| !item.is_directory);
        files.step_by(10).map(|item| item.path.clone()).collect()
    }

    fn locked(cost: Option<PublicationCost>) -> Duration {
        cost.expect("the event is not outdated").locked
    }

    fn timed(work: impl FnOnce()) -> Duration {
        let started = std::time::Instant::now();
        work();
        started.elapsed()
    }

    fn millis(durations: &[Duration]) -> String {
        let (min, max) = (
            durations.iter().min().unwrap(),
            durations.iter().max().unwrap(),
        );
        let (min, max) = (min.as_secs_f64() * 1000.0, max.as_secs_f64() * 1000.0);
        if max - min < 0.05 {
            format!("{max:.1} ms")
        } else {
            format!("{min:.1}-{max:.1} ms")
        }
    }

    const ROUNDS: usize = 4;
    for listed in [200_000usize, 1_000_000] {
        for scenario in [
            Scenario::Plain,
            Scenario::NameFilter,
            Scenario::WorkedIn,
            Scenario::RemoveEmpty,
        ] {
            println!(
                "{listed} listed, {}: locked in steps (plain way)",
                match scenario {
                    Scenario::Plain => "no filter",
                    Scenario::NameFilter => "file name filter",
                    Scenario::WorkedIn => "all expanded and selected",
                    Scenario::RemoveEmpty => "empty directories removed",
                }
            );
            let snapshot = |s: &mut AppState| drop(generate_ui_state(s));

            for batch in [4_096usize, listed / 10] {
                let (state, token, later) = scanning_state(listed, ROUNDS * batch, scenario);
                let state = SharedState::new(state);
                let (mut steps, mut total) = (Vec::new(), Vec::new());
                for items in later.chunks(batch).take(ROUNDS) {
                    let cost = add_scanned_items(&state, &token, items.to_vec(), |s| {
                        drop(generate_ui_state(s))
                    })
                    .unwrap();
                    steps.push(cost.locked);
                    total.push(cost.total);
                }
                let (mut state, token, later) = scanning_state(listed, ROUNDS * batch, scenario);
                let mut plain = Vec::new();
                for items in later.chunks(batch).take(ROUNDS) {
                    let items = items.to_vec();
                    plain.push(timed(|| {
                        assert!(state.merge_scanned_items(&token, items));
                        filtering::apply_filters(&mut state);
                        snapshot(&mut state);
                    }));
                }
                println!(
                    "  publication of {batch} items: {} ({}), as a whole {}",
                    millis(&steps),
                    millis(&plain),
                    millis(&total)
                );
            }

            // A level loaded on demand.
            let (state, _token, _) = scanning_state(listed, 0, scenario);
            let lazy_load = state.begin_lazy_load();
            let state = SharedState::new(state);
            let (directory, items) = level();
            let steps = locked(in_three_steps(
                &state,
                |s| {
                    let patterns = HashSet::new();
                    staged::stage_directory_level(s, &lazy_load, &directory, items, patterns)
                },
                |s, prepared| staged::commit_directory_level(s, &lazy_load, prepared),
                snapshot,
            ));
            let (mut state, _token, _) = scanning_state(listed, 0, scenario);
            let lazy_load = state.begin_lazy_load();
            let (directory, items) = level();
            let plain = timed(|| {
                assert!(state.merge_directory_level(&lazy_load, &directory, items, HashSet::new()));
                filtering::apply_filters(&mut state);
                snapshot(&mut state);
            });
            println!(
                "  level loaded on demand: {} ({})",
                millis(&[steps]),
                millis(&[plain])
            );

            // The matches of a content search.
            let (mut state, _token, _) = scanning_state(listed, 0, scenario);
            state.content_search_query = "fn".to_string();
            let search = state.begin_content_search();
            let found = matches(&state);
            let state = SharedState::new(state);
            let steps = locked(in_three_steps(
                &state,
                |s| staged::stage_content_matches(s, &search, found),
                |s, prepared| staged::commit_content_matches(s, &search, prepared),
                |s| {
                    auto_expand_for_matches(s);
                    snapshot(s);
                },
            ));
            let (mut state, _token, _) = scanning_state(listed, 0, scenario);
            state.content_search_query = "fn".to_string();
            let search = state.begin_content_search();
            let found = matches(&state);
            let plain = timed(|| {
                assert!(state.finish_content_search(&search, found));
                filtering::apply_filters(&mut state);
                auto_expand_for_matches(&mut state);
                snapshot(&mut state);
            });
            println!(
                "  matches of a content search: {} ({})",
                millis(&[steps]),
                millis(&[plain])
            );

            // The first level of a re-scan, which replaces the lists.
            let first_level = |state: &AppState| -> Vec<FileItem> {
                let items = state.full_file_list().iter();
                items.filter(|item| item.depth == 1).cloned().collect()
            };
            let (mut state, _token, _) = scanning_state(listed, 0, scenario);
            let files = first_level(&state);
            let _token = state.begin_scan();
            // As `apply_shallow_scan_result` does it: what is replaced is
            // dropped after the lock.
            let mut replaced = None;
            let steps = timed(|| {
                replaced = Some(state.lists_to_outlive_the_lock());
                state.replace_with_shallow_file_list(Path::new("/root"), files);
                filtering::apply_filters(&mut state);
                snapshot(&mut state);
            });
            drop(replaced);
            let (mut state, _token, _) = scanning_state(listed, 0, scenario);
            let files = first_level(&state);
            let _token = state.begin_scan();
            let plain = timed(|| {
                state.replace_with_shallow_file_list(Path::new("/root"), files);
                filtering::apply_filters(&mut state);
                snapshot(&mut state);
            });
            println!(
                "  first level of a re-scan: {} ({})",
                millis(&[steps]),
                millis(&[plain])
            );

            // The completion of the list.
            let (state, token, _) = scanning_state(listed, 0, scenario);
            let state = SharedState::new(state);
            let steps = locked(in_three_steps(
                &state,
                |s| staged::stage_scan_completion(s, &token, HashSet::new()),
                |s, prepared| staged::commit_scan_completion(s, &token, prepared),
                snapshot,
            ));
            let (mut state, token, _) = scanning_state(listed, 0, scenario);
            let plain = timed(|| {
                assert!(state.complete_scanned_file_list(&token, HashSet::new()));
                filtering::apply_filters(&mut state);
                snapshot(&mut state);
            });
            println!(
                "  completion of the list: {} ({})",
                millis(&[steps]),
                millis(&[plain])
            );
        }
    }
}
