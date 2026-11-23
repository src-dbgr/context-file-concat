use super::*;

// --- Commands and the snapshots that answer them -------------------------------------
//
// The UI numbers its commands, and every snapshot states the number of the last
// command it accounts for (see `correlation`).

struct NoWake;

impl transport::WakeSink for NoWake {
    fn wake(&self) {}
}

/// Snapshots nobody has fetched yet are coalesced to the latest one. That one
/// accounts for every command, so the UI is not left waiting for a number.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_coalesced_snapshot_carries_the_number_of_the_last_command() {
    let harness = Harness::new().await;
    let (proxy, outbox) = transport::channel(NoWake);

    let mut last = 0;
    for start in 0..50 {
        last = start + 1;
        dispatch_ipc_message(
            &harness.dispatcher,
            serde_json::json!({
                "session": UI,
                "seq": last,
                "command": "setViewport",
                "payload": { "start": start, "count": 10 },
            })
            .to_string(),
            harness.dialog.clone(),
            proxy.clone(),
            harness.state.clone(),
        );
    }
    harness.quiescence().await;

    let Some(UserEvent::StateUpdate(snapshot)) = outbox.next_event() else {
        panic!("expected a snapshot");
    };
    assert_eq!(snapshot.accounts_for(), command_of_ui(last));
    assert!(outbox.next_event().is_none(), "one snapshot is left");
}

/// A scan sends snapshots of its own while commands are executed. Whatever the
/// interleaving, the numbers never go back, every command is answered under its
/// number, and a snapshot that accounts for a command shows what it has done.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn snapshots_of_background_work_never_run_ahead_of_the_commands() {
    let project = tempfile::tempdir().unwrap();
    for dir in 0..40 {
        let dir_path = project.path().join(format!("dir-{dir:02}"));
        std::fs::create_dir_all(&dir_path).unwrap();
        for file in 0..25 {
            std::fs::write(dir_path.join(format!("file-{file:02}.txt")), "x").unwrap();
        }
    }
    let mut harness = Harness::with_project(project.path()).await;

    harness.send("rescanDirectory", serde_json::Value::Null);
    let mut filename_of = std::collections::BTreeMap::new();
    for round in 0..200 {
        let name = format!("{round}.txt");
        let seq = harness.send(
            "setConfigOption",
            serde_json::json!({ "option": "output_filename", "value": name }),
        );
        filename_of.insert(seq, name);
        if round % 20 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    let last = *filename_of.keys().last().unwrap();
    harness.quiescence().await;

    let snapshots: Vec<_> = std::iter::from_fn(|| harness.events.try_recv().ok())
        .filter_map(|event| match event {
            UserEvent::StateUpdate(snapshot) => Some(snapshot),
            _ => None,
        })
        .collect();
    let seqs: Vec<_> = snapshots.iter().map(|s| s.last_processed_seq).collect();
    assert!(seqs.windows(2).all(|pair| pair[0] <= pair[1]), "{seqs:?}");
    assert_eq!(seqs.last(), Some(&last));
    for snapshot in &snapshots {
        // What the snapshot shows is the edit of the command it accounts for:
        // not an older one, and (the commands run one after the other) no newer.
        if let Some(name) = filename_of.get(&snapshot.last_processed_seq) {
            assert_eq!(&snapshot.config.output_filename, name);
        }
    }
    for seq in filename_of.keys() {
        assert!(seqs.contains(seq), "command {seq} was not answered");
    }
}

/// A command that waits for a content search is accounted for when it suspends:
/// its first answer carries its number, and the matches come later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_that_suspends_is_accounted_for_by_its_first_answer() {
    let project = project_on_disk("a.txt");
    let mut harness = Harness::with_scanned_project(project.path()).await;

    let seq = harness.send(
        "updateFilters",
        serde_json::json!({ "contentSearchQuery": "main" }),
    );
    let first = harness.events(1).await;
    assert_eq!(seqs_of(&first), [seq]);
    let UserEvent::StateUpdate(first) = &first[0] else {
        unreachable!()
    };
    assert_eq!(first.content_search_query, "main");

    // The matches arrive with a later snapshot, which states the number that is
    // current then.
    let later = harness.send("selectAll", serde_json::Value::Null);
    harness.quiescence().await;
    let rest: Vec<_> = std::iter::from_fn(|| harness.events.try_recv().ok()).collect();
    let seqs = seqs_of(&rest);
    assert!(seqs.len() >= 2, "{seqs:?}");
    assert!(seqs.iter().all(|seq_of| [seq, later].contains(seq_of)));
    assert_eq!(seqs.last(), Some(&later));
}

