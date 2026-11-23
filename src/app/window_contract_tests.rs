//! Cross-language conformance test for the window of rows.
//!
//! The UI's tests have no host. What stands in for it there (the E2E bridge and the
//! fake host of the component tests) is one TypeScript model of the host's windowing,
//! `src/ui/src/lib/dev/treeHostModel.ts`. This test is what keeps that model honest:
//! it posts a scenario to the real host, message by message, the way the WebView
//! does (`dispatch_ipc_message`): `setViewport` including a window that is too large
//! or behind the end, `locateRow` of every kind, a list that changes, filters that
//! change, messages that are refused. It writes down, step by step, what was posted
//! and what the host answered, including the number of the last command the answer
//! accounts for (`last_processed_seq`). The UI replays the file against its model
//! (`src/ui/src/tests/contracts/tree-window-scenario.test.ts`).
//!
//! The file also states the limits of the window, which the UI mirrors as constants.
//!
//! After an intentional change, regenerate the fixture with
//! `UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture` and adapt the model.

use super::correlation::{CommandSeq, SessionId};
use super::dispatch::CommandDispatcher;
use super::dispatch_ipc_message;
use super::events::UserEvent;
use super::file_dialog::DialogService;
use super::filtering;
use super::proxy::EventProxy;
use super::shared_state::SharedState;
use super::state::AppState;
use super::view_model::rows::all_rows;
use super::view_model::{UiState, Viewport};
use crate::core::FileItem;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/src/tests/contracts/fixtures/tree_window_scenario.json"
);
const UPDATE_ENV: &str = "UPDATE_CONTRACT_FIXTURES";

/// Keeps the snapshots the commands send.
#[derive(Clone, Default)]
struct Snapshots(Arc<Mutex<Vec<UiState>>>);

impl EventProxy for Snapshots {
    fn send_event(&self, event: UserEvent) {
        if let UserEvent::StateUpdate(ui_state) = event {
            self.0.lock().unwrap().push(*ui_state.into_ui_state());
        }
    }
}

impl Snapshots {
    /// The answer to the message that has just been dealt with: the one snapshot
    /// that was sent.
    fn take_answer(&self) -> UiState {
        let mut sent = self.0.lock().unwrap();
        assert_eq!(sent.len(), 1, "every message is answered once");
        sent.pop().expect("one snapshot")
    }
}

/// No message of the scenario opens a dialog.
struct NoDialog;

impl DialogService for NoDialog {
    fn pick_directory(&self) -> Option<PathBuf> {
        None
    }
    fn pick_config_to_import(&self) -> Option<PathBuf> {
        None
    }
    fn export_config_path(&self) -> Option<PathBuf> {
        None
    }
    fn save_output_file_path(&self, _config: &crate::config::AppConfig) -> Option<PathBuf> {
        None
    }
}

fn item(path: &str, is_directory: bool) -> FileItem {
    FileItem {
        path: PathBuf::from(path),
        is_directory,
        size: if is_directory { 0 } else { 100 },
        ..Default::default()
    }
}

