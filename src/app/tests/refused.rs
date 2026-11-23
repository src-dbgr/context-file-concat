use super::*;

// --- Messages that are refused -----------------------------------------------------

/// Whatever is wrong with a message, it is treated the same way: nothing changes,
/// and it is answered with one snapshot that accounts for its number. That
/// includes a whole configuration, the payload of the former `updateConfig`: no
/// command accepts one.
#[tokio::test]
async fn a_refused_message_changes_nothing_and_is_answered_under_its_number() {
    use serde_json::json;
    let mut harness = Harness::new().await;
    let whole_config = serde_json::to_value(AppConfig {
        output_filename: "smuggled.txt".to_string(),
        ..harness.state.lock().config.clone()
    })
    .unwrap();
    let before = harness.fingerprint();

    let refused = [
        ("noSuchCommand", json!(null)),
        ("updateConfig", whole_config.clone()),
        ("selectAll", json!({ "all": true })),
        (
            "setConfigOption",
            json!({ "some_random_key": "some_value" }),
        ),
        (
            "setConfigOption",
            json!({ "option": "window_size", "value": [1.0, 1.0] }),
        ),
        (
            "setConfigOption",
            json!({ "option": "last_directory", "value": "/elsewhere" }),
        ),
        (
            "setConfigOption",
            json!({ "option": "max_file_size_mb", "value": 1 }),
        ),
        (
            "setConfigOption",
            json!({ "option": "case_sensitive_search", "value": "yes" }),
        ),
        (
            "setConfigOption",
            json!({ "option": "case_sensitive_search", "value": true, "extra": 1 }),
        ),
        ("setConfigOption", json!({ "option": "output_filename" })),
        ("setConfigOption", whole_config.clone()),
        ("addIgnorePattern", json!("*.log")),
        ("addIgnorePattern", json!({ "pattern": 7 })),
        (
            "addIgnorePattern",
            json!({ "pattern": "*.log", "extra": true }),
        ),
        ("addIgnorePattern", whole_config.clone()),
        ("removeIgnorePattern", json!("*.log")),
        (
            "removeIgnorePattern",
            json!({ "pattern": "*.log", "extra": true }),
        ),
        ("addIgnorePath", json!({ "path": "src" })),
        ("clearIgnorePatterns", json!(["all"])),
        ("setViewport", json!(null)),
        ("setViewport", json!({ "start": -1, "count": 10 })),
        ("setViewport", json!({ "start": 1 })),
        ("setViewport", json!({ "start": 1, "count": 0 })),
        (
            "setViewport",
            json!({ "start": 1, "count": 2, "extra": true }),
        ),
        ("locateRow", json!(null)),
        ("locateRow", json!({ "request": 1 })),
        (
            "locateRow",
            json!({ "request": 1, "target": { "kind": "nope" } }),
        ),
        (
            "locateRow",
            json!({ "request": 1, "target": { "kind": "path", "path": "/x", "extra": 1 } }),
        ),
        (
            "locateRow",
            json!({ "request": -1, "target": { "kind": "path", "path": "/x" } }),
        ),
        ("setExpansion", json!("/repo/src")),
        ("setExpansion", json!({ "path": "/repo/src" })),
        ("setExpansion", json!({ "expanded": true })),
        (
            "setExpansion",
            json!({ "path": "/repo/src", "expanded": "yes" }),
        ),
        (
            "setExpansion",
            json!({ "path": "/repo/src", "expanded": true, "extra": 1 }),
        ),
        ("setExpansion", json!(null)),
        ("expandCollapseAll", json!("not a boolean")),
        ("toggleSelection", json!({ "path": "/repo/src/main.rs" })),
        ("toggleDirectorySelection", json!(null)),
        ("loadDirectoryLevel", json!({ "path": "/repo/src" })),
        ("loadFilePreview", json!(["/repo/src/main.rs"])),
        ("updateFilters", json!("not a map")),
        (
            "updateFilters",
            json!({ "searchQuery": "main", "search_query": "x" }),
        ),
        (
            "updateFilters",
            json!({ "searchQuery": "main", "extra": true }),
        ),
        ("updateFilters", json!({ "searchQuery": 7 })),
        (
            "updateFilters",
            json!({ "searchQuery": "main", "extensionFilter": null }),
        ),
        // A dialog command that is refused opens no dialog.
        ("saveFile", json!(12345)),
        ("saveFile", json!({ "kind": "generated" })),
        (
            "saveFile",
            json!({ "kind": "edited", "content": "x", "documentId": 1 }),
        ),
        ("selectDirectory", json!("/repo")),
    ];
    for (command, payload) in refused {
        let seq = harness.send(command, payload.clone());
        let events = harness.events(1).await;
        assert_eq!(seqs_of(&events), [seq], "the answer to {command} {payload}");
        assert_eq!(harness.fingerprint(), before, "{command} {payload}");
    }

    // An envelope that holds more than a command is refused as well.
    harness.post(
        json!({
            "session": UI,
            "seq": 900,
            "command": "selectAll",
            "payload": null,
            "extra": 1,
        })
        .to_string(),
    );
    assert_eq!(seqs_of(&harness.events(1).await), [900]);
    assert_eq!(harness.fingerprint(), before);

    harness.quiescence().await;
    assert!(harness.events.try_recv().is_err(), "one answer each");
    assert!(
        harness.dialog_opened.try_recv().is_err(),
        "no dialog was opened"
    );
}

