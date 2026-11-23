//! The `app` module orchestrates the interaction between the `core` logic and the `ui`.
//!
//! It manages the application state, handles events from the WebView (IPC messages),
//! and sends updates back to the UI. It acts as the "controller" in an MVC-like pattern.

#[cfg(test)]
mod command_contract_tests;
pub mod commands;
pub mod correlation;
pub mod dispatch;
#[cfg(test)]
mod event_contract_tests;
pub mod events;
pub mod file_dialog;
pub mod filtering;
mod helpers;
pub(crate) mod ipc;
pub mod proxy;
pub mod selection;
pub mod shared_state;
pub mod snapshot;
mod staged;
pub mod state;
pub mod tasks;
pub mod tracked;
pub mod transport;
pub mod tree_index;
pub mod view_model;
#[cfg(test)]
mod window_contract_tests;
pub mod window_geometry;

use crate::app::file_dialog::DialogService;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use wry::WebView;

use crate::app::proxy::EventProxy;
use correlation::Correlated;
use dispatch::CommandDispatcher;
use events::UserEvent;
use ipc::{excerpt, Command, IpcMessage};
use shared_state::SharedState;

/// The main handler for IPC messages from the WebView: reads the message and queues
/// its command on the matching lane of `dispatcher`, which executes commands in arrival
/// order (see [`dispatch`]). Returns immediately, so the event loop is never blocked.
///
/// The dispatcher is owned by the caller (the composition root in `main.rs`), so there
/// is no process-wide state and tests can use their own instance.
///
/// - Commands that open a native dialog block for as long as the user likes. They run
///   on the blocking lane: they start after the commands that arrived before them, and
///   they neither hold up nor are ordered relative to the commands that arrive later,
///   while the dialog is open.
/// - Every other command runs on the ordered lane, in arrival order.
/// - A message that is no command is refused: it is logged and answered with the
///   unchanged snapshot, at its place in the order (see [`ipc`]).
///
/// The name of the message is handed to the dispatcher, and the events of the
/// command are sent through a proxy that writes into every snapshot how far the
/// commands have got (see [`correlation`]). The commands know nothing of either.
pub fn dispatch_ipc_message<P: EventProxy, D: DialogService + 'static>(
    dispatcher: &CommandDispatcher,
    message: String,
    dialog_service: Arc<D>,
    proxy: P,
    state: SharedState,
) {
    let proxy = Correlated::new(proxy, dispatcher.progress().clone());

    // What is logged of a message is short, whatever its size (see [`ipc`]).
    let (id, command) = match IpcMessage::parse(&message) {
        IpcMessage::Command { id, command } => (Some(id), Some(command)),
        IpcMessage::Refused { id, reason } => {
            tracing::warn!(
                "Refused IPC message {} of UI {}: {reason}",
                id.seq,
                id.session
            );
            (Some(id), None)
        }
        IpcMessage::Unreadable { reason } => {
            tracing::error!(
                "Refused an IPC message that cannot be read ({reason}): {}",
                excerpt(&message)
            );
            (None, None)
        }
    };

    let work = match command {
        Some(command) => command.into_work(dialog_service, proxy, state),
        None => at_once(move || commands::answer_unchanged(&proxy, &state)),
    };
    match work {
        Work::Ordered(command) => dispatcher.dispatch(id, command),
        Work::Blocking(command) => dispatcher.dispatch_blocking(id, command),
    }
}

/// Queues the opening of a path that was dropped onto the window.
///
/// A drop is a command like any other: it replaces the loaded directory, so it has to
/// be ordered relative to the IPC commands (and to other drops). It therefore runs on
/// the ordered lane of the same `dispatcher`. It has no number (the UI did not post
/// it); the snapshots it leads to carry the number of the last command of the UI.
pub fn dispatch_dropped_path<P: EventProxy>(
    dispatcher: &CommandDispatcher,
    path: PathBuf,
    proxy: P,
    state: SharedState,
) {
    let proxy = Correlated::new(proxy, dispatcher.progress().clone());
    dispatcher.dispatch(None, async move {
        commands::open_dropped_path(path, proxy, state)
    });
}

/// What a command does, on the lane it is done on.
enum Work {
    /// Runs on the ordered lane.
    Ordered(Pin<Box<dyn Future<Output = ()> + Send>>),
    /// Waits for a native dialog; runs on the blocking lane.
    Blocking(Box<dyn FnOnce() + Send>),
}

