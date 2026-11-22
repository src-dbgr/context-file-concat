//! Defines the central, mutable state of the application.

use crate::config::AppConfig;
use crate::core::{FileItem, ScanProgress};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Identifies one run of a full directory scan.
///
/// A token is handed out by [`AppState::begin_scan`] and must accompany every state
/// mutation performed on behalf of that scan. Once the scan has finished, was cancelled
/// or was superseded by a newer scan, the token is stale and `AppState` ignores it, so a
/// task that is still winding down can never corrupt the state of its successor.
#[derive(Debug, Clone)]
pub struct ScanToken {
    id: u64,
    cancel_flag: Arc<AtomicBool>,
}

impl ScanToken {
    /// The cancellation flag owned by this scan, to be handed to the scanner.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }

    /// `true` once this scan has been cancelled or superseded by a newer scan.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

/// Ties a lazy (single directory level) load to the file list it was started for.
///
/// Handed out by [`AppState::begin_lazy_load`]. The token goes stale as soon as the file
/// list is replaced wholesale (new scan, directory reset), in which case the results of
/// the lazy load must be discarded instead of being merged into an unrelated list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LazyLoadToken {
    file_list_generation: u64,
}

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
    /// Indicates whether patterns were removed and a re-scan is recommended.
    /// This flag is set when ignore patterns are removed, as the current file list
    /// might be missing files that were previously filtered out.
    pub patterns_need_rescan: bool,
    /// Incremented whenever the file list is replaced wholesale (a scan begins or the
    /// directory is reset). Lets background tasks detect that their results are outdated.
    file_list_generation: u64,
    /// The id of the scan that currently owns the scanning state, if any.
    active_scan_id: Option<u64>,
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
            patterns_need_rescan: false,
            file_list_generation: 0,
            active_scan_id: None,
        }
    }
}

impl AppState {
    /// Starts a new scan run and returns the token identifying it.
    ///
    /// Any scan that is still active is superseded: its task is aborted, its cancellation
    /// flag is raised and its token becomes stale. The returned token is the only key
    /// that allows a scan task to modify the scanning state from now on.
    pub fn begin_scan(&mut self) -> ScanToken {
        self.abort_scan_task();
        self.scan_cancellation_flag.store(true, Ordering::SeqCst);

        self.file_list_generation += 1;
        let token = ScanToken {
            id: self.file_list_generation,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        };
        self.scan_cancellation_flag = token.cancel_flag();
        self.active_scan_id = Some(token.id);
        self.is_scanning = true;
        token
    }

    /// `true` if `token` belongs to the scan that currently owns the scanning state.
    pub fn is_current_scan(&self, token: &ScanToken) -> bool {
        self.active_scan_id == Some(token.id)
    }

    /// Stores the handle of the task that runs the scan identified by `token`.
    ///
    /// If the scan is no longer current (it already finished, was cancelled or was
    /// superseded before the handle could be registered), the task is aborted instead,
    /// so a stale handle never replaces the one of a newer scan.
    pub fn attach_scan_task(&mut self, token: &ScanToken, handle: JoinHandle<()>) {
        if self.is_current_scan(token) {
            self.scan_task = Some(handle);
        } else {
            handle.abort();
        }
    }

    /// Marks the scan identified by `token` as finished and releases the scanning state.
    ///
    /// Does nothing for a stale token. Returns `true` if this call ended a scan that was
    /// still flagged as running, i.e. if the UI needs to be told about the change.
    pub fn finish_scan(&mut self, token: &ScanToken) -> bool {
        if !self.is_current_scan(token) {
            return false;
        }
        self.active_scan_id = None;
        self.scan_task = None;
        std::mem::replace(&mut self.is_scanning, false)
    }

    /// Returns a token for a lazy load of a single directory level, or `None` if a full
    /// scan is in progress (which will load that level anyway).
    pub fn begin_lazy_load(&self) -> Option<LazyLoadToken> {
        if self.is_scanning {
            return None;
        }
        Some(LazyLoadToken {
            file_list_generation: self.file_list_generation,
        })
    }

    /// `true` if the file list the lazy load was started for is still the current one.
    pub fn is_current_lazy_load(&self, token: &LazyLoadToken) -> bool {
        self.file_list_generation == token.file_list_generation
    }