/// A message without a number is no command either. It is answered as well, at
/// its place in the order, and the snapshots keep the number they had.
#[tokio::test]
async fn a_message_without_a_number_is_answered_and_accounts_for_nothing() {
    use serde_json::json;
    let mut harness = Harness::new().await;
    let seq = harness.send("selectAll", serde_json::Value::Null);
    assert_eq!(seqs_of(&harness.events(1).await), [seq]);
    let before = harness.fingerprint();

    for message in [
        "not json".to_string(),
        String::new(),
        "[]".to_string(),
        json!({ "command": "deselectAll", "payload": null }).to_string(),
        json!({ "seq": 7, "command": "deselectAll", "payload": null }).to_string(),
        json!({ "session": UI, "command": "deselectAll", "payload": null }).to_string(),
        json!({ "session": UI, "seq": "7", "command": "deselectAll", "payload": null }).to_string(),
        json!({ "session": UI, "seq": -7, "command": "deselectAll", "payload": null }).to_string(),
        json!({ "session": "31", "seq": 7, "command": "deselectAll", "payload": null }).to_string(),
    ] {
        harness.post(message.clone());
        assert_eq!(seqs_of(&harness.events(1).await), [seq], "{message}");
        assert_eq!(harness.fingerprint(), before, "{message}");
    }
}

/// `loadDirectoryLevel` loads a directory of the loaded list and nothing else. A
/// path that is not one is refused, however it is written and whatever it would
/// resolve to: nothing is expanded, nothing outside the project is read, and the
/// request is answered with the unchanged snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lazy_load_of_anything_but_a_listed_directory_is_refused() {
    // Next to the project: a directory whose name starts like the project's, and
    // one that a `..` leads to. Both hold a file that must never be listed.
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("repo");
    let sibling = parent.path().join("repository");
    let secret = parent.path().join("secret");
    for dir in [&project.join("src"), &sibling, &secret] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("inside.txt"), "content").unwrap();
    }
    std::fs::write(project.join("a.txt"), "content").unwrap();
    let src = project.join("src");
    let text = |path: &Path| path.to_string_lossy().into_owned();

    let mut harness = Harness::with_project(&project).await;
    harness.list_directory(&src);
    let before = harness.fingerprint();

    let refused = [
        // Leaves the project and comes back, or does not.
        format!("{}/../secret", text(&project)),
        format!("{}/src/../../secret", text(&project)),
        format!("{}/../repo/src", text(&project)),
        format!("{}/src/../src", text(&project)),
        format!("{}/./../secret", text(&project)),
        format!("{}//../secret", text(&project)),
        format!("{}/src/..", text(&project)),
        format!("{}/..", text(&project)),
        // Outside, without any `..`.
        text(&secret),
        text(&sibling),
        text(parent.path()),
        "/".to_string(),
        String::new(),
        "src".to_string(),
        "../secret".to_string(),
        // Inside, but not a directory of the list: the root itself, a file, a
        // directory the scan has not listed.
        text(&project),
        text(&project.join("a.txt")),
        text(&project.join("src/inside.txt")),
        text(&project.join("not-listed")),
    ];
    for path in &refused {
        let seq = harness.send("loadDirectoryLevel", serde_json::json!(path));
        assert_eq!(seqs_of(&harness.events(1).await), [seq], "{path:?}");
        assert_eq!(harness.fingerprint(), before, "{path:?}");
    }
    harness.quiescence().await;
    assert!(harness.events.try_recv().is_err(), "one answer each");
    {
        let state = harness.state.lock();
        assert_eq!(listed_names(&state), ["src"]);
        assert!(state.loaded_dirs.is_empty());
    }

    // The listed directory itself is loaded, also when it is written with `.`,
    // doubled or trailing separators: that is the same path, and what is listed
    // is written the way the list writes it.
    for spelling in [
        format!("{}/./src", text(&project)),
        format!("{}//src/", text(&project)),
        text(&src),
    ] {
        harness.send("loadDirectoryLevel", serde_json::json!(spelling));
        harness.quiescence().await;
        let state = harness.state.lock();
        assert!(state.loaded_dirs.contains(&src), "{spelling}");
        assert_eq!(listed_names(&state), ["inside.txt", "src"], "{spelling}");
        assert!(
            state.full_file_list().iter().all(|item| text(&item.path)
                .starts_with(&format!("{}/", text(&src)))
                || item.path == src),
            "{spelling}: {:?}",
            state.full_file_list()
        );
        assert!(state
            .expanded_dirs
            .iter()
            .all(|dir| dir.as_os_str() == src.as_os_str()));
    }

    // Without a loaded directory nothing is one of its directories.
    harness.sent_errors();
    harness.state.lock().current_path.clear();
    let unloaded = harness.fingerprint();
    let seq = harness.send("loadDirectoryLevel", serde_json::json!(src));
    assert_eq!(seqs_of(&harness.events(1).await), [seq]);
    assert_eq!(harness.fingerprint(), unloaded);
}

