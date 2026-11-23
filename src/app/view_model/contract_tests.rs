//! Cross-language contract test for the `UiState` payload.
//!
//! This test serializes a representative `UiState` (a window of rows) and compares it with a JSON
//! fixture that is committed inside the UI's test directory. The UI parses that exact
//! file with its full runtime schema (`src/ui/src/tests/contracts/rust-fixture.test.ts`),
//! so the Rust structs and the TypeScript schema cannot drift apart unnoticed.
//!
//! After an intentional change of the payload, regenerate the fixture with
//! `UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture` and adapt the UI schema.

use super::{generate_ui_state, LocatedRow, UiState, Viewport};
use crate::app::selection::SelectionState;
use crate::app::state::{AppState, StatusNotice};
use crate::config::{AppConfig, GeometryUnits};
use crate::core::FileItem;
use std::collections::HashSet;
use std::path::PathBuf;

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/src/tests/contracts/fixtures/ui_state_window.json"
);
const UPDATE_ENV: &str = "UPDATE_CONTRACT_FIXTURES";

fn item(path: &str, is_directory: bool, is_binary: bool, size: u64) -> FileItem {
    FileItem {
        path: PathBuf::from(path),
        is_directory,
        is_binary,
        size,
        ..Default::default()
    }
}

fn paths(paths: &[&str]) -> HashSet<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

/// A state that exercises every kind of node the UI has to render.
///
/// Sets with more than one element are avoided wherever they are serialized as arrays,
/// so that the output is deterministic.
fn representative_state() -> AppState {
    let mut state = AppState::default();
    state.config = AppConfig {
        ignore_patterns: HashSet::from(["target".to_string()]),
        tree_ignore_patterns: HashSet::new(),
        last_directory: Some(PathBuf::from("/project")),
        output_directory: None,
        output_filename: "output.txt".to_string(),
        case_sensitive_search: false,
        include_tree_by_default: true,
        use_relative_paths: true,
        remove_empty_directories: false,
        window_size: (1200.0, 800.0),
        window_position: (100.0, 50.0),
        geometry_units: GeometryUnits::Logical,
        max_file_size_mb: 20,
    };
    state.current_path = "/project".to_string();
    state.set_full_file_list(vec![
        // Expanded directory, partially selected.
        item("/project/src", true, false, 0),
        // Collapsed directory inside it whose hidden files are all selected.
        item("/project/src/core", true, false, 0),
        item("/project/src/core/mod.rs", false, false, 2048),
        item("/project/src/core/scanner.rs", false, false, 4096),
        item("/project/src/lib.rs", false, false, 512),
        // Selected, previewed and matching the filename query.
        item("/project/src/main.rs", false, false, 1024),
        // Collapsed directory without any selection.
        item("/project/docs", true, false, 0),
        item("/project/docs/guide.md", false, false, 300),
        // Directory whose children have not been loaded yet.
        item("/project/vendor", true, false, 0),
        // Loaded directory without children.
        item("/project/empty", true, false, 0),
        item("/project/logo.png", false, true, 65536),
        item("/project/README.md", false, false, 256),
    ]);
    let listed = state.full_file_list().to_vec();
    state.set_filtered_file_list(listed);

    state.expanded_dirs = paths(&["/project/src"]).into();
    state.loaded_dirs = paths(&[
        "/project",
        "/project/src",
        "/project/src/core",
        "/project/docs",
        "/project/empty",
    ]);
    state.selected_files = paths(&[
        "/project/src/core/mod.rs",
        "/project/src/core/scanner.rs",
        "/project/src/main.rs",
    ])
    .into();
    // The UI has scrolled one row down and asked where the previewed file is.
    state.tree_view.set_viewport(Viewport::new(1, 8));
    state
        .tree_view
        .answer_locate(7, Some(PathBuf::from("/project/src/main.rs")));
    state.previewed_file_path = Some(PathBuf::from("/project/src/main.rs"));
    state.search_query = "main".to_string();
    state.current_config_filename = Some("project.cfc.json".to_string());
    state.active_ignore_patterns = HashSet::from(["target".to_string()]);
    state.scan_progress.files_scanned = 12;
    state.scan_progress.large_files_skipped = 1;
    state.status = StatusNotice::IndexingComplete { visible_items: 12 };
    state
}

/// The snapshot of the representative state as it leaves the host: the transport
/// has written the number of the last command into it (see `app::correlation`).
fn representative_snapshot() -> UiState {
    let mut ui_state = generate_ui_state(&representative_state());
    ui_state.last_processed_seq = 41;
    ui_state
}

fn to_fixture_json(ui_state: &UiState) -> String {
    let mut json = serde_json::to_string_pretty(ui_state).expect("UiState is serializable");
    json.push('\n');
    json
}

#[test]
fn contract_fixture_matches_the_serialized_ui_state() {
    let actual = to_fixture_json(&representative_snapshot());

    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(FIXTURE_PATH, &actual).expect("fixture is writable");
        return;
    }

    let expected = std::fs::read_to_string(FIXTURE_PATH)
        .unwrap_or_else(|e| panic!("cannot read {FIXTURE_PATH}: {e}"))
        .replace("\r\n", "\n");
    assert_eq!(
        actual, expected,
        "The serialized UiState no longer matches the fixture shared with the UI. If the \
         change is intended, run `{UPDATE_ENV}=1 cargo test contract_fixture`, commit the \
         fixture and update src/ui/src/lib/ipc/schema.ts."
    );
}

/// The fixture is only worth sharing if it really covers the shapes of a row and a
/// window that is not the whole list.
#[test]
fn contract_fixture_state_covers_the_shapes_of_rows_and_window() {
    let ui_state = generate_ui_state(&representative_state());
    let row = |name: &str| {
        ui_state
            .row_named(name)
            .unwrap_or_else(|| panic!("{name} is missing"))
    };

    // docs, empty, src, [core, lib.rs, main.rs], vendor, logo.png, README.md
    assert_eq!(ui_state.rows_total, 9);
    assert_eq!(ui_state.rows_start, 1);
    assert_eq!(ui_state.rows.len(), 8);
    assert!(ui_state.row_named("docs").is_none(), "above the window");
    assert_eq!(
        ui_state.located,
        Some(LocatedRow {
            request: 7,
            row: Some(5)
        })
    );
    assert_eq!(ui_state.rows[5 - ui_state.rows_start].name, "main.rs");

    let src = row("src");
    assert!(src.is_expanded && src.has_children && src.children_loaded);
    assert_eq!(src.selection_state, SelectionState::Partial);
    assert_eq!(src.depth, 0);
    assert_eq!(ui_state.children_of(src).len(), 3);

    let core = row("core");
    assert_eq!(core.depth, 1);
    assert!(!core.is_expanded && core.has_children);
    assert!(ui_state.children_of(core).is_empty());
    assert_eq!(core.selection_state, SelectionState::Full);

    let main = row("main.rs");
    assert!(main.is_match && main.is_previewed);
    assert_eq!(main.selection_state, SelectionState::Full);

    let vendor = row("vendor");
    assert!(!vendor.children_loaded && !vendor.has_children);
    assert!(!row("empty").has_children && row("empty").children_loaded);
    assert!(row("logo.png").is_binary);

    assert_eq!(ui_state.tree_files_count, 7);
    assert_eq!(ui_state.tree_dirs_count, 5);
    assert_eq!(ui_state.selected_files_count, 3);
}