/// A command of the ordered lane that can suspend.
fn ordered(command: impl Future<Output = ()> + Send + 'static) -> Work {
    Work::Ordered(Box::pin(command))
}

/// A command of the ordered lane that runs to its end at once.
fn at_once(command: impl FnOnce() + Send + 'static) -> Work {
    ordered(async move { command() })
}

/// A command that blocks its thread while a dialog is open, so it must run on a
/// thread that is allowed to block (the dispatcher's blocking lane).
fn blocking(command: impl FnOnce() + Send + 'static) -> Work {
    Work::Blocking(Box::new(command))
}

impl Command {
    /// The handler of the command, bound to its payload, on the lane it belongs to.
    ///
    /// This is the one place that says what a command runs and where. A command that
    /// is added to [`Command`] does not compile until it is listed here.
    ///
    /// Most commands are synchronous: they mutate the state, possibly launch a
    /// background task (scan, lazy load, generation, file preview) and return.
    /// Whatever such a command means for the state (the scan is registered, the
    /// directory is expanded, the file is the previewed one) is done by the command
    /// itself, not by the task, so that the next command finds it done. The commands
    /// that are awaited can suspend while they wait for a content search
    /// (`updateFilters`, and a change of the case sensitivity while a content search
    /// is active); the dispatcher then completes them on its continuation lane.
    fn into_work<P: EventProxy, D: DialogService + 'static>(
        self,
        dialog: Arc<D>,
        proxy: P,
        state: SharedState,
    ) -> Work {
        use commands as c;
        match self {
            // --- Commands that wait for a native dialog ---
            Self::SelectDirectory => {
                blocking(move || c::select_directory(dialog.as_ref(), proxy, state))
            }
            Self::SaveFile(payload) => {
                blocking(move || c::save_file(dialog.as_ref(), payload, proxy, state))
            }
            Self::PickOutputDirectory => {
                blocking(move || c::pick_output_directory(dialog.as_ref(), proxy, state))
            }
            Self::ImportConfig => blocking(move || c::import_config(dialog.as_ref(), proxy, state)),
            Self::ExportConfig => blocking(move || c::export_config(dialog.as_ref(), proxy, state)),

            // --- Commands that can suspend ---
            Self::UpdateFilters(filters) => ordered(c::update_filters(filters, proxy, state)),
            Self::AddIgnorePattern(pattern) => {
                ordered(c::add_ignore_pattern(pattern, proxy, state))
            }
            Self::RemoveIgnorePattern(pattern) => {
                ordered(c::remove_ignore_pattern(pattern, proxy, state))
            }
            Self::ClearIgnorePatterns => ordered(c::clear_ignore_patterns(proxy, state)),
            Self::AddIgnorePath(path) => ordered(c::add_ignore_path(path, proxy, state)),
            Self::SetConfigOption(option) => ordered(c::set_config_option(option, proxy, state)),

            // --- Synchronous commands & task launchers ---
            Self::Initialize => at_once(move || c::initialize(proxy, state)),
            Self::RescanDirectory => at_once(move || c::rescan_directory(proxy, state)),
            Self::ClearDirectory => at_once(move || c::clear_directory(proxy, state)),
            Self::CancelScan => at_once(move || c::cancel_scan(proxy, state)),
            Self::LoadDirectoryLevel(path) => {
                at_once(move || c::load_directory_level(path, proxy, state))
            }
            Self::SetExpansion(payload) => at_once(move || c::set_expansion(payload, proxy, state)),
            Self::ExpandCollapseAll(expand) => {
                at_once(move || c::expand_collapse_all(expand, proxy, state))
            }
            Self::SetViewport(viewport) => at_once(move || c::set_viewport(viewport, proxy, state)),
            Self::LocateRow(lookup) => at_once(move || c::locate_row(lookup, proxy, state)),
            Self::ToggleSelection(path) => at_once(move || c::toggle_selection(path, proxy, state)),
            Self::ToggleDirectorySelection(path) => {
                at_once(move || c::toggle_directory_selection(path, proxy, state))
            }
            Self::SelectAll => at_once(move || c::select_all(proxy, state)),
            Self::DeselectAll => at_once(move || c::deselect_all(proxy, state)),
            Self::LoadFilePreview(path) => {
                at_once(move || c::load_file_preview(path, proxy, state))
            }
            Self::ClearPreviewState => at_once(move || c::clear_preview_state(proxy, state)),
            Self::GeneratePreview => at_once(move || c::generate_preview(proxy, state)),
            Self::CancelGeneration => at_once(move || c::cancel_generation(proxy, state)),
        }
    }
}