/// A project whose rows do not fit into the largest window: `big` holds 640 files.
fn project() -> AppState {
    let mut items = vec![
        item("/project/alpha", true),
        item("/project/alpha/a1.rs", false),
        item("/project/alpha/a2.rs", false),
        item("/project/alpha/notes.md", false),
        item("/project/big", true),
        item("/project/zoo", true),
        item("/project/zoo/zebra.md", false),
        item("/project/Makefile", false),
        item("/project/readme.md", false),
        item("/project/zeta.txt", false),
    ];
    for file in 0..640 {
        items.push(item(&format!("/project/big/file-{file:03}.txt"), false));
    }

    let mut state = AppState::default();
    state.config.remove_empty_directories = false;
    state.current_path = "/project".to_string();
    state.loaded_dirs = items
        .iter()
        .filter(|item| item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    state.set_full_file_list(items);
    state.is_fully_scanned = true;
    filtering::apply_filters(&mut state);
    state.expand_directories(["/project/alpha", "/project/big", "/project/zoo"].map(PathBuf::from));
    state
}

/// The scenario as it is written to the fixture.
/// The UI that posts the scenario, and the one that is loaded after it.
const FIRST_UI: SessionId = 1001;
const RELOADED_UI: SessionId = 1002;

struct Recorder {
    dispatcher: CommandDispatcher,
    state: SharedState,
    snapshots: Snapshots,
    /// The UI that posts, and the number of its last message, as it counts them.
    session: SessionId,
    seq: CommandSeq,
    /// The distinct lists of rows the scenario shows, by name.
    lists: BTreeMap<String, Value>,
    steps: Vec<Value>,
    /// `rows_revision` of the previous answer.
    last_revision: Option<u64>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            dispatcher: CommandDispatcher::new(),
            state: SharedState::new(project()),
            snapshots: Snapshots::default(),
            session: FIRST_UI,
            seq: 0,
            lists: BTreeMap::new(),
            steps: Vec::new(),
            last_revision: None,
        }
    }

    /// What the UI's model has to answer with after this step.
    ///
    /// Of `rows_revision` the contract says when it changes, not which number it is,
    /// so the scenario states whether it differs from the answer before.
    fn expectation(&mut self, answer: &UiState) -> Value {
        let revision_changed = self.last_revision != Some(answer.rows_revision);
        self.last_revision = Some(answer.rows_revision);
        let path = |row: Option<&super::view_model::TreeRow>| {
            row.map(|row| row.path.to_string_lossy().into_owned())
        };
        json!({
            "rows_start": answer.rows_start,
            "rows_count": answer.rows.len(),
            "rows_total": answer.rows_total,
            "rows_revision_changed": revision_changed,
            "located": answer.located,
            "first_path": path(answer.rows.first()),
            "last_path": path(answer.rows.last()),
            "last_processed_session": answer.last_processed_session,
            "last_processed_seq": answer.last_processed_seq,
        })
    }

    /// The next message of the UI: `command` with `payload`, numbered.
    fn message(&mut self, command: &str, payload: Value) -> String {
        self.seq += 1;
        json!({
            "session": self.session,
            "seq": self.seq,
            "command": command,
            "payload": payload,
        })
        .to_string()
    }

    /// Posts `message` to the host the way the WebView does and waits for the
    /// answer.
    async fn post(&mut self, message: &str) -> UiState {
        dispatch_ipc_message(
            &self.dispatcher,
            message.to_string(),
            Arc::new(NoDialog),
            self.snapshots.clone(),
            self.state.clone(),
        );
        // The marker suspends once, so it passes through the ordered lane and the
        // continuation lane behind the message.
        let (done, marker_ran) = tokio::sync::oneshot::channel();
        self.dispatcher.dispatch(None, async move {
            tokio::task::yield_now().await;
            let _ = done.send(());
        });
        marker_ran.await.expect("the marker command was dropped");
        self.snapshots.take_answer()
    }

    /// A step in which the UI posts a message the model deals with itself: the
    /// model receives the same message and has to answer like the host.
    async fn posts(&mut self, name: &str, message: String) {
        let answer = self.post(&message).await;
        let expect = self.expectation(&answer);
        self.steps.push(json!({
            "name": name,
            "action": { "kind": "command", "message": message },
            "expect": expect,
        }));
    }

    /// A step in which a command changes what the host shows in a way the model
    /// does not work out itself (a directory expanded, a filter applied, a file
    /// selected). The model receives the message, for its number, and is told what
    /// is shown now: the list of all rows, by the name it has in `lists`, and the
    /// filter texts.
    async fn changes(&mut self, name: &str, list: &str, command: &str, payload: Value) {
        let message = self.message(command, payload);
        let answer = self.post(&message).await;
        self.shows(name, list, message, answer);
    }

    fn shows(&mut self, name: &str, list: &str, message: String, answer: UiState) {
        let (rows, search_query) = {
            let state = self.state.lock();
            let rows: Vec<Value> = all_rows(&state)
                .iter()
                .map(|row| {
                    // One line per row: "<depth> <d|f> <path>".
                    Value::String(format!(
                        "{} {} {}",
                        row.depth,
                        if row.is_directory { 'd' } else { 'f' },
                        row.path.to_string_lossy()
                    ))
                })
                .collect();
            (Value::Array(rows), state.search_query.clone())
        };
        let known = self.lists.entry(list.to_string()).or_insert(rows.clone());
        assert_eq!(*known, rows, "the list `{list}` is not the same as before");
        let expect = self.expectation(&answer);
        self.steps.push(json!({
            "name": name,
            "action": {
                "kind": "shows",
                "message": message,
                "list": list,
                "current_path": answer.current_path,
                "search_query": search_query,
            },
            "expect": expect,
        }));
    }

    async fn set_viewport(&mut self, name: &str, payload: Value) {
        let message = self.message("setViewport", payload);
        self.posts(name, message).await;
    }

    async fn locate(&mut self, name: &str, request: u32, target: Value) {
        let message = self.message("locateRow", json!({ "request": request, "target": target }));
        self.posts(name, message).await;
    }

    async fn set_expansion(&mut self, name: &str, list: &str, path: &str, expanded: bool) {
        self.changes(
            name,
            list,
            "setExpansion",
            json!({ "path": path, "expanded": expanded }),
        )
        .await;
    }

    async fn update_filters(&mut self, name: &str, list: &str, search_query: &str) {
        self.changes(
            name,
            list,
            "updateFilters",
            json!({ "searchQuery": search_query }),
        )
        .await;
    }

    fn fixture(self) -> String {
        let fixture = json!({
            "limits": {
                "default_rows": Viewport::DEFAULT_ROWS,
                "max_rows": Viewport::MAX_ROWS,
            },
            "lists": self.lists,
            "steps": self.steps,
        });
        let mut json = serde_json::to_string_pretty(&fixture).expect("fixture is serializable");
        json.push('\n');
        json
    }
}

