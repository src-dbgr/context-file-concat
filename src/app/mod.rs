//! The `app` module orchestrates the interaction between the `core` logic and the `ui`.
//!
//! It manages the application state, handles events from the WebView (IPC messages),
//! and sends updates back to the UI. It acts as the "controller" in an MVC-like pattern.

#[cfg(test)]
mod command_contract_tests;
pub mod commands;
pub mod dispatch;
pub mod events;
pub mod file_dialog;
pub mod filtering;
mod helpers;
pub mod proxy;
pub mod selection;
pub mod shared_state;
pub mod state;
pub mod tasks;
pub mod tracked;
pub mod transport;
pub mod tree_index;
pub mod view_model;
pub mod window_geometry;

use crate::app::file_dialog::DialogService;
use std::path::PathBuf;
use std::sync::Arc;
use wry::WebView;

use crate::app::proxy::EventProxy;
use dispatch::CommandDispatcher;
use events::{IpcMessage, UserEvent};
use shared_state::SharedState;

/// The main handler for IPC messages from the WebView: parses the message and queues
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
pub fn dispatch_ipc_message<P: EventProxy, D: DialogService + 'static>(
    dispatcher: &CommandDispatcher,
    message: String,
    dialog_service: Arc<D>,
    proxy: P,
    state: SharedState,
) {
    let Ok(msg) = serde_json::from_str::<IpcMessage>(&message) else {
        tracing::error!("Failed to parse IPC message: {}", message);
        return;
    };

    match Command::from_name(&msg.command) {
        Some(Command::Dialog(command)) => dispatcher.dispatch_blocking(move || {
            run_dialog_command(command, msg.payload, dialog_service.as_ref(), proxy, state);
        }),
        Some(Command::Ordered(command)) => {
            dispatcher.dispatch(run_command(command, msg.payload, proxy, state))
        }
        None => tracing::warn!("Unknown IPC command: {}", msg.command),
    }
}

/// Queues the opening of a path that was dropped onto the window.
///
/// A drop is a command like any other: it replaces the loaded directory, so it has to
/// be ordered relative to the IPC commands (and to other drops). It therefore runs on
/// the ordered lane of the same `dispatcher`.
pub fn dispatch_dropped_path<P: EventProxy>(
    dispatcher: &CommandDispatcher,
    path: PathBuf,
    proxy: P,
    state: SharedState,
) {
    dispatcher.dispatch(async move { commands::open_dropped_path(path, proxy, state) });
}

/// A command of the IPC contract together with the lane it runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// Waits for a native dialog; runs on the blocking lane.
    Dialog(DialogCommand),
    /// Runs on the ordered lane.
    Ordered(OrderedCommand),
}

/// The commands that wait for the user to answer a native dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogCommand {
    SelectDirectory,
    SaveFile,
    PickOutputDirectory,
    ImportConfig,
    ExportConfig,
}

/// The commands of the ordered lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrderedCommand {
    Initialize,
    RescanDirectory,
    ClearDirectory,
    CancelScan,
    LoadDirectoryLevel,
    SetExpansion,
    ExpandCollapseAll,
    SetViewport,
    LocateRow,
    ToggleSelection,
    ToggleDirectorySelection,
    SelectAll,
    DeselectAll,
    UpdateFilters,
    LoadFilePreview,
    ClearPreviewState,
    GeneratePreview,
    CancelGeneration,
    AddIgnorePattern,
    RemoveIgnorePattern,
    ClearIgnorePatterns,
    AddIgnorePath,
    SetConfigOption,
}