    /// Cancels the current scan, if any, and resets the scanning state.
    ///
    /// A scan counts as active from `begin_scan` on, even before its task handle has
    /// been attached, so a cancellation issued in that window is not lost.
    pub fn cancel_current_scan(&mut self) {
        tracing::info!("LOG: AppState::cancel_current_scan called.");
        let had_task = self.abort_scan_task();
        if !had_task && self.active_scan_id.is_none() {
            tracing::warn!("LOG: cancel_current_scan called, but no active scan found.");
            return;
        }

        self.scan_cancellation_flag.store(true, Ordering::SeqCst);
        self.active_scan_id = None;
        self.is_scanning = false;
        self.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Scan cancelled.".to_string(),
        };
        tracing::info!("LOG: AppState has been reset to 'cancelled' state.");
    }

    /// Aborts the registered scan task, if any. Returns `true` if there was one.
    fn abort_scan_task(&mut self) -> bool {
        match self.scan_task.take() {
            Some(handle) => {
                handle.abort();
                true
            }
            None => false,
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
        // The file list is about to be thrown away: outdate pending lazy loads.
        self.file_list_generation += 1;

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
        self.is_fully_scanned = false;
        self.patterns_need_rescan = false;
        self.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Ready.".to_string(),
        };
    }

    /// Applies the complete set of current ignore patterns to the in-memory file lists.
    /// This function re-builds the matcher from `self.config.ignore_patterns`
    /// and filters `full_file_list` and `selected_files` accordingly.
    pub fn apply_ignore_patterns(&mut self) {
        // CHANGED: Use the complete, current set of patterns from the config.
        let patterns = &self.config.ignore_patterns;
        if patterns.is_empty() {
            return;
        }

        let root_path = PathBuf::from(&self.current_path);
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&root_path);

        // CHANGED: Iterate over all configured patterns.
        for pattern in patterns {
            builder.add_line(None, pattern).ok();
        }

        if let Ok(matcher) = builder.build() {
            let path_info: std::collections::HashMap<_, _> = self
                .full_file_list
                .iter()
                .map(|item| (item.path.clone(), item.is_directory))
                .collect();

            self.full_file_list.retain(|item| {
                !matcher
                    .matched_path_or_any_parents(&item.path, item.is_directory)
                    .is_ignore()
            });

            self.selected_files.retain(|path| {
                let is_dir = path_info.get(path).copied().unwrap_or(false);
                !matcher
                    .matched_path_or_any_parents(path, is_dir)
                    .is_ignore()
            });

            // Note: active_ignore_patterns are typically recalculated during a full scan
            // or could be updated here if needed, but for local filtering this is sufficient.
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
    async fn test_begin_scan_marks_state_as_scanning_with_fresh_flag() {
        let mut state = AppState::default();

        let token = state.begin_scan();

        assert!(state.is_scanning);
        assert!(state.is_current_scan(&token));
        assert!(!token.is_cancelled());
        assert!(
            Arc::ptr_eq(&state.scan_cancellation_flag, &token.cancel_flag()),
            "State must expose the cancellation flag of the current scan"
        );
    }

    #[tokio::test]
    async fn test_begin_scan_supersedes_previous_scan() {
        let mut state = AppState::default();
        let old_token = state.begin_scan();
        let old_handle = create_dummy_task();
        let old_abort = old_handle.abort_handle();
        state.attach_scan_task(&old_token, old_handle);

        let new_token = state.begin_scan();
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(old_token.is_cancelled(), "Old scan must be told to stop");
        assert!(old_abort.is_finished(), "Old scan task must be aborted");
        assert!(!state.is_current_scan(&old_token));
        assert!(state.is_current_scan(&new_token));
        assert!(!new_token.is_cancelled());
        assert!(state.is_scanning);
    }

    #[tokio::test]
    async fn test_finish_scan_with_stale_token_leaves_new_scan_untouched() {
        let mut state = AppState::default();
        let old_token = state.begin_scan();
        let new_token = state.begin_scan();
        state.attach_scan_task(&new_token, create_dummy_task());

        let changed = state.finish_scan(&old_token);

        assert!(!changed);
        assert!(state.is_scanning, "New scan must still be running");
        assert!(state.scan_task.is_some(), "New scan must stay cancellable");
        assert!(state.is_current_scan(&new_token));
        state.cancel_current_scan();
    }

    #[tokio::test]
    async fn test_finish_scan_with_current_token_releases_scanning_state() {
        let mut state = AppState::default();
        let token = state.begin_scan();
        state.attach_scan_task(&token, create_dummy_task());

        assert!(state.finish_scan(&token));

        assert!(!state.is_scanning);
        assert!(state.scan_task.is_none());
        assert!(!state.is_current_scan(&token));
        assert!(!state.finish_scan(&token), "Finishing twice is a no-op");
    }

    #[tokio::test]
    async fn test_attach_scan_task_with_stale_token_aborts_instead_of_storing() {
        let mut state = AppState::default();
        let old_token = state.begin_scan();
        let new_token = state.begin_scan();
        let new_handle = create_dummy_task();
        let new_abort = new_handle.abort_handle();
        state.attach_scan_task(&new_token, new_handle);

        let stale_handle = create_dummy_task();
        let stale_abort = stale_handle.abort_handle();
        state.attach_scan_task(&old_token, stale_handle);
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(stale_abort.is_finished(), "Stale task must be aborted");
        assert!(!new_abort.is_finished(), "Current task must keep running");
        assert!(state.scan_task.is_some());
        state.cancel_current_scan();
    }

    #[tokio::test]
    async fn test_cancel_scan_before_task_is_attached_is_not_lost() {
        let mut state = AppState::default();
        let token = state.begin_scan();

        state.cancel_current_scan();

        assert!(!state.is_scanning);
        assert!(token.is_cancelled());
        assert!(!state.is_current_scan(&token));
        assert_eq!(state.scan_progress.current_scanning_path, "Scan cancelled.");

        // The task handle arriving late must not resurrect the cancelled scan.
        let late_handle = create_dummy_task();
        let late_abort = late_handle.abort_handle();
        state.attach_scan_task(&token, late_handle);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(state.scan_task.is_none());
        assert!(late_abort.is_finished());
    }

    #[tokio::test]
    async fn test_begin_lazy_load_is_refused_while_scanning() {
        let mut state = AppState::default();
        assert!(state.begin_lazy_load().is_some());

        let _token = state.begin_scan();

        assert!(state.begin_lazy_load().is_none());
    }

    #[tokio::test]
    async fn test_lazy_load_token_goes_stale_when_file_list_is_replaced() {
        let mut state = AppState::default();

        let token = state.begin_lazy_load().expect("not scanning");
        assert!(state.is_current_lazy_load(&token));
        let scan = state.begin_scan();
        assert!(!state.is_current_lazy_load(&token), "stale after new scan");
        state.finish_scan(&scan);

        let token = state.begin_lazy_load().expect("not scanning");
        state.reset_directory_state();
        assert!(!state.is_current_lazy_load(&token), "stale after reset");
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
        state.patterns_need_rescan = true;

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
        assert!(!state.patterns_need_rescan);
    }

    #[tokio::test]
    async fn test_apply_ignore_patterns_removes_files_and_selections() {
        // Arrange
        let mut state = AppState::default();
        // NOTE: Ensure a clean state, independent of any config file on disk.
        state.config.ignore_patterns.clear();
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

        // Set the patterns for this specific test case.
        state.config.ignore_patterns = HashSet::from(["node_modules/".to_string()]);

        // Act
        state.apply_ignore_patterns();

        // Assert
        assert_eq!(
            state.full_file_list.len(),
            1,
            "Only one item should remain in the full list"
        );
        assert_eq!(
            state.full_file_list[0].path, file_to_keep.path,
            "The remaining file should be main.rs"
        );
        assert_eq!(
            state.selected_files.len(),
            1,
            "Only one item should remain selected"
        );
        assert!(
            state.selected_files.contains(&file_to_keep.path),
            "main.rs should still be selected"
        );
    }

    #[tokio::test]
    async fn test_apply_ignore_patterns_with_empty_set_does_nothing() {
        // Arrange
        let mut state = AppState::default();
        // NOTE: Ensure a clean state, independent of any config file on disk.
        state.config.ignore_patterns.clear();

        let initial_file_list = vec![create_test_file_item("/project/file.txt", false)];
        state.full_file_list = initial_file_list.clone();

        // Act
        state.apply_ignore_patterns();

        // Assert
        assert_eq!(state.full_file_list.len(), initial_file_list.len());
        assert_eq!(state.full_file_list[0].path, initial_file_list[0].path);
    }
}