async fn record_scenario() -> String {
    let mut scenario = Recorder::new();
    let file_320 = "/project/big/file-320.txt";

    // --- The window ---
    scenario
        .changes(
            "a tree is shown: the default window",
            "expanded",
            "initialize",
            Value::Null,
        )
        .await;

    scenario
        .set_viewport(
            "a window in the middle",
            json!({ "start": 200, "count": 80 }),
        )
        .await;
    scenario
        .set_viewport(
            "a window larger than the host sends",
            json!({ "start": 10, "count": 5000 }),
        )
        .await;
    scenario
        .set_viewport(
            "a window behind the end is moved up to the end",
            json!({ "start": 100_000, "count": 50 }),
        )
        .await;
    scenario
        .set_viewport(
            "a window that reaches over the end",
            json!({ "start": 630, "count": 50 }),
        )
        .await;
    scenario
        .set_viewport(
            "a malformed viewport changes nothing",
            json!({ "start": 5 }),
        )
        .await;
    scenario
        .set_viewport(
            "a viewport without rows changes nothing",
            json!({ "start": 5, "count": 0 }),
        )
        .await;

    // --- Looking up a row ---
    scenario
        .locate(
            "the row of a path",
            1,
            json!({ "kind": "path", "path": file_320 }),
        )
        .await;
    scenario
        .locate(
            "the row of the parent, far above the window",
            2,
            json!({ "kind": "parent_of", "path": file_320 }),
        )
        .await;
    scenario
        .locate(
            "a row at the top level has no parent row",
            3,
            json!({ "kind": "parent_of", "path": "/project/big" }),
        )
        .await;
    scenario
        .locate(
            "type-ahead finds the first row from `from` on",
            4,
            json!({ "kind": "typeahead", "prefix": "ze", "from": 0 }),
        )
        .await;
    scenario
        .locate(
            "type-ahead continues behind the row it found, ignoring case",
            5,
            json!({ "kind": "typeahead", "prefix": "ZE", "from": 647 }),
        )
        .await;
    scenario
        .locate(
            "type-ahead wraps around at the end of the list",
            6,
            json!({ "kind": "typeahead", "prefix": "a", "from": 600 }),
        )
        .await;
    scenario
        .locate(
            "type-ahead from behind the list starts at the top",
            7,
            json!({ "kind": "typeahead", "prefix": "ze", "from": 9_999 }),
        )
        .await;
    scenario
        .locate(
            "type-ahead without a match",
            8,
            json!({ "kind": "typeahead", "prefix": "qq", "from": 3 }),
        )
        .await;
    scenario
        .locate(
            "a path that is not listed",
            9,
            json!({ "kind": "path", "path": "/project/nope.txt" }),
        )
        .await;
    scenario
        .locate(
            "a type-ahead for nothing changes nothing",
            10,
            json!({ "kind": "typeahead", "prefix": "", "from": 0 }),
        )
        .await;
    scenario
        .locate(
            "a malformed lookup changes nothing",
            10,
            json!({ "kind": "sideways", "path": file_320 }),
        )
        .await;

    // --- The list changes ---
    scenario
        .set_viewport(
            "a window in the long directory",
            json!({ "start": 300, "count": 60 }),
        )
        .await;
    scenario
        .locate(
            "the row of a file in it",
            11,
            json!({ "kind": "path", "path": file_320 }),
        )
        .await;
    scenario
        .set_expansion(
            "the directory is collapsed: a shorter list, the window at its end, the file no row",
            "collapsed",
            "/project/big",
            false,
        )
        .await;
    scenario
        .set_expansion(
            "the directory is expanded again: the window and the row are back",
            "expanded",
            "/project/big",
            true,
        )
        .await;
    scenario
        .changes(
            "a change of the selection leaves the list alone",
            "expanded",
            "toggleSelection",
            json!(file_320),
        )
        .await;

    // --- The filters change ---
    scenario
        .update_filters(
            "a filter: another list, shown from the top, the lookup void",
            "filtered",
            "file-1",
        )
        .await;
    scenario
        .set_viewport(
            "a window of the filtered list",
            json!({ "start": 40, "count": 30 }),
        )
        .await;
    scenario
        .locate(
            "a lookup in the filtered list",
            12,
            json!({ "kind": "typeahead", "prefix": "file-15", "from": 0 }),
        )
        .await;
    scenario
        .update_filters(
            "the same filter again changes nothing about the window",
            "filtered",
            "file-1",
        )
        .await;
    // The host lists the rows again although they are the same; whether that counts
    // as another list is not part of the contract.
    scenario.steps.last_mut().unwrap()["expect"]["rows_revision_changed"] = Value::Null;
    scenario.last_revision = None;
    scenario
        .update_filters("the filter is cleared: from the top again", "expanded", "")
        .await;

    // --- Before a new UI ---
    scenario
        .set_viewport(
            "a window further down",
            json!({ "start": 500, "count": 70 }),
        )
        .await;
    scenario
        .locate(
            "and a lookup",
            13,
            json!({ "kind": "path", "path": file_320 }),
        )
        .await;

    // --- Messages that are refused ---
    let message = scenario.message("noSuchCommand", Value::Null);
    scenario
        .posts(
            "a command that does not exist changes nothing but the number",
            message,
        )
        .await;
    scenario.seq += 1;
    let message = json!({
        "session": scenario.session,
        "seq": scenario.seq,
        "command": "setViewport",
        "payload": { "start": 0, "count": 10 },
        "extra": true,
    })
    .to_string();
    scenario
        .posts("an envelope with more than a command is refused", message)
        .await;
    scenario
        .posts(
            "a message that is not JSON is answered and changes no number",
            "not json".to_string(),
        )
        .await;
    let message = json!({
        "command": "setViewport",
        "payload": { "start": 0, "count": 10 },
    })
    .to_string();
    scenario
        .posts("a command without a name is not executed", message)
        .await;
    let message = json!({
        "seq": 999,
        "command": "setViewport",
        "payload": { "start": 0, "count": 10 },
    })
    .to_string();
    scenario
        .posts("a number without a session is no name", message)
        .await;
    let message = json!({
        "session": scenario.session,
        "seq": "12",
        "command": "setViewport",
        "payload": { "start": 0, "count": 10 },
    })
    .to_string();
    scenario.posts("a number has to be a number", message).await;

    // --- A new UI ---
    // A UI that is loaded (anew) has a session of its own and counts from 1.
    scenario.session = RELOADED_UI;
    scenario.seq = 0;
    let message = scenario.message("initialize", Value::Null);
    scenario
        .posts(
            "a UI that starts gets the top of the list, no answer, and its own numbers",
            message,
        )
        .await;

    scenario.fixture()
}

