//! Contains all the command handlers that are callable from the frontend via IPC.
//!
//! Each function in this module corresponds to a specific `IpcMessage::command`.
//! These handlers are responsible for interacting with the `AppState` and the `core`
//! logic, and for sending `UserEvent`s back to the UI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::events::UserEvent;
use super::filtering; // SRP: Use the new filtering module
use super::helpers::with_state_and_notify;
use super::proxy::EventProxy;
use super::state::AppState;
use super::tasks::{generation_task, search_in_files, start_lazy_load_scan, start_scan_on_path};
use super::view_model::{auto_expand_for_matches, generate_ui_state, get_language_from_path};
use crate::config::{self, AppConfig}; // Import AppConfig for explicit deserialization
use crate::core::FileHandler;

/// Opens a file dialog for the user to select a directory to scan.
///
/// Triggers a new proactive, two-phase scan on the selected path.
/// This always performs a hard reset of the application state.
pub fn select_directory<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        // A new directory selection should always reset the state.
        start_scan_on_path(path, proxy, state, false);
    } else {
        tracing::info!("LOG: User cancelled directory selection.");
        // Manually notify on cancellation as no state mutation happens that would trigger the helper
        let mut state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        state_guard.is_scanning = false;
        let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
        proxy.send_event(event);
    }
}

/// Clears the currently loaded directory and resets the application state.
pub fn clear_directory<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        s.reset_directory_state();
        s.config.last_directory = None;
        if let Err(e) = config::settings::save_config(&s.config) {
            tracing::warn!("Failed to save config after clearing directory: {}", e);
        }
    });
}

/// Re-scans the currently loaded directory path.
///
/// This preserves the current UI state (selections, expansions) while refreshing
/// the file list from the filesystem.
pub fn rescan_directory<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    let current_path_str = {
        state
            .lock()
            .expect("Mutex was poisoned. This should not happen.")
            .current_path
            .clone()
    };
    if !current_path_str.is_empty() {
        // A manual rescan should preserve the current UI state (expansions, selections).
        start_scan_on_path(PathBuf::from(current_path_str), proxy, state, true);
    }
}

/// Cancels the ongoing directory scan.
pub fn cancel_scan<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        tracing::info!("LOG: IPC 'cancelScan' received.");
        s.cancel_current_scan();
    });
}

/// Updates the application configuration and persists it.
///
/// This function handles different types of configuration changes intelligently:
/// - If `ignore_patterns` are changed in any way (added or removed), a full re-scan of
///   the directory is triggered while preserving the UI state (selections, expansions).
///   This is the most robust way to ensure the file list is perfectly synchronized
///   with the current ignore rules, fixing potential inconsistencies.
/// - If only filter-related settings change (e.g., `remove_empty_directories`),
///   the existing file list is re-filtered in-place for a fast UI update.
/// - Other changes (e.g., output settings) are just saved without triggering any UI update.
pub fn update_config<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(new_config) = serde_json::from_value::<AppConfig>(payload.clone()) {
        let mut state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");

        // Check what kind of change we have.
        let patterns_changed = state_guard.config.ignore_patterns != new_config.ignore_patterns;
        let needs_refilter = state_guard.config.remove_empty_directories
            != new_config.remove_empty_directories
            || state_guard.config.case_sensitive_search != new_config.case_sensitive_search;

        // Always update the config in the state first.
        state_guard.config = new_config;
        if let Err(e) = config::settings::save_config(&state_guard.config) {
            tracing::warn!("Failed to save config on update: {}", e);
        }

        let current_path = state_guard.current_path.clone();
        if current_path.is_empty() {
            // No directory loaded, nothing more to do.
            drop(state_guard);
            return;
        }

        if patterns_changed {
            tracing::info!("🔄 Ignore patterns changed. Restarting scan (preserving UI state).");
            // Important: Release the lock *before* calling the async task spawner
            // to prevent potential deadlocks.
            drop(state_guard);
            start_scan_on_path(PathBuf::from(current_path), proxy, state, true);
        } else if needs_refilter {
            tracing::info!("🚀 Re-applying filters due to config change.");
            // This is a synchronous operation, so we can complete it and notify from within the lock.
            filtering::apply_filters(&mut state_guard);
            let ui_state = generate_ui_state(&state_guard);
            let event = UserEvent::StateUpdate(Box::new(ui_state));
            proxy.send_event(event);
        }
        // If only other settings changed (like output filename), no re-scan or re-filter is needed.
    } else {
        tracing::warn!(
            "Failed to deserialize AppConfig from payload: {:?}",
            payload
        );
    }
}

/// Handles the initial request for state from the frontend when it loads.
pub fn initialize<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    let state_guard = state
        .lock()
        .expect("Mutex was poisoned. This should not happen.");
    let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
    proxy.send_event(event);
}

