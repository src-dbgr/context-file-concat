//! Tests of the dispatch of messages (`dispatch_ipc_message`, `dispatch_dropped_path`)
//! and of the events (`handle_user_event`). The harness is here; the tests are grouped
//! by topic in the files beside it.

use super::*;
use crate::app::correlation::{CommandId, CommandSeq, SessionId};
use crate::app::events::{OutputSizeNotice, SaveResult};
use crate::app::snapshot::{send_snapshot, Snapshot};
use crate::app::state::{AppState, GeneratedDocument, StatusNotice};
use crate::app::test_support::{lenient_event_channel, EventStream, TestEventProxy};
use crate::config::AppConfig;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

mod accounting;
mod background;
mod event_calls;
mod lanes;
mod refused;

/// A dialog that stays "open" until the test closes it, and is then cancelled.
struct BlockingDialog {
    opened: Mutex<std_mpsc::Sender<()>>,
    close: Mutex<std_mpsc::Receiver<()>>,
}

impl BlockingDialog {
    fn wait_for_the_user(&self) -> Option<PathBuf> {
        self.opened.lock().unwrap().send(()).unwrap();
        self.close.lock().unwrap().recv().unwrap();
        None
    }
}

impl DialogService for BlockingDialog {
    fn pick_directory(&self) -> Option<PathBuf> {
        self.wait_for_the_user()
    }
    fn pick_config_to_import(&self) -> Option<PathBuf> {
        self.wait_for_the_user()
    }
    fn export_config_path(&self) -> Option<PathBuf> {
        self.wait_for_the_user()
    }
    fn save_output_file_path(&self, _config: &AppConfig) -> Option<PathBuf> {
        self.wait_for_the_user()
    }
}

struct Harness {
    dispatcher: CommandDispatcher,
    dialog: Arc<BlockingDialog>,
    dialog_opened: std_mpsc::Receiver<()>,
    close_dialog: std_mpsc::Sender<()>,
    proxy: TestEventProxy,
    events: EventStream,
    state: SharedState,
    idle_tasks: usize,
    /// The number of the last message sent, as the UI counts them.
    last_seq: AtomicU64,
}

/// The UI of these tests: what it puts into the envelope of its messages.
const UI: SessionId = 31;

/// Command `seq` of the UI of these tests.
fn command_of_ui(seq: CommandSeq) -> CommandId {
    CommandId { session: UI, seq }
}

/// The numbers of the commands the snapshots among `events` account for, in
/// order. They are commands of the UI of these tests.
fn seqs_of(events: &[UserEvent]) -> Vec<CommandSeq> {
    events
        .iter()
        .filter_map(|event| match event {
            UserEvent::StateUpdate(snapshot) => {
                let id = snapshot.accounts_for();
                assert_eq!(id.session, UI, "a snapshot for another UI");
                Some(id.seq)
            }
            _ => None,
        })
        .collect()
}