/// `toggleSelection` selects a file of the tree and `setExpansion` expands or
/// collapses a directory of the tree. A path that names neither is refused: the
/// state stays as it is (no selected file that is none, no expansion kept for a
/// directory that might come), and the request is answered with the unchanged
/// snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_command_for_a_path_that_is_no_such_row_is_refused() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("repo");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(project.join("a.txt"), "marker").unwrap();
    std::fs::create_dir_all(parent.path().join("repository/src")).unwrap();
    std::fs::write(parent.path().join("secret.txt"), "secret").unwrap();
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let (file, directory) = (project.join("a.txt"), project.join("src"));

    let mut harness = Harness::with_project(&project).await;
    harness.send("rescanDirectory", serde_json::Value::Null);
    harness.quiescence().await;
    harness.sent_errors();
    let before = harness.fingerprint();
    let count = |harness: &Harness| harness.state.lock().selected_files.len();

    // Paths that name no row at all.
    let no_rows = [
        format!("{}/../secret.txt", text(&project)),
        format!("{}/src/../a.txt", text(&project)),
        format!("{}/../repo/a.txt", text(&project)),
        format!("{}/src/../src", text(&project)),
        format!("{}/..", text(&project)),
        text(&parent.path().join("secret.txt")),
        text(&parent.path().join("repository/src")),
        text(&project.join("not-listed.txt")),
        text(&project.join("not-listed")),
        // The loaded directory itself is no row of its tree.
        text(&project),
        "a.txt".to_string(),
        String::new(),
        // What the UI sends back for a name that is not valid Unicode: the
        // lossy spelling, which is not the path of the entry.
        text(&project.join("caf\u{fffd}.txt")),
        text(&project.join("caf\u{fffd}")),
    ];
    // A row of the other kind: a directory is not selected like a file, a file
    // is not expanded.
    let selections = no_rows.iter().cloned().chain([text(&directory)]);
    let expansions = no_rows.iter().cloned().chain([text(&file)]);

    for path in selections {
        let seq = harness.send("toggleSelection", serde_json::json!(path));
        assert_eq!(seqs_of(&harness.events(1).await), [seq], "{path:?}");
        assert_eq!(harness.fingerprint(), before, "select {path:?}");
        assert_eq!(count(&harness), 0, "{path:?}");
    }
    for path in expansions {
        for expanded in [true, false] {
            let seq = harness.send(
                "setExpansion",
                serde_json::json!({ "path": path, "expanded": expanded }),
            );
            assert_eq!(seqs_of(&harness.events(1).await), [seq], "{path:?}");
            assert_eq!(harness.fingerprint(), before, "expand {path:?}");
            assert!(harness.state.lock().expanded_dirs.is_empty(), "{path:?}");
        }
    }

    // The rows themselves are acted on, also when their path is written with
    // `.` or doubled separators (that is the same path), and what is recorded
    // is the path as the tree writes it.
    harness.send(
        "toggleSelection",
        serde_json::json!(format!("{}/./a.txt", text(&project))),
    );
    harness.send(
        "setExpansion",
        serde_json::json!({ "path": format!("{}//src/", text(&project)), "expanded": true }),
    );
    let answers = harness.events(2).await;
    let UserEvent::StateUpdate(last) = &answers[1] else {
        panic!("expected a snapshot");
    };
    assert_eq!(last.selected_files_count, 1);
    {
        let state = harness.state.lock();
        assert!(state.selected_files.contains(&file));
        assert!(state
            .expanded_dirs
            .iter()
            .all(|dir| dir.as_os_str() == directory.as_os_str()));
        assert_eq!(state.expanded_dirs.len(), 1);
    }

    // Nothing is left behind that a rescan would bring to life.
    harness.send("rescanDirectory", serde_json::Value::Null);
    harness.quiescence().await;
    let state = harness.state.lock();
    assert_eq!(state.selected_files.len(), 1);
    assert_eq!(state.expanded_dirs.len(), 1);
}

