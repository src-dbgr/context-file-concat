use super::*;

#[tokio::test]
async fn test_load_directory_level_starts_lazy_scan() {
    let mut harness = TestHarness::new();
    let sub_dir = harness.create_dir("src/components");
    harness.create_file("src/components/button.js", "");
    harness.set_initial_files(&["src", "src/components"]);
    // The snapshot only contains children of expanded directories, and a user can
    // only reach `src/components` with `src` open.
    harness
        .state
        .lock()
        .expanded_dirs
        .insert(harness.root_path.join("src"));

    let payload = json!(sub_dir);
    load_directory_level(typed(payload), harness.proxy.clone(), harness.state.clone());

    // The command itself marks the directory as expanded and says so.
    let components_of = |ui_state: &UiState| {
        let src_node = ui_state.row_named("src").unwrap();
        ui_state
            .children_of(src_node)
            .into_iter()
            .find(|n| n.name == "components")
            .cloned()
            .unwrap()
    };
    let announced = harness
        .last_state_update()
        .expect("The command must answer synchronously");
    let components_node = components_of(&announced);
    assert!(components_node.is_expanded && !components_node.children_loaded);
    assert!(harness.state.lock().expanded_dirs.contains(&sub_dir));

    // The lazy load runs in the background and reports its result when it is done.
    let final_state = harness.get_next_event().await;
    let Some(UserEvent::StateUpdate(final_state)) = final_state else {
        panic!("Expected the state update of the lazy load, got {final_state:?}");
    };
    let components_node = components_of(&final_state);
    assert!(components_node.children_loaded);
    let components_row = final_state.row_named("components").unwrap();
    assert!(final_state
        .children_of(components_row)
        .into_iter()
        .any(|n| n.name == "button.js"));
}