impl Harness {
    /// Creates the harness on a runtime that is at rest.
    ///
    /// The tasks alive right after the dispatcher has been created are the
    /// reference for [`Self::quiescence`], so nothing else may be alive at that
    /// moment. A test that goes through several harnesses in a loop drops one and
    /// creates the next; the lanes of the dropped one end a moment later, and
    /// counting them would make the next harness consider itself at rest too early.
    async fn new() -> Self {
        let metrics = tokio::runtime::Handle::current().metrics();
        tokio::time::timeout(Duration::from_secs(10), async {
            while metrics.num_alive_tasks() > 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("tasks of an earlier harness are still running");

        let (opened, dialog_opened) = std_mpsc::channel();
        let (close_dialog, close) = std_mpsc::channel();
        // The receiver may already be gone when a test has finished.
        let (proxy, events) = lenient_event_channel();
        let mut state = AppState::default();
        state.config = AppConfig::default();
        state.current_path = "/repo".to_string();
        // What the tree shows: commands that act on a row act on one of these.
        state.set_full_file_list(
            [("/repo/src", true), ("/repo/src/main.rs", false)]
                .map(|(path, is_directory)| crate::core::FileItem {
                    path: PathBuf::from(path),
                    is_directory,
                    parent: Path::new(path).parent().map(Path::to_path_buf),
                    depth: path.matches('/').count() - 1,
                    ..Default::default()
                })
                .to_vec(),
        );
        filtering::apply_filters(&mut state);
        let dispatcher = CommandDispatcher::new();
        Self {
            // The lanes of the dispatcher are the only tasks of an idle harness.
            idle_tasks: metrics.num_alive_tasks(),
            dispatcher,
            dialog: Arc::new(BlockingDialog {
                opened: Mutex::new(opened),
                close: Mutex::new(close),
            }),
            dialog_opened,
            close_dialog,
            proxy,
            events,
            state: SharedState::new(state),
            last_seq: AtomicU64::new(0),
        }
    }

    /// A harness whose loaded directory is `project` (not scanned yet).
    async fn with_project(project: &Path) -> Self {
        let harness = Self::new().await;
        {
            let mut state = harness.state.lock();
            state.current_path = project.to_string_lossy().to_string();
            state.set_full_file_list(Vec::new());
            filtering::apply_filters(&mut state);
        }
        harness
    }

    /// Lists `directory` (not loaded yet), as the first level of a scan does.
    fn list_directory(&self, directory: &Path) {
        let mut state = self.state.lock();
        state.set_full_file_list(vec![crate::core::FileItem {
            path: directory.to_path_buf(),
            is_directory: true,
            parent: directory.parent().map(Path::to_path_buf),
            depth: 1,
            ..Default::default()
        }]);
        filtering::apply_filters(&mut state);
    }

    fn drop_path(&self, path: &Path) {
        dispatch_dropped_path(
            &self.dispatcher,
            path.to_path_buf(),
            self.proxy.clone(),
            self.state.clone(),
        );
    }

    /// Waits until everything sent so far has been dealt with completely: the
    /// commands have run (including the part of a command that suspended) and the
    /// background tasks they started have ended.
    ///
    /// The reference is the number of tasks that were alive when the harness was
    /// created, i.e. the lanes of its dispatcher (see [`Self::new`]).
    async fn quiescence(&self) {
        // The marker suspends once, so it passes through the ordered lane and the
        // continuation lane behind everything that was sent before it.
        let (done, marker_ran) = tokio::sync::oneshot::channel();
        self.dispatcher.dispatch(None, async move {
            tokio::task::yield_now().await;
            let _ = done.send(());
        });
        let metrics = tokio::runtime::Handle::current().metrics();
        tokio::time::timeout(Duration::from_secs(10), async {
            marker_ran.await.expect("the marker command was dropped");
            while metrics.num_alive_tasks() > self.idle_tasks {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the application did not come to rest");
    }

    /// Posts a command the way the UI does, numbered; returns its number.
    fn send(&self, command: &str, payload: serde_json::Value) -> CommandSeq {
        let seq = self.last_seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.post(
            serde_json::json!({
                "session": UI,
                "seq": seq,
                "command": command,
                "payload": payload,
            })
            .to_string(),
        );
        seq
    }

    /// Posts a message as it is.
    fn post(&self, message: String) {
        dispatch_ipc_message(
            &self.dispatcher,
            message,
            self.dialog.clone(),
            self.proxy.clone(),
            self.state.clone(),
        );
    }

    /// Everything about the state a command could change, for comparisons.
    fn fingerprint(&self) -> String {
        let state = self.state.lock();
        format!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
            state.config,
            state.current_path,
            state.expanded_dirs,
            state.selected_files,
            (
                &state.search_query,
                &state.extension_filter,
                &state.content_search_query
            ),
            state.previewed_file_path,
            (state.is_scanning(), state.is_generating()),
            serde_json::to_string(&view_model::generate_ui_state(&state)).unwrap(),
        )
    }

    /// Waits until `count` events have been sent to the UI.
    async fn events(&mut self, count: usize) -> Vec<UserEvent> {
        let mut events = Vec::with_capacity(count);
        while events.len() < count {
            let event = tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                .await
                .expect("timed out waiting for an event")
                .expect("event channel closed");
            events.push(event);
        }
        events
    }

    fn is_expanded(&self, path: &str) -> bool {
        self.state
            .lock()
            .expanded_dirs
            .contains(&PathBuf::from(path))
    }
}

/// A project directory on disk with a few files and a sub-directory.
fn project_on_disk(marker_file: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("Failed to create temp dir");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src").join("main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join(marker_file), "marker").unwrap();
    dir
}

fn listed_names(state: &AppState) -> Vec<String> {
    let mut names: Vec<String> = state
        .full_file_list()
        .iter()
        .filter_map(|item| item.path.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

// --- Opening a path that may not exist ---------------------------------------------
//
// Whether a dropped or selected path exists is only known once the scan task has
// looked at it. Until then the loaded directory stays; a path that does not exist
// leaves nothing behind but its error message.

impl Harness {
    /// A harness whose loaded directory is `project`, scanned completely, with
    /// `a.txt` selected, `src` expanded and a filename filter set.
    async fn with_scanned_project(project: &Path) -> Self {
        let mut harness = Self::with_project(project).await;
        harness.send("rescanDirectory", serde_json::Value::Null);
        // The rows have to be there before they can be acted on.
        harness.quiescence().await;
        harness.send("toggleSelection", serde_json::json!(project.join("a.txt")));
        harness.send(
            "setExpansion",
            serde_json::json!({ "path": project.join("src"), "expanded": true }),
        );
        harness.send(
            "updateFilters",
            serde_json::json!({
                "searchQuery": "a",
                "extensionFilter": "",
                "contentSearchQuery": "",
            }),
        );
        harness.quiescence().await;
        assert!(harness.state.lock().is_fully_scanned);
        harness.sent_errors();
        harness
    }

    /// The error messages sent to the UI so far; all pending events are consumed.
    fn sent_errors(&mut self) -> Vec<String> {
        std::iter::from_fn(|| self.events.try_recv().ok())
            .filter_map(|event| match event {
                UserEvent::ShowError(message) => Some(message),
                _ => None,
            })
            .collect()
    }

    /// Asserts that the project of [`Self::with_scanned_project`] is loaded just
    /// as that function left it.
    #[track_caller]
    fn assert_scanned_project_is_untouched(&self, project: &Path, context: &str) {
        let state = self.state.lock();
        assert_eq!(Path::new(&state.current_path), project, "{context}");
        assert!(!state.is_scanning() && !state.is_opening(), "{context}");
        assert!(state.is_fully_scanned, "{context}");
        assert_eq!(
            listed_names(&state),
            ["a.txt", "main.rs", "src"],
            "{context}"
        );
        assert_eq!(
            state.selected_files,
            std::collections::HashSet::from([project.join("a.txt")]),
            "{context}"
        );
        assert!(
            state.expanded_dirs.contains(&project.join("src")),
            "{context}"
        );
        assert_eq!(state.search_query, "a", "{context}");
        assert_eq!(
            state.config.last_directory.as_deref(),
            Some(project),
            "{context}"
        );
    }
}
