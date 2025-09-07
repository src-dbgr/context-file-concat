//! The `app` module orchestrates the interaction between the `core` logic and the `ui`.
//!
//! It manages the application state, handles events from the WebView (IPC messages),
//! and sends updates back to the UI. It acts as the "controller" in an MVC-like pattern.

pub mod commands;
pub mod events;
pub mod file_dialog;
pub mod filtering;
mod helpers;
pub mod proxy;
pub mod state;
pub mod tasks;
pub mod view_model;

use crate::app::file_dialog::DialogService;
use std::sync::{Arc, Mutex};
use wry::WebView;

use crate::app::proxy::EventProxy;
use events::{IpcMessage, UserEvent};
use state::AppState;

/// The main handler for IPC messages from the WebView.
///
/// It parses the message and delegates to the appropriate command handler function
/// in the `commands` module. Each command is spawned as a separate Tokio task
/// to avoid blocking the main event loop.
///
/// # Asynchronous vs. Synchronous Commands
/// - **Asynchronous** commands must be awaited. These are functions that internally
///   `.await` another operation (e.g., waiting for search results or a config update).
/// - **Synchronous** commands, even those that launch a background task (`tokio::spawn`),
///   are not awaited because the command function itself completes immediately.
pub fn handle_ipc_message<P: EventProxy>(
    message: String,
    dialog_service: Arc<impl DialogService + 'static>,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(msg) = serde_json::from_str::<IpcMessage>(&message) {
        // Clone resources to be moved into the async task.
        let proxy = proxy.clone();
        let dialog = dialog_service.clone();

        tokio::spawn(async move {
            match msg.command.as_str() {
                // --- Asynchronous Commands (must be awaited) ---
                "updateConfig" => commands::update_config(msg.payload, proxy, state).await,
                "updateFilters" => commands::update_filters(msg.payload, proxy, state).await,
                "addIgnorePath" => commands::add_ignore_path(msg.payload, proxy, state).await,
                "importConfig" => commands::import_config(dialog.as_ref(), proxy, state).await,

                // --- Synchronous Commands & Task Launchers (do not await) ---
                "selectDirectory" => commands::select_directory(dialog.as_ref(), proxy, state),
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
                "expandCollapseAll" => commands::expand_collapse_all(msg.payload, proxy, state),
                "selectAll" => commands::select_all(proxy, state),
                "deselectAll" => commands::deselect_all(proxy, state),
                "expandAllFully" => commands::expand_all_fully(proxy, state),
                "selectAllFully" => commands::select_all_fully(proxy, state),
                "cancelGeneration" => commands::cancel_generation(proxy, state),
                "clearPreviewState" => commands::clear_preview_state(proxy, state),
                "saveFile" => commands::save_file(dialog.as_ref(), msg.payload, proxy, state),
                "pickOutputDirectory" => {
                    commands::pick_output_directory(dialog.as_ref(), proxy, state)
                }
                "exportConfig" => commands::export_config(dialog.as_ref(), proxy, state),

                // --- Legacy Command Names ---
                "expand_all_fully" => commands::expand_all_fully(proxy, state),
                "select_all_fully" => commands::select_all_fully(proxy, state),

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
