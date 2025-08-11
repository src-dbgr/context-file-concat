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
            // Create a lookup map from the original list to get `is_directory`
            // information without hitting the filesystem. This fixes a latent bug.
            let path_info: std::collections::HashMap<_, _> = self
                .full_file_list
                .iter()
                .map(|item| (item.path.clone(), item.is_directory))
                .collect();

            // Filter full_file_list using the correct matching method.
            self.full_file_list.retain(|item| {
                !matcher
                    .matched_path_or_any_parents(&item.path, item.is_directory)
                    .is_ignore()
            });

            // Filter selected_files using the same robust logic.
            self.selected_files.retain(|path| {
                // Get `is_dir` from our map, defaulting to `false` (file).
                let is_dir = path_info.get(path).copied().unwrap_or(false);
                !matcher
                    .matched_path_or_any_parents(path, is_dir)
                    .is_ignore()
            });

            // Mark patterns as active
            self.active_ignore_patterns.extend(patterns_to_add.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::FileItem;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// Creates a dummy tokio JoinHandle for testing purposes.
    fn create_dummy_task() -> JoinHandle<()> {
        tokio::spawn(async {
            // The task can sleep for a bit to simulate work,
            // though it's not strictly necessary as we abort it immediately.
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
    }

    /// Creates a simple FileItem for testing.
    fn create_test_file_item(path_str: &str, is_dir: bool) -> FileItem {
        FileItem {
            path: PathBuf::from(path_str),
            is_directory: is_dir,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_cancel_scan_with_active_task() {
        // Arrange
        let mut state = AppState::default();
        state.is_scanning = true;
        state.scan_task = Some(create_dummy_task());
        state.scan_progress.current_scanning_path = "In progress...".to_string();
        state.scan_cancellation_flag.store(false, Ordering::SeqCst);

        // Act
        state.cancel_current_scan();
        // Give tokio a moment to process the abort
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Assert
        assert!(
            state.scan_task.is_none(),
            "Scan task should be taken from the state"
        );
        assert!(!state.is_scanning, "is_scanning should be false");
        assert_eq!(
            state.scan_progress.current_scanning_path, "Scan cancelled.",
            "Progress message should be updated"
        );
        assert!(
            state.scan_cancellation_flag.load(Ordering::SeqCst),
            "Cancellation flag should be set to true"
        );
    }

    #[tokio::test]
    async fn test_cancel_scan_with_no_task_does_not_panic() {
        // Arrange
        let mut state = AppState::default();
        state.is_scanning = false; // precondition
        state.scan_task = None; // precondition

        // Act & Assert
        // The test passes if this call does not panic.
        state.cancel_current_scan();
        assert!(!state.is_scanning);
    }

    #[tokio::test]
    async fn test_cancel_generation_with_active_task() {
        // Arrange
        let mut state = AppState::default();
        state.is_generating = true;
        state.generation_task = Some(create_dummy_task());
        state
            .generation_cancellation_flag
            .store(false, Ordering::SeqCst);

        // Act
        state.cancel_current_generation();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Assert
        assert!(
            state.generation_task.is_none(),
            "Generation task should be cleared"
        );
        assert!(!state.is_generating, "is_generating should be false");
        assert!(
            state.generation_cancellation_flag.load(Ordering::SeqCst),
            "Generation cancellation flag should be set"
        );
    }

    #[tokio::test]
    async fn test_cancel_generation_with_no_task_does_not_panic() {
        // Arrange
        let mut state = AppState::default();
        state.is_generating = false;
        state.generation_task = None;

        // Act & Assert
        // The test passes if this call does not panic.
        state.cancel_current_generation();
        assert!(!state.is_generating);
    }

    #[tokio::test]
    async fn test_reset_directory_state_clears_all_relevant_fields() {
        // Arrange
        let mut state = AppState::default();
        state.current_path = "/test/project".to_string();
        state.full_file_list = vec![create_test_file_item("/test/project/file.txt", false)];
        state.filtered_file_list = state.full_file_list.clone();
        state.selected_files = HashSet::from([PathBuf::from("/test/project/file.txt")]);
        state.expanded_dirs = HashSet::from([PathBuf::from("/test/project")]);
        state.loaded_dirs = HashSet::from([PathBuf::from("/test/project")]);
        state.search_query = "test".to_string();
        state.is_fully_scanned = true;
        state.scan_task = Some(create_dummy_task());
        state.generation_task = Some(create_dummy_task());
        state.is_scanning = true;
        state.is_generating = true;

        // Act
        state.reset_directory_state();

        // Assert
        assert!(state.current_path.is_empty());
        assert!(state.full_file_list.is_empty());
        assert!(state.filtered_file_list.is_empty());
        assert!(state.selected_files.is_empty());
        assert!(state.expanded_dirs.is_empty());
        assert!(state.loaded_dirs.is_empty());
        assert!(state.search_query.is_empty());
        assert!(!state.is_fully_scanned);
        assert!(state.previewed_file_path.is_none());
        assert!(!state.is_generating);
        assert_eq!(state.scan_progress.current_scanning_path, "Ready.");

        // Assert that the tasks were cancelled and removed
        assert!(state.scan_task.is_none());
        assert!(state.generation_task.is_none());
    }

    #[tokio::test]
    async fn test_apply_ignore_patterns_removes_files_and_selections() {
        // Arrange
        let mut state = AppState::default();
        state.current_path = "/project".to_string();

        let file_to_keep = create_test_file_item("/project/src/main.rs", false);
        let dir_to_ignore = create_test_file_item("/project/node_modules", true);
        let file_to_ignore = create_test_file_item("/project/node_modules/dep.js", false);

        state.full_file_list = vec![
            file_to_keep.clone(),
            dir_to_ignore.clone(),
            file_to_ignore.clone(),
        ];
        state.selected_files =
            HashSet::from([file_to_keep.path.clone(), file_to_ignore.path.clone()]);

        let patterns = HashSet::from(["node_modules/".to_string()]);

        // Act
        state.apply_ignore_patterns(&patterns);

        // Assert - Check full file list
        assert_eq!(
            state.full_file_list.len(),
            1,
            "Only one item should remain in the full list"
        );
        assert_eq!(
            state.full_file_list[0].path, file_to_keep.path,
            "The remaining file should be main.rs"
        );

        // Assert - Check selected files
        assert_eq!(
            state.selected_files.len(),
            1,
            "Only one item should remain selected"
        );
        assert!(
            state.selected_files.contains(&file_to_keep.path),
            "main.rs should still be selected"
        );
        assert!(
            !state.selected_files.contains(&file_to_ignore.path),
            "The ignored file should be deselected"
        );

        // Assert - Check active patterns
        assert!(state.active_ignore_patterns.contains("node_modules/"));
    }

    #[tokio::test]
    async fn test_apply_ignore_patterns_with_empty_set_does_nothing() {
        // Arrange
        let mut state = AppState::default();
        let initial_file_list = vec![create_test_file_item("/project/file.txt", false)];
        state.full_file_list = initial_file_list.clone();
        let patterns = HashSet::new();

        // Act
        state.apply_ignore_patterns(&patterns);

        // Assert
        assert_eq!(state.full_file_list.len(), initial_file_list.len());
    }
}