/// Updates the filename, extension, and content search filters.
///
/// If the content search query has changed, it triggers a new content search task.
/// Otherwise, it just re-applies the filename and extension filters on the existing file list.
pub async fn update_filters<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(filters) = serde_json::from_value::<HashMap<String, String>>(payload.clone()) {
        let should_search_content = {
            let mut state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            state_guard.search_query = filters.get("searchQuery").cloned().unwrap_or_default();
            state_guard.extension_filter =
                filters.get("extensionFilter").cloned().unwrap_or_default();
            let new_content_query = filters
                .get("contentSearchQuery")
                .cloned()
                .unwrap_or_default();

            if new_content_query != state_guard.content_search_query {
                state_guard.content_search_query = new_content_query;
                true
            } else {
                false
            }
        };

        if should_search_content {
            search_in_files(proxy, state).await;
        } else {
            with_state_and_notify(&state, &proxy, |s| {
                filtering::apply_filters(s);
                if !s.search_query.is_empty() || !s.extension_filter.is_empty() {
                    auto_expand_for_matches(s);
                }
            });
        }
    } else {
        tracing::warn!("Failed to deserialize filters from payload: {:?}", payload);
    }
}

/// Loads a file's content and sends it to the UI for preview.
pub fn load_file_preview<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload.clone()) {
        let path = PathBuf::from(path_str);
        let search_term;
        {
            let mut state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");

            state_guard.previewed_file_path = Some(path.clone());

            search_term = if state_guard.content_search_query.is_empty() {
                None
            } else {
                Some(state_guard.content_search_query.clone())
            };
        }

        match FileHandler::get_file_preview(&path, 1500) {
            Ok(content) => {
                let event = UserEvent::ShowFilePreview {
                    content,
                    language: get_language_from_path(&path),
                    search_term,
                    path: path.clone(),
                };
                proxy.send_event(event);
            }
            Err(e) => {
                proxy.send_event(UserEvent::ShowError(e.to_string()));
            }
        }
        // Send a state update to reflect the `previewed_file_path` change in the UI (highlighting).
        let state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
        proxy.send_event(event);
    } else {
        tracing::warn!(
            "Failed to deserialize path string from payload: {:?}",
            payload
        );
    }
}

/// Loads the children of a specific directory for lazy loading.
/// This is triggered when a user expands a directory that hasn't been fully scanned yet.
pub fn load_directory_level<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload.clone()) {
        let path = PathBuf::from(path_str);
        start_lazy_load_scan(path, proxy, state);
    } else {
        tracing::warn!(
            "Failed to deserialize path string from payload: {:?}",
            payload
        );
    }
}

/// Adds a new ignore pattern from a specific file path (via UI button click).
///
/// This acts as a convenience wrapper around `update_config` to ensure
/// logic is centralized and DRY.
pub fn add_ignore_path<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");

        if state_guard.current_path.is_empty() {
            return;
        }

        let path_to_ignore = PathBuf::from(path_str);
        let root_path = PathBuf::from(&state_guard.current_path);

        if let Ok(relative_path) = path_to_ignore.strip_prefix(&root_path) {
            let mut pattern_to_add = relative_path.to_string_lossy().to_string();

            // Ensure directory patterns end with a slash for correctness
            if path_to_ignore.is_dir() && !pattern_to_add.ends_with('/') {
                pattern_to_add.push('/');
            }

            // Create a mutable copy of the config and add the new pattern.
            let mut new_config = state_guard.config.clone();
            if new_config.ignore_patterns.insert(pattern_to_add) {
                // Now, convert the updated config to JSON and call the robust `update_config` handler.
                match serde_json::to_value(new_config) {
                    Ok(config_payload) => {
                        // Release the lock *before* calling the other command handler.
                        drop(state_guard);
                        update_config(config_payload, proxy, state);
                    }
                    Err(e) => {
                        tracing::error!("Failed to serialize config for update: {}", e);
                    }
                }
            }
        }
    } else {
        tracing::warn!("Failed to deserialize path string from payload for add_ignore_path");
    }
}

/// Toggles the selection state of a single file.
pub fn toggle_selection<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload.clone()) {
        with_state_and_notify(&state, &proxy, |s| {
            let path = PathBuf::from(path_str);
            if s.selected_files.contains(&path) {
                s.selected_files.remove(&path);
            } else {
                s.selected_files.insert(path);
            }
        });
    } else {
        tracing::warn!(
            "Failed to deserialize path string from payload: {:?}",
            payload
        );
    }
}

/// Toggles the selection state of all files within a directory.
pub fn toggle_directory_selection<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload.clone()) {
        with_state_and_notify(&state, &proxy, |s| {
            let dir_path = PathBuf::from(path_str);
            let selection_state = super::view_model::get_directory_selection_state(
                &dir_path,
                &s.filtered_file_list,
                &s.selected_files,
            );

            // Important: only operate on the currently *visible* files in that directory
            let files_in_dir: Vec<PathBuf> = s
                .filtered_file_list
                .iter()
                .filter(|item| !item.is_directory && item.path.starts_with(&dir_path))
                .map(|item| item.path.clone())
                .collect();

            if selection_state == "full" {
                // If fully selected, deselect all
                for file in files_in_dir {
                    s.selected_files.remove(&file);
                }
            } else {
                // If partially or not selected, select all
                for file in files_in_dir {
                    s.selected_files.insert(file);
                }
            }
        });
    } else {
        tracing::warn!(
            "Failed to deserialize path string from payload: {:?}",
            payload
        );
    }
}