/// The answer to a command is the first snapshot that names it, and nothing that
/// is sent before it does: not even while the command is already running.
///
/// The command is held right before it runs, on the ordered lane. A background
/// task sends a snapshot at that moment; it shows the state from before the
/// command and has to say so. (Naming the command as soon as it starts, or
/// taking "a command is running" for "this is the command that sends", would
/// let that snapshot pass for the answer.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_snapshot_sent_while_a_command_is_about_to_run_does_not_name_it() {
    let mut harness = Harness::new().await;
    let old_name = harness.state.lock().config.output_filename.clone();
    let before = harness.send("selectAll", serde_json::Value::Null);
    assert_eq!(seqs_of(&harness.events(1).await), [before]);

    let proxy = Correlated::new(harness.proxy.clone(), harness.dispatcher.progress().clone());
    let command = Command::SetConfigOption(commands::ConfigOption::OutputFilename(
        "new.txt".to_string(),
    ));
    let Work::Ordered(command) =
        command.into_work(harness.dialog.clone(), proxy.clone(), harness.state.clone())
    else {
        panic!("setConfigOption runs on the ordered lane");
    };
    let (entered, has_entered) = std_mpsc::channel();
    let (proceed, may_proceed) = std_mpsc::channel::<()>();
    harness
        .dispatcher
        .dispatch(Some(command_of_ui(before + 1)), async move {
            entered.send(()).unwrap();
            may_proceed.recv().unwrap();
            command.await;
        });
    has_entered
        .recv_timeout(Duration::from_secs(5))
        .expect("the command was not started");

    // What a background task does: lock the state, send its snapshot.
    send_snapshot(&proxy, &harness.state.lock());
    proceed.send(()).unwrap();

    let events = harness.events(2).await;
    assert_eq!(seqs_of(&events), [before, before + 1]);
    let filenames: Vec<_> = events
        .iter()
        .map(|event| match event {
            UserEvent::StateUpdate(snapshot) => snapshot.config.output_filename.clone(),
            other => panic!("expected a snapshot, got {other:?}"),
        })
        .collect();
    assert_eq!(filenames, [old_name, "new.txt".to_string()]);
}

/// A UI that is loaded anew has another session and counts from 1 again. The
/// snapshots name the UI whose command they account for, so one that was made
/// for the UI before cannot pass for an answer to the new one.
#[tokio::test]
async fn the_snapshots_name_the_ui_whose_command_they_account_for() {
    let mut harness = Harness::new().await;
    let named = |events: &[UserEvent]| match &events[0] {
        UserEvent::StateUpdate(snapshot) => snapshot.accounts_for(),
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let post = |harness: &Harness, session: SessionId, seq: CommandSeq| {
        harness.post(
            serde_json::json!({
                "session": session,
                "seq": seq,
                "command": "selectAll",
                "payload": null,
            })
            .to_string(),
        );
        CommandId { session, seq }
    };

    let old = post(&harness, 1001, 500);
    assert_eq!(named(&harness.events(1).await), old);
    // Something else sends a snapshot: still for the UI that was there.
    harness.drop_path(Path::new("/no/such/path"));
    assert_eq!(named(&harness.events(1).await), old);

    let new = post(&harness, 1002, 1);
    harness.quiescence().await;
    let rest: Vec<_> = std::iter::from_fn(|| harness.events.try_recv().ok())
        .filter(|event| matches!(event, UserEvent::StateUpdate(_)))
        .collect();
    assert_eq!(named(&rest[rest.len() - 1..]), new);
    assert!(rest
        .iter()
        .all(|event| [old, new].contains(&named(std::slice::from_ref(event)))));
}

/// A dialog command is accounted for when the ordered lane reaches it, not when
/// its dialog closes: a snapshot sent while the dialog is open carries its number.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dialog_command_is_accounted_for_when_its_turn_has_come() {
    let mut harness = Harness::new().await;

    let dialog = harness.send("pickOutputDirectory", serde_json::Value::Null);
    harness
        .dialog_opened
        .recv_timeout(Duration::from_secs(5))
        .expect("the dialog was not opened");
    assert_eq!(
        harness.dispatcher.progress().last_processed(),
        command_of_ui(dialog)
    );

    // Something that is no command of the UI sends a snapshot meanwhile.
    harness.drop_path(Path::new("/no/such/path"));
    let events = harness.events(1).await;
    assert_eq!(seqs_of(&events), [dialog]);

    let next = harness.send("selectAll", serde_json::Value::Null);
    harness.quiescence().await;
    let rest: Vec<_> = std::iter::from_fn(|| harness.events.try_recv().ok()).collect();
    assert_eq!(seqs_of(&rest).last(), Some(&next));

    // The dialog is dismissed: nothing is sent, and the number stays.
    harness.close_dialog.send(()).unwrap();
    harness.quiescence().await;
    assert_eq!(
        harness.dispatcher.progress().last_processed(),
        command_of_ui(next)
    );
}
