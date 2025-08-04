//! Defines the central, mutable state of the application.

use crate::config::AppConfig;
use crate::core::{FileItem, ScanProgress};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Holds the complete, mutable state of the application.
///
/// This struct is wrapped in an `Arc<Mutex<...>>` to allow for safe, shared access
/// from different threads (e.g., the main event loop, IPC handlers, and async tasks).
pub struct AppState {
    /// The application's configuration settings.
    pub config: AppConfig,
    /// The absolute path to the currently loaded directory.
    pub current_path: String,
    /// The complete, unfiltered list of all files and directories found in the scan.
    pub full_file_list: Vec<FileItem>,
    /// The list of files and directories visible in the UI after applying filters.
    pub filtered_file_list: Vec<FileItem>,
    /// The set of absolute paths to files that are currently selected by the user.
    pub selected_files: HashSet<PathBuf>,
    /// The set of absolute paths to directories that are expanded in the UI tree.
    pub expanded_dirs: HashSet<PathBuf>,
    /// The set of absolute paths to directories whose children have been loaded.
    pub loaded_dirs: HashSet<PathBuf>,
    /// `true` if a directory scan is currently in progress.
    pub is_scanning: bool,
    /// `true` if the concatenation process is currently running.
    pub is_generating: bool,
    /// The current search query for filenames.
    pub search_query: String,
    /// The current filter for file extensions.
    pub extension_filter: String,
    /// The current search query for file content.
    pub content_search_query: String,
    /// The set of paths that match the current content search query.
    pub content_search_results: HashSet<PathBuf>,
    /// The filename of the currently loaded configuration file, if any.
    pub current_config_filename: Option<String>,
    /// The current progress of the directory scan.
    pub scan_progress: ScanProgress,
    /// The path of the file currently being previewed in the editor.
    pub previewed_file_path: Option<PathBuf>,
    /// A handle to the currently running scan task, allowing it to be aborted.
    pub scan_task: Option<JoinHandle<()>>,
    /// A flag used to signal cancellation to the scan task.
    pub scan_cancellation_flag: Arc<AtomicBool>,
    /// A handle to the currently running generation task, allowing it to be aborted.
    pub generation_task: Option<JoinHandle<()>>,
    /// A flag used to signal cancellation to the generation task.
    pub generation_cancellation_flag: Arc<AtomicBool>,
    /// The set of ignore patterns that were actually matched during the last scan.
    pub active_ignore_patterns: HashSet<String>,
    /// `true` if a full, non-lazy scan has been completed successfully.
    pub is_fully_scanned: bool,
}

impl Default for AppState {
    /// Creates a default `AppState` instance, loading the configuration from disk.
    fn default() -> Self {
        Self {
            config: AppConfig::load().unwrap_or_default(),
            current_path: String::new(),
            full_file_list: Vec::new(),
            filtered_file_list: Vec::new(),
            selected_files: HashSet::new(),
            expanded_dirs: HashSet::new(),
            loaded_dirs: HashSet::new(),
            is_scanning: false,
            is_generating: false,
            search_query: String::new(),
            extension_filter: String::new(),
            content_search_query: String::new(),
            content_search_results: HashSet::new(),
            current_config_filename: None,
            scan_progress: ScanProgress {
                files_scanned: 0,
                large_files_skipped: 0,
                current_scanning_path: "Ready.".to_string(),
            },
            previewed_file_path: None,
            scan_task: None,
            scan_cancellation_flag: Arc::new(AtomicBool::new(false)),
            generation_task: None,
            generation_cancellation_flag: Arc::new(AtomicBool::new(false)),
            active_ignore_patterns: HashSet::new(),
            is_fully_scanned: false,
        }
    }
}

impl AppState {
    /// Cancels the current scan task, if any, and resets the scanning state.
    pub fn cancel_current_scan(&mut self) {
        tracing::info!("LOG: AppState::cancel_current_scan called.");
        if let Some(handle) = self.scan_task.take() {
            tracing::info!("LOG: Active scan task found. Calling handle.abort()...");
            handle.abort();
            tracing::info!("LOG: handle.abort() was called.");

            tracing::info!("LOG: Setting cancellation flag (AtomicBool) to true.");
            self.scan_cancellation_flag.store(true, Ordering::SeqCst);

            self.is_scanning = false;
            self.scan_progress = ScanProgress {
                files_scanned: 0,
                large_files_skipped: 0,
                current_scanning_path: "Scan cancelled.".to_string(),
            };
            tracing::info!("LOG: AppState has been reset to 'cancelled' state.");
        } else {
            tracing::warn!("LOG: cancel_current_scan called, but no active scan task found.");
        }
    }

    /// Cancels the current generation task, if any.
    pub fn cancel_current_generation(&mut self) {
        if let Some(handle) = self.generation_task.take() {
            handle.abort();
        }
        self.generation_cancellation_flag
            .store(true, Ordering::SeqCst);
        self.is_generating = false;
    }

    /// Resets all state related to a loaded directory.
    pub fn reset_directory_state(&mut self) {
        self.cancel_current_scan();
        self.cancel_current_generation();

        self.current_path = String::new();
        self.full_file_list.clear();
        self.filtered_file_list.clear();
        self.selected_files.clear();
        self.expanded_dirs.clear();
        self.loaded_dirs.clear();
        self.search_query.clear();
        self.extension_filter.clear();
        self.content_search_query.clear();
        self.content_search_results.clear();
        self.previewed_file_path = None;
        self.active_ignore_patterns.clear();
        self.is_generating = false;
        self.is_fully_scanned = false; // Reset the flag here

        self.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Ready.".to_string(),
        };
    }

    /// Applies ignore patterns consistently across all file lists
    pub fn apply_ignore_patterns(&mut self, patterns_to_add: &HashSet<String>) {
        if patterns_to_add.is_empty() {
            return;
        }

        let root_path = PathBuf::from(&self.current_path);
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&root_path);
        for pattern in patterns_to_add {
            builder.add_line(None, pattern).ok();
        }

        if let Ok(matcher) = builder.build() {
            // Filter full_file_list
            self.full_file_list
                .retain(|item| !matcher.matched(&item.path, item.is_directory).is_ignore());

            // Filter selected_files
            self.selected_files.retain(|path| {
                let is_dir = path.is_dir();
                !matcher.matched(path, is_dir).is_ignore()
            });

            // Mark patterns as active
            self.active_ignore_patterns.extend(patterns_to_add.clone());
        }
    }
}