/// Toggles the expanded/collapsed state of a directory in the UI tree.
pub fn toggle_expansion<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload.clone()) {
        with_state_and_notify(&state, &proxy, |s| {
            let path = PathBuf::from(path_str);
            if s.expanded_dirs.contains(&path) {
                s.expanded_dirs.remove(&path);
            } else {
                s.expanded_dirs.insert(path);
            }
        });
    } else {
        tracing::warn!(
            "Failed to deserialize path string from payload: {:?}",
            payload
        );
    }
}

/// Expands or collapses all *currently visible* directories in the file tree.
pub fn expand_collapse_all<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(expand) = serde_json::from_value::<bool>(payload.clone()) {
        with_state_and_notify(&state, &proxy, |s| {
            if expand {
                s.expanded_dirs = s
                    .filtered_file_list
                    .iter()
                    .filter(|i| i.is_directory)
                    .map(|i| i.path.clone())
                    .collect();
            } else {
                s.expanded_dirs.clear();
            }
        });
    } else {
        tracing::warn!("Failed to deserialize boolean from payload: {:?}", payload);
    }
}

/// Selects all *currently visible* files in the file tree.
pub fn select_all<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        let paths_to_select: Vec<PathBuf> = s
            .filtered_file_list
            .iter()
            .filter(|item| !item.is_directory)
            .map(|item| item.path.clone())
            .collect();
        s.selected_files.extend(paths_to_select);
    });
}

/// Deselects all files.
pub fn deselect_all<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        s.selected_files.clear();
    });
}

/// Expands all directories after a full scan has completed.
/// This command is intended to be used after the `is_fully_scanned` flag is true.
pub fn expand_all_fully<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        if !s.is_fully_scanned {
            tracing::warn!("expand_all_fully called before full scan completed. Ignoring.");
            return;
        }
        s.expanded_dirs = s
            .filtered_file_list
            .iter()
            .filter(|i| i.is_directory)
            .map(|i| i.path.clone())
            .collect();
    });
}

/// Selects all filter-conformant files after a full scan has completed.
/// This command is intended to be used after the `is_fully_scanned` flag is true.
pub fn select_all_fully<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        if !s.is_fully_scanned {
            tracing::warn!("select_all_fully called before full scan completed. Ignoring.");
            return;
        }
        let paths_to_select: Vec<PathBuf> = s
            .filtered_file_list
            .iter()
            .filter(|item| !item.is_directory)
            .map(|item| item.path.clone())
            .collect();
        s.selected_files.extend(paths_to_select);
    });
}

/// Generates the final concatenated output from selected files by spawning a cancellable task.
pub fn generate_preview<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state
        .lock()
        .expect("Mutex was poisoned. This should not happen.");

    state_guard.cancel_current_generation();
    state_guard.is_generating = true;
    state_guard.previewed_file_path = None;

    let new_cancel_flag = Arc::new(AtomicBool::new(false));
    state_guard.generation_cancellation_flag = new_cancel_flag.clone();

    // Send an immediate state update to the UI to show the 'generating' state.
    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
        &state_guard,
    ))));

    let proxy_clone = proxy.clone();
    let state_clone = state.clone();

    // Spawn the actual generation logic as a separate, managed task.
    let handle = tokio::spawn(async move {
        generation_task(proxy_clone, state_clone, new_cancel_flag).await;
    });

    state_guard.generation_task = Some(handle);
}

/// Cancels the ongoing file content generation task.
pub fn cancel_generation<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        s.cancel_current_generation();
    });
}

/// Resets the preview state in the UI.
pub fn clear_preview_state<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        s.previewed_file_path = None;
    });
}

/// Saves the provided content to a file, prompting the user for a location.
pub fn save_file<P: EventProxy>(payload: serde_json::Value, proxy: P, state: Arc<Mutex<AppState>>) {
    if let Some(content) = payload.as_str() {
        let content_clone = content.to_string();
        let (output_dir, filename) = {
            let state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            (
                state_guard.config.output_directory.clone(),
                state_guard.config.output_filename.clone(),
            )
        };

        let mut dialog = rfd::FileDialog::new()
            .add_filter("Text File", &["txt"])
            .set_file_name(&filename);
        if let Some(dir) = output_dir {
            dialog = dialog.set_directory(dir);
        }

        if let Some(path) = dialog.save_file() {
            match std::fs::write(&path, content_clone) {
                Ok(_) => {
                    let event = UserEvent::SaveComplete(true, path.to_string_lossy().to_string());
                    proxy.send_event(event);
                }
                Err(e) => {
                    let event = UserEvent::SaveComplete(false, e.to_string());
                    proxy.send_event(event);
                }
            };
        } else {
            let event = UserEvent::SaveComplete(false, "cancelled".to_string());
            proxy.send_event(event);
        }
    } else {
        tracing::warn!(
            "Failed to deserialize content string from payload: {:?}",
            payload
        );
    }
}

/// Opens a file dialog for the user to select a default output directory.
pub fn pick_output_directory<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        with_state_and_notify(&state, &proxy, |s| {
            s.config.output_directory = Some(path);
        });
    }
}