/// Processes events sent from the backend to the UI thread.
///
/// It translates each `UserEvent` into a JavaScript call in the WebView to update the UI.
/// Events reach this function via [`transport::EventOutbox::next_event`], which has
/// already dropped superseded `StateUpdate`s, so no stale snapshot is serialized here.
///
/// An event that cannot be serialized is logged and skipped: a call without its
/// argument (`window.render();`) would only make the UI report an invalid payload.
pub fn handle_user_event(event: UserEvent, webview: &WebView) {
    let script = match script_for(&event) {
        Ok(script) => script,
        Err(e) => {
            tracing::error!(
                "Failed to serialize a {} event for the UI: {e}. The event is skipped.",
                event_name(&event)
            );
            return;
        }
    };
    if let Err(e) = webview.evaluate_script(&script) {
        tracing::error!("Failed to evaluate script: {}", e);
    }
}

/// The `window.*` call that delivers `event` to the UI.
fn script_for(event: &UserEvent) -> serde_json::Result<String> {
    use serde_json::to_string as json;
    Ok(match event {
        UserEvent::StateUpdate(state) => format!("window.render({});", json(state)?),
        UserEvent::ShowFilePreview {
            content,
            language,
            search_term,
            path,
        } => format!(
            "window.showPreviewContent({}, {}, {}, {});",
            json(content)?,
            json(language)?,
            json(search_term)?,
            // Lossy like the paths of the tree, see `view_model::lossy_path`.
            json(&path.to_string_lossy())?,
        ),
        UserEvent::ShowGeneratedContent {
            document,
            token_count,
        } => generated_content_script(document.text(), *token_count, document.id())?,
        UserEvent::OutputSize(notice) => {
            format!("window.outputSizeNotice({});", json(notice)?)
        }
        UserEvent::IgnorePathRefused(refusal) => {
            format!("window.ignorePathRefused({});", json(refusal)?)
        }
        UserEvent::ShowError(msg) => format!("window.showError({});", json(msg)?),
        UserEvent::SaveResult(result) => format!("window.saveResult({});", json(result)?),
        UserEvent::ConfigExported(success) => format!(
            "window.showStatus('{}');",
            if *success {
                "Config exported successfully."
            } else {
                "Failed to export config."
            }
        ),
        UserEvent::ScanProgress(progress) => {
            format!("window.updateScanProgress({});", json(progress)?)
        }
        UserEvent::DragStateChanged(is_dragging) => {
            format!("window.setDragState({is_dragging});")
        }
    })
}

/// The call that shows a generated document.
///
/// A document can weigh many megabytes, so the script is written into one buffer of
/// about its final size: the text is escaped straight into it and exists once more,
/// not three times (as a JSON string, as the formatted script and while that grows).
fn generated_content_script(
    text: &str,
    token_count: usize,
    document_id: u64,
) -> serde_json::Result<String> {
    use std::io::Write;

    // Room for the escapes of a typical text: a line end takes two bytes instead of one.
    let mut script = Vec::with_capacity(text.len() + text.len() / 8 + 96);
    script.extend_from_slice(b"window.showGeneratedContent(");
    serde_json::to_writer(&mut script, text)?;
    // Writing into a `Vec` cannot fail.
    let _ = write!(script, ", {token_count}, {document_id});");
    // JSON is UTF-8, and so is everything else that was written.
    String::from_utf8(script).map_err(serde::ser::Error::custom)
}

