//! The `app` module orchestrates the interaction between the `core` logic and the `ui`.
//!
//! It manages the application state, handles events from the WebView (IPC messages),
//! and sends updates back to the UI. It acts as the "controller" in an MVC-like pattern.

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

    match DialogCommand::from_name(&msg.command) {
        Some(command) => dispatcher.dispatch_blocking(move || {
            run_dialog_command(command, msg.payload, dialog_service.as_ref(), proxy, state);
        }),
        None => dispatcher.dispatch(run_command(msg, proxy, state)),
    }
}

/// The commands that wait for the user to answer a native dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogCommand {
    SelectDirectory,
    SaveFile,
    PickOutputDirectory,
    ImportConfig,
    ExportConfig,
}

impl DialogCommand {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "selectDirectory" => Some(Self::SelectDirectory),
            "saveFile" => Some(Self::SaveFile),
            "pickOutputDirectory" => Some(Self::PickOutputDirectory),
            "importConfig" => Some(Self::ImportConfig),
            "exportConfig" => Some(Self::ExportConfig),
            _ => None,
        }
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
/// task (scan, generation) and return. `updateFilters` is the one command that can
/// suspend, while it waits for a content search; the dispatcher then completes it on
/// its continuation lane.
async fn run_command<P: EventProxy>(msg: IpcMessage, proxy: P, state: Arc<Mutex<AppState>>) {
    match msg.command.as_str() {
        // --- Asynchronous commands (must be awaited) ---
        "updateConfig" => commands::update_config(msg.payload, proxy, state).await,
        "updateFilters" => commands::update_filters(msg.payload, proxy, state).await,
        "addIgnorePath" => commands::add_ignore_path(msg.payload, proxy, state).await,

        // --- Synchronous commands & task launchers ---
        "rescanDirectory" => commands::rescan_directory(proxy, state),
        "loadDirectoryLevel" => commands::load_directory_level(msg.payload, proxy, state),
        "generatePreview" => commands::generate_preview(proxy, state),
        "clearDirectory" => commands::clear_directory(proxy, state),
        "cancelScan" => commands::cancel_scan(proxy, state),
        "initialize" => commands::initialize(proxy, state),
        "loadFilePreview" => commands::load_file_preview(msg.payload, proxy, state),
        "toggleSelection" => commands::toggle_selection(msg.payload, proxy, state),
        "toggleDirectorySelection" => {
            commands::toggle_directory_selection(msg.payload, proxy, state)
        }
        "toggleExpansion" => commands::toggle_expansion(msg.payload, proxy, state),
        "setExpansion" => commands::set_expansion(msg.payload, proxy, state),
        "expandCollapseAll" => commands::expand_collapse_all(msg.payload, proxy, state),
        "selectAll" => commands::select_all(proxy, state),
        "deselectAll" => commands::deselect_all(proxy, state),
        "cancelGeneration" => commands::cancel_generation(proxy, state),
        "clearPreviewState" => commands::clear_preview_state(proxy, state),

        _ => tracing::warn!("Unknown IPC command: {}", msg.command),
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
            assert!(DialogCommand::from_name(name).is_some(), "{name}");
        }
        for name in ["setExpansion", "updateFilters", "generatePreview", "nope"] {
            assert_eq!(DialogCommand::from_name(name), None, "{name}");
        }
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn an_odd_number_of_toggles_toggles_once() {
        let mut harness = Harness::new();

        for _ in 0..101 {
            harness.send("toggleExpansion", serde_json::json!("/repo/src"));
        }
        harness.events(101).await;

        assert!(harness.is_expanded("/repo/src"));
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