/// Imports an application configuration from a JSON file.
///
/// This action is treated as a "hard reset" of the application's context.
/// It first completely clears the current state (file lists, selections, previews),
/// sends an immediate UI update to reflect this clean state, and then applies
/// the new configuration. If the imported config specifies a directory, a new
/// scan is initiated on that path from a clean slate.
pub fn import_config<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()
    {
        match config::settings::import_config(&path) {
            Ok(new_config) => {
                let filename = path.file_name().and_then(|n| n.to_str()).map(String::from);
                let dir_to_scan = new_config.last_directory.clone();

                // Lock the state to perform the reset and config update atomically.
                let mut state_guard = state
                    .lock()
                    .expect("Mutex was poisoned. This should not happen.");

                // 1. Reset the entire directory-related state to a clean slate.
                state_guard.reset_directory_state();

                // 2. Apply the new configuration.
                state_guard.config = new_config;
                state_guard.current_config_filename = filename;
                if let Err(e) = config::settings::save_config(&state_guard.config) {
                    tracing::warn!("Failed to save imported config: {}", e);
                }

                // 3. IMPORTANT: Immediately send a UI update to reflect the clean state.
                //    This ensures the GUI is wiped clean *before* any new scan begins.
                let clean_ui_state = generate_ui_state(&state_guard);
                proxy.send_event(UserEvent::StateUpdate(Box::new(clean_ui_state)));

                // 4. Release the lock before potentially starting a new scan task.
                drop(state_guard);

                // 5. If a directory is specified, start scanning it. The UI is already clean.
                if let Some(dir) = dir_to_scan {
                    if dir.exists() {
                        start_scan_on_path(dir, proxy, state, false);
                    }
                }
            }
            Err(e) => {
                let event = UserEvent::ShowError(format!("Failed to import config: {e}"));
                proxy.send_event(event);
            }
        }
    }
}

