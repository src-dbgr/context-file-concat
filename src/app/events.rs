//! Defines the event and message structures for communication between the backend and frontend.

use serde::Deserialize;
use std::path::PathBuf;

use super::view_model::UiState;
use crate::core::ScanProgress;

/// Events sent from the Rust backend to the WebView (UI thread).
///
/// Each variant corresponds to a specific JavaScript function (`window.*`) that will be called in the frontend.
#[derive(Debug)]
pub enum UserEvent {
    /// A complete state update to re-render the UI.
    StateUpdate(Box<UiState>),
    /// Content for the file preview panel.
    ShowFilePreview {
        content: String,
        language: String,
        search_term: Option<String>,
        path: PathBuf,
    },
    /// The generated, concatenated content for the main preview.
    ShowGeneratedContent(String),
    /// An error message to be displayed to the user.
    ShowError(String),
    /// The result of a file save operation.
    SaveComplete(bool, String),
    /// The result of a configuration export.
    ConfigExported(bool),
    /// A progress update during a directory scan.
    ScanProgress(ScanProgress),
    /// Indicates that a file is being dragged over the window.
    DragStateChanged(bool),
}

/// A message received from the WebView via the IPC channel.
#[derive(Deserialize, Debug)]
pub struct IpcMessage {
    /// The name of the command to execute.
    pub command: String,
    /// The payload associated with the command, as a JSON value.
    pub payload: serde_json::Value,
}
