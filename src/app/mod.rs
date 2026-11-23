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
pub mod state;
pub mod tasks;
pub mod transport;
pub mod tree_index;
pub mod view_model;
pub mod window_geometry;

use crate::app::file_dialog::DialogService;
use std::sync::{Arc, Mutex};
use wry::WebView;

use crate::app::proxy::EventProxy;
use dispatch::CommandDispatcher;
use events::{IpcMessage, UserEvent};
use state::AppState;

/// The main handler for IPC messages from the WebView: parses the message and queues
/// its command on the matching lane of `dispatcher`, which executes commands in arrival
/// order (see [`dispatch`]). Returns immediately, so the event loop is never blocked.
///
/// The dispatcher is owned by the caller (the composition root in `main.rs`), so there
/// is no process-wide state and tests can use their own instance.
///
/// - Commands that open a native dialog block for as long as the user likes. They run
///   on the blocking lane and therefore do not hold up, and are not ordered relative
///   to, the commands that arrive while the dialog is open.
/// - Every other command runs on the ordered lane, in arrival order.
pub fn dispatch_ipc_message<P: EventProxy, D: DialogService + 'static>(
    dispatcher: &CommandDispatcher,
    message: String,
    dialog_service: Arc<D>,
    proxy: P,
    state: Arc<Mutex<AppState>>,
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
    state: Arc<Mutex<AppState>>,
) {
    match command {
        DialogCommand::SelectDirectory => commands::select_directory(dialog, proxy, state),
        DialogCommand::SaveFile => commands::save_file(dialog, payload, proxy, state),
        DialogCommand::PickOutputDirectory => commands::pick_output_directory(dialog, proxy, state),
        DialogCommand::ExportConfig => commands::export_config(dialog, proxy, state),
        DialogCommand::ImportConfig => tokio::runtime::Handle::current()
            .block_on(commands::import_config(dialog, proxy, state)),
    }
}

/// Executes a command of the ordered lane.
///
/// Most commands are synchronous: they mutate the state, possibly launch a background
/// task (scan, generation) and return. The commands that are awaited can suspend while
/// they wait for a content search (`updateFilters`, and a change of the case
/// sensitivity while a content search is active); the dispatcher then completes them
/// on its continuation lane.
async fn run_command<P: EventProxy>(
    command: OrderedCommand,
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
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
pub fn handle_user_event(event: UserEvent, webview: &WebView) {
    let script = match event {
        UserEvent::StateUpdate(state) => {
            format!(
                "window.render({});",
                serde_json::to_string(&*state).unwrap_or_default()
            )
        }
        UserEvent::ShowFilePreview {
            content,
            language,
            search_term,
            path,
        } => format!(
            "window.showPreviewContent({}, {}, {}, {});",
            serde_json::to_string(&content).unwrap_or_default(),
            serde_json::to_string(&language).unwrap_or_default(),
            serde_json::to_string(&search_term).unwrap_or_default(),
            serde_json::to_string(&path).unwrap_or_default(),
        ),
        UserEvent::ShowGeneratedContent {
            content,
            token_count,
        } => format!(
            "window.showGeneratedContent({}, {});",
            serde_json::to_string(&content).unwrap_or_default(),
            token_count
        ),
        UserEvent::ShowError(msg) => {
            format!(
                "window.showError({});",
                serde_json::to_string(&msg).unwrap_or_default()
            )
        }
        UserEvent::SaveComplete(success, path) => format!(
            "window.fileSaveStatus({}, {});",
            success,
            serde_json::to_string(&path).unwrap_or_default()
        ),
        UserEvent::ConfigExported(success) => format!(
            "window.showStatus('{}');",
            if success {
                "Config exported successfully."
            } else {
                "Failed to export config."
            }
        ),
        UserEvent::ScanProgress(progress) => {
            format!(
                "window.updateScanProgress({});",
                serde_json::to_string(&progress).unwrap_or_default()
            )
        }
        UserEvent::DragStateChanged(is_dragging) => {
            format!("window.setDragState({is_dragging});")
        }
    };
    if let Err(e) = webview.evaluate_script(&script) {
        tracing::error!("Failed to evaluate script: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::path::PathBuf;
    use std::sync::mpsc as std_mpsc;
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
        state: Arc<Mutex<AppState>>,
    }

    impl Harness {
        fn new() -> Self {
            let (opened, dialog_opened) = std_mpsc::channel();
            let (close_dialog, close) = std_mpsc::channel();
            let (tx, events) = mpsc::unbounded_channel();
            let mut state = AppState::default();
            state.config = AppConfig::default();
            state.current_path = "/repo".to_string();
            Self {
                dispatcher: CommandDispatcher::new(),
                dialog: Arc::new(BlockingDialog {
                    opened: Mutex::new(opened),
                    close: Mutex::new(close),
                }),
                dialog_opened,
                close_dialog,
                proxy: TestEventProxy(tx),
                events,
                state: Arc::new(Mutex::new(state)),
            }
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
                .unwrap()
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
        let mut harness = Harness::new();
        {
            let mut state = harness.state.lock().unwrap();
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
        assert_eq!(
            harness.state.lock().unwrap().config.window_size,
            (640.0, 480.0)
        );

        harness.send("clearIgnorePatterns", serde_json::Value::Null);
        harness.events(1).await;
        assert!(harness
            .state
            .lock()
            .unwrap()
            .config
            .ignore_patterns
            .is_empty());
    }

    /// The former whole-config command is gone; a stale UI cannot replace the config.
    #[tokio::test]
    async fn update_config_is_not_a_command_anymore() {
        let mut harness = Harness::new();
        let before = harness.state.lock().unwrap().config.clone();
        let mut replaced = before.clone();
        replaced.ignore_patterns.clear();

        harness.send("updateConfig", serde_json::to_value(replaced).unwrap());
        harness.send("initialize", serde_json::Value::Null);
        harness.events(1).await;

        assert_eq!(harness.state.lock().unwrap().config, before);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn commands_take_effect_in_the_order_they_were_sent() {
        let mut harness = Harness::new();

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
        let mut harness = Harness::new();

        for _ in 0..101 {
            harness.send("toggleSelection", serde_json::json!("/repo/src/main.rs"));
        }
        harness.events(101).await;

        let state = harness.state.lock().unwrap();
        assert!(state
            .selected_files
            .contains(&PathBuf::from("/repo/src/main.rs")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_open_dialog_does_not_hold_up_other_commands() {
        let mut harness = Harness::new();

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
        let mut harness = Harness::new();

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
        let mut harness = Harness::new();

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
}