/// The routing table: every command of the IPC contract by its wire name.
///
/// This is the one list of command names on the host. The router resolves names with
/// it, and the contract test writes it into the fixture that the UI compares with its
/// `CommandSchemas`, so the two sides cannot drift apart unnoticed.
pub(crate) const COMMANDS: &[(&str, Command)] = {
    use Command::{Dialog, Ordered};
    &[
        ("selectDirectory", Dialog(DialogCommand::SelectDirectory)),
        ("saveFile", Dialog(DialogCommand::SaveFile)),
        (
            "pickOutputDirectory",
            Dialog(DialogCommand::PickOutputDirectory),
        ),
        ("importConfig", Dialog(DialogCommand::ImportConfig)),
        ("exportConfig", Dialog(DialogCommand::ExportConfig)),
        ("initialize", Ordered(OrderedCommand::Initialize)),
        ("rescanDirectory", Ordered(OrderedCommand::RescanDirectory)),
        ("clearDirectory", Ordered(OrderedCommand::ClearDirectory)),
        ("cancelScan", Ordered(OrderedCommand::CancelScan)),
        (
            "loadDirectoryLevel",
            Ordered(OrderedCommand::LoadDirectoryLevel),
        ),
        ("setExpansion", Ordered(OrderedCommand::SetExpansion)),
        (
            "expandCollapseAll",
            Ordered(OrderedCommand::ExpandCollapseAll),
        ),
        ("setViewport", Ordered(OrderedCommand::SetViewport)),
        ("locateRow", Ordered(OrderedCommand::LocateRow)),
        ("toggleSelection", Ordered(OrderedCommand::ToggleSelection)),
        (
            "toggleDirectorySelection",
            Ordered(OrderedCommand::ToggleDirectorySelection),
        ),
        ("selectAll", Ordered(OrderedCommand::SelectAll)),
        ("deselectAll", Ordered(OrderedCommand::DeselectAll)),
        ("updateFilters", Ordered(OrderedCommand::UpdateFilters)),
        ("loadFilePreview", Ordered(OrderedCommand::LoadFilePreview)),
        (
            "clearPreviewState",
            Ordered(OrderedCommand::ClearPreviewState),
        ),
        ("generatePreview", Ordered(OrderedCommand::GeneratePreview)),
        (
            "cancelGeneration",
            Ordered(OrderedCommand::CancelGeneration),
        ),
        (
            "addIgnorePattern",
            Ordered(OrderedCommand::AddIgnorePattern),
        ),
        (
            "removeIgnorePattern",
            Ordered(OrderedCommand::RemoveIgnorePattern),
        ),
        (
            "clearIgnorePatterns",
            Ordered(OrderedCommand::ClearIgnorePatterns),
        ),
        ("addIgnorePath", Ordered(OrderedCommand::AddIgnorePath)),
        ("setConfigOption", Ordered(OrderedCommand::SetConfigOption)),
    ]
};

impl Command {
    fn from_name(name: &str) -> Option<Self> {
        COMMANDS
            .iter()
            .find(|(command_name, _)| *command_name == name)
            .map(|(_, command)| *command)
    }
}

/// Executes a dialog command. Blocks the calling thread while the dialog is open, so
/// it must run on a thread that is allowed to block (the dispatcher's blocking lane).
fn run_dialog_command<P: EventProxy, D: DialogService + ?Sized>(
    command: DialogCommand,
    payload: serde_json::Value,
    dialog: &D,
    proxy: P,
    state: SharedState,
) {
    match command {
        DialogCommand::SelectDirectory => commands::select_directory(dialog, proxy, state),
        DialogCommand::SaveFile => commands::save_file(dialog, payload, proxy, state),
        DialogCommand::PickOutputDirectory => commands::pick_output_directory(dialog, proxy, state),
        DialogCommand::ExportConfig => commands::export_config(dialog, proxy, state),
        DialogCommand::ImportConfig => commands::import_config(dialog, proxy, state),
    }
}