/// The other commands that take a path of the UI do not reach beyond the loaded
/// list either, whatever the path is: a preview reads listed files only, an
/// ignore pattern is made of the names below the root, a directory selection
/// selects listed files, and what is generated is made of listed files.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paths_that_leave_the_loaded_directory_reach_nothing() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("repo");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(project.join("a.txt"), "marker").unwrap();
    std::fs::create_dir_all(parent.path().join("repository")).unwrap();
    std::fs::write(parent.path().join("secret.txt"), "secret").unwrap();
    std::fs::write(parent.path().join("repository/secret.txt"), "secret").unwrap();
    let text = |path: &Path| path.to_string_lossy().into_owned();

    let mut harness = Harness::with_scanned_project(&project).await;
    harness.send("updateFilters", serde_json::json!({ "searchQuery": "" }));
    harness.quiescence().await;
    harness.sent_errors();
    let patterns = harness.state.lock().config.ignore_patterns.clone();

    let outside = [
        format!("{}/../secret.txt", text(&project)),
        format!("{}/src/../../secret.txt", text(&project)),
        format!("{}/./../secret.txt", text(&project)),
        format!("{}//../secret.txt", text(&project)),
        format!("{}/../repo/a.txt", text(&project)),
        text(&parent.path().join("secret.txt")),
        text(&parent.path().join("repository/secret.txt")),
        text(&parent.path().join("repository")),
    ];
    for path in &outside {
        // No preview: an error and the unchanged snapshot, no content.
        harness.send("loadFilePreview", serde_json::json!(path));
        harness.quiescence().await;
        let events: Vec<_> = std::iter::from_fn(|| harness.events.try_recv().ok()).collect();
        assert!(
            matches!(
                events.as_slice(),
                [UserEvent::ShowError(_), UserEvent::StateUpdate(_)]
            ),
            "{path}: {events:?}"
        );
        assert_eq!(harness.state.lock().previewed_file_path, None, "{path}");

        // No ignore pattern, and no selection of a directory.
        harness.send("addIgnorePath", serde_json::json!(path));
        harness.send("toggleDirectorySelection", serde_json::json!(path));
        harness.quiescence().await;
        let state = harness.state.lock();
        assert_eq!(state.config.ignore_patterns, patterns, "{path}");
        assert_eq!(state.selected_files.len(), 1, "{path}");
        drop(state);
        harness.sent_errors();
    }

    // Such a path selects nothing either, and what is generated is made of the
    // files of the list in any case.
    for path in &outside {
        harness.send("toggleSelection", serde_json::json!(path));
    }
    harness.quiescence().await;
    let state = harness.state.lock();
    assert_eq!(state.selected_files.len(), 1);
    let plan = tasks::GenerationPlan::of(&state);
    assert_eq!(plan.selected_files, [project.join("a.txt")]);
}