/// End to end through the command: the user collapses the directory while its
/// content is being loaded. The result of the load must not open it again.
#[tokio::test]
async fn test_load_directory_level_then_collapse_stays_collapsed() {
    let mut harness = TestHarness::new();
    let src = harness.create_dir("src");
    harness.create_file("src/main.rs", "");
    harness.set_initial_files(&["src"]);

    load_directory_level(
        typed(json!(src)),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    // On the single-threaded test runtime the load has not run yet.
    assert!(harness.state.lock().expanded_dirs.contains(&src));
    set_expansion(
        typed(json!({ "path": src, "expanded": false })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let _ = harness.last_state_update();

    // The result of the load arrives.
    let loaded = harness
        .wait_for_state(|s| {
            s.root_rows()
                .into_iter()
                .any(|n| n.name == "src" && n.children_loaded)
        })
        .await;
    let src_node = loaded
        .root_rows()
        .into_iter()
        .find(|n| n.name == "src")
        .unwrap();
    assert!(src_node.has_children);
    assert!(!src_node.is_expanded, "Directory must stay collapsed");
    let state = harness.state.lock();
    assert!(state.loaded_dirs.contains(&src));
    assert!(!state.expanded_dirs.contains(&src));
}

// ... All other synchronous tests remain unchanged ...
#[tokio::test]
async fn test_toggle_selection_adds_and_removes_file() {
    let mut harness = TestHarness::new();
    let file_path = harness.create_file("test.rs", "");
    harness.set_initial_files(&["test.rs"]);
    let payload = json!(file_path);

    toggle_selection(
        typed(payload.clone()),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let ui_state1 = harness.last_state_update().unwrap();
    assert_eq!(ui_state1.selected_files_count, 1);

    toggle_selection(typed(payload), harness.proxy.clone(), harness.state.clone());
    let ui_state2 = harness.last_state_update().unwrap();
    assert_eq!(ui_state2.selected_files_count, 0);
}

#[tokio::test]
async fn test_toggle_directory_selection_selects_and_deselects_all_children() {
    let mut harness = TestHarness::new();
    harness.create_file("src/main.rs", "");
    let dir_path = harness.create_dir("src");
    harness.set_initial_files(&["src", "src/main.rs"]);
    let payload = json!(dir_path);

    toggle_directory_selection(
        typed(payload.clone()),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let ui_state_select = harness.last_state_update().unwrap();
    assert_eq!(ui_state_select.selected_files_count, 1);

    toggle_directory_selection(typed(payload), harness.proxy.clone(), harness.state.clone());
    let ui_state_deselect = harness.last_state_update().unwrap();
    assert_eq!(ui_state_deselect.selected_files_count, 0);
}

/// A project of `files` files directly below the root.
fn harness_with_files(files: usize) -> TestHarness {
    let harness = TestHarness::new();
    {
        let mut state = harness.state.lock();
        let items: Vec<FileItem> = (0..files)
            .map(|i| file_item(harness.root_path.join(format!("file{i:04}.rs")), false))
            .collect();
        state.set_full_file_list(items.clone());
        state.set_filtered_file_list(items);
    }
    harness
}

#[tokio::test]
async fn test_set_viewport_answers_with_the_rows_of_the_window() {
    let mut harness = harness_with_files(1_000);
    initialize(harness.proxy.clone(), harness.state.clone());
    let initial = harness.last_state_update().unwrap();
    assert_eq!(initial.rows_total, 1_000);
    assert_eq!(initial.rows_start, 0);
    assert_eq!(initial.rows.len(), Viewport::DEFAULT_ROWS);

    set_viewport(
        typed(json!({ "start": 600, "count": 40 })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let scrolled = harness.last_state_update().unwrap();
    assert_eq!((scrolled.rows_start, scrolled.rows.len()), (600, 40));
    assert_eq!(scrolled.rows[0].name, "file0600.rs");
    assert_eq!(scrolled.rows_revision, initial.rows_revision);

    // Every later snapshot carries that window, whatever it answers.
    toggle_selection(
        typed(json!(harness.root_path.join("file0601.rs"))),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let selected = harness.last_state_update().unwrap();
    assert_eq!(selected.rows_start, 600);
    assert_eq!(
        selected.rows[1].selection_state,
        crate::app::selection::SelectionState::Full
    );

    // The host bounds the window; a window behind the end is moved up.
    set_viewport(
        typed(json!({ "start": 990, "count": 1_000_000 })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let capped = harness.last_state_update().unwrap();
    assert_eq!(capped.rows.len(), Viewport::MAX_ROWS);
    assert_eq!(capped.rows_start, 1_000 - Viewport::MAX_ROWS);

    // A new UI starts at the top.
    initialize(harness.proxy.clone(), harness.state.clone());
    assert_eq!(harness.last_state_update().unwrap().rows_start, 0);
}

/// A type-ahead without a prefix is a lookup the types allow, but it asks for
/// nothing: it is answered with the unchanged snapshot.
#[tokio::test]
async fn test_a_lookup_of_nothing_is_answered_unchanged() {
    let mut harness = harness_with_files(300);
    set_viewport(
        typed(json!({ "start": 200, "count": 10 })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    harness.last_state_update().unwrap();

    locate_row(
        typed(json!({ "request": 1, "target": { "kind": "typeahead", "prefix": "", "from": 0 } })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let answer = harness.last_state_update().expect("answered");
    assert_eq!(answer.located, None);
    assert_eq!((answer.rows_start, answer.rows.len()), (200, 10));
}

#[tokio::test]
async fn test_locate_row_answers_with_the_row_of_the_target() {
    let mut harness = TestHarness::new();
    let src = harness.create_dir("src");
    let main = harness.create_file("src/main.rs", "");
    harness.create_file("src/mod.rs", "");
    harness.create_file("readme.md", "");
    harness.set_initial_files(&["src", "src/main.rs", "src/mod.rs", "readme.md"]);
    set_expansion(
        typed(json!({ "path": src, "expanded": true })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    // 0 src, 1 main.rs, 2 mod.rs, 3 readme.md
    let mut locate = |request: u32, target: serde_json::Value| {
        locate_row(
            typed(json!({ "request": request, "target": target })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let located = harness.last_state_update().unwrap().located;
        let located = located.expect("the request is answered");
        assert_eq!(located.request, request);
        located.row
    };

    assert_eq!(locate(1, json!({ "kind": "path", "path": main })), Some(1));
    assert_eq!(
        locate(2, json!({ "kind": "parent_of", "path": main })),
        Some(0)
    );
    // The parent of a row at the top is the loaded directory, which is no row.
    assert_eq!(locate(3, json!({ "kind": "parent_of", "path": src })), None);
    assert_eq!(
        locate(4, json!({ "kind": "typeahead", "prefix": "MO", "from": 0 })),
        Some(2)
    );
    // From the row after the focused one, wrapping around.
    assert_eq!(
        locate(5, json!({ "kind": "typeahead", "prefix": "m", "from": 2 })),
        Some(2)
    );
    assert_eq!(
        locate(6, json!({ "kind": "typeahead", "prefix": "m", "from": 3 })),
        Some(1)
    );
    assert_eq!(
        locate(7, json!({ "kind": "typeahead", "prefix": "zz", "from": 0 })),
        None
    );

    // The answer stays true when the list changes: `main.rs` is no row any more.
    assert_eq!(locate(8, json!({ "kind": "path", "path": main })), Some(1));
    set_expansion(
        typed(json!({ "path": src, "expanded": false })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let collapsed = harness.last_state_update().unwrap();
    assert_eq!(
        collapsed.located.as_ref().map(|located| located.row),
        Some(None)
    );
    assert_eq!(collapsed.rows_total, 2);
}

#[tokio::test]
async fn test_set_expansion_is_idempotent() {
    let mut harness = TestHarness::new();
    let dir = harness.create_dir("src");
    harness.set_initial_files(&["src"]);
    let set = |expanded: bool| json!({ "path": dir, "expanded": expanded });

    // Expanding twice (key repeat, double click) leaves the directory expanded,
    // and each request is answered with a snapshot.
    for _ in 0..2 {
        set_expansion(
            typed(set(true)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.root_rows()[0].is_expanded);
    }
    assert_eq!(harness.state.lock().expanded_dirs.len(), 1);

    for _ in 0..2 {
        set_expansion(
            typed(set(false)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.root_rows()[0].is_expanded);
    }
    assert!(harness.state.lock().expanded_dirs.is_empty());
}

#[tokio::test]
async fn test_set_expansion_only_affects_the_given_directory() {
    let mut harness = TestHarness::new();
    let src = harness.create_dir("src");
    let docs = harness.create_dir("docs");
    harness.set_initial_files(&["docs", "src"]);
    harness.state.lock().expanded_dirs.insert(docs.clone());

    set_expansion(
        typed(json!({ "path": src, "expanded": true })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    set_expansion(
        typed(json!({ "path": src, "expanded": false })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let _ = harness.last_state_update().unwrap();

    let state = harness.state.lock();
    assert_eq!(state.expanded_dirs.len(), 1);
    assert!(state.expanded_dirs.contains(&docs));
}

#[tokio::test]
async fn test_expand_collapse_all() {
    let mut harness = TestHarness::new();
    harness.create_dir("src");
    harness.set_initial_files(&["src"]);

    expand_collapse_all(
        typed(json!(true)),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let ui_state1 = harness.last_state_update().unwrap();
    assert_eq!(
        ui_state1
            .root_rows()
            .into_iter()
            .filter(|n| n.is_expanded)
            .count(),
        1
    );

    expand_collapse_all(
        typed(json!(false)),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    let ui_state2 = harness.last_state_update().unwrap();
    assert!(ui_state2.root_rows().into_iter().all(|n| !n.is_expanded));
}

#[tokio::test]
async fn test_select_all_and_deselect_all() {
    let mut harness = TestHarness::new();
    harness.create_file("file1.txt", "");
    harness.create_file("file2.txt", "");
    harness.set_initial_files(&["file1.txt", "file2.txt"]);

    select_all(harness.proxy.clone(), harness.state.clone());
    let ui_state1 = harness.last_state_update().unwrap();
    assert_eq!(ui_state1.selected_files_count, 2);

    deselect_all(harness.proxy.clone(), harness.state.clone());
    let ui_state2 = harness.last_state_update().unwrap();
    assert_eq!(ui_state2.selected_files_count, 0);
}

// --- Expansion under a filter ---

/// `a/x.rs`, `a/deep/y.rs` and `b/z.md`, completely loaded.
fn harness_with_two_branches() -> (TestHarness, PathBuf, PathBuf, PathBuf) {
    let harness = TestHarness::new();
    let (a, deep, b) = (
        harness.root_path.join("a"),
        harness.root_path.join("a").join("deep"),
        harness.root_path.join("b"),
    );
    {
        let mut state = harness.state.lock();
        state.replace_with_complete_file_list(vec![
            file_item(a.clone(), true),
            file_item(deep.clone(), true),
            file_item(b.clone(), true),
            file_item(a.join("x.rs"), false),
            file_item(deep.join("y.rs"), false),
            file_item(b.join("z.md"), false),
        ]);
        state.is_fully_scanned = true;
        filtering::apply_filters(&mut state);
    }
    (harness, a, deep, b)
}

async fn type_filter(harness: &TestHarness, search_query: &str, extension_filter: &str) {
    let payload = UpdateFiltersPayload {
        search_query: search_query.to_string(),
        extension_filter: extension_filter.to_string(),
        content_search_query: String::new(),
    };
    update_filters(
        typed(serde_json::to_value(payload).unwrap()),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
}

fn expanded(harness: &TestHarness) -> HashSet<PathBuf> {
    harness.state.lock().expanded_dirs.clone()
}

/// "Expand all" under a filter expands what the filter shows and keeps what was
/// expanded outside of it.
#[tokio::test]
async fn test_expand_all_under_a_filter_keeps_expansions_outside_the_filter() {
    let (harness, a, deep, b) = harness_with_two_branches();
    harness.state.lock().expand_directories([b.clone()]);
    type_filter(&harness, "", "rs").await;
    harness
        .state
        .lock()
        .collapse_directories([a.clone(), deep.clone()]);

    expand_collapse_all(
        typed(json!(true)),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    assert_eq!(
        expanded(&harness),
        HashSet::from([a.clone(), deep.clone(), b.clone()]),
        "`b` is hidden by the filter and must stay expanded"
    );
    type_filter(&harness, "", "").await;
    assert_eq!(expanded(&harness), HashSet::from([a, deep, b]));
}

/// "Collapse all" under a filter collapses what the filter shows, nothing else.
#[tokio::test]
async fn test_collapse_all_under_a_filter_keeps_expansions_outside_the_filter() {
    let (mut harness, a, deep, b) = harness_with_two_branches();
    harness
        .state
        .lock()
        .expand_directories([a, deep, b.clone()]);
    type_filter(&harness, "", "rs").await;

    expand_collapse_all(
        typed(json!(false)),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    assert_eq!(expanded(&harness), HashSet::from([b.clone()]));
    let ui_state = harness.last_state_update().unwrap();
    assert!(ui_state
        .root_rows()
        .into_iter()
        .all(|node| !node.is_expanded));

    type_filter(&harness, "", "").await;
    let ui_state = harness.last_state_update().unwrap();
    let b_node = ui_state
        .root_rows()
        .into_iter()
        .find(|node| node.path == b)
        .unwrap();
    assert!(b_node.is_expanded, "expanded before the filter, and still");
}

/// Without a filter the two reach every directory, as they always did.
#[tokio::test]
async fn test_expand_and_collapse_all_without_a_filter_reach_every_directory() {
    let (harness, a, deep, b) = harness_with_two_branches();

    expand_collapse_all(
        typed(json!(true)),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    assert_eq!(expanded(&harness), HashSet::from([a, deep, b]));

    expand_collapse_all(
        typed(json!(false)),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    assert!(expanded(&harness).is_empty());
}

/// The filter opens the directories of its matches. A directory the user then
/// collapses stays collapsed while the user keeps typing.
#[tokio::test]
async fn test_collapse_then_type_stays_collapsed() {
    let (mut harness, a, deep, _) = harness_with_two_branches();
    type_filter(&harness, "r", "").await;
    assert_eq!(expanded(&harness), HashSet::from([a.clone(), deep.clone()]));

    set_expansion(
        typed(json!({ "path": a, "expanded": false })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    type_filter(&harness, "rs", "").await;
    type_filter(&harness, "x", "rs").await;

    assert!(!expanded(&harness).contains(&a), "must not be re-opened");
    let ui_state = harness.last_state_update().unwrap();
    let a_node = ui_state
        .root_rows()
        .into_iter()
        .find(|node| node.path == a)
        .unwrap();
    assert!(!a_node.is_expanded && a_node.has_children);
}

/// A match in a directory the user has not collapsed is still shown, also while
/// another directory is held collapsed.
#[tokio::test]
async fn test_new_filter_text_still_expands_the_directory_of_a_new_match() {
    let (harness, a, deep, b) = harness_with_two_branches();
    type_filter(&harness, "", "rs").await;
    set_expansion(
        typed(json!({ "path": a, "expanded": false })),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    // The filter now matches in `b`, which was not expanded so far.
    type_filter(&harness, "", "md").await;
    assert_eq!(expanded(&harness), HashSet::from([deep.clone(), b.clone()]));

    // Expanding by hand ends the hold on that directory.
    set_expansion(
        typed(json!({ "path": a, "expanded": true })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    set_expansion(
        typed(json!({ "path": a, "expanded": false })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    // ... and so does clearing the filters: the next filter starts afresh.
    type_filter(&harness, "", "").await;
    type_filter(&harness, "x", "").await;
    assert!(expanded(&harness).contains(&a));
}

/// "Collapse all" under a filter is a collapse by the user as well.
#[tokio::test]
async fn test_collapse_all_then_type_stays_collapsed() {
    let (harness, a, deep, _) = harness_with_two_branches();
    type_filter(&harness, "r", "").await;
    assert_eq!(expanded(&harness), HashSet::from([a.clone(), deep.clone()]));

    expand_collapse_all(
        typed(json!(false)),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    type_filter(&harness, "rs", "").await;

    assert!(!expanded(&harness).contains(&a));
    assert!(!expanded(&harness).contains(&deep));
}

/// Changing the case sensitivity applies the filters again and repeats the
/// content search; neither re-opens what the user collapsed.
#[tokio::test]
async fn test_case_toggle_does_not_reopen_a_collapsed_directory() {
    let mut harness = TestHarness::new();
    let file = harness.create_file("src/main.rs", "fn main() { needle }");
    let src = harness.root_path.join("src");
    harness.set_initial_files(&["src", "src/main.rs"]);
    update_filters(
        typed(json!({ "contentSearchQuery": "needle" })),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
    assert_eq!(expanded(&harness), HashSet::from([src.clone()]));

    set_expansion(
        typed(json!({ "path": src, "expanded": false })),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;

    let state = harness.state.lock();
    assert_eq!(*state.content_search_results(), HashSet::from([file]));
    assert!(state.expanded_dirs.is_empty(), "must not be re-opened");
    drop(state);
    let _ = harness.last_state_update();
}

#[tokio::test]
async fn test_toggle_directory_selection_from_partial() {
    let mut harness = TestHarness::new();
    let dir_path = harness.create_dir("src");
    let file1 = harness.create_file("src/file1.rs", "");
    harness.create_file("src/file2.rs", "");
    harness.set_initial_files(&["src", "src/file1.rs", "src/file2.rs"]);

    // 1. Select one file to create a "partial" state
    {
        let mut state = harness.state.lock();
        state.selected_files.insert(file1);
    }

    // 2. Toggle the directory. It should now select all files.
    toggle_directory_selection(
        typed(json!(dir_path)),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    let ui_state = harness.last_state_update().unwrap();
    assert_eq!(
        ui_state.selected_files_count, 2,
        "Directory should be fully selected from partial state"
    );
}
