//! Defines the events the backend sends to the frontend. (What the frontend sends is
//! defined in [`super::ipc`].)

use serde::Serialize;
use std::path::PathBuf;

use super::snapshot::Snapshot;
use super::state::GeneratedDocument;
use crate::core::ScanProgress;

/// Events sent from the Rust backend to the WebView (UI thread).
///
/// Each variant corresponds to a specific JavaScript function (`window.*`) that will be called in the frontend.
#[derive(Debug)]
pub enum UserEvent {
    /// A complete state update to re-render the UI. It is made and sent in one step
    /// (see [`super::snapshot::send_snapshot`]).
    StateUpdate(Snapshot),
    /// Content for the file preview panel.
    ShowFilePreview {
        content: String,
        language: String,
        search_term: Option<String>,
        path: PathBuf,
    },
    /// The generated, concatenated content for the main preview. The host keeps the
    /// document; the UI names its id when it asks for it to be saved.
    ShowGeneratedContent {
        document: GeneratedDocument,
        token_count: usize,
    },
    /// What the size of the output means for a generation that was asked for.
    OutputSize(OutputSizeNotice),
    /// The ignore button of a row has added no pattern, and why.
    IgnorePathRefused(IgnorePathRefusal),
    /// An error message to be displayed to the user.
    ShowError(String),
    /// The result of a file save operation.
    SaveResult(SaveResult),
    /// The result of a configuration export.
    ConfigExported(bool),
    /// A progress update during a directory scan.
    ScanProgress(ScanProgress),
    /// Indicates that a file is being dragged over the window.
    DragStateChanged(bool),
}

/// How a save of the output ended. Every `saveFile` command that names something to
/// save is answered with exactly one of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SaveResult {
    /// The file was written to `path` (written lossily if it is not valid Unicode,
    /// like every path the UI is sent).
    Saved { path: String },
    /// The user dismissed the save dialog. Nothing was written.
    Cancelled,
    /// Nothing was saved, for the reason `error` gives: the message of the operating
    /// system, or the host's own if it had nothing to write.
    Failed { error: String },
}

/// What the size of the output means for a generation that was asked for. The sizes
/// are bytes; `estimated_bytes` is told from the file sizes of the scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OutputSizeNotice {
    /// The generation runs, but its output is larger than `limit_bytes`: showing it
    /// will take a moment.
    Large {
        estimated_bytes: u64,
        limit_bytes: u64,
    },
    /// The generation was refused: its output would be larger than `limit_bytes`.
    TooLarge {
        estimated_bytes: u64,
        limit_bytes: u64,
    },
}

/// Why the ignore button of a tree row (`addIgnorePath`) has added no pattern: the
/// entry has a name that no pattern of the application can name. `name` is that
/// name, written lossily if it is not valid Unicode. The UI words the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum IgnorePathRefusal {
    /// The name of the file ends with whitespace outside ASCII (a no-break space,
    /// an ideographic space), which the matcher drops from the end of a pattern.
    TrailingWhitespace { name: String },
    /// A name on the way to the entry is not valid Unicode; a pattern is text.
    NotUnicode { name: String },
}
