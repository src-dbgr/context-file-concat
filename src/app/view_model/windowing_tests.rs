//! Tests for the window of rows a snapshot carries: which rows it holds, when the row
//! list is rebuilt, how a row is looked up, and what a snapshot of a very large,
//! fully expanded project costs.

use super::rows::{all_rows, rows_build_count, typeahead_match};
use super::{generate_ui_state, LocatedRow, UiState, Viewport};
use crate::app::filtering;
use crate::app::selection::SelectionState;
use crate::app::state::AppState;
use crate::config::AppConfig;
use crate::core::FileItem;
use proptest::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn item(path: &str, is_directory: bool) -> FileItem {
    FileItem {
        path: PathBuf::from(path),
        is_directory,
        size: if is_directory { 0 } else { 100 },
        ..Default::default()
    }
}

fn state_of(items: Vec<FileItem>) -> AppState {
    let mut state = AppState::default();
    *state.config_mut() = AppConfig {
        ignore_patterns: HashSet::new(),
        remove_empty_directories: false,
        ..Default::default()
    };
    state.current_path = "/root".to_string();
    state.loaded_dirs = items
        .iter()
        .filter(|item| item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    state.set_full_file_list(items);
    state.is_fully_scanned = true;
    filtering::apply_filters(&mut state);
    state
}

/// `/root/src/{core/{a.rs,b.rs},main.rs}`, `/root/docs/guide.md`, `/root/readme.md`.
fn project() -> AppState {
    state_of(vec![
        item("/root/src", true),
        item("/root/src/core", true),
        item("/root/src/core/a.rs", false),
        item("/root/src/core/b.rs", false),
        item("/root/src/main.rs", false),
        item("/root/docs", true),
        item("/root/docs/guide.md", false),
        item("/root/readme.md", false),
    ])
}

fn expand(state: &mut AppState, paths: &[&str]) {
    state.expand_directories(paths.iter().map(PathBuf::from));
}

fn view(state: &mut AppState, start: usize, count: usize) -> UiState {
    state.tree_view.set_viewport(Viewport::new(start, count));
    generate_ui_state(state)
}

fn names(ui_state: &UiState) -> Vec<&str> {
    ui_state.rows.iter().map(|row| row.name.as_str()).collect()
}

fn locate(state: &mut AppState, request: u32, path: Option<&str>) -> Option<LocatedRow> {
    state
        .tree_view
        .answer_locate(request, path.map(PathBuf::from));
    generate_ui_state(state).located
}

// --- The window --------------------------------------------------------------------

#[test]
fn a_snapshot_carries_the_rows_of_the_window_and_the_size_of_the_list() {
    let mut state = project();
    expand(&mut state, &["/root/src", "/root/src/core"]);
    // docs, src, core, a.rs, b.rs, main.rs, readme.md

    let all = view(&mut state, 0, 100);
    assert_eq!(
        names(&all),
        [
            "docs",
            "src",
            "core",
            "a.rs",
            "b.rs",
            "main.rs",
            "readme.md"
        ]
    );
    assert_eq!((all.rows_start, all.rows_total), (0, 7));
    assert_eq!(
        all.rows.iter().map(|row| row.depth).collect::<Vec<_>>(),
        [0, 0, 1, 2, 2, 1, 0]
    );

    let middle = view(&mut state, 2, 3);
    assert_eq!(names(&middle), ["core", "a.rs", "b.rs"]);
    assert_eq!((middle.rows_start, middle.rows_total), (2, 7));
    assert_eq!(middle.rows, all.rows[2..5]);

    // The counters describe the tree, not the window.
    assert_eq!(middle.tree_files_count, 5);
    assert_eq!(middle.tree_dirs_count, 3);
}

#[test]
fn a_window_behind_the_end_is_moved_up_to_the_end() {
    let mut state = project();
    expand(&mut state, &["/root/src", "/root/src/core"]);

    let at_end = view(&mut state, 5, 4);
    assert_eq!(names(&at_end), ["a.rs", "b.rs", "main.rs", "readme.md"]);
    assert_eq!(at_end.rows_start, 3);

    // The list becomes shorter than the window starts: the UI still gets rows.
    state.collapse_directories([PathBuf::from("/root/src")]);
    let collapsed = generate_ui_state(&state);
    assert_eq!(names(&collapsed), ["docs", "src", "readme.md"]);
    assert_eq!((collapsed.rows_start, collapsed.rows_total), (0, 3));
}

#[test]
fn the_host_bounds_the_size_of_the_window() {
    assert_eq!(Viewport::new(0, 0).window(10), 0..1);
    assert_eq!(Viewport::new(3, usize::MAX).window(10), 0..10);
    assert_eq!(
        Viewport::new(0, usize::MAX).window(100_000),
        0..Viewport::MAX_ROWS
    );
    assert_eq!(Viewport::new(usize::MAX, 5).window(100), 95..100);
    assert_eq!(Viewport::new(7, 5).window(0), 0..0);
    assert_eq!(Viewport::default().window(1_000), 0..Viewport::DEFAULT_ROWS);
}

#[test]
fn a_new_tree_is_shown_from_the_top() {
    let mut state = project();
    expand(&mut state, &["/root/src", "/root/src/core"]);
    assert_eq!(view(&mut state, 4, 2).rows_start, 4);
    assert!(locate(&mut state, 1, Some("/root/src")).is_some());

    state.reset_directory_state();
    state.current_path = "/root".to_string();
    state.set_full_file_list(
        (0..10)
            .map(|i| item(&format!("/root/file{i}.rs"), false))
            .collect(),
    );
    filtering::apply_filters(&mut state);

    let ui_state = generate_ui_state(&state);
    assert_eq!(ui_state.rows_start, 0);
    assert_eq!(ui_state.rows.len(), 2, "the size of the window is the UI's");
    assert_eq!(ui_state.located, None);
}

#[test]
fn selection_of_rows_outside_the_window_and_below_collapsed_directories_counts() {
    let mut state = project();
    expand(&mut state, &["/root/src"]);
    // docs, src, core (collapsed), main.rs, readme.md
    state
        .selected_files
        .insert(PathBuf::from("/root/src/core/a.rs"));
    state
        .selected_files
        .insert(PathBuf::from("/root/docs/guide.md"));

    // The window holds `src` and `core` only: neither the selected file below the
    // collapsed `core` nor anything else that is selected is a row of it.
    let ui_state = view(&mut state, 1, 2);
    assert_eq!(names(&ui_state), ["src", "core"]);
    assert_eq!(ui_state.rows[0].selection_state, SelectionState::Partial);
    assert_eq!(ui_state.rows[1].selection_state, SelectionState::Partial);

    state
        .selected_files
        .insert(PathBuf::from("/root/src/core/b.rs"));
    let ui_state = generate_ui_state(&state);
    assert_eq!(ui_state.rows[0].selection_state, SelectionState::Partial);
    assert_eq!(ui_state.rows[1].selection_state, SelectionState::Full);

    let top = view(&mut state, 0, 1);
    assert_eq!(names(&top), ["docs"]);
    assert_eq!(top.rows[0].selection_state, SelectionState::Full);
}

// --- The cached row list ---------------------------------------------------------------

#[test]
fn the_row_list_is_rebuilt_for_structure_and_expansion_only() {
    let mut state = project();
    let first = generate_ui_state(&state);
    assert_eq!(rows_build_count(&state), 1);

    // Selection, preview, lazy-load status, the viewport and a row lookup leave the
    // list alone.
    state
        .selected_files
        .insert(PathBuf::from("/root/src/main.rs"));
    state.previewed_file_path = Some(PathBuf::from("/root/readme.md"));
    state.loaded_dirs.remove(&PathBuf::from("/root/docs"));
    state.tree_view.set_viewport(Viewport::new(1, 2));
    state
        .tree_view
        .answer_locate(1, Some(PathBuf::from("/root/src")));
    let same_list = generate_ui_state(&state);
    assert_eq!(rows_build_count(&state), 1);
    assert_eq!(same_list.rows_revision, first.rows_revision);

    // Asking for the state a directory already has is no change either.
    state.collapse_directories([PathBuf::from("/root/src")]);
    assert_eq!(generate_ui_state(&state).rows_revision, first.rows_revision);
    assert_eq!(rows_build_count(&state), 1);

    // Expansion changes the list...
    expand(&mut state, &["/root/src"]);
    let expanded = generate_ui_state(&state);
    assert_eq!(rows_build_count(&state), 2);
    assert_ne!(expanded.rows_revision, first.rows_revision);
    assert_eq!(expanded.rows_total, 5);

    // ...and so does another file list.
    state.search_query = "main".to_string();
    filtering::apply_filters(&mut state);
    let filtered = generate_ui_state(&state);
    assert_eq!(rows_build_count(&state), 3);
    assert_ne!(filtered.rows_revision, expanded.rows_revision);
    assert_eq!(filtered.rows_total, 2);
}

// --- Looking up a row ------------------------------------------------------------------

#[test]
fn a_located_path_is_stated_as_its_row_in_every_snapshot() {
    let mut state = project();
    expand(&mut state, &["/root/src"]);
    // docs, src, core, main.rs, readme.md
    let row = |row| Some(Some(row));
    let answer = |located: Option<LocatedRow>| located.map(|located| located.row);

    assert_eq!(generate_ui_state(&state).located, None);
    assert_eq!(
        locate(&mut state, 3, Some("/root/src/main.rs")),
        Some(LocatedRow {
            request: 3,
            row: Some(3)
        })
    );
    // The window is not moved by a lookup, and the row need not be part of it.
    let ui_state = view(&mut state, 0, 2);
    assert_eq!(names(&ui_state), ["docs", "src"]);
    assert_eq!(answer(ui_state.located), row(3));

    // The answer follows the list: one more row above it, then none at all.
    expand(&mut state, &["/root/src/core"]);
    assert_eq!(answer(generate_ui_state(&state).located), row(5));
    state.collapse_directories([PathBuf::from("/root/src")]);
    assert_eq!(answer(generate_ui_state(&state).located), Some(None));

    // Not listed, the loaded directory itself, nothing found.
    assert_eq!(
        answer(locate(&mut state, 4, Some("/root/nope"))),
        Some(None)
    );
    assert_eq!(answer(locate(&mut state, 5, Some("/root"))), Some(None));
    assert_eq!(
        locate(&mut state, 6, None),
        Some(LocatedRow {
            request: 6,
            row: None
        })
    );
}

#[test]
fn typeahead_finds_the_next_row_by_the_start_of_its_name() {
    let mut state = project();
    expand(&mut state, &["/root/src", "/root/src/core"]);
    // 0 docs, 1 src, 2 core, 3 a.rs, 4 b.rs, 5 main.rs, 6 readme.md
    let found = |state: &AppState, prefix: &str, from: usize| {
        typeahead_match(state, prefix, from)
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
    };

    assert_eq!(found(&state, "m", 0).as_deref(), Some("main.rs"));
    assert_eq!(found(&state, "MAIN", 0).as_deref(), Some("main.rs"));
    // From the row after the match on: wraps around to the same row.
    assert_eq!(found(&state, "m", 6).as_deref(), Some("main.rs"));
    // The first match from `from` on wins.
    assert_eq!(found(&state, "", 0), None);
    assert_eq!(found(&state, "zzz", 0), None);
    assert_eq!(found(&state, "ain", 0), None, "the start of the name");
    assert_eq!(found(&state, "r", 100).as_deref(), Some("readme.md"));

    // Only rows are found: `a.rs` is not one while `core` is collapsed.
    assert_eq!(found(&state, "a", 0).as_deref(), Some("a.rs"));
    state.collapse_directories([PathBuf::from("/root/src/core")]);
    assert_eq!(found(&state, "a", 0), None);

    let mut state = state_of(vec![
        item("/root/Ärger.txt", false),
        item("/root/apple.txt", false),
        item("/root/Apfel.txt", false),
    ]);
    // Apfel.txt, apple.txt, Ärger.txt
    assert_eq!(found(&state, "ä", 0).as_deref(), Some("Ärger.txt"));
    assert_eq!(found(&state, "ap", 1).as_deref(), Some("apple.txt"));
    assert_eq!(found(&state, "ap", 2).as_deref(), Some("Apfel.txt"));
    state.reset_directory_state();
    assert_eq!(found(&state, "a", 0), None);
}

// --- Equivalence of window and list --------------------------------------------------

fn path_strategy() -> impl Strategy<Value = PathBuf> {
    prop::collection::vec(prop::sample::select(vec!["a", "b", "c", "d.rs"]), 1..5).prop_map(
        |segments| {
            let mut path = PathBuf::from("/root");
            path.extend(segments);
            path
        },
    )
}

/// Connected trees: every ancestor of an item is listed as a directory.
fn tree_strategy() -> impl Strategy<Value = Vec<FileItem>> {
    prop::collection::vec((path_strategy(), any::<bool>()), 0..40).prop_map(|entries| {
        let mut kinds: std::collections::BTreeMap<PathBuf, bool> = Default::default();
        for (path, is_directory) in entries {
            let mut ancestor = path.parent();
            while let Some(dir) = ancestor.filter(|dir| *dir != std::path::Path::new("/root")) {
                kinds.insert(dir.to_path_buf(), true);
                ancestor = dir.parent();
            }
            kinds.entry(path).or_insert(is_directory);
        }
        kinds
            .into_iter()
            .map(|(path, is_directory)| item(&path.to_string_lossy(), is_directory))
            .collect()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever the UI asks for, the snapshot holds exactly that part of the list of
    /// all rows (moved up if it lies behind the end), and every listed path is found
    /// at its row.
    #[test]
    fn a_window_is_a_slice_of_the_list_of_all_rows(
        items in tree_strategy(),
        expanded in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
        selected in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
        start in 0usize..60,
        count in 0usize..30,
    ) {
        let mut state = state_of(items);
        let listed: Vec<PathBuf> = state
            .filtered_file_list()
            .iter()
            .map(|item| item.path.clone())
            .collect();
        if !listed.is_empty() {
            for index in expanded {
                state.expand_directories([index.get(&listed).clone()]);
            }
            for index in selected {
                state.selected_files.insert(index.get(&listed).clone());
            }
        }

        let all = all_rows(&state);
        let ui_state = view(&mut state, start, count);

        let size = count.clamp(1, Viewport::MAX_ROWS).min(all.len());
        let expected_start = start.min(all.len() - size);
        prop_assert_eq!(ui_state.rows_total, all.len());
        prop_assert_eq!(ui_state.rows_start, expected_start);
        prop_assert_eq!(&ui_state.rows[..], &all[expected_start..expected_start + size]);

        for (row, shown) in all.iter().enumerate() {
            let located = locate(&mut state, 1, Some(&shown.path.to_string_lossy()));
            prop_assert_eq!(located, Some(LocatedRow { request: 1, row: Some(row) }));
        }
        let shown: HashSet<&PathBuf> = all.iter().map(|row| &row.path).collect();
        for path in listed.iter().filter(|path| !shown.contains(path)) {
            let located = locate(&mut state, 2, Some(&path.to_string_lossy()));
            prop_assert_eq!(located, Some(LocatedRow { request: 2, row: None }));
        }
    }
}

// --- Cost of a snapshot ----------------------------------------------------------------

/// `packages` × `modules` directories with `files` files each, everything expanded.
fn expanded_project(packages: usize, modules: usize, files: usize) -> AppState {
    let mut items = Vec::with_capacity(packages * (1 + modules * (1 + files)));
    for package in 0..packages {
        let package_path = format!("/root/package_{package:03}");
        items.push(item(&package_path, true));
        for module in 0..modules {
            let module_path = format!("{package_path}/module_{module:03}");
            items.push(item(&module_path, true));
            for file in 0..files {
                items.push(item(
                    &format!("{module_path}/source_file_{file:02}.rs"),
                    false,
                ));
            }
        }
    }
    let mut state = state_of(items);
    let directories: Vec<PathBuf> = state
        .filtered_file_list()
        .iter()
        .filter(|item| item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    state.expand_directories(directories);
    state
}

/// A snapshot and what it took to build and to serialize it.
struct Cost {
    build: Duration,
    serialize: Duration,
    bytes: usize,
}

fn cost_of_snapshot(state: &AppState) -> Cost {
    let started = Instant::now();
    let ui_state = generate_ui_state(state);
    let build = started.elapsed();
    let started = Instant::now();
    let json = serde_json::to_string(&ui_state).expect("UiState is serializable");
    Cost {
        build,
        serialize: started.elapsed(),
        bytes: json.len(),
    }
}

impl std::fmt::Display for Cost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "build {:?}, serialize {:?}, {} B",
            self.build, self.serialize, self.bytes
        )
    }
}

/// The point of the window: with everything expanded, the size of a snapshot does not
/// depend on the size of the project, and neither a click on a checkbox nor scrolling
/// costs a pass over the tree.
///
/// 500,000 files in 25,050 directories, all expanded: 525,050 rows. Prints the numbers
/// (`cargo test --release snapshot_of_a_huge -- --nocapture`). The time budgets are
/// generous on purpose: they hold in a debug build on a loaded machine, where the work
/// of the previous design (every row built and serialized, more than 100 MB of JSON)
/// took seconds; in a release build the three updates take well under a millisecond.
#[test]
fn snapshot_of_a_huge_expanded_project_is_bounded_by_the_window() {
    const WINDOW: usize = 200;
    const MAX_BYTES: usize = 100_000;
    const BUDGET: Duration = Duration::from_millis(100);

    let mut state = expanded_project(50, 500, 20);
    state.tree_view.set_viewport(Viewport::new(0, WINDOW));

    // The first snapshot of a tree pays for the derived data: index, rows, counts.
    let first = cost_of_snapshot(&state);
    let ui_state = generate_ui_state(&state);
    assert_eq!(ui_state.tree_files_count, 500_000);
    assert_eq!(ui_state.tree_dirs_count, 25_050);
    assert_eq!(ui_state.rows_total, 525_050);
    assert_eq!(ui_state.rows.len(), WINDOW);
    println!("500,000 files, all expanded, window of {WINDOW} rows");
    println!("  first snapshot:           {first}");

    // A click on a checkbox: the file of row 2, far away from where the UI will look
    // next, and with it the state of its two directories at the top of the window.
    let clicked = PathBuf::from("/root/package_000/module_000/source_file_00.rs");
    state.selected_files.insert(clicked);
    let counted = state.selection_count_count();
    let built = rows_build_count(&state);
    let click = cost_of_snapshot(&state);
    println!("  click on a checkbox:      {click}");
    let ui_state = generate_ui_state(&state);
    assert_eq!(ui_state.rows[0].selection_state, SelectionState::Partial);
    assert_eq!(ui_state.rows[1].selection_state, SelectionState::Partial);
    assert_eq!(ui_state.rows[2].selection_state, SelectionState::Full);
    assert_eq!(state.selection_count_count(), counted, "not recounted");
    assert_eq!(rows_build_count(&state), built, "rows not rebuilt");

    // Scrolling: another window, at the far end of the list.
    state.tree_view.set_viewport(Viewport::new(400_000, WINDOW));
    let scroll = cost_of_snapshot(&state);
    println!("  scroll (setViewport):     {scroll}");
    let ui_state = generate_ui_state(&state);
    assert_eq!(ui_state.rows_start, 400_000);
    assert_eq!(ui_state.rows.len(), WINDOW);
    assert_eq!(rows_build_count(&state), built, "rows not rebuilt");

    // The same click with every file selected (the journal, not the size of the
    // selection, decides what it costs).
    let every_file: Vec<PathBuf> = state
        .filtered_file_list()
        .iter()
        .filter(|item| !item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    state.selected_files.extend(every_file);
    generate_ui_state(&state);
    let counted = state.selection_count_count();
    state.selected_files.remove(&PathBuf::from(
        "/root/package_038/module_047/source_file_05.rs",
    ));
    let click_with_all_selected = cost_of_snapshot(&state);
    println!("  click, all 500k selected: {click_with_all_selected}");
    assert_eq!(state.selection_count_count(), counted, "not recounted");

    // Collapsing one directory rebuilds the row list: the one update of these that
    // is proportional to the number of rows.
    state.collapse_directories([PathBuf::from("/root/package_049")]);
    let collapse = cost_of_snapshot(&state);
    println!("  collapse one directory:   {collapse}");
    assert_eq!(generate_ui_state(&state).rows_total, 525_050 - 10_500);

    for (what, cost) in [
        ("first snapshot", &first),
        ("click", &click),
        ("scroll", &scroll),
        ("click with everything selected", &click_with_all_selected),
        ("collapse", &collapse),
    ] {
        assert!(
            cost.bytes < MAX_BYTES,
            "{what}: a snapshot of {WINDOW} rows takes {} B",
            cost.bytes
        );
    }
    for (what, cost) in [
        ("click", &click),
        ("scroll", &scroll),
        ("click with everything selected", &click_with_all_selected),
    ] {
        assert!(
            cost.build + cost.serialize < BUDGET,
            "{what} took {:?}",
            cost.build + cost.serialize
        );
    }
}

/// The size of a snapshot follows the window, not the project: the same window on a
/// project twenty times as large is the same payload.
#[test]
fn payload_does_not_depend_on_the_size_of_the_project() {
    let payload = |mut state: AppState| {
        state.tree_view.set_viewport(Viewport::new(0, 200));
        cost_of_snapshot(&state).bytes
    };

    let small = payload(expanded_project(5, 50, 20));
    let large = payload(expanded_project(50, 100, 20));
    let largest_window = {
        let mut state = expanded_project(5, 50, 20);
        state.tree_view.set_viewport(Viewport::new(0, usize::MAX));
        cost_of_snapshot(&state).bytes
    };

    println!(
        "UiState payload, everything expanded: 5,000 files {small} B, 100,000 files \
         {large} B (window of 200 rows); largest window ({} rows) {largest_window} B",
        Viewport::MAX_ROWS
    );
    // Only the counters differ in length.
    assert!(small.abs_diff(large) < 64, "{small} B vs {large} B");
    assert!(large < 100_000);
    assert!(largest_window < 300_000);
}
