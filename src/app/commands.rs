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
