use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancel_sent_right_after_a_rescan_cancels_that_rescan() {
    for round in 0..20 {
        let project = project_on_disk("a.txt");
        let harness = Harness::with_project(project.path()).await;

        harness.send("rescanDirectory", serde_json::Value::Null);
        harness.send("cancelScan", serde_json::Value::Null);
        harness.quiescence().await;

        let state = harness.state.lock();
        assert!(!state.is_scanning(), "round {round}");
        assert_eq!(state.status, StatusNotice::ScanCancelled, "round {round}");
        assert!(!state.is_fully_scanned, "round {round}");
        assert!(state.full_file_list().is_empty(), "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_clear_sent_right_after_a_rescan_leaves_no_directory_loaded() {
    for round in 0..20 {
        let project = project_on_disk("a.txt");
        let harness = Harness::with_project(project.path()).await;

        harness.send("rescanDirectory", serde_json::Value::Null);
        harness.send("clearDirectory", serde_json::Value::Null);
        harness.quiescence().await;

        let state = harness.state.lock();
        assert_eq!(state.current_path, "", "round {round}");
        assert!(!state.is_scanning(), "round {round}");
        assert!(state.full_file_list().is_empty(), "round {round}");
        assert_eq!(state.config().last_directory, None, "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn of_two_drops_the_second_one_is_loaded() {
    for round in 0..20 {
        let first = project_on_disk("first.txt");
        let second = project_on_disk("second.txt");
        let harness = Harness::new().await;

        harness.drop_path(first.path());
        harness.drop_path(second.path());
        harness.quiescence().await;

        let state = harness.state.lock();
        assert_eq!(
            Path::new(&state.current_path),
            second.path(),
            "round {round}"
        );
        assert!(state.is_fully_scanned, "round {round}");
        assert!(!state.is_scanning(), "round {round}");
        assert_eq!(
            listed_names(&state),
            ["main.rs", "second.txt", "src"],
            "round {round}"
        );
        assert_eq!(
            state.config().last_directory.as_deref(),
            Some(second.path()),
            "round {round}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_collapse_sent_right_after_a_lazy_load_ends_collapsed() {
    for round in 0..20 {
        let project = project_on_disk("a.txt");
        let harness = Harness::with_project(project.path()).await;
        let src = project.path().join("src");
        harness.list_directory(&src);

        harness.send("loadDirectoryLevel", serde_json::json!(src));
        harness.send(
            "setExpansion",
            serde_json::json!({ "path": src, "expanded": false }),
        );
        harness.quiescence().await;

        let state = harness.state.lock();
        assert!(!state.expanded_dirs.contains(&src), "round {round}");
        // The load itself is not affected by the collapse.
        assert!(state.loaded_dirs.contains(&src), "round {round}");
        assert_eq!(listed_names(&state), ["main.rs", "src"], "round {round}");
    }
}

/// The counterpart: a request to load a directory that follows a collapse expands it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_lazy_load_sent_right_after_a_collapse_ends_expanded() {
    let project = project_on_disk("a.txt");
    let harness = Harness::with_project(project.path()).await;
    let src = project.path().join("src");
    harness.list_directory(&src);

    harness.send(
        "setExpansion",
        serde_json::json!({ "path": src, "expanded": false }),
    );
    harness.send("loadDirectoryLevel", serde_json::json!(src));
    harness.quiescence().await;

    let state = harness.state.lock();
    assert!(state.expanded_dirs.contains(&src));
    assert!(state.loaded_dirs.contains(&src));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_dropped_path_that_does_not_exist_keeps_the_loaded_directory() {
    for round in 0..10 {
        let project = project_on_disk("a.txt");
        let mut harness = Harness::with_scanned_project(project.path()).await;

        harness.drop_path(&project.path().join("does-not-exist"));
        harness.quiescence().await;

        harness.assert_scanned_project_is_untouched(project.path(), &format!("round {round}"));
        let errors = harness.sent_errors();
        assert_eq!(errors.len(), 1, "round {round}: {errors:?}");
        assert!(errors[0].contains("Path does not exist"), "round {round}");
    }
}

/// The same through the directory dialog (a directory can vanish between being
/// picked and being looked at).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_selected_directory_that_does_not_exist_keeps_the_loaded_directory() {
    struct PicksMissingDirectory(PathBuf);
    impl DialogService for PicksMissingDirectory {
        fn pick_directory(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
        fn pick_config_to_import(&self) -> Option<PathBuf> {
            None
        }
        fn export_config_path(&self) -> Option<PathBuf> {
            None
        }
        fn save_output_file_path(&self, _config: &AppConfig) -> Option<PathBuf> {
            None
        }
    }

    let project = project_on_disk("a.txt");
    let mut harness = Harness::with_scanned_project(project.path()).await;

    dispatch_ipc_message(
        &harness.dispatcher,
        serde_json::json!({
            "session": UI,
            "seq": 1,
            "command": "selectDirectory",
            "payload": null,
        })
        .to_string(),
        Arc::new(PicksMissingDirectory(project.path().join("gone"))),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    // The dialog command runs on the blocking lane; wait for its answer.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match harness.events.recv().await.expect("event channel closed") {
                UserEvent::ShowError(message) => {
                    assert!(message.contains("Path does not exist"));
                    break;
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("the missing directory was not reported");
    harness.quiescence().await;

    harness.assert_scanned_project_is_untouched(project.path(), "after the dialog");
}

/// A scan of the loaded directory is running when the path is dropped. The drop
/// supersedes it; when the path turns out not to exist, the scan is started again
/// instead of leaving the directory half listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_missing_drop_during_a_rescan_lets_the_directory_be_scanned_completely() {
    for round in 0..10 {
        let project = project_on_disk("a.txt");
        let mut harness = Harness::with_scanned_project(project.path()).await;

        harness.send("rescanDirectory", serde_json::Value::Null);
        harness.drop_path(&project.path().join("does-not-exist"));
        harness.quiescence().await;

        harness.assert_scanned_project_is_untouched(project.path(), &format!("round {round}"));
        assert_eq!(harness.sent_errors().len(), 1, "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_missing_drop_does_not_undo_the_drop_before_it() {
    for round in 0..10 {
        let project = project_on_disk("a.txt");
        let second = project_on_disk("second.txt");
        let mut harness = Harness::with_scanned_project(project.path()).await;

        harness.drop_path(second.path());
        harness.drop_path(&project.path().join("does-not-exist"));
        harness.quiescence().await;

        let state = harness.state.lock();
        assert_eq!(
            Path::new(&state.current_path),
            second.path(),
            "round {round}"
        );
        assert!(
            state.is_fully_scanned && !state.is_scanning(),
            "round {round}"
        );
        assert_eq!(
            listed_names(&state),
            ["main.rs", "second.txt", "src"],
            "round {round}"
        );
        assert!(state.selected_files.is_empty(), "round {round}");
        drop(state);
        assert_eq!(harness.sent_errors().len(), 1, "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_drop_that_follows_a_missing_one_is_loaded() {
    for round in 0..10 {
        let project = project_on_disk("a.txt");
        let second = project_on_disk("second.txt");
        let harness = Harness::with_scanned_project(project.path()).await;

        harness.drop_path(&project.path().join("does-not-exist"));
        harness.drop_path(second.path());
        harness.quiescence().await;

        let state = harness.state.lock();
        assert_eq!(
            Path::new(&state.current_path),
            second.path(),
            "round {round}"
        );
        assert!(
            state.is_fully_scanned && !state.is_scanning(),
            "round {round}"
        );
        assert_eq!(
            listed_names(&state),
            ["main.rs", "second.txt", "src"],
            "round {round}"
        );
    }
}

/// The ordered-lane guarantee holds for a drop as for a rescan: the cancel that
/// follows it cancels it. What was loaded before is still there, because the
/// cancel is there before the scan has looked at the dropped path.
///
/// That last part is what the single-threaded runtime is for. Both commands are
/// queued before the lane runs, and the lane runs them back to back, so the task
/// of the scan gets its first turn after the cancel. With worker threads the
/// lane may start the drop before the cancel has been queued; the scan then has
/// time to find its path and enter the directory, and the cancel, which still
/// cancels it, leaves the dropped directory loaded (the other legal outcome, see
/// the test below).
#[tokio::test(flavor = "current_thread")]
async fn a_cancel_sent_right_after_a_drop_cancels_the_drop() {
    for round in 0..20 {
        let project = project_on_disk("a.txt");
        let second = project_on_disk("second.txt");
        let harness = Harness::with_scanned_project(project.path()).await;

        harness.drop_path(second.path());
        harness.send("cancelScan", serde_json::Value::Null);
        harness.quiescence().await;

        harness.assert_scanned_project_is_untouched(project.path(), &format!("round {round}"));
        assert_eq!(
            harness.state.lock().status,
            StatusNotice::ScanCancelled,
            "round {round}"
        );
    }
}

/// However a drop and the cancel behind it are timed: the scan ends cancelled,
/// and the directory that is loaded is whole. It is the previous one, untouched,
/// if the cancel came before the scan had found the dropped path, and the
/// dropped one (as far as the scan got) otherwise; never a mixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancel_after_a_drop_leaves_one_of_the_two_directories_loaded() {
    for round in 0..20 {
        let project = project_on_disk("a.txt");
        let second = project_on_disk("second.txt");
        let harness = Harness::with_scanned_project(project.path()).await;

        harness.drop_path(second.path());
        if round % 2 == 1 {
            // Time for the scan to get somewhere.
            tokio::task::yield_now().await;
        }
        harness.send("cancelScan", serde_json::Value::Null);
        harness.quiescence().await;

        let (loaded, status, is_scanning) = {
            let state = harness.state.lock();
            (
                PathBuf::from(&state.current_path),
                state.status.clone(),
                state.is_scanning(),
            )
        };
        assert!(!is_scanning, "round {round}");
        if loaded == project.path() {
            assert_eq!(status, StatusNotice::ScanCancelled, "round {round}");
            harness.assert_scanned_project_is_untouched(project.path(), &format!("round {round}"));
        } else {
            assert_eq!(loaded, second.path(), "round {round}");
            let state = harness.state.lock();
            // Cancelled, unless the scan was through before the cancel came.
            assert!(
                state.status == StatusNotice::ScanCancelled || state.is_fully_scanned,
                "round {round}: {:?}",
                state.status
            );
            // Nothing of the previous directory is left.
            assert!(state.selected_files.is_empty(), "round {round}");
            assert!(state.search_query.is_empty(), "round {round}");
            assert!(
                state
                    .full_file_list()
                    .iter()
                    .all(|item| item.path.starts_with(second.path())),
                "round {round}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_clear_sent_right_after_a_drop_leaves_no_directory_loaded() {
    for round in 0..20 {
        let project = project_on_disk("a.txt");
        let second = project_on_disk("second.txt");
        let harness = Harness::with_scanned_project(project.path()).await;

        harness.drop_path(second.path());
        harness.send("clearDirectory", serde_json::Value::Null);
        harness.quiescence().await;

        let state = harness.state.lock();
        assert_eq!(state.current_path, "", "round {round}");
        assert!(!state.is_scanning(), "round {round}");
        assert!(state.full_file_list().is_empty(), "round {round}");
        assert_eq!(state.config().last_directory, None, "round {round}");
    }
}

/// A rescan asks for a fresh listing of "the directory". Right after a drop that is
/// the dropped one, which is about to be scanned anyway: the rescan must not bring
/// the previous directory back.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_rescan_sent_right_after_a_drop_does_not_undo_the_drop() {
    for round in 0..20 {
        let project = project_on_disk("a.txt");
        let second = project_on_disk("second.txt");
        let harness = Harness::with_scanned_project(project.path()).await;

        harness.drop_path(second.path());
        harness.send("rescanDirectory", serde_json::Value::Null);
        harness.quiescence().await;

        let state = harness.state.lock();
        assert_eq!(
            Path::new(&state.current_path),
            second.path(),
            "round {round}"
        );
        assert!(
            state.is_fully_scanned && !state.is_scanning(),
            "round {round}"
        );
        assert_eq!(
            listed_names(&state),
            ["main.rs", "second.txt", "src"],
            "round {round}"
        );
    }
}

/// An ignore pattern that is added while the dropped path has not been looked at
/// yet applies to the scan of the dropped directory, and does not replace that
/// scan by one of the previous directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_ignore_pattern_added_right_after_a_drop_applies_to_the_dropped_directory() {
    for round in 0..20 {
        let project = project_on_disk("a.txt");
        let second = project_on_disk("second.txt");
        let harness = Harness::with_scanned_project(project.path()).await;

        harness.drop_path(second.path());
        harness.send(
            "addIgnorePattern",
            serde_json::json!({ "pattern": "*.txt" }),
        );
        harness.quiescence().await;

        let state = harness.state.lock();
        assert_eq!(
            Path::new(&state.current_path),
            second.path(),
            "round {round}"
        );
        assert!(
            state.is_fully_scanned && !state.is_scanning(),
            "round {round}"
        );
        assert_eq!(listed_names(&state), ["main.rs", "src"], "round {round}");
    }
}