#[tokio::test]
async fn contract_fixture_matches_the_window_scenario() {
    let actual = record_scenario().await;

    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(FIXTURE_PATH, &actual).expect("fixture is writable");
        return;
    }

    let expected = std::fs::read_to_string(FIXTURE_PATH)
        .unwrap_or_else(|e| panic!("cannot read {FIXTURE_PATH}: {e}"))
        .replace("\r\n", "\n");
    assert_eq!(
        actual, expected,
        "The windowing of the host no longer matches the scenario shared with the UI. If \
         the change is intended, run `{UPDATE_ENV}=1 cargo test contract_fixture`, commit \
         the fixture and update src/ui/src/lib/dev/treeHostModel.ts."
    );
}

/// The scenario is only worth sharing if it really reaches the edges.
#[tokio::test]
async fn window_scenario_covers_the_edges_of_the_contract() {
    let fixture: Value = serde_json::from_str(&record_scenario().await).unwrap();
    let steps = fixture["steps"].as_array().unwrap();
    let expect = |name: &str| {
        &steps
            .iter()
            .find(|step| step["name"].as_str().unwrap().starts_with(name))
            .unwrap_or_else(|| panic!("no step `{name}`"))["expect"]
    };
    let total = fixture["lists"]["expanded"].as_array().unwrap().len();
    assert!(total > Viewport::MAX_ROWS, "{total} rows fit into a window");

    assert_eq!(
        expect("a tree is shown")["rows_count"],
        Viewport::DEFAULT_ROWS
    );
    assert_eq!(
        expect("a window larger than")["rows_count"],
        Viewport::MAX_ROWS
    );
    assert_eq!(expect("a window behind the end")["rows_start"], total - 50);
    assert_eq!(
        expect("a window that reaches over")["rows_start"],
        total - 50
    );
    assert_eq!(expect("the row of a path")["located"]["row"], 325);
    assert_eq!(expect("the row of the parent")["located"]["row"], 4);
    assert_eq!(
        expect("a row at the top level")["located"]["row"],
        Value::Null
    );
    assert_eq!(expect("type-ahead finds")["located"]["row"], 646);
    assert_eq!(expect("type-ahead continues")["located"]["row"], 649);
    assert_eq!(expect("type-ahead wraps")["located"]["row"], 0);
    assert_eq!(expect("type-ahead from behind")["located"]["row"], 646);
    assert_eq!(expect("type-ahead without")["located"]["row"], Value::Null);
    assert_eq!(expect("a malformed lookup")["located"]["request"], 9);
    assert_eq!(expect("a type-ahead for nothing")["located"]["request"], 9);
    // What an answer shows, without the number it accounts for.
    let view = |name: &str| {
        let mut view = expect(name).clone();
        view["last_processed_seq"] = Value::Null;
        view["last_processed_session"] = Value::Null;
        view["rows_revision_changed"] = Value::Null;
        view
    };
    assert_eq!(
        view("a viewport without rows"),
        view("a window that reaches over")
    );

    let collapsed = expect("the directory is collapsed");
    assert_eq!(collapsed["rows_total"], 10);
    assert_eq!(collapsed["rows_start"], 0);
    assert_eq!(collapsed["located"]["row"], Value::Null);
    let expanded = expect("the directory is expanded again");
    assert_eq!(expanded["rows_start"], 300);
    assert_eq!(expanded["located"]["row"], 325);
    assert_eq!(collapsed["rows_revision_changed"], true);
    assert_eq!(expanded["rows_revision_changed"], true);
    assert_eq!(
        expect("a change of the selection")["rows_revision_changed"],
        false
    );
    assert_eq!(expect("the row of a path")["rows_revision_changed"], false);
    assert_eq!(
        expect("a window in the middle")["rows_revision_changed"],
        false
    );

    let filtered = expect("a filter:");
    assert_eq!(filtered["rows_start"], 0);
    assert_eq!(filtered["rows_count"], 60, "the size of the window stays");
    assert_eq!(filtered["located"], Value::Null);
    assert_eq!(filtered["rows_revision_changed"], true);
    let same_filter = expect("the same filter again");
    assert_eq!(same_filter["rows_start"], 40);
    assert_ne!(same_filter["located"], Value::Null);

    let restarted = expect("a UI that starts");
    assert_eq!(restarted["rows_start"], 0);
    assert_eq!(restarted["located"], Value::Null);

    // The numbers: every step that posts a numbered message is answered under that
    // number, whether the message is a command or not; a message without a number
    // leaves it alone; and a UI that starts anew is followed.
    let seq_of = |name: &str| expect(name)["last_processed_seq"].as_u64().unwrap();
    assert_eq!(seq_of("a tree is shown"), 1);
    assert_eq!(seq_of("a window in the middle"), 2);
    assert_eq!(seq_of("a malformed viewport"), 6);
    let unknown = seq_of("a command that does not exist");
    assert!(unknown > 30, "{unknown}");
    assert_eq!(seq_of("an envelope with more"), unknown + 1);
    assert_eq!(seq_of("a message that is not JSON"), unknown + 1);
    assert_eq!(seq_of("a command without a name"), unknown + 1);
    assert_eq!(seq_of("a number without a session"), unknown + 1);
    assert_eq!(seq_of("a number has to be"), unknown + 1);
    assert_eq!(restarted["last_processed_seq"], 1);
    for step in steps {
        let session = if step["name"]
            .as_str()
            .unwrap()
            .starts_with("a UI that starts")
        {
            RELOADED_UI
        } else {
            FIRST_UI
        };
        assert_eq!(step["expect"]["last_processed_session"], session, "{step}");
    }
    for refused in [
        "a command that does not exist",
        "an envelope with more",
        "a message that is not JSON",
        "a command without a name",
        "a number without a session",
        "a number has to be",
    ] {
        let (answer, before) = (view(refused), view("and a lookup"));
        assert_eq!(answer, before, "{refused}");
    }
}
