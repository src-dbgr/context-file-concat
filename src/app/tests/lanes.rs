use super::*;

/// The lane of a command is decided where its handler is bound
/// (`Command::into_work`): exactly the commands that open a dialog block.
#[tokio::test]
async fn every_dialog_command_is_routed_to_the_blocking_lane() {
    let harness = Harness::new().await;
    let mut blocking = std::collections::BTreeSet::new();
    for command in super::command_contract_tests::example_commands() {
        let name = command.wire_name();
        let work = command.into_work(
            harness.dialog.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        if matches!(work, Work::Blocking(_)) {
            blocking.insert(name);
        }
    }
    assert_eq!(
        blocking.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "exportConfig",
            "importConfig",
            "pickOutputDirectory",
            "saveFile",
            "selectDirectory",
        ]
    );
}

/// Two quick edits arrive before the UI has rendered the answer to the first one.
/// Each is an intent that the host applies to its own configuration, so neither
/// is lost and nothing the UI does not edit is touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quick_config_intents_are_all_applied() {
    let mut harness = Harness::new().await;
    {
        let mut state = harness.state.lock();
        state.config.ignore_patterns.clear();
        state.config.max_file_size_mb = 7;
    }

    harness.send("addIgnorePattern", serde_json::json!({ "pattern": "a" }));
    harness.send("addIgnorePattern", serde_json::json!({ "pattern": "b" }));
    harness.send(
        "setConfigOption",
        serde_json::json!({ "option": "use_relative_paths", "value": false }),
    );
    harness.send("removeIgnorePattern", serde_json::json!({ "pattern": "a" }));
    harness.send("addIgnorePattern", serde_json::json!({ "pattern": "c" }));
    let events = harness.events(5).await;

    let UserEvent::StateUpdate(last) = events.last().unwrap() else {
        panic!("expected a state update");
    };
    assert_eq!(last.config.ignore_patterns, ["b", "c"]);
    assert!(!last.config.use_relative_paths);
    assert_eq!(harness.state.lock().config.max_file_size_mb, 7);

    harness.send("clearIgnorePatterns", serde_json::Value::Null);
    harness.events(1).await;
    assert!(harness.state.lock().config.ignore_patterns.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn commands_take_effect_in_the_order_they_were_sent() {
    let mut harness = Harness::new().await;

    // Each round ends with a different final command; with unordered execution
    // some rounds would end in the wrong state.
    for round in 0..200 {
        let ends_expanded = round % 2 == 0;
        let mut sent = Vec::new();
        for step in 0..5 {
            let expanded = (step % 2 == 0) == ends_expanded;
            sent.push(harness.send(
                "setExpansion",
                serde_json::json!({ "path": "/repo/src", "expanded": expanded }),
            ));
        }
        // Each command is answered by a snapshot that carries its number.
        assert_eq!(seqs_of(&harness.events(5).await), sent, "{round}");
        assert_eq!(harness.is_expanded("/repo/src"), ends_expanded, "{round}");
    }
}

/// A command that flips a state shows whether every command is executed exactly
/// once: a lost or repeated one would leave the file deselected.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_odd_number_of_toggles_toggles_once() {
    let mut harness = Harness::new().await;

    for _ in 0..101 {
        harness.send("toggleSelection", serde_json::json!("/repo/src/main.rs"));
    }
    harness.events(101).await;

    let state = harness.state.lock();
    assert!(state
        .selected_files
        .contains(&PathBuf::from("/repo/src/main.rs")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_dialog_does_not_hold_up_other_commands() {
    let mut harness = Harness::new().await;

    harness.send("pickOutputDirectory", serde_json::Value::Null);
    harness
        .dialog_opened
        .recv_timeout(Duration::from_secs(5))
        .expect("the dialog was not opened");

    harness.send(
        "setExpansion",
        serde_json::json!({ "path": "/repo/src", "expanded": true }),
    );
    harness.events(1).await;
    assert!(harness.is_expanded("/repo/src"));

    harness.close_dialog.send(()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_dialog_is_open_at_a_time() {
    let mut harness = Harness::new().await;

    harness.send(
        "saveFile",
        serde_json::json!({ "kind": "edited", "content": "content" }),
    );
    harness.send("importConfig", serde_json::Value::Null);
    harness
        .dialog_opened
        .recv_timeout(Duration::from_secs(5))
        .expect("the first dialog was not opened");
    assert!(
        harness
            .dialog_opened
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "the second dialog must wait for the first one"
    );

    // Cancelling the save dialog is answered, then the import dialog opens.
    harness.close_dialog.send(()).unwrap();
    let events = harness.events(1).await;
    assert!(matches!(
        events[0],
        UserEvent::SaveResult(SaveResult::Cancelled)
    ));
    harness
        .dialog_opened
        .recv_timeout(Duration::from_secs(5))
        .expect("the second dialog was not opened");
    harness.close_dialog.send(()).unwrap();
}

// --- Panics ------------------------------------------------------------------------

/// A command panics while it holds the state lock. The panic is contained by the
/// dispatcher, and the lock it poisoned is taken over: the commands that follow
/// work, on every lane, and so do background tasks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_that_panics_under_the_state_lock_does_not_end_the_application() {
    let project = project_on_disk("a.txt");
    let mut harness = Harness::with_project(project.path()).await;

    let state = harness.state.clone();
    harness.dispatcher.dispatch(None, async move {
        let mut guard = state.lock();
        guard.search_query = "set before the panic".to_string();
        panic!("deliberate panic in a command holding the state lock");
    });

    // The ordered lane, a background task (the scan) and its state updates.
    harness.send("rescanDirectory", serde_json::Value::Null);
    harness.send("selectAll", serde_json::Value::Null);
    harness.quiescence().await;
    // The blocking lane.
    harness.send("selectDirectory", serde_json::Value::Null);
    harness
        .dialog_opened
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    harness.close_dialog.send(()).unwrap();
    harness.quiescence().await;

    let state = harness.state.lock();
    assert_eq!(state.search_query, "set before the panic");
    assert!(state.is_fully_scanned && !state.is_scanning());
    drop(state);
    let mut last_update = None;
    while let Ok(event) = harness.events.try_recv() {
        if let UserEvent::StateUpdate(update) = event {
            last_update = Some(update);
        }
    }
    let last_update = last_update.expect("state updates keep being sent");
    assert!(last_update.is_fully_scanned);
}

// --- Dialog commands and the ordered lane ---

/// A save dialog that is cancelled at once and remembers the name it was to
/// propose.
#[derive(Default)]
struct RecordingDialog {
    proposed: Mutex<Vec<String>>,
}

impl DialogService for RecordingDialog {
    fn pick_directory(&self) -> Option<PathBuf> {
        None
    }
    fn pick_config_to_import(&self) -> Option<PathBuf> {
        None
    }
    fn export_config_path(&self) -> Option<PathBuf> {
        None
    }
    fn save_output_file_path(&self, config: &AppConfig) -> Option<PathBuf> {
        self.proposed
            .lock()
            .unwrap()
            .push(config.output_filename.clone());
        None
    }
}

/// Renaming the output file and pressing "Save" right away: the dialog command
/// starts after the rename that was posted before it, so it proposes the new name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn save_dialog_sees_the_config_edit_posted_right_before_it() {
    let dispatcher = CommandDispatcher::new();
    let dialog = Arc::new(RecordingDialog::default());
    let (tx, mut events) = mpsc::unbounded_channel();
    let proxy = TestEventProxy::lenient(tx);
    let state = SharedState::new(AppState::default());
    let last_seq = AtomicU64::new(0);
    let send = |command: &str, payload: serde_json::Value| {
        let seq = last_seq.fetch_add(1, Ordering::SeqCst) + 1;
        dispatch_ipc_message(
            &dispatcher,
            serde_json::json!({
                "session": UI,
                "seq": seq,
                "command": command,
                "payload": payload,
            })
            .to_string(),
            dialog.clone(),
            proxy.clone(),
            state.clone(),
        );
    };

    // Each round waits for its dialog: commands posted *after* a dialog command
    // are deliberately not held back by it, so they would run ahead.
    for round in 0..200 {
        let name = format!("{round}.txt");
        send(
            "setConfigOption",
            serde_json::json!({ "option": "output_filename", "value": name }),
        );
        send(
            "saveFile",
            serde_json::json!({ "kind": "edited", "content": "content" }),
        );

        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .expect("the save dialog did not run")
                .expect("the event channel was closed");
            if matches!(event, UserEvent::SaveResult(SaveResult::Cancelled)) {
                break;
            }
        }
        assert_eq!(dialog.proposed.lock().unwrap().last(), Some(&name));
    }
}