/// Exports the current application configuration to a JSON file.
pub fn export_config<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name("cfc-config.json")
        .save_file()
    {
        let state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        let result = config::settings::export_config(&state_guard.config, &path).is_ok();
        proxy.send_event(UserEvent::ConfigExported(result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use crate::app::view_model::UiState;
    use crate::core::FileItem;
    use serde_json::json;
    use std::fs as std_fs;
    use tempfile::{tempdir, TempDir};
    use tokio::sync::mpsc;

    // A mock EventProxy for capturing events sent to the UI.
    #[derive(Clone)]
    struct TestEventProxy {
        sender: mpsc::UnboundedSender<UserEvent>,
    }

    impl EventProxy for TestEventProxy {
        fn send_event(&self, event: UserEvent) {
            // Sending might fail if the receiver is dropped, which is a panic condition in tests.
            self.sender.send(event).expect("Test receiver dropped");
        }
    }

    // A harness to set up a consistent and isolated test environment.
    struct TestHarness {
        state: Arc<Mutex<AppState>>,
        proxy: TestEventProxy,
        event_rx: mpsc::UnboundedReceiver<UserEvent>,
        _temp_dir: TempDir, // Kept for its Drop trait, which cleans up the temp directory.
        root_path: PathBuf,
    }

    impl TestHarness {
        // Creates a new harness with a default state.
        fn new() -> Self {
            let temp_dir = tempdir().expect("Failed to create temp dir");
            let root_path = temp_dir.path().to_path_buf();
            let (tx, rx) = mpsc::unbounded_channel();
            let proxy = TestEventProxy { sender: tx };

            // FIX: Explicitly create a clean state instead of using AppState::default().
            // This ensures the test is hermetic and does not load any user-level config files
            // from the file system, which was the cause of the original failure.
            let mut state = AppState::default();
            state.config = AppConfig::default(); // Overwrite with a pristine, in-memory default config.
            state.current_path = root_path.to_string_lossy().to_string(); // Set path for the test context.

            Self {
                state: Arc::new(Mutex::new(state)),
                proxy,
                event_rx: rx,
                _temp_dir: temp_dir,
                root_path,
            }
        }

        // Helper to create a file within the test's temporary directory.
        fn create_file(&self, relative_path: &str) -> PathBuf {
            let path = self.root_path.join(relative_path);
            if let Some(parent) = path.parent() {
                std_fs::create_dir_all(parent).unwrap();
            }
            std_fs::write(&path, format!("content of {}", relative_path)).unwrap();
            path
        }

        // Helper to create a directory within the test's temporary directory.
        fn create_dir(&self, relative_path: &str) -> PathBuf {
            let path = self.root_path.join(relative_path);
            std_fs::create_dir_all(&path).unwrap();
            path
        }

        // Helper to populate the AppState with a list of FileItems.
        fn set_initial_files(&self, paths: &[&str]) {
            let mut state = self.state.lock().unwrap();
            let mut items = Vec::new();
            for p_str in paths {
                let path = self.root_path.join(p_str);
                // Corrected: Clone path for the first argument to avoid move-then-borrow error.
                items.push(file_item(path.clone(), path.is_dir()));
            }
            state.full_file_list = items.clone();
            state.filtered_file_list = items;
        }

        // Helper to receive the last `StateUpdate` event from the channel, consuming any intermediate events.
        async fn get_last_state_update(&mut self) -> Option<Box<UiState>> {
            let mut last_update = None;
            // Set a short timeout to prevent tests from hanging if no event is sent.
            let timeout = tokio::time::sleep(std::time::Duration::from_millis(200));
            tokio::pin!(timeout);

            loop {
                tokio::select! {
                    event = self.event_rx.recv() => {
                        if let Some(event) = event {
                            if let UserEvent::StateUpdate(ui_state) = event {
                                last_update = Some(ui_state);
                            }
                        } else {
                            break; // Channel closed
                        }
                    },
                    _ = &mut timeout => {
                        break; // Timeout elapsed
                    }
                }
            }
            last_update
        }

        // Helper to get the next event, regardless of its type.
        async fn get_next_event(&mut self) -> Option<UserEvent> {
            tokio::time::timeout(std::time::Duration::from_secs(1), self.event_rx.recv())
                .await
                .ok()
                .flatten()
        }

        // Helper to wait for an async scan to complete by watching the `is_scanning` flag.
        async fn wait_for_scan_completion(&mut self) -> Option<Box<UiState>> {
            let timeout = tokio::time::sleep(std::time::Duration::from_secs(2));
            tokio::pin!(timeout);

            loop {
                tokio::select! {
                    event = self.get_next_event() => {
                         if let Some(UserEvent::StateUpdate(ui_state)) = event {
                            if !ui_state.is_scanning {
                                return Some(ui_state);
                            }
                        } else if event.is_none() {
                            return None; // Channel closed
                        }
                    },
                    _ = &mut timeout => {
                        return None; // Timeout
                    }
                }
            }
        }
    }

    // Helper to create a mock FileItem for testing.
    fn file_item(path: PathBuf, is_dir: bool) -> FileItem {
        FileItem {
            path,
            is_directory: is_dir,
            is_binary: false,
            size: if is_dir { 0 } else { 123 },
            depth: 1,
            parent: None,
        }
    }

    #[tokio::test]
    async fn test_initialize_sends_initial_state() {
        // Arrange
        let mut harness = TestHarness::new();

        // Act
        initialize(harness.proxy.clone(), harness.state.clone());

        // Assert
        let ui_state = harness.get_last_state_update().await.unwrap();
        assert_eq!(ui_state.current_path, harness.root_path.to_string_lossy());
        assert!(ui_state.tree.is_empty());
    }

    #[tokio::test]
    async fn test_clear_directory_resets_state() {
        // Arrange
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("file.txt");
        harness.set_initial_files(&["file.txt"]);
        {
            let mut state = harness.state.lock().unwrap();
            state.selected_files.insert(file_path);
            state.config.last_directory = Some(harness.root_path.clone());
        }

        // Act
        clear_directory(harness.proxy.clone(), harness.state.clone());

        // Assert (Corrected Structure)
        let ui_state = harness.get_last_state_update().await.unwrap();
        assert!(ui_state.current_path.is_empty());
        assert_eq!(ui_state.visible_files_count, 0);
        {
            let state = harness.state.lock().unwrap();
            assert!(state.current_path.is_empty());
            assert!(state.full_file_list.is_empty());
            assert!(state.filtered_file_list.is_empty());
            assert!(state.selected_files.is_empty());
            assert!(state.config.last_directory.is_none());
        }
    }

    #[tokio::test]
    async fn test_rescan_directory_on_empty_path_does_nothing() {
        // Arrange
        let mut harness = TestHarness::new();
        {
            // Ensure no path is loaded
            let mut state = harness.state.lock().unwrap();
            state.current_path = String::new();
        }

        // Act
        rescan_directory(harness.proxy.clone(), harness.state.clone());

        // Assert
        // No StateUpdate event should be sent, so the channel should be empty.
        let event = harness.get_next_event().await;
        assert!(
            event.is_none(),
            "Rescan should not trigger any event when path is empty"
        );
    }

    #[tokio::test]
    async fn test_toggle_selection_adds_and_removes_file() {
        // Arrange
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("test.rs");
        harness.set_initial_files(&["test.rs"]);
        let payload = json!(file_path);

        // Act 1: Select the file
        toggle_selection(
            payload.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        // Assert 1 (Corrected Structure)
        let ui_state1 = harness.get_last_state_update().await.unwrap();
        assert_eq!(ui_state1.selected_files_count, 1);
        {
            let state = harness.state.lock().unwrap();
            assert!(state.selected_files.contains(&file_path));
        }

        // Act 2: Deselect the file
        toggle_selection(payload, harness.proxy.clone(), harness.state.clone());

        // Assert 2 (Corrected Structure)
        let ui_state2 = harness.get_last_state_update().await.unwrap();
        assert_eq!(ui_state2.selected_files_count, 0);
        {
            let state = harness.state.lock().unwrap();
            assert!(!state.selected_files.contains(&file_path));
        }
    }

    #[tokio::test]
    async fn test_toggle_directory_selection_selects_and_deselects_all_children() {
        // Arrange
        let mut harness = TestHarness::new();
        let file1 = harness.create_file("src/main.rs");
        let file2 = harness.create_file("src/lib.rs");
        let dir_path = harness.root_path.join("src");
        harness.set_initial_files(&["src", "src/main.rs", "src/lib.rs"]);
        let payload = json!(dir_path);

        // Act 1: Select all children
        toggle_directory_selection(
            payload.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        // Assert 1: All children are selected
        let ui_state_select = harness.get_last_state_update().await.unwrap();
        assert_eq!(ui_state_select.selected_files_count, 2);
        let src_node_select = ui_state_select
            .tree
            .iter()
            .find(|n| n.name == "src")
            .unwrap();
        assert_eq!(
            src_node_select.selection_state, "full",
            "Directory should be fully selected"
        );

        // Act 2: Deselect all children
        toggle_directory_selection(payload, harness.proxy.clone(), harness.state.clone());

        // Assert 2: All children are deselected
        let ui_state_deselect = harness.get_last_state_update().await.unwrap();
        assert_eq!(
            ui_state_deselect.selected_files_count, 0,
            "Files should be deselected"
        );
        let src_node_deselect = ui_state_deselect
            .tree
            .iter()
            .find(|n| n.name == "src")
            .unwrap();
        assert_eq!(
            src_node_deselect.selection_state, "none",
            "Directory should have no selection"
        );

        let state = harness.state.lock().unwrap();
        assert!(!state.selected_files.contains(&file1));
        assert!(!state.selected_files.contains(&file2));
    }

    #[tokio::test]
    async fn test_select_all_and_deselect_all() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("file1.txt");
        harness.create_file("file2.txt");
        harness.set_initial_files(&["file1.txt", "file2.txt"]);

        // Act 1: Select all
        select_all(harness.proxy.clone(), harness.state.clone());

        // Assert 1
        let ui_state1 = harness.get_last_state_update().await.unwrap();
        assert_eq!(ui_state1.selected_files_count, 2);

        // Act 2: Deselect all
        deselect_all(harness.proxy.clone(), harness.state.clone());

        // Assert 2
        let ui_state2 = harness.get_last_state_update().await.unwrap();
        assert_eq!(ui_state2.selected_files_count, 0);
    }

    #[tokio::test]
    async fn test_fully_scanned_guards() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("file1.txt");
        harness.set_initial_files(&["file1.txt"]);
        {
            let mut state = harness.state.lock().unwrap();
            state.is_fully_scanned = false; // Explicitly set to false
            state.expanded_dirs.clear();
            state.selected_files.clear();
        }

        // Act 1: Call expand_all_fully when not fully scanned
        expand_all_fully(harness.proxy.clone(), harness.state.clone());
        let ui_state1 = harness.get_last_state_update().await.unwrap();

        // Assert 1: Nothing changed
        assert!(ui_state1.tree.iter().all(|n| !n.is_expanded));
        assert_eq!(ui_state1.selected_files_count, 0);

        // Act 2: Call select_all_fully when not fully scanned
        select_all_fully(harness.proxy.clone(), harness.state.clone());
        let ui_state2 = harness.get_last_state_update().await.unwrap();

        // Assert 2: Nothing changed
        assert_eq!(ui_state2.selected_files_count, 0);
    }

    #[tokio::test]
    async fn test_expand_collapse_all() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs");
        harness.create_file("docs/guide.md");
        harness.set_initial_files(&["src", "docs", "src/main.rs", "docs/guide.md"]);

        // Act 1: Expand all
        expand_collapse_all(json!(true), harness.proxy.clone(), harness.state.clone());

        // Assert 1 (Corrected Structure)
        let ui_state1 = harness.get_last_state_update().await.unwrap();
        assert_eq!(ui_state1.tree.iter().filter(|n| n.is_expanded).count(), 2);
        {
            let state = harness.state.lock().unwrap();
            assert_eq!(state.expanded_dirs.len(), 2);
        }

        // Act 2: Collapse all
        expand_collapse_all(json!(false), harness.proxy.clone(), harness.state.clone());

        // Assert 2 (Corrected Structure)
        let ui_state2 = harness.get_last_state_update().await.unwrap();
        assert_eq!(ui_state2.tree.iter().filter(|n| n.is_expanded).count(), 0);
        {
            let state = harness.state.lock().unwrap();
            assert!(state.expanded_dirs.is_empty());
        }
    }

    #[tokio::test]
    async fn test_load_file_preview_sends_search_term() {
        // Arrange
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("preview.txt");
        let search_term = "magic_word";
        {
            let mut state = harness.state.lock().unwrap();
            state.content_search_query = search_term.to_string();
        }
        let payload = json!(file_path);

        // Act
        load_file_preview(payload, harness.proxy.clone(), harness.state.clone());

        // Assert
        // We expect two events: ShowFilePreview and StateUpdate
        let mut saw_preview = false;
        for _ in 0..2 {
            if let Some(event) = harness.get_next_event().await {
                if let UserEvent::ShowFilePreview {
                    search_term: term, ..
                } = event
                {
                    assert_eq!(
                        term,
                        Some(search_term.to_string()),
                        "Search term should be passed to the preview event"
                    );
                    saw_preview = true;
                }
            }
        }
        assert!(saw_preview, "Did not receive the ShowFilePreview event");
    }

    #[tokio::test]
    async fn test_load_file_preview_success_and_error() {
        // Arrange
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("preview.txt");
        harness.set_initial_files(&["preview.txt"]); // Corrected: ensure file is in state for UI update
        let bad_path = harness.root_path.join("nonexistent.txt");
        let payload_good = json!(file_path);
        let payload_bad = json!(bad_path);

        // Act 1: Successful preview
        load_file_preview(payload_good, harness.proxy.clone(), harness.state.clone());

        // Assert 1
        let mut saw_preview = false;
        let mut saw_state_update = false;
        for _ in 0..2 {
            // Expect two events
            if let Some(event) = harness.get_next_event().await {
                match event {
                    UserEvent::ShowFilePreview { content, path, .. } => {
                        assert!(content.contains("content of preview.txt"));
                        assert_eq!(path, file_path);
                        saw_preview = true;
                    }
                    UserEvent::StateUpdate(ui_state) => {
                        assert_eq!(ui_state.tree[0].path, file_path);
                        assert!(ui_state.tree[0].is_previewed);
                        saw_state_update = true;
                    }
                    _ => panic!("Unexpected event received"),
                }
            }
        }
        assert!(
            saw_preview && saw_state_update,
            "Did not receive both required events for preview"
        );

        // Act 2: Failed preview
        load_file_preview(payload_bad, harness.proxy.clone(), harness.state.clone());

        // Assert 2
        if let Some(UserEvent::ShowError(msg)) = harness.get_next_event().await {
            assert!(msg.contains("No such file or directory"));
        } else {
            panic!("Expected a ShowError event for non-existent file");
        }
    }

    // In src/app/commands.rs, inside #[cfg(test)] mod tests { ... }
    #[tokio::test]
    async fn test_update_config_triggers_refilter() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs");
        harness.create_file("src/empty_dir/placeholder.txt");
        // Manually remove the placeholder to make the directory genuinely empty.
        std_fs::remove_file(harness.root_path.join("src/empty_dir/placeholder.txt")).unwrap();
        let src_path = harness.root_path.join("src");
        let empty_dir_path = harness.root_path.join("src/empty_dir");

        harness.set_initial_files(&["src", "src/main.rs", "src/empty_dir"]);
        {
            let mut state = harness.state.lock().unwrap();
            // FIX: Accurately simulate a full scan by setting is_fully_scanned AND
            // marking all found directories as "loaded". This is crucial for the
            // "remove empty" logic to correctly identify and prune `src/empty_dir`.
            state.is_fully_scanned = true;
            state.loaded_dirs.insert(src_path);
            state.loaded_dirs.insert(empty_dir_path);
        }

        let mut new_config;
        {
            let state = harness.state.lock().unwrap();
            new_config = state.config.clone();
        }
        new_config.remove_empty_directories = true; // Change the setting that triggers a refilter
        let payload = serde_json::to_value(new_config).unwrap();

        // Act
        update_config(payload, harness.proxy.clone(), harness.state.clone());

        // Assert
        let ui_state = harness
            .get_last_state_update()
            .await
            .expect("Test timed out waiting for StateUpdate event");

        // The visible file count should now be 2: 'src' and 'src/main.rs'. 'src/empty_dir' is removed.
        assert_eq!(
            ui_state.visible_files_count, 2,
            "Expected 'src/empty_dir' to be removed"
        );

        // Add a more robust assertion to check the tree structure directly.
        let src_node = ui_state
            .tree
            .iter()
            .find(|n| n.name == "src")
            .expect("'src' node should be present in the tree");
        assert!(
            !src_node.children.iter().any(|n| n.name == "empty_dir"),
            "The node for 'empty_dir' should have been filtered out and not be a child of 'src'"
        );
    }

    #[tokio::test]
    async fn test_update_filters_applies_filename_filter_without_content_search() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs");
        harness.create_file("src/lib.rs");
        harness.create_file("README.md");
        harness.set_initial_files(&["src", "src/main.rs", "src/lib.rs", "README.md"]);

        let filters = json!({
            "searchQuery": "main",
            "extensionFilter": "",
            "contentSearchQuery": "" // Ensure content search is not triggered
        });

        // Act
        update_filters(filters, harness.proxy.clone(), harness.state.clone()).await;

        // Assert
        let ui_state = harness.get_last_state_update().await.unwrap();
        // Visible items should be 'src' and 'src/main.rs'
        assert_eq!(ui_state.visible_files_count, 2);
        assert!(ui_state.tree.iter().any(|n| n.name == "src"));
        assert!(!ui_state.tree.iter().any(|n| n.name == "README.md"));
        let src_node = ui_state.tree.iter().find(|n| n.name == "src").unwrap();
        assert!(src_node.children.iter().any(|c| c.name == "main.rs"));
        assert!(!src_node.children.iter().any(|c| c.name == "lib.rs"));

        // Verify that the internal state was updated correctly.
        let state = harness.state.lock().unwrap();
        assert_eq!(state.search_query, "main");
    }

    #[tokio::test]
    async fn test_add_ignore_path_for_file() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs");
        harness.create_file("README.md"); // This file will be ignored
        harness.set_initial_files(&["src", "src/main.rs", "README.md"]);

        let path_to_ignore = harness.root_path.join("README.md");
        let payload = json!(path_to_ignore);

        // Act: Ignore the 'README.md' file
        add_ignore_path(payload, harness.proxy.clone(), harness.state.clone());

        // Assert: A rescan is triggered, so we wait for it to complete.
        let final_state = harness
            .wait_for_scan_completion()
            .await
            .expect("Scan did not complete after ignoring path");

        // The 'README.md' file should be gone.
        assert_eq!(final_state.visible_files_count, 2); // 'src' and 'src/main.rs'
        assert!(final_state.tree.iter().any(|n| n.name == "src"));
        assert!(!final_state.tree.iter().any(|n| n.name == "README.md"));

        let state = harness.state.lock().unwrap();
        assert!(state.config.ignore_patterns.contains("README.md"));
    }

    #[tokio::test]
    async fn test_add_ignore_path_retriggers_scan() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs");
        harness.create_dir("docs");
        harness.create_file("docs/guide.md"); // This file will be ignored

        // This simulates a state after an initial scan
        harness.set_initial_files(&["src", "docs", "src/main.rs", "docs/guide.md"]);

        let path_to_ignore = harness.root_path.join("docs");
        let payload = json!(path_to_ignore);

        // Act: Ignore the 'docs' directory
        // This is async because it triggers `update_config` which triggers `start_scan_on_path`
        add_ignore_path(payload, harness.proxy.clone(), harness.state.clone());

        // Assert
        // We expect a final state update after the rescan finishes.
        let final_state = harness
            .wait_for_scan_completion()
            .await
            .expect("Scan did not complete after ignoring path");

        // The 'docs' directory and its contents should be gone.
        assert_eq!(final_state.visible_files_count, 2); // 'src' and 'src/main.rs'
        assert!(final_state.tree.iter().any(|n| n.name == "src"));
        assert!(!final_state.tree.iter().any(|n| n.name == "docs"));
        {
            let state = harness.state.lock().unwrap();
            assert!(
                state.config.ignore_patterns.contains("docs/"),
                "Pattern should end with a slash for directories"
            );
        }
    }

    #[tokio::test]
    async fn test_commands_with_invalid_payloads_do_not_panic() {
        // This single test covers the error paths for multiple commands.
        // Arrange
        let mut harness = TestHarness::new();
        let initial_state_snapshot = harness.state.lock().unwrap().config.clone();

        // Act & Assert for toggle_selection (expects a string, gets a number)
        toggle_selection(json!(123), harness.proxy.clone(), harness.state.clone());
        assert!(
            harness.event_rx.try_recv().is_err(),
            "Should not send event on bad payload"
        );

        // Act & Assert for update_filters (expects an object, gets a string)
        update_filters(
            json!("bad data"),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
        assert!(
            harness.event_rx.try_recv().is_err(),
            "Should not send event on bad payload"
        );

        // Act & Assert for update_config (expects AppConfig, gets a boolean)
        update_config(json!(false), harness.proxy.clone(), harness.state.clone());
        assert!(
            harness.event_rx.try_recv().is_err(),
            "Should not send event on bad payload"
        );

        // Final check: Ensure state was not mutated by any of the invalid calls
        let final_state_snapshot = harness.state.lock().unwrap().config.clone();
        assert_eq!(
            initial_state_snapshot.output_filename, final_state_snapshot.output_filename,
            "State should not change on bad payload"
        );
    }

    #[tokio::test]
    async fn test_generate_preview_sets_generating_state_and_spawns_task() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("file.txt");
        harness.set_initial_files(&["file.txt"]);

        // Act
        generate_preview(harness.proxy.clone(), harness.state.clone());

        // Assert: Check for the *immediate* state update to 'generating: true'
        let event = harness
            .get_next_event()
            .await
            .expect("Did not receive an event after calling generate_preview");

        let ui_state = match event {
            UserEvent::StateUpdate(ui_state) => ui_state,
            _ => panic!("Expected a StateUpdate event first, got {:?}", event),
        };
        assert!(
            ui_state.is_generating,
            "UI state should immediately switch to generating"
        );

        // Assert: Check the internal app state is correctly configured
        {
            let state = harness.state.lock().unwrap();
            assert!(state.is_generating);
            assert!(
                state.generation_task.is_some(),
                "Generation task handle should be stored in state"
            );
        }

        // Assert: Wait for the generation to complete by looking for the final state update.
        // This is more robust than get_last_state_update.
        let mut final_event_found = false;
        let timeout = tokio::time::sleep(std::time::Duration::from_secs(2));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                event = harness.get_next_event() => {
                    if let Some(UserEvent::StateUpdate(ui_state)) = event {
                        if !ui_state.is_generating { // This is the specific condition we are waiting for
                            final_event_found = true;
                            break;
                        }
                    } else if event.is_none() {
                        panic!("Event channel closed unexpectedly while waiting for final state");
                    }
                },
                _ = &mut timeout => {
                    break; // Timeout elapsed
                }
            }
        }

        assert!(
            final_event_found,
            "Did not receive final state update where is_generating is false"
        );
    }

    #[tokio::test]
    async fn test_cancel_generation_resets_generating_state() {
        // Arrange
        let mut harness = TestHarness::new();
        harness.create_file("file.txt");
        harness.set_initial_files(&["file.txt"]);

        // Start generation first
        generate_preview(harness.proxy.clone(), harness.state.clone());

        // Wait for the initial "generating" state update to be processed
        let _ = harness.get_last_state_update().await;

        // Act: Now cancel it
        cancel_generation(harness.proxy.clone(), harness.state.clone());

        // Assert
        let ui_state = harness
            .get_last_state_update()
            .await
            .expect("Did not receive state update after cancelling generation");
        assert!(
            !ui_state.is_generating,
            "UI State should not be 'generating'"
        );
        {
            let state = harness.state.lock().unwrap();
            assert!(!state.is_generating, "App State should not be 'generating'");
            assert!(
                state.generation_task.is_none(),
                "Task handle should be cleared after cancellation"
            );
        }
    }
}
