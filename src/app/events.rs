use serde::Deserialize;
use std::path::PathBuf;

use super::view_model::UiState;
use crate::core::ScanProgress;

/// Events, die vom Rust-Backend an die WebView (UI-Thread) gesendet werden.
#[derive(Debug)]
pub enum UserEvent {
    /// Ein komplettes Zustandsupdate, um die UI neu zu rendern.
    StateUpdate(UiState),
    /// Inhalt für die Dateivorschau.
    ShowFilePreview {
        content: String,
        language: String,
        search_term: Option<String>,
        path: PathBuf,
    },
    /// Der generierte, zusammengefügte Inhalt für die Hauptvorschau.
    ShowGeneratedContent(String),
    /// Eine Fehlermeldung, die dem Benutzer angezeigt werden soll.
    ShowError(String),
    /// Das Ergebnis einer Dateispeicheroperation.
    SaveComplete(bool, String),
    /// Das Ergebnis eines Konfigurationsexports.
    ConfigExported(bool),
    /// Ein Fortschrittsupdate während eines Verzeichnisscans.
    ScanProgress(ScanProgress),
    /// Zeigt an, dass eine Datei über das Fenster gezogen wird.
    DragStateChanged(bool),
}

/// Eine Nachricht, die von der WebView über den IPC-Kanal empfangen wird.
#[derive(Deserialize, Debug)]
pub struct IpcMessage {
    pub command: String,
    pub payload: serde_json::Value,
}
