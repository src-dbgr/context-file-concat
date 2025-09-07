//! An abstraction layer for native file dialogs to enable testing.

use crate::config::AppConfig;
use std::path::PathBuf;

/// Defines a common interface for file and folder selection dialogs.
/// This allows for a mock implementation during tests, avoiding the need
/// to interact with actual OS dialog windows.
pub trait DialogService: Send + Sync {
    /// Opens a dialog to select a single directory.
    fn pick_directory(&self) -> Option<PathBuf>;

    /// Opens a dialog to select a single file for config import.
    fn pick_config_to_import(&self) -> Option<PathBuf>;

    /// Opens a dialog to select a save location for a config export.
    fn export_config_path(&self) -> Option<PathBuf>;

    /// Opens a dialog to select a save location for the final output file.
    /// It uses the provided config to suggest a default name and directory.
    fn save_output_file_path(&self, config: &AppConfig) -> Option<PathBuf>;
}

/// The production implementation that uses the `rfd` crate to show native OS dialogs.
pub struct NativeDialogService;

impl DialogService for NativeDialogService {
    fn pick_directory(&self) -> Option<PathBuf> {
        rfd::FileDialog::new().pick_folder()
    }

    fn pick_config_to_import(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
    }

    fn export_config_path(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name("cfc-config.json")
            .save_file()
    }

    fn save_output_file_path(&self, config: &AppConfig) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Text File", &["txt"])
            .set_file_name(&config.output_filename);
        if let Some(dir) = &config.output_directory {
            dialog = dialog.set_directory(dir);
        }
        dialog.save_file()
    }
}
