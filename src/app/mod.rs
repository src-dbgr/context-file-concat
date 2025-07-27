//! The `app` module orchestrates the interaction between the `core` logic and the `ui`.
//!
//! It manages the application state, handles events from the WebView (IPC messages),
//! and sends updates back to the UI. It acts as the "controller" in an MVC-like pattern.

pub mod commands;
pub mod events;
pub mod state;
pub mod tasks;
pub mod view_model;

use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;
use wry::WebView;

use events::{IpcMessage, UserEvent};
use state::AppState;

/// The main handler for IPC messages from the WebView.
///
/// It parses the message and delegates to the appropriate command handler function
/// in the `commands` module. Each command is spawned as a separate Tokio task.
pub fn handle_ipc_message(
    message: String,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(msg) = serde_json::from_str::<IpcMessage>(&message) {
        tokio::spawn(async move {
            match msg.command.as_str() {
                "selectDirectory" => commands::select_directory(proxy, state),
                "clearDirectory" => commands::clear_directory(proxy, state),
                "rescanDirectory" => commands::rescan_directory(proxy, state),
                "cancelScan" => commands::cancel_scan(proxy, state),
                "updateConfig" => commands::update_config(msg.payload, proxy, state),
                "initialize" => commands::initialize(proxy, state),
                "updateFilters" => commands::update_filters(msg.payload, proxy, state).await,
                "loadFilePreview" => commands::load_file_preview(msg.payload, proxy, state),
                "addIgnorePath" => commands::add_ignore_path(msg.payload, proxy, state),
                "toggleSelection" => commands::toggle_selection(msg.payload, proxy, state),
                "toggleDirectorySelection" => {
                    commands::toggle_directory_selection(msg.payload, proxy, state)
                }
                "toggleExpansion" => commands::toggle_expansion(msg.payload, proxy, state),
                "expandCollapseAll" => commands::expand_collapse_all(msg.payload, proxy, state),
                "selectAll" => commands::select_all(proxy, state),
                "deselectAll" => commands::deselect_all(proxy, state),
                "generatePreview" => commands::generate_preview(proxy, state).await,
                "clearPreviewState" => commands::clear_preview_state(proxy, state),
                "saveFile" => commands::save_file(msg.payload, proxy, state),
                "pickOutputDirectory" => commands::pick_output_directory(proxy, state),
                "importConfig" => commands::import_config(proxy, state),
                "exportConfig" => commands::export_config(proxy, state),
                _ => tracing::warn!("Unknown IPC command: {}", msg.command),
            }
        });
    } else {
        tracing::error!("Failed to parse IPC message: {}", message);
    }
}

/// Processes events sent from the backend to the UI thread.
///
/// It translates each `UserEvent` into a JavaScript call in the WebView to update the UI.
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
            serde_json::to_string(&path).unwrap_or_default()
        ),
        UserEvent::ShowGeneratedContent(content) => format!(
            "window.showGeneratedContent({});",
            serde_json::to_string(&content).unwrap_or_default()
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