fn event_name(event: &UserEvent) -> &'static str {
    match event {
        UserEvent::StateUpdate(_) => "StateUpdate",
        UserEvent::ShowFilePreview { .. } => "ShowFilePreview",
        UserEvent::ShowGeneratedContent { .. } => "ShowGeneratedContent",
        UserEvent::IgnorePathRefused(_) => "IgnorePathRefused",
        UserEvent::ShowError(_) => "ShowError",
        UserEvent::OutputSize(_) => "OutputSize",
        UserEvent::SaveResult(_) => "SaveResult",
        UserEvent::ConfigExported(_) => "ConfigExported",
        UserEvent::ScanProgress(_) => "ScanProgress",
        UserEvent::DragStateChanged(_) => "DragStateChanged",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::correlation::{CommandId, CommandSeq, SessionId};
    use crate::app::events::{OutputSizeNotice, SaveResult};
    use crate::app::snapshot::{send_snapshot, Snapshot};
    use crate::app::state::{AppState, GeneratedDocument, StatusNotice};
    use crate::config::AppConfig;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc as std_mpsc;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[derive(Clone)]
    struct TestEventProxy(mpsc::UnboundedSender<UserEvent>);

    impl EventProxy for TestEventProxy {
        fn send_event(&self, event: UserEvent) {
            // The receiver may already be gone when a test has finished.
            let _ = self.0.send(event);
        }
    }

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
        events: mpsc::UnboundedReceiver<UserEvent>,
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
            let (tx, events) = mpsc::unbounded_channel();
            let mut state = AppState::default();
            state.config = AppConfig::default();
            state.current_path = "/repo".to_string();
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
                proxy: TestEventProxy(tx),
                events,
                state: SharedState::new(state),
                last_seq: AtomicU64::new(0),
            }
        }

        /// A harness whose loaded directory is `project` (not scanned yet).
        async fn with_project(project: &Path) -> Self {
            let harness = Self::new().await;
            harness.state.lock().current_path = project.to_string_lossy().to_string();
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
            state.config.window_size = (640.0, 480.0);
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
        assert_eq!(harness.state.lock().config.window_size, (640.0, 480.0));

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
        let before = harness.fingerprint();
        let seq = harness.send("selectAll", serde_json::Value::Null);
        assert_eq!(seqs_of(&harness.events(1).await), [seq]);

        for message in [
            "not json".to_string(),
            String::new(),
            "[]".to_string(),
            json!({ "command": "deselectAll", "payload": null }).to_string(),
            json!({ "seq": 7, "command": "deselectAll", "payload": null }).to_string(),
            json!({ "session": UI, "command": "deselectAll", "payload": null }).to_string(),
            json!({ "session": UI, "seq": "7", "command": "deselectAll", "payload": null })
                .to_string(),
            json!({ "session": UI, "seq": -7, "command": "deselectAll", "payload": null })
                .to_string(),
            json!({ "session": "31", "seq": 7, "command": "deselectAll", "payload": null })
                .to_string(),
        ] {
            harness.post(message.clone());
            assert_eq!(seqs_of(&harness.events(1).await), [seq], "{message}");
            assert_eq!(harness.fingerprint(), before, "{message}");
        }
    }

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

        // A file that is selected by such a path is no file of the list, so it is
        // not part of what is generated.
        for path in &outside {
            harness.send("toggleSelection", serde_json::json!(path));
        }
        harness.quiescence().await;
        let plan = tasks::GenerationPlan::of(&harness.state.lock());
        assert_eq!(plan.selected_files, [project.join("a.txt")]);
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

    // --- Commands that start background work -------------------------------------------
    //
    // Starting a scan or a lazy load is a state change of its own (the scan is
    // registered, the directory is marked as expanded). It has to happen in the command,
    // not in the task the command spawns: otherwise the next command overtakes it.

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
            assert_eq!(state.config.last_directory, None, "round {round}");
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
                state.config.last_directory.as_deref(),
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
                harness
                    .assert_scanned_project_is_untouched(project.path(), &format!("round {round}"));
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
            assert_eq!(state.config.last_directory, None, "round {round}");
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
        let proxy = TestEventProxy(tx);
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

    // --- Serialization of events ---

    fn ui_state_with_tree(paths: &[PathBuf]) -> view_model::UiState {
        let mut state = AppState::default();
        state.current_path = "/repo".to_string();
        state.set_full_file_list(
            paths
                .iter()
                .map(|path| crate::core::FileItem {
                    path: path.clone(),
                    parent: path.parent().map(Path::to_path_buf),
                    depth: 1,
                    ..Default::default()
                })
                .collect(),
        );
        filtering::apply_filters(&mut state);
        view_model::generate_ui_state(&state)
    }

    #[test]
    fn every_event_becomes_one_call_with_its_arguments() {
        let script = |event| script_for(&event).unwrap();

        let render = script(UserEvent::StateUpdate(Snapshot::for_test(
            ui_state_with_tree(&[PathBuf::from("/repo/a.rs")]),
        )));
        assert!(
            render.starts_with("window.render({\"config\":{"),
            "{render}"
        );
        assert!(render.contains("\"path\":\"/repo/a.rs\""), "{render}");
        assert!(render.ends_with("});"), "{render}");

        assert_eq!(
            script(UserEvent::ShowFilePreview {
                content: "let a = \"b\";".to_string(),
                language: "rust".to_string(),
                search_term: None,
                path: PathBuf::from("/repo/a.rs"),
            }),
            r#"window.showPreviewContent("let a = \"b\";", "rust", null, "/repo/a.rs");"#
        );
        assert_eq!(
            script(UserEvent::ShowGeneratedContent {
                document: GeneratedDocument::for_test(7, "x \"y\"\n"),
                token_count: 3,
            }),
            r#"window.showGeneratedContent("x \"y\"\n", 3, 7);"#
        );
        assert_eq!(
            script(UserEvent::OutputSize(OutputSizeNotice::Large {
                estimated_bytes: 30,
                limit_bytes: 20,
            })),
            r#"window.outputSizeNotice({"status":"large","estimated_bytes":30,"limit_bytes":20});"#
        );
        assert_eq!(
            script(UserEvent::OutputSize(OutputSizeNotice::TooLarge {
                estimated_bytes: 300,
                limit_bytes: 200,
            })),
            r#"window.outputSizeNotice({"status":"too_large","estimated_bytes":300,"limit_bytes":200});"#
        );
        assert_eq!(
            script(UserEvent::ShowError("it's \"broken\"".to_string())),
            r#"window.showError("it's \"broken\"");"#
        );
        assert_eq!(
            script(UserEvent::SaveResult(SaveResult::Saved {
                path: "/out/a.txt".to_string()
            })),
            r#"window.saveResult({"status":"saved","path":"/out/a.txt"});"#
        );
        assert_eq!(
            script(UserEvent::SaveResult(SaveResult::Cancelled)),
            r#"window.saveResult({"status":"cancelled"});"#
        );
        assert_eq!(
            script(UserEvent::SaveResult(SaveResult::Failed {
                error: "disk \"full\"".to_string()
            })),
            r#"window.saveResult({"status":"failed","error":"disk \"full\""});"#
        );
        assert_eq!(
            script(UserEvent::ConfigExported(false)),
            "window.showStatus('Failed to export config.');"
        );
        assert_eq!(
            script(UserEvent::DragStateChanged(true)),
            "window.setDragState(true);"
        );
        let progress = script(UserEvent::ScanProgress(crate::core::ScanProgress {
            files_scanned: 4,
            large_files_skipped: 1,
            current_scanning_path: "/repo/src".to_string(),
        }));
        assert!(
            progress.starts_with("window.updateScanProgress({"),
            "{progress}"
        );
        assert!(progress.contains("\"files_scanned\":4"), "{progress}");
    }

    /// A file name that is not valid Unicode must not cost the UI its snapshot: serde
    /// refuses such a path, so paths are sent lossily.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_unicode_does_not_make_an_event_unserializable() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let odd = PathBuf::from("/repo").join(OsStr::from_bytes(b"caf\xe9.txt"));
        assert!(
            serde_json::to_string(&odd).is_err(),
            "what serde does on its own"
        );

        let mut ui_state = ui_state_with_tree(&[odd.clone(), PathBuf::from("/repo/b.txt")]);
        ui_state.config.output_directory = Some(odd.clone());
        let render = script_for(&UserEvent::StateUpdate(Snapshot::for_test(ui_state))).unwrap();
        assert!(
            render.contains("\"path\":\"/repo/caf\u{fffd}.txt\""),
            "{render}"
        );
        assert!(render.contains("\"path\":\"/repo/b.txt\""), "{render}");
        assert!(
            render.contains("\"output_directory\":\"/repo/caf\u{fffd}.txt\""),
            "{render}"
        );

        let preview = script_for(&UserEvent::ShowFilePreview {
            content: String::new(),
            language: "plaintext".to_string(),
            search_term: Some("x".to_string()),
            path: odd,
        })
        .unwrap();
        assert_eq!(
            preview,
            "window.showPreviewContent(\"\", \"plaintext\", \"x\", \"/repo/caf\u{fffd}.txt\");"
        );
    }
}
