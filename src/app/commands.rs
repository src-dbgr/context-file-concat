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
use super::helpers::with_state_and_notify;
use super::proxy::EventProxy;
use super::state::AppState;
use super::tasks::{generation_task, search_in_files, start_scan_on_path};
use super::view_model::{
    apply_filters, auto_expand_for_matches, generate_ui_state, get_language_from_path,
};
use crate::config;
use crate::core::FileHandler;

/// Opens a file dialog for the user to select a directory to scan.
pub fn select_directory<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        start_scan_on_path(path, proxy, state);
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
pub fn rescan_directory<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    let current_path_str = {
        state
            .lock()
            .expect("Mutex was poisoned. This should not happen.")
            .current_path
            .clone()
    };
    if !current_path_str.is_empty() {
        start_scan_on_path(PathBuf::from(current_path_str), proxy, state);
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
/// If ignore patterns have changed, a re-scan of the current directory is triggered.
/// Otherwise, filters are just re-applied to the existing file list.
pub fn update_config<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(new_config) = serde_json::from_value(payload) {
        let (should_restart_scan, path_if_needed) = {
            let mut state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            let old_ignore_patterns = state_guard.config.ignore_patterns.clone();
            state_guard.config = new_config;

            if let Err(e) = config::settings::save_config(&state_guard.config) {
                tracing::warn!("Failed to save config on update: {}", e);
            }

            let ignore_patterns_changed = old_ignore_patterns != state_guard.config.ignore_patterns;
            if ignore_patterns_changed && !state_guard.current_path.is_empty() {
                (true, Some(PathBuf::from(state_guard.current_path.clone())))
            } else {
                (false, None)
            }
        };

        if should_restart_scan {
            tracing::info!("🔄 Restarting scan due to ignore pattern changes");
            if let Some(path) = path_if_needed {
                start_scan_on_path(path, proxy, state);
            }
        } else {
            with_state_and_notify(&state, &proxy, |s| {
                apply_filters(s);
            });
        }
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
/// If the content search query has changed, it triggers a new content search.
/// Otherwise, it just re-applies the filename and extension filters.
pub async fn update_filters<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(filters) = serde_json::from_value::<HashMap<String, String>>(payload) {
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
                apply_filters(s);
                if !s.search_query.is_empty() {
                    auto_expand_for_matches(s);
                }
            });
        }
    }
}

/// Loads a file's content and sends it to the UI for preview.
pub fn load_file_preview<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path = PathBuf::from(path_str);
        let search_term = {
            let state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            if state_guard.content_search_query.is_empty() {
                None
            } else {
                Some(state_guard.content_search_query.clone())
            }
        };

        // This command sends multiple, different events, so it doesn't fit the helper.
        {
            state
                .lock()
                .expect("Mutex was poisoned. This should not happen.")
                .previewed_file_path = Some(path.clone());
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

        // Send a state update to reflect the `previewed_file_path` change
        let state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
        proxy.send_event(event);
    }
}

/// Adds a new ignore pattern to the configuration from a specific file path.
pub fn add_ignore_path<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        with_state_and_notify(&state, &proxy, |s| {
            let path_to_ignore = PathBuf::from(path_str);
            let root_path = PathBuf::from(&s.current_path);

            if let Ok(relative_path) = path_to_ignore.strip_prefix(&root_path) {
                let mut pattern_to_add = relative_path.to_string_lossy().to_string();
                if path_to_ignore.is_dir() {
                    pattern_to_add.push('/');
                }
                s.selected_files.retain(|p| !p.starts_with(&path_to_ignore));

                // Add the pattern to both the configuration and the set of active patterns.
                // This ensures it immediately appears as "active" (green) in the UI.
                s.config.ignore_patterns.insert(pattern_to_add.clone());
                s.active_ignore_patterns.insert(pattern_to_add);

                if let Err(e) = config::settings::save_config(&s.config) {
                    tracing::warn!("Failed to save config after adding ignore path: {}", e);
                }
                apply_filters(s); // Re-apply filters to update the view
            }
        });
    }
}

/// Toggles the selection state of a single file.
pub fn toggle_selection<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        with_state_and_notify(&state, &proxy, |s| {
            let path = PathBuf::from(path_str);
            if s.selected_files.contains(&path) {
                s.selected_files.remove(&path);
            } else {
                s.selected_files.insert(path);
            }
        });
    }
}

/// Toggles the selection state of all files within a directory.
pub fn toggle_directory_selection<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        with_state_and_notify(&state, &proxy, |s| {
            let dir_path = PathBuf::from(path_str);
            let selection_state = super::view_model::get_directory_selection_state(
                &dir_path,
                &s.filtered_file_list,
                &s.selected_files,
            );

            let files_in_dir: Vec<PathBuf> = s
                .filtered_file_list
                .iter()
                .filter(|item| !item.is_directory && item.path.starts_with(&dir_path))
                .map(|item| item.path.clone())
                .collect();

            if selection_state == "full" {
                for file in files_in_dir {
                    s.selected_files.remove(&file);
                }
            } else {
                for file in files_in_dir {
                    s.selected_files.insert(file);
                }
            }
        });
    }
}

/// Toggles the expanded/collapsed state of a directory in the UI tree.
pub fn toggle_expansion<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        with_state_and_notify(&state, &proxy, |s| {
            let path = PathBuf::from(path_str);
            if s.expanded_dirs.contains(&path) {
                s.expanded_dirs.remove(&path);
            } else {
                s.expanded_dirs.insert(path);
            }
        });
    }
}

/// Expands or collapses all directories in the file tree.
pub fn expand_collapse_all<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(expand) = serde_json::from_value::<bool>(payload) {
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
    }
}

/// Selects all currently visible files in the file tree.
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

/// Generates the final concatenated output from selected files by spawning a cancellable task.
pub fn generate_preview<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state
        .lock()
        .expect("Mutex was poisoned. This should not happen.");

    // Ensure any previous generation task is cancelled before starting a new one.
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
pub fn import_config<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()
    {
        match config::settings::import_config(&path) {
            Ok(new_config) => {
                let filename = path.file_name().and_then(|n| n.to_str()).map(String::from);
                let dir_to_scan = {
                    let mut state_guard = state
                        .lock()
                        .expect("Mutex was poisoned. This should not happen.");
                    state_guard.cancel_current_scan();
                    state_guard.config = new_config;
                    state_guard.current_config_filename = filename;
                    if let Err(e) = config::settings::save_config(&state_guard.config) {
                        tracing::warn!("Failed to save imported config: {}", e);
                    }
                    state_guard.config.last_directory.clone()
                };

                if let Some(dir) = dir_to_scan {
                    if dir.exists() {
                        start_scan_on_path(dir, proxy, state);
                    }
                } else {
                    let state_guard = state
                        .lock()
                        .expect("Mutex was poisoned. This should not happen.");
                    let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
                    proxy.send_event(event);
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