/// Executes a command of the ordered lane.
///
/// Most commands are synchronous: they mutate the state, possibly launch a background
/// task (scan, lazy load, generation, file preview) and return. Whatever such a command
/// means for the state (the scan is registered, the directory is expanded, the file is
/// the previewed one) is done by the command itself, not by the task, so that the next
/// command finds it done. The commands that are awaited can suspend while they wait for
/// a content search (`updateFilters`, and a change of the case sensitivity while a
/// content search is active); the dispatcher then completes them on its continuation
/// lane.
async fn run_command<P: EventProxy>(
    command: OrderedCommand,
    payload: serde_json::Value,
    proxy: P,
    state: SharedState,
) {
    use OrderedCommand as C;
    match command {
        // --- Asynchronous commands (must be awaited) ---
        C::UpdateFilters => commands::update_filters(payload, proxy, state).await,
        C::AddIgnorePattern => commands::add_ignore_pattern(payload, proxy, state).await,
        C::RemoveIgnorePattern => commands::remove_ignore_pattern(payload, proxy, state).await,
        C::ClearIgnorePatterns => commands::clear_ignore_patterns(proxy, state).await,
        C::AddIgnorePath => commands::add_ignore_path(payload, proxy, state).await,
        C::SetConfigOption => commands::set_config_option(payload, proxy, state).await,

        // --- Synchronous commands & task launchers ---
        C::RescanDirectory => commands::rescan_directory(proxy, state),
        C::LoadDirectoryLevel => commands::load_directory_level(payload, proxy, state),
        C::GeneratePreview => commands::generate_preview(proxy, state),
        C::ClearDirectory => commands::clear_directory(proxy, state),
        C::CancelScan => commands::cancel_scan(proxy, state),
        C::Initialize => commands::initialize(proxy, state),
        C::LoadFilePreview => commands::load_file_preview(payload, proxy, state),
        C::ToggleSelection => commands::toggle_selection(payload, proxy, state),
        C::ToggleDirectorySelection => commands::toggle_directory_selection(payload, proxy, state),
        C::SetExpansion => commands::set_expansion(payload, proxy, state),
        C::ExpandCollapseAll => commands::expand_collapse_all(payload, proxy, state),
        C::SetViewport => commands::set_viewport(payload, proxy, state),
        C::LocateRow => commands::locate_row(payload, proxy, state),
        C::SelectAll => commands::select_all(proxy, state),
        C::DeselectAll => commands::deselect_all(proxy, state),
        C::CancelGeneration => commands::cancel_generation(proxy, state),
        C::ClearPreviewState => commands::clear_preview_state(proxy, state),
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
            content,
            token_count,
        } => format!(
            "window.showGeneratedContent({}, {});",
            json(content)?,
            token_count
        ),
        UserEvent::ShowError(msg) => format!("window.showError({});", json(msg)?),
        UserEvent::SaveComplete(success, path) => {
            format!("window.fileSaveStatus({}, {});", success, json(path)?)
        }
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

fn event_name(event: &UserEvent) -> &'static str {
    match event {
        UserEvent::StateUpdate(_) => "StateUpdate",
        UserEvent::ShowFilePreview { .. } => "ShowFilePreview",
        UserEvent::ShowGeneratedContent { .. } => "ShowGeneratedContent",
        UserEvent::ShowError(_) => "ShowError",
        UserEvent::SaveComplete(..) => "SaveComplete",
        UserEvent::ConfigExported(_) => "ConfigExported",
        UserEvent::ScanProgress(_) => "ScanProgress",
        UserEvent::DragStateChanged(_) => "DragStateChanged",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::{AppState, StatusNotice};
    use crate::config::AppConfig;
    use std::path::Path;
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
            }
        }

        /// A harness whose loaded directory is `project` (not scanned yet).
        async fn with_project(project: &Path) -> Self {
            let harness = Self::new().await;
            harness.state.lock().current_path = project.to_string_lossy().to_string();
            harness
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
            self.dispatcher.dispatch(async move {
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

        fn send(&self, command: &str, payload: serde_json::Value) {
            dispatch_ipc_message(
                &self.dispatcher,
                serde_json::json!({ "command": command, "payload": payload }).to_string(),
                self.dialog.clone(),
                self.proxy.clone(),
                self.state.clone(),
            );
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

    #[test]
    fn every_dialog_command_is_routed_to_the_blocking_lane() {
        for name in [
            "selectDirectory",
            "saveFile",
            "pickOutputDirectory",
            "importConfig",
            "exportConfig",
        ] {
            assert!(
                matches!(Command::from_name(name), Some(Command::Dialog(_))),
                "{name}"
            );
        }
        for name in ["setExpansion", "updateFilters", "generatePreview"] {
            assert!(
                matches!(Command::from_name(name), Some(Command::Ordered(_))),
                "{name}"
            );
        }
        let dialog_commands = COMMANDS
            .iter()
            .filter(|(_, command)| matches!(command, Command::Dialog(_)))
            .count();
        assert_eq!(dialog_commands, 5);
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

    /// The former whole-config command is gone; a stale UI cannot replace the config.
    #[tokio::test]
    async fn update_config_is_not_a_command_anymore() {
        let mut harness = Harness::new().await;
        let before = harness.state.lock().config.clone();
        let mut replaced = before.clone();
        replaced.ignore_patterns.clear();

        harness.send("updateConfig", serde_json::to_value(replaced).unwrap());
        harness.send("initialize", serde_json::Value::Null);
        harness.events(1).await;

        assert_eq!(harness.state.lock().config, before);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn commands_take_effect_in_the_order_they_were_sent() {
        let mut harness = Harness::new().await;

        // Each round ends with a different final command; with unordered execution
        // some rounds would end in the wrong state.
        for round in 0..200 {
            let ends_expanded = round % 2 == 0;
            for step in 0..5 {
                let expanded = (step % 2 == 0) == ends_expanded;
                harness.send(
                    "setExpansion",
                    serde_json::json!({ "path": "/repo/src", "expanded": expanded }),
                );
            }
            harness.events(5).await;
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

        harness.send("saveFile", serde_json::json!("content"));
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
        assert!(matches!(events[0], UserEvent::SaveComplete(false, _)));
        harness
            .dialog_opened
            .recv_timeout(Duration::from_secs(5))
            .expect("the second dialog was not opened");
        harness.close_dialog.send(()).unwrap();
    }

    #[tokio::test]
    async fn malformed_and_unknown_messages_are_ignored() {
        let mut harness = Harness::new().await;

        harness.send("noSuchCommand", serde_json::Value::Null);
        dispatch_ipc_message(
            &harness.dispatcher,
            "not json".to_string(),
            harness.dialog.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        harness.send("initialize", serde_json::Value::Null);

        let events = harness.events(1).await;
        assert!(matches!(events[0], UserEvent::StateUpdate(_)));
        assert!(harness.events.try_recv().is_err());
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
            .full_file_list
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
            assert!(state.full_file_list.is_empty(), "round {round}");
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
            assert!(state.full_file_list.is_empty(), "round {round}");
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
            assert_eq!(listed_names(&state), ["main.rs"], "round {round}");
        }
    }

    /// The counterpart: a request to load a directory that follows a collapse expands it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn a_lazy_load_sent_right_after_a_collapse_ends_expanded() {
        let project = project_on_disk("a.txt");
        let harness = Harness::with_project(project.path()).await;
        let src = project.path().join("src");

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
            serde_json::json!({ "command": "selectDirectory", "payload": null }).to_string(),
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
    /// follows it cancels it. What was loaded before is still there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
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
            assert!(state.full_file_list.is_empty(), "round {round}");
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
        harness.dispatcher.dispatch(async move {
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
        let send = |command: &str, payload: serde_json::Value| {
            dispatch_ipc_message(
                &dispatcher,
                serde_json::json!({ "command": command, "payload": payload }).to_string(),
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
            send("saveFile", serde_json::json!("content"));

            loop {
                let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
                    .await
                    .expect("the save dialog did not run")
                    .expect("the event channel was closed");
                if matches!(event, UserEvent::SaveComplete(false, _)) {
                    break;
                }
            }
            assert_eq!(dialog.proposed.lock().unwrap().last(), Some(&name));
        }
    }

    // --- Serialization of events ---

    fn ui_state_with_tree(paths: &[PathBuf]) -> Box<view_model::UiState> {
        let mut state = AppState::default();
        state.current_path = "/repo".to_string();
        state.full_file_list = paths
            .iter()
            .map(|path| crate::core::FileItem {
                path: path.clone(),
                parent: path.parent().map(Path::to_path_buf),
                depth: 1,
                ..Default::default()
            })
            .collect();
        filtering::apply_filters(&mut state);
        Box::new(view_model::generate_ui_state(&state))
    }

    #[test]
    fn every_event_becomes_one_call_with_its_arguments() {
        let script = |event| script_for(&event).unwrap();

        let render = script(UserEvent::StateUpdate(ui_state_with_tree(&[
            PathBuf::from("/repo/a.rs"),
        ])));
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
                content: "x".to_string(),
                token_count: 3,
            }),
            r#"window.showGeneratedContent("x", 3);"#
        );
        assert_eq!(
            script(UserEvent::ShowError("it's \"broken\"".to_string())),
            r#"window.showError("it's \"broken\"");"#
        );
        assert_eq!(
            script(UserEvent::SaveComplete(true, "/out/a.txt".to_string())),
            r#"window.fileSaveStatus(true, "/out/a.txt");"#
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
        let render = script_for(&UserEvent::StateUpdate(ui_state)).unwrap();
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
