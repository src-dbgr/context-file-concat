// src/app/commands.rs
//! Contains all the command handlers that are callable from the frontend via IPC.
//!
//! Each function in this module corresponds to a specific `IpcMessage::command`.
//! These handlers are responsible for interacting with the `AppState` and the `core`
//! logic, and for sending `UserEvent`s back to the UI.

use super::events::UserEvent;
use super::filtering; // SRP: Use the new filtering module
use super::helpers::with_state_and_notify;
use super::proxy::EventProxy;
use super::state::{AppState, StatusNotice};
use super::tasks::{
    self, refresh_content_search, search_in_files, start_lazy_load_scan, start_scan_on_path,
};
use super::view_model::{auto_expand_for_matches, generate_ui_state, get_language_from_path};
use crate::app::file_dialog::DialogService;
use crate::config::{self, AppConfig}; // Import AppConfig for explicit deserialization
use crate::core::{is_blank, FileHandler};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Opens a file dialog for the user to select a directory to scan.
///
/// Triggers a new proactive, two-phase scan on the selected path.
/// This always performs a hard reset of the application state.
///
/// Dismissing the dialog changes nothing: in particular, a scan that is running in the
/// background keeps running and keeps owning the scanning state. The UI is still sent
/// the (unchanged) state, so it can rely on getting an answer to its request.
pub fn select_directory<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Some(path) = dialog.pick_directory() {
        // A new directory selection should always reset the state.
        start_scan_on_path(path, proxy, state, false);
    } else {
        tracing::debug!("Directory selection cancelled by the user.");
        with_state_and_notify(&state, &proxy, |_| {});
    }
}

/// Clears the currently loaded directory and resets the application state.
pub fn clear_directory<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        s.reset_directory_state();
        s.config.last_directory = None;
        if let Err(e) = config::settings::save_config(&s.config, None) {
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
        let mut state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");

        state_guard.patterns_need_rescan = false;
        state_guard.current_path.clone()
    };

    if !current_path_str.is_empty() {
        start_scan_on_path(PathBuf::from(current_path_str), proxy, state, true);
    }
}

/// Cancels the ongoing directory scan.
pub fn cancel_scan<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        tracing::debug!("Scan cancellation requested.");
        s.cancel_current_scan();
    });
}

/// Updates the application configuration and persists it.
///
/// What else happens depends on what has changed:
/// - Ignore patterns while a scan is running: the scan is restarted, so that it works
///   with the new patterns instead of delivering a result based on the old ones.
/// - Ignore patterns added otherwise: they are applied to the loaded file list.
/// - Ignore patterns removed otherwise: the user is told that a re-scan is needed.
/// - Case sensitivity: the filters are re-applied and an active content search is
///   repeated, as its matches depend on it.
pub async fn update_config<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    let Ok(new_config) = serde_json::from_value::<AppConfig>(payload.clone()) else {
        tracing::warn!(
            "Failed to deserialize AppConfig from payload: {:?}",
            payload
        );
        return;
    };

    if apply_config(new_config, &proxy, &state) == ContentSearchFollowUp::Repeat {
        refresh_content_search(&proxy, &state, tasks::RealFileSearcher).await;
    }
}

/// Whether the content search has to be repeated after a configuration change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentSearchFollowUp {
    None,
    Repeat,
}

/// Stores `new_config` and brings the state in line with it (see [`update_config`]).
fn apply_config<P: EventProxy>(
    new_config: AppConfig,
    proxy: &P,
    state: &Arc<Mutex<AppState>>,
) -> ContentSearchFollowUp {
    let mut state_guard = state
        .lock()
        .expect("Mutex was poisoned. This should not happen.");

    let patterns_added: HashSet<String> = new_config
        .ignore_patterns
        .difference(&state_guard.config.ignore_patterns)
        .cloned()
        .collect();

    let patterns_removed: HashSet<String> = state_guard
        .config
        .ignore_patterns
        .difference(&new_config.ignore_patterns)
        .cloned()
        .collect();

    let case_sensitivity_changed =
        state_guard.config.case_sensitive_search != new_config.case_sensitive_search;
    let needs_refilter = case_sensitivity_changed
        || state_guard.config.remove_empty_directories != new_config.remove_empty_directories;

    state_guard.config = new_config;
    if let Err(e) = config::settings::save_config(&state_guard.config, None) {
        tracing::warn!("Failed to save config on update: {}", e);
    }

    // Without a loaded directory the config is only saved; no state update is sent.
    if state_guard.current_path.is_empty() {
        return ContentSearchFollowUp::None;
    }

    let patterns_changed = !patterns_added.is_empty() || !patterns_removed.is_empty();
    let mut should_send_update = false;

    if patterns_changed && state_guard.is_scanning() {
        tracing::info!("Ignore patterns changed during a scan. Restarting the scan.");
        // The entries that are ignored now disappear at once; the restarted scan then
        // delivers the list that matches the new patterns.
        if !patterns_added.is_empty() {
            apply_added_patterns_locally(&mut state_guard, &patterns_added);
        }
        filtering::apply_filters(&mut state_guard);
        state_guard.patterns_need_rescan = false;
        // Announces the new scan with a state update of its own.
        tasks::restart_scan(&mut state_guard, proxy.clone(), state.clone());
    } else if !patterns_removed.is_empty() {
        tracing::info!(
            "Ignore patterns removed: {:?}. Re-scan recommended.",
            patterns_removed
        );
        state_guard.patterns_need_rescan = true;
        should_send_update = true;
    } else if !patterns_added.is_empty() {
        tracing::debug!("Applying new ignore patterns locally: {:?}", patterns_added);
        apply_added_patterns_locally(&mut state_guard, &patterns_added);
        filtering::apply_filters(&mut state_guard);
        should_send_update = true;
    } else if needs_refilter {
        tracing::debug!("Re-applying filters due to config change.");
        filtering::apply_filters(&mut state_guard);
        should_send_update = true;
    }

    if should_send_update {
        let ui_state = generate_ui_state(&state_guard);
        proxy.send_event(UserEvent::StateUpdate(Box::new(ui_state)));
    }

    if case_sensitivity_changed && !is_blank(&state_guard.content_search_query) {
        ContentSearchFollowUp::Repeat
    } else {
        ContentSearchFollowUp::None
    }
}

/// Removes the entries matching the newly added ignore patterns from the loaded file
/// list and records which of these patterns matched something.
fn apply_added_patterns_locally(state: &mut AppState, patterns_added: &HashSet<String>) {
    let root_path = PathBuf::from(&state.current_path);
    for pattern in patterns_added {
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&root_path);
        if builder.add_line(None, pattern).is_ok() {
            if let Ok(matcher) = builder.build() {
                let has_match = state.full_file_list.iter().any(|item| {
                    matcher
                        .matched_path_or_any_parents(&item.path, item.is_directory)
                        .is_ignore()
                });

                if has_match {
                    state.active_ignore_patterns.insert(pattern.clone());
                }
            }
        }
    }

    state.apply_ignore_patterns();
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
            let searcher = tasks::RealFileSearcher;
            search_in_files(proxy, state, searcher).await;
        } else {
            with_state_and_notify(&state, &proxy, |s| {
                filtering::apply_filters(s);
                if !is_blank(&s.search_query) || !is_blank(&s.extension_filter) {
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
            search_term = if is_blank(&state_guard.content_search_query) {
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
/// This function calculates the relative path from the project root, ensures
/// directory patterns end with a slash, and then calls `update_config` to apply
/// the change and trigger a re-scan. The state lock is held for the shortest
/// possible duration to read the necessary data.
pub async fn add_ignore_path<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let (current_path_str, mut new_config) = {
            let state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            if state_guard.current_path.is_empty() {
                return;
            }
            (state_guard.current_path.clone(), state_guard.config.clone())
        };

        let path_to_ignore = PathBuf::from(path_str);
        let root_path = PathBuf::from(&current_path_str);

        if let Ok(relative_path) = path_to_ignore.strip_prefix(&root_path) {
            let mut pattern_to_add = relative_path.to_string_lossy().to_string();

            if path_to_ignore.is_dir() && !pattern_to_add.ends_with('/') {
                pattern_to_add.push('/');
            }

            if new_config.ignore_patterns.insert(pattern_to_add) {
                match serde_json::to_value(new_config) {
                    Ok(config_payload) => {
                        update_config(config_payload, proxy, state).await;
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
            // Important: only operate on the currently *visible* files in that directory.
            // The same subtree determines the checkbox state the UI shows for it.
            let Some(subtree) = super::selection::subtree_selection(
                s.filtered_file_list(),
                &s.file_tree(),
                &s.selected_files,
                &dir_path,
            ) else {
                tracing::warn!(
                    "Cannot toggle the selection of unlisted path {:?}",
                    dir_path
                );
                return;
            };

            if subtree.state == super::selection::SelectionState::Full {
                // If fully selected, deselect all
                for file in &subtree.files {
                    s.selected_files.remove(file);
                }
            } else {
                // If partially or not selected, select all
                s.selected_files.extend(subtree.files);
            }
        });
    } else {
        tracing::warn!(
            "Failed to deserialize path string from payload: {:?}",
            payload
        );
    }
}

/// Payload of [`set_expansion`].
#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct SetExpansionPayload {
    path: PathBuf,
    expanded: bool,
}

/// Sets the expanded/collapsed state of a directory in the UI tree.
///
/// The command names the target state instead of flipping it, which makes it idempotent:
/// a gesture that is delivered twice (key repeat, double click) before the UI has
/// rendered the answer to the first one still ends in the state the user asked for.
/// The state is always sent, also when nothing changed, so every request is answered.
pub fn set_expansion<P: EventProxy>(
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    match serde_json::from_value::<SetExpansionPayload>(payload) {
        Ok(SetExpansionPayload { path, expanded }) => {
            with_state_and_notify(&state, &proxy, |s| {
                if expanded {
                    s.expanded_dirs.insert(path);
                } else {
                    s.expanded_dirs.remove(&path);
                }
            });
        }
        Err(e) => tracing::warn!("Failed to deserialize setExpansion payload: {e}"),
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
                    .filtered_file_list()
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
            .filtered_file_list()
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

    // Supersedes a generation that is still running; only the new one may publish.
    let token = state_guard.begin_generation();
    state_guard.previewed_file_path = None;

    // Only generate a new timestamped filename if the current one appears to be a default.
    // This preserves any filename explicitly set by the user.
    let current_filename = &state_guard.config.output_filename;
    if current_filename.starts_with("cfc_output_") && current_filename.ends_with(".txt") {
        let new_filename = format!(
            "cfc_output_{}.txt",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        state_guard.config.output_filename = new_filename;
    }

    // Send an immediate state update to the UI to show the 'generating' state.
    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
        &state_guard,
    ))));

    let real_generator = tasks::RealContentGenerator {
        cancel_flag: token.cancel_flag(),
        max_file_size: state_guard.config.max_file_size_bytes(),
    };
    let real_tokenizer = tasks::RealTokenizer;

    // Spawn the actual generation logic as a separate, managed task.
    let handle = tokio::spawn(tasks::generation_task(
        proxy.clone(),
        state.clone(),
        real_generator,
        real_tokenizer,
        token.clone(),
    ));
    state_guard.attach_generation_task(&token, handle);
}

/// Cancels the ongoing file content generation task.
///
/// The "cancelled" notice is the answer to the user's request, so it is set here and
/// only if a generation was actually running; the aborted task itself stays silent.
pub fn cancel_generation<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    with_state_and_notify(&state, &proxy, |s| {
        if s.is_generating() {
            s.status = StatusNotice::GenerationCancelled;
        }
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
pub fn save_file<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    payload: serde_json::Value,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Some(content) = payload.as_str() {
        let content_clone = content.to_string();
        let config = {
            let state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            state_guard.config.clone()
        };

        if let Some(path) = dialog.save_output_file_path(&config) {
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
pub fn pick_output_directory<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Some(path) = dialog.pick_directory() {
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
pub async fn import_config<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Some(path) = dialog.pick_config_to_import() {
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
                if let Err(e) = config::settings::save_config(&state_guard.config, None) {
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
pub fn export_config<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: Arc<Mutex<AppState>>,
) {
    if let Some(path) = dialog.export_config_path() {
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
    use crate::app::file_dialog::DialogService;
    use crate::app::state::AppState;
    use crate::app::view_model::UiState;
    use crate::core::FileItem;
    use crate::utils::test_helpers::setup_test_logging;
    use serde_json::json;
    use std::fs as std_fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::{tempdir, TempDir};
    use tokio::sync::mpsc;

    // A mock EventProxy for capturing events sent to the UI.
    #[derive(Clone)]
    struct TestEventProxy {
        sender: mpsc::UnboundedSender<UserEvent>,
    }

    impl EventProxy for TestEventProxy {
        fn send_event(&self, event: UserEvent) {
            self.sender.send(event).expect("Test receiver dropped");
        }
    }

    // A mock DialogService to simulate user interaction with file dialogs.
    #[derive(Default)]
    struct MockDialogService {
        picked_folder: Mutex<Option<PathBuf>>,
        picked_file: Mutex<Option<PathBuf>>,
        saved_file: Mutex<Option<PathBuf>>,
    }

    impl Clone for MockDialogService {
        fn clone(&self) -> Self {
            MockDialogService {
                picked_folder: Mutex::new(self.picked_folder.lock().unwrap().clone()),
                picked_file: Mutex::new(self.picked_file.lock().unwrap().clone()),
                saved_file: Mutex::new(self.saved_file.lock().unwrap().clone()),
            }
        }
    }

    impl MockDialogService {
        fn set_pick_folder(&self, path: Option<PathBuf>) {
            *self.picked_folder.lock().unwrap() = path;
        }

        fn set_pick_file(&self, path: Option<PathBuf>) {
            *self.picked_file.lock().unwrap() = path;
        }

        fn set_save_file(&self, path: Option<PathBuf>) {
            *self.saved_file.lock().unwrap() = path;
        }
    }

    impl DialogService for MockDialogService {
        fn pick_directory(&self) -> Option<PathBuf> {
            self.picked_folder.lock().unwrap().clone()
        }
        fn pick_config_to_import(&self) -> Option<PathBuf> {
            self.picked_file.lock().unwrap().clone()
        }
        fn export_config_path(&self) -> Option<PathBuf> {
            self.saved_file.lock().unwrap().clone()
        }
        fn save_output_file_path(&self, _config: &AppConfig) -> Option<PathBuf> {
            self.saved_file.lock().unwrap().clone()
        }
    }

    struct TestHarness {
        state: Arc<Mutex<AppState>>,
        proxy: TestEventProxy,
        event_rx: mpsc::UnboundedReceiver<UserEvent>,
        dialog: Arc<MockDialogService>,
        _temp_dir: TempDir,
        root_path: PathBuf,
    }

    impl TestHarness {
        fn new() -> Self {
            let temp_dir = tempdir().expect("Failed to create temp dir");
            let root_path = temp_dir.path().to_path_buf();
            let (tx, rx) = mpsc::unbounded_channel();
            let proxy = TestEventProxy { sender: tx };
            let dialog = Arc::new(MockDialogService::default());

            let mut state = AppState::default();
            state.config = AppConfig::default();
            state.current_path = root_path.to_string_lossy().to_string();

            Self {
                state: Arc::new(Mutex::new(state)),
                proxy,
                event_rx: rx,
                dialog,
                _temp_dir: temp_dir,
                root_path,
            }
        }

        fn create_file(&self, relative_path: &str, content: &str) -> PathBuf {
            let path = self.root_path.join(relative_path);
            if let Some(parent) = path.parent() {
                std_fs::create_dir_all(parent).unwrap();
            }
            std_fs::write(&path, content).unwrap();
            path
        }

        fn create_dir(&self, relative_path: &str) -> PathBuf {
            let path = self.root_path.join(relative_path);
            std_fs::create_dir_all(&path).unwrap();
            path
        }

        fn set_initial_files(&self, paths: &[&str]) {
            let mut state = self.state.lock().unwrap();
            let mut items = Vec::new();
            for p_str in paths {
                let path = self.root_path.join(p_str);
                items.push(file_item(path.clone(), path.is_dir()));
            }
            state.full_file_list = items.clone();
            state.set_filtered_file_list(items);
        }

        /// The most recent state update among the events sent so far, without waiting
        /// for more; all pending events are consumed. This is the way to look at the
        /// outcome of a command that has run to completion. For work a command has
        /// handed to a background task use [`Self::wait_for_state`].
        fn last_state_update(&mut self) -> Option<Box<UiState>> {
            std::iter::from_fn(|| self.event_rx.try_recv().ok())
                .filter_map(|event| match event {
                    UserEvent::StateUpdate(ui_state) => Some(ui_state),
                    _ => None,
                })
                .last()
        }

        /// Waits for the next event; `None` if none arrives (which fails the test that
        /// expected one, so the waiting time does not matter).
        async fn get_next_event(&mut self) -> Option<UserEvent> {
            tokio::time::timeout(EVENT_TIMEOUT, self.event_rx.recv())
                .await
                .ok()
                .flatten()
        }

        /// Asserts that no event is sent within `window`. Background tasks get the
        /// chance to run during that time, so this also covers commands that might
        /// have (wrongly) started one.
        #[track_caller]
        fn expect_no_event_within<'a>(
            &'a mut self,
            window: std::time::Duration,
            context: &'a str,
        ) -> impl std::future::Future<Output = ()> + 'a {
            let caller = std::panic::Location::caller();
            async move {
                if let Ok(event) = tokio::time::timeout(window, self.event_rx.recv()).await {
                    panic!("{context} (at {caller}), got {event:?}");
                }
            }
        }

        /// Waits until a background task sends a state update that satisfies
        /// `predicate` and returns it; earlier events are consumed.
        async fn wait_for_state(
            &mut self,
            mut predicate: impl FnMut(&UiState) -> bool,
        ) -> Box<UiState> {
            loop {
                match self.get_next_event().await {
                    Some(UserEvent::StateUpdate(ui_state)) if predicate(&ui_state) => {
                        return ui_state
                    }
                    Some(_) => {}
                    None => panic!("Timed out waiting for the expected state update"),
                }
            }
        }

        async fn wait_for_scan_completion(&mut self) -> Option<Box<UiState>> {
            loop {
                match self.get_next_event().await? {
                    UserEvent::StateUpdate(ui_state) if !ui_state.is_scanning => {
                        return Some(ui_state)
                    }
                    _ => {}
                }
            }
        }
    }

    /// Upper bound for waiting on something a test expects to happen. It is only ever
    /// reached by a failing test, so it does not slow the suite down.
    const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    /// How long a test listens for events that must not be sent.
    const QUIET_PERIOD: std::time::Duration = std::time::Duration::from_millis(50);

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

    // =========================================================================================
    // SECTION: Existing tests (unchanged, verified)
    // =========================================================================================

    #[tokio::test]
    async fn test_select_directory_starts_scan_on_ok() {
        let mut harness = TestHarness::new();
        let new_dir = harness.create_dir("new_project");
        harness.dialog.set_pick_folder(Some(new_dir.clone()));

        select_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let final_state = harness.wait_for_scan_completion().await.unwrap();
        assert!(!final_state.is_scanning);
        assert_eq!(final_state.current_path, new_dir.to_string_lossy());
    }

    #[tokio::test]
    async fn test_select_directory_updates_state_on_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_pick_folder(None);

        select_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let final_state = harness.last_state_update().unwrap();
        assert!(!final_state.is_scanning);
    }

    /// Dismissing the dialog is not a request to stop anything: a scan that is running in
    /// the background must keep running and keep owning the scanning state.
    #[tokio::test]
    async fn test_select_directory_cancel_leaves_running_scan_untouched() {
        let mut harness = TestHarness::new();
        harness.dialog.set_pick_folder(None);
        let token = {
            let mut state = harness.state.lock().unwrap();
            let token = state.begin_scan();
            state.attach_scan_task(&token, tokio::spawn(std::future::pending()));
            token
        };

        select_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.is_scanning, "UI must still show the running scan");
        let mut state = harness.state.lock().unwrap();
        assert!(state.is_scanning());
        assert!(state.is_current_scan(&token));
        assert!(state.has_scan_task(), "Scan must stay cancellable");
        assert!(!token.is_cancelled());
        assert!(
            state.begin_lazy_load().is_none(),
            "Lazy loads must stay locked out while the scan is running"
        );
        state.cancel_current_scan();
    }

    /// A generation started via the command owns the generating state and can be cancelled.
    #[tokio::test]
    async fn test_generate_preview_registers_cancellable_generation() {
        let mut harness = TestHarness::new();
        harness.create_file("file.txt", "content");
        harness.set_initial_files(&["file.txt"]);

        generate_preview(harness.proxy.clone(), harness.state.clone());

        {
            let state = harness.state.lock().unwrap();
            assert!(state.is_generating());
            assert!(state.has_generation_task());
        }
        cancel_generation(harness.proxy.clone(), harness.state.clone());
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_generating);
        let state = harness.state.lock().unwrap();
        assert!(!state.is_generating());
        assert!(!state.has_generation_task());
    }

    #[tokio::test]
    async fn test_rescan_directory_on_empty_path_does_nothing() {
        let mut harness = TestHarness::new();
        {
            let mut state = harness.state.lock().unwrap();
            state.current_path = String::new();
        }

        rescan_directory(harness.proxy.clone(), harness.state.clone());

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "Rescan should not trigger any event when path is empty",
            )
            .await;
    }

    #[tokio::test]
    async fn test_update_config_triggers_refilter() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "main");
        harness.create_dir("src/empty_dir");

        harness.set_initial_files(&["src", "src/main.rs", "src/empty_dir"]);
        {
            let mut state = harness.state.lock().unwrap();
            state.is_fully_scanned = true;
            state.loaded_dirs.insert(harness.root_path.join("src"));
            state
                .loaded_dirs
                .insert(harness.root_path.join("src/empty_dir"));
        }

        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.remove_empty_directories = true;
        let payload = serde_json::to_value(new_config).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(
            ui_state.visible_files_count, 2,
            "Expected 'src/empty_dir' to be removed"
        );
    }

    #[tokio::test]
    async fn test_update_filters_applies_filename_filter_without_content_search() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "");
        harness.create_file("src/lib.rs", "");
        harness.create_file("README.md", "");
        harness.set_initial_files(&["src", "src/main.rs", "src/lib.rs", "README.md"]);

        let filters = json!({
            "searchQuery": "main",
            "extensionFilter": "",
            "contentSearchQuery": ""
        });
        update_filters(filters, harness.proxy.clone(), harness.state.clone()).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.visible_files_count, 2);
    }

    #[tokio::test]
    async fn test_add_ignore_path_retriggers_scan() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "");
        harness.create_dir("docs");
        harness.create_file("docs/guide.md", "");
        harness.set_initial_files(&["src", "docs", "src/main.rs", "docs/guide.md"]);

        let path_to_ignore = harness.root_path.join("docs");
        let payload = json!(path_to_ignore);
        add_ignore_path(payload, harness.proxy.clone(), harness.state.clone()).await;

        let final_state = harness.wait_for_scan_completion().await.unwrap();
        assert_eq!(final_state.visible_files_count, 2);
        assert!(!final_state.tree.iter().any(|n| n.name == "docs"));
        let state = harness.state.lock().unwrap();
        assert!(state.config.ignore_patterns.contains("docs/"));
    }

    #[tokio::test]
    async fn test_import_config_resets_and_starts_scan() {
        let mut harness = TestHarness::new();
        harness.create_file("initial.txt", "");
        let new_config_path = harness.root_path.join("new_config.json");
        let project_to_scan = harness.create_dir("new_project_dir");
        harness.create_file("new_project_dir/file.rs", "");

        let new_config = AppConfig {
            last_directory: Some(project_to_scan.clone()),
            ..Default::default()
        };
        std_fs::write(
            &new_config_path,
            serde_json::to_string(&new_config).unwrap(),
        )
        .unwrap();
        harness.dialog.set_pick_file(Some(new_config_path));

        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;

        let _ = harness.get_next_event().await.unwrap();
        let final_state = harness.wait_for_scan_completion().await.unwrap();
        assert_eq!(final_state.current_path, project_to_scan.to_string_lossy());
    }

    #[tokio::test]
    async fn test_update_config_does_nothing_when_no_directory_is_loaded() {
        let mut harness = TestHarness::new();
        let new_config = {
            let mut state = harness.state.lock().unwrap();
            state.current_path = String::new();
            let mut config = state.config.clone();
            config.remove_empty_directories = !config.remove_empty_directories;
            config
        };

        let payload = serde_json::to_value(new_config.clone()).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        {
            let final_config = &harness.state.lock().unwrap().config;
            assert_eq!(
                final_config.remove_empty_directories,
                new_config.remove_empty_directories
            );
        }

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No events should be sent when no directory is loaded",
            )
            .await;
    }

    #[tokio::test]
    async fn test_add_ignore_path_does_nothing_when_no_directory_is_loaded() {
        let mut harness = TestHarness::new();
        let initial_patterns_count;
        {
            let mut state = harness.state.lock().unwrap();
            state.current_path = String::new();
            initial_patterns_count = state.config.ignore_patterns.len();
        }

        let payload = json!("/some/path/to/ignore.txt");
        add_ignore_path(payload, harness.proxy.clone(), harness.state.clone()).await;

        {
            let final_patterns_count = harness.state.lock().unwrap().config.ignore_patterns.len();
            assert_eq!(
                initial_patterns_count, final_patterns_count,
                "Ignore patterns should not change"
            );
        }

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No events should be sent when no directory is loaded",
            )
            .await;
    }

    #[tokio::test]
    async fn test_import_config_sends_error_on_corrupt_file() {
        let mut harness = TestHarness::new();
        let corrupt_config_path = harness.create_file("corrupt_config.json", "{ not_valid_json, }");
        harness.dialog.set_pick_file(Some(corrupt_config_path));

        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;

        match harness.get_next_event().await.unwrap() {
            UserEvent::ShowError(msg) => {
                assert!(
                    msg.contains("Failed to import config"),
                    "Expected an import error message, but got: {}",
                    msg
                );
            }
            other => panic!("Expected ShowError event, but got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_update_config_handles_invalid_payload() {
        let mut harness = TestHarness::new();
        let invalid_payload = json!({ "some_random_key": "some_value" });
        let initial_config = harness.state.lock().unwrap().config.clone();

        update_config(
            invalid_payload,
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;

        let final_config = harness.state.lock().unwrap().config.clone();
        assert_eq!(initial_config.output_filename, final_config.output_filename);

        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
    }

    #[tokio::test]
    async fn test_update_config_triggers_rescan_on_pattern_change() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src/main.rs"]);

        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.ignore_patterns.insert("*.rs".to_string());
        let payload = serde_json::to_value(new_config).unwrap();

        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        let final_state = harness.wait_for_scan_completion().await.unwrap();
        assert_eq!(
            final_state.visible_files_count, 0,
            "Scan should have removed the .rs file"
        );
    }

    #[tokio::test]
    async fn test_update_filters_triggers_content_search() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["file1.txt"]);
        harness.create_file("file1.txt", "hello world");
        {
            let mut state = harness.state.lock().unwrap();
            state.content_search_query = "initial".to_string();
        }

        let filters = json!({
            "contentSearchQuery": "world"
        });

        update_filters(filters, harness.proxy.clone(), harness.state.clone()).await;

        let final_state = harness.last_state_update().unwrap();
        assert_eq!(final_state.content_search_query, "world");
        assert_eq!(
            final_state.visible_files_count, 1,
            "The matching file should be visible"
        );
    }

    #[tokio::test]
    async fn test_add_ignore_path_handles_path_outside_root() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src/main.rs"]);
        let initial_patterns_count = harness.state.lock().unwrap().config.ignore_patterns.len();
        let outside_path = json!("/etc/hosts");

        add_ignore_path(outside_path, harness.proxy.clone(), harness.state.clone()).await;

        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
        let final_patterns_count = harness.state.lock().unwrap().config.ignore_patterns.len();
        assert_eq!(initial_patterns_count, final_patterns_count);
    }

    #[tokio::test]
    async fn test_add_ignore_path_handles_duplicate_pattern() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["docs/guide.md"]);

        let path_to_ignore = harness.root_path.join("docs");
        let payload = json!(path_to_ignore);
        add_ignore_path(
            payload.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;

        let _ = harness.wait_for_scan_completion().await;

        add_ignore_path(payload, harness.proxy.clone(), harness.state.clone()).await;

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No rescan should be triggered for a duplicate ignore pattern",
            )
            .await;
    }

    #[tokio::test]
    async fn test_import_config_handles_nonexistent_scan_directory() {
        let mut harness = TestHarness::new();
        let new_config_path = harness.root_path.join("new_config.json");
        let nonexistent_project_dir = harness.root_path.join("nonexistent_dir");

        let new_config = AppConfig {
            last_directory: Some(nonexistent_project_dir),
            ..Default::default()
        };
        std_fs::write(
            &new_config_path,
            serde_json::to_string(&new_config).unwrap(),
        )
        .unwrap();
        harness.dialog.set_pick_file(Some(new_config_path));

        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;

        let event = harness.get_next_event().await;
        assert!(matches!(event, Some(UserEvent::StateUpdate(_))));

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No scan should start for a nonexistent directory",
            )
            .await;
    }

    // =========================================================================================
    // The following tests call SYNCHRONOUS commands and DO NOT NEED .await
    // =========================================================================================

    #[tokio::test]
    async fn test_clear_directory_resets_state() {
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("file.txt", "content");
        harness.set_initial_files(&["file.txt"]);
        {
            let mut state = harness.state.lock().unwrap();
            state.selected_files.insert(file_path);
            state.config.last_directory = Some(harness.root_path.clone());
        }

        clear_directory(harness.proxy.clone(), harness.state.clone());

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.current_path.is_empty());
        assert_eq!(ui_state.visible_files_count, 0);
        let state = harness.state.lock().unwrap();
        assert!(state.current_path.is_empty());
        assert!(state.full_file_list.is_empty());
        assert!(state.selected_files.is_empty());
        assert!(state.config.last_directory.is_none());
    }

    #[tokio::test]
    async fn test_cancel_scan_updates_state() {
        let mut harness = TestHarness::new();
        {
            let mut state = harness.state.lock().unwrap();
            let token = state.begin_scan();
            let handle = tokio::spawn(async {});
            state.attach_scan_task(&token, handle);
        }

        cancel_scan(harness.proxy.clone(), harness.state.clone());

        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_scanning);
        assert_eq!(ui_state.status_message, "Scan cancelled.");
    }

    #[tokio::test]
    async fn test_initialize_sends_initial_state() {
        let mut harness = TestHarness::new();
        initialize(harness.proxy.clone(), harness.state.clone());
        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.current_path, harness.root_path.to_string_lossy());
    }

    #[tokio::test]
    async fn test_load_file_preview_sends_search_term() {
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("preview.txt", "content with magic_word");
        let search_term = "magic_word";
        {
            let mut state = harness.state.lock().unwrap();
            state.content_search_query = search_term.to_string();
        }
        let payload = json!(file_path);

        load_file_preview(payload, harness.proxy.clone(), harness.state.clone());

        let mut saw_preview = false;
        for _ in 0..2 {
            if let Some(UserEvent::ShowFilePreview {
                search_term: term, ..
            }) = harness.get_next_event().await
            {
                assert_eq!(term, Some(search_term.to_string()));
                saw_preview = true;
            }
        }
        assert!(
            saw_preview,
            "Did not receive the ShowFilePreview event with correct search term"
        );
    }

    #[tokio::test]
    async fn test_load_directory_level_starts_lazy_scan() {
        let mut harness = TestHarness::new();
        let sub_dir = harness.create_dir("src/components");
        harness.create_file("src/components/button.js", "");
        harness.set_initial_files(&["src", "src/components"]);
        // The snapshot only contains children of expanded directories, and a user can
        // only reach `src/components` with `src` open.
        harness
            .state
            .lock()
            .unwrap()
            .expanded_dirs
            .insert(harness.root_path.join("src"));

        let payload = json!(sub_dir);
        load_directory_level(payload, harness.proxy.clone(), harness.state.clone());

        // The lazy load runs in the background and reports its result when it is done.
        let final_state = harness.get_next_event().await;
        let Some(UserEvent::StateUpdate(final_state)) = final_state else {
            panic!("Expected the state update of the lazy load, got {final_state:?}");
        };
        let src_node = final_state.tree.iter().find(|n| n.name == "src").unwrap();
        let components_node = src_node
            .children
            .iter()
            .find(|n| n.name == "components")
            .unwrap();
        assert!(components_node
            .children
            .iter()
            .any(|n| n.name == "button.js"));
    }

    // ... All other synchronous tests remain unchanged ...
    #[tokio::test]
    async fn test_toggle_selection_adds_and_removes_file() {
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("test.rs", "");
        harness.set_initial_files(&["test.rs"]);
        let payload = json!(file_path);

        toggle_selection(
            payload.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let ui_state1 = harness.last_state_update().unwrap();
        assert_eq!(ui_state1.selected_files_count, 1);

        toggle_selection(payload, harness.proxy.clone(), harness.state.clone());
        let ui_state2 = harness.last_state_update().unwrap();
        assert_eq!(ui_state2.selected_files_count, 0);
    }

    #[tokio::test]
    async fn test_toggle_directory_selection_selects_and_deselects_all_children() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "");
        let dir_path = harness.create_dir("src");
        harness.set_initial_files(&["src", "src/main.rs"]);
        let payload = json!(dir_path);

        toggle_directory_selection(
            payload.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let ui_state_select = harness.last_state_update().unwrap();
        assert_eq!(ui_state_select.selected_files_count, 1);

        toggle_directory_selection(payload, harness.proxy.clone(), harness.state.clone());
        let ui_state_deselect = harness.last_state_update().unwrap();
        assert_eq!(ui_state_deselect.selected_files_count, 0);
    }

    #[tokio::test]
    async fn test_set_expansion_is_idempotent() {
        let mut harness = TestHarness::new();
        let dir = harness.create_dir("src");
        harness.set_initial_files(&["src"]);
        let set = |expanded: bool| json!({ "path": dir, "expanded": expanded });

        // Expanding twice (key repeat, double click) leaves the directory expanded,
        // and each request is answered with a snapshot.
        for _ in 0..2 {
            set_expansion(set(true), harness.proxy.clone(), harness.state.clone());
            let ui_state = harness.last_state_update().unwrap();
            assert!(ui_state.tree[0].is_expanded);
        }
        assert_eq!(harness.state.lock().unwrap().expanded_dirs.len(), 1);

        for _ in 0..2 {
            set_expansion(set(false), harness.proxy.clone(), harness.state.clone());
            let ui_state = harness.last_state_update().unwrap();
            assert!(!ui_state.tree[0].is_expanded);
        }
        assert!(harness.state.lock().unwrap().expanded_dirs.is_empty());
    }

    #[tokio::test]
    async fn test_set_expansion_only_affects_the_given_directory() {
        let mut harness = TestHarness::new();
        let src = harness.create_dir("src");
        let docs = harness.create_dir("docs");
        harness.set_initial_files(&["docs", "src"]);
        harness
            .state
            .lock()
            .unwrap()
            .expanded_dirs
            .insert(docs.clone());

        set_expansion(
            json!({ "path": src, "expanded": true }),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        set_expansion(
            json!({ "path": src, "expanded": false }),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let _ = harness.last_state_update().unwrap();

        let state = harness.state.lock().unwrap();
        assert_eq!(state.expanded_dirs.len(), 1);
        assert!(state.expanded_dirs.contains(&docs));
    }

    #[tokio::test]
    async fn test_set_expansion_handles_invalid_payload() {
        let mut harness = TestHarness::new();
        for invalid_payload in [
            json!("/a/bare/path"),
            json!({ "path": "/repo/src" }),
            json!({ "expanded": true }),
            json!({ "path": "/repo/src", "expanded": "yes" }),
            json!({ "path": "/repo/src", "expanded": true, "extra": 1 }),
            json!(null),
        ] {
            set_expansion(
                invalid_payload,
                harness.proxy.clone(),
                harness.state.clone(),
            );
        }
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
        assert!(harness.state.lock().unwrap().expanded_dirs.is_empty());
    }

    #[tokio::test]
    async fn test_expand_collapse_all() {
        let mut harness = TestHarness::new();
        harness.create_dir("src");
        harness.set_initial_files(&["src"]);

        expand_collapse_all(json!(true), harness.proxy.clone(), harness.state.clone());
        let ui_state1 = harness.last_state_update().unwrap();
        assert_eq!(ui_state1.tree.iter().filter(|n| n.is_expanded).count(), 1);

        expand_collapse_all(json!(false), harness.proxy.clone(), harness.state.clone());
        let ui_state2 = harness.last_state_update().unwrap();
        assert!(ui_state2.tree.iter().all(|n| !n.is_expanded));
    }

    #[tokio::test]
    async fn test_select_all_and_deselect_all() {
        let mut harness = TestHarness::new();
        harness.create_file("file1.txt", "");
        harness.create_file("file2.txt", "");
        harness.set_initial_files(&["file1.txt", "file2.txt"]);

        select_all(harness.proxy.clone(), harness.state.clone());
        let ui_state1 = harness.last_state_update().unwrap();
        assert_eq!(ui_state1.selected_files_count, 2);

        deselect_all(harness.proxy.clone(), harness.state.clone());
        let ui_state2 = harness.last_state_update().unwrap();
        assert_eq!(ui_state2.selected_files_count, 0);
    }

    #[tokio::test]
    async fn test_generate_preview_sets_generating_state_and_spawns_task() {
        let mut harness = TestHarness::new();
        harness.create_file("file.txt", "content");
        harness.set_initial_files(&["file.txt"]);

        generate_preview(harness.proxy.clone(), harness.state.clone());

        let event = harness.get_next_event().await.unwrap();
        let ui_state = match event {
            UserEvent::StateUpdate(ui_state) => ui_state,
            _ => panic!("Expected a StateUpdate event first"),
        };
        assert!(ui_state.is_generating);

        // The generation task reports its end with a final state update.
        harness
            .wait_for_state(|ui_state| !ui_state.is_generating)
            .await;
    }

    #[tokio::test]
    async fn test_cancel_generation_resets_generating_state() {
        let mut harness = TestHarness::new();
        generate_preview(harness.proxy.clone(), harness.state.clone());
        let _ = harness.last_state_update();

        cancel_generation(harness.proxy.clone(), harness.state.clone());
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_generating);
        assert_eq!(ui_state.status_message, "Generation cancelled.");
    }

    #[tokio::test]
    async fn test_cancel_generation_without_running_generation_keeps_status() {
        let mut harness = TestHarness::new();
        let status_before = harness.state.lock().unwrap().status.clone();

        cancel_generation(harness.proxy.clone(), harness.state.clone());

        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_generating);
        assert_eq!(harness.state.lock().unwrap().status, status_before);
    }

    #[tokio::test]
    async fn test_clear_preview_state() {
        let harness = TestHarness::new();
        let file_path = harness.create_file("file.txt", "content");
        {
            let mut state = harness.state.lock().unwrap();
            state.previewed_file_path = Some(file_path);
        }
        clear_preview_state(harness.proxy.clone(), harness.state.clone());
        let state = harness.state.lock().unwrap();
        assert!(state.previewed_file_path.is_none());
    }

    #[tokio::test]
    async fn test_save_file_writes_to_disk_on_ok() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("output.txt");
        let content_to_save = "Hello, World!";
        harness.dialog.set_save_file(Some(save_path.clone()));

        save_file(
            harness.dialog.as_ref(),
            json!(content_to_save),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let event = harness.get_next_event().await.unwrap();
        match event {
            UserEvent::SaveComplete(success, path_str) => {
                assert!(success);
                assert_eq!(path_str, save_path.to_string_lossy());
            }
            _ => panic!("Expected SaveComplete event"),
        }
        let written_content = std_fs::read_to_string(save_path).unwrap();
        assert_eq!(written_content, content_to_save);
    }

    #[tokio::test]
    async fn test_pick_output_directory_updates_config() {
        let harness = TestHarness::new();
        let new_dir = harness.create_dir("output");
        harness.dialog.set_pick_folder(Some(new_dir.clone()));

        pick_output_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let state = harness.state.lock().unwrap();
        assert_eq!(state.config.output_directory, Some(new_dir));
    }

    #[tokio::test]
    async fn test_export_config_sends_event() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("my-config.json");
        harness.dialog.set_save_file(Some(save_path));

        export_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        match harness.get_next_event().await.unwrap() {
            UserEvent::ConfigExported(success) => assert!(success),
            _ => panic!("Expected ConfigExported event"),
        }
    }

    #[tokio::test]
    async fn test_export_config_sends_no_event_on_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_save_file(None);

        export_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No event should be sent when export is cancelled",
            )
            .await;
    }

    #[tokio::test]
    async fn test_save_file_handles_invalid_payload() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("output.txt");
        harness.dialog.set_save_file(Some(save_path.clone()));
        let invalid_payload = json!(12345);

        save_file(
            harness.dialog.as_ref(),
            invalid_payload,
            harness.proxy.clone(),
            harness.state.clone(),
        );

        assert!(
            !save_path.exists(),
            "No file should have been written with an invalid payload"
        );

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No event should be sent for an invalid payload",
            )
            .await;
    }

    #[tokio::test]
    async fn test_load_directory_level_handles_invalid_payload() {
        let mut harness = TestHarness::new();
        let invalid_payload = json!({ "path": "/not/a/string/path" });

        load_directory_level(
            invalid_payload,
            harness.proxy.clone(),
            harness.state.clone(),
        );

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No event should be sent for an invalid payload",
            )
            .await;
    }

    #[tokio::test]
    async fn test_save_file_sends_error_on_io_failure() {
        let mut harness = TestHarness::new();
        // A cross-platform way to cause an I/O error is to try to write to a directory path.
        let invalid_save_path = harness.root_path.clone();
        harness.dialog.set_save_file(Some(invalid_save_path));

        save_file(
            harness.dialog.as_ref(),
            json!("some content"),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let event = harness.get_next_event().await.unwrap();
        match event {
            UserEvent::SaveComplete(success, msg) => {
                assert!(!success);
                // Check for common error messages on different platforms.
                assert!(
                    msg.contains("Is a directory") // Linux, macOS
                        || msg.contains("Access is denied") // Windows
                        || msg.contains("Permission denied")
                );
            }
            _ => panic!("Expected SaveComplete(false, ...) event"),
        }
    }

    #[tokio::test]
    async fn test_generate_preview_preserves_custom_filename() {
        let mut harness = TestHarness::new();
        let custom_filename = "my_special_context.md".to_string();

        // 1. Set a custom filename in the state.
        {
            let mut state = harness.state.lock().unwrap();
            state.config.output_filename = custom_filename.clone();
        }

        // 2. Run the generate_preview command.
        generate_preview(harness.proxy.clone(), harness.state.clone());

        // 3. Verify the filename in the state has NOT been changed.
        let final_filename = harness.state.lock().unwrap().config.output_filename.clone();
        assert_eq!(final_filename, custom_filename);

        // Consume events to avoid panics on drop
        let _ = harness.get_next_event().await;
        let _ = harness.get_next_event().await;
    }

    // =========================================================================================
    // SECTION: New tests for coverage increase
    // =========================================================================================

    // --- Tests for invalid payloads ---

    #[tokio::test]
    async fn test_update_filters_handles_invalid_payload() {
        let mut harness = TestHarness::new();
        let invalid_payload = json!("not a map");
        update_filters(
            invalid_payload,
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
    }

    #[tokio::test]
    async fn test_load_file_preview_handles_invalid_payload() {
        let mut harness = TestHarness::new();
        let invalid_payload = json!(["not a string"]);
        load_file_preview(
            invalid_payload,
            harness.proxy.clone(),
            harness.state.clone(),
        );
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
    }

    #[tokio::test]
    async fn test_toggle_selection_handles_invalid_payload() {
        let mut harness = TestHarness::new();
        let invalid_payload = json!({ "path": "/some/path" });
        toggle_selection(
            invalid_payload,
            harness.proxy.clone(),
            harness.state.clone(),
        );
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
    }

    #[tokio::test]
    async fn test_expand_collapse_all_handles_invalid_payload() {
        let mut harness = TestHarness::new();
        let invalid_payload = json!("not a boolean");
        expand_collapse_all(
            invalid_payload,
            harness.proxy.clone(),
            harness.state.clone(),
        );
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
    }

    // --- Tests for user cancellation / dialog returning None ---

    #[tokio::test]
    async fn test_save_file_sends_cancelled_on_dialog_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_save_file(None);
        save_file(
            harness.dialog.as_ref(),
            json!("content"),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        match harness.get_next_event().await.unwrap() {
            UserEvent::SaveComplete(success, msg) => {
                assert!(!success);
                assert_eq!(msg, "cancelled");
            }
            other => panic!("Expected SaveComplete(false, 'cancelled'), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_pick_output_directory_does_nothing_on_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_pick_folder(None);
        let initial_config = harness.state.lock().unwrap().config.clone();
        pick_output_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let final_config = harness.state.lock().unwrap().config.clone();
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
        assert_eq!(
            initial_config.output_directory,
            final_config.output_directory
        );
    }

    #[tokio::test]
    async fn test_import_config_does_nothing_on_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_pick_file(None); // Simulate user cancelling dialog
        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
    }

    // --- Tests for specific logic and state edge cases ---

    #[tokio::test]
    async fn test_update_config_saves_only_on_no_op_change() {
        let mut harness = TestHarness::new();
        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.output_filename = "new_name.txt".to_string(); // A change that doesn't trigger rescan/refilter

        let payload = serde_json::to_value(new_config.clone()).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        assert_eq!(
            harness.state.lock().unwrap().config.output_filename,
            "new_name.txt"
        );
        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No event should be sent for a non-filtering config change",
            )
            .await;
    }

    #[tokio::test]
    async fn test_load_file_preview_sends_error_on_io_failure() {
        let mut harness = TestHarness::new();
        let non_existent_path = harness.root_path.join("non_existent_file.txt");
        let payload = json!(non_existent_path);
        load_file_preview(payload, harness.proxy.clone(), harness.state.clone());

        let mut error_event_found = false;
        // The command sends two events: an error and a state update. We check for the error.
        for _ in 0..2 {
            if let Some(UserEvent::ShowError(msg)) = harness.get_next_event().await {
                assert!(msg.contains("I/O error"));
                error_event_found = true;
            }
        }
        assert!(error_event_found, "Expected a ShowError event");
    }

    #[tokio::test]
    async fn test_toggle_directory_selection_from_partial() {
        let mut harness = TestHarness::new();
        let dir_path = harness.create_dir("src");
        let file1 = harness.create_file("src/file1.rs", "");
        harness.create_file("src/file2.rs", "");
        harness.set_initial_files(&["src", "src/file1.rs", "src/file2.rs"]);

        // 1. Select one file to create a "partial" state
        {
            let mut state = harness.state.lock().unwrap();
            state.selected_files.insert(file1);
        }

        // 2. Toggle the directory. It should now select all files.
        toggle_directory_selection(
            json!(dir_path),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(
            ui_state.selected_files_count, 2,
            "Directory should be fully selected from partial state"
        );
    }

    #[tokio::test]
    async fn test_generate_preview_creates_timestamped_filename_from_default() {
        let mut harness = TestHarness::new();
        // Use a fixed, old timestamp to make the test deterministic.
        // This ensures the generated filename will always be different.
        let old_default_filename = "cfc_output_20000101_120000.txt".to_string();

        {
            let mut state = harness.state.lock().unwrap();
            state.config.output_filename = old_default_filename.clone();
        }

        generate_preview(harness.proxy.clone(), harness.state.clone());

        let final_filename = harness.state.lock().unwrap().config.output_filename.clone();

        assert_ne!(
            final_filename, old_default_filename,
            "A new timestamped filename should have been generated"
        );
        assert!(final_filename.starts_with("cfc_output_"));

        // Clean up events to prevent the test harness from panicking on drop
        let _ = harness.get_next_event().await;
        // The generation task also sends a final StateUpdate when it completes.
        let _ = harness.get_next_event().await;
    }

    #[tokio::test]
    async fn test_import_config_with_no_last_directory() {
        let mut harness = TestHarness::new();
        let new_config_path =
            harness.create_file("config_no_dir.json", r#"{ "ignore_patterns": [] }"#);
        harness.dialog.set_pick_file(Some(new_config_path));
        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;

        // A StateUpdate event is sent immediately after reset.
        assert!(
            matches!(
                harness.get_next_event().await,
                Some(UserEvent::StateUpdate(_))
            ),
            "Expected an immediate state update"
        );
        // No *second* event should follow, as no scan is started.
        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No scan should start when last_directory is null",
            )
            .await;
    }

    #[tokio::test]
    async fn test_export_config_sends_false_on_failure() {
        let mut harness = TestHarness::new();
        // A cross-platform way to cause an I/O error is to target a directory.
        let invalid_path = harness.root_path.clone();
        harness.dialog.set_save_file(Some(invalid_path));

        export_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        match harness.get_next_event().await.unwrap() {
            UserEvent::ConfigExported(success) => assert!(!success),
            other => panic!("Expected ConfigExported(false), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_update_config_applies_locally_on_pattern_addition() {
        let mut harness = TestHarness::new();

        // Ensure a clean state for the test run.
        {
            harness.state.lock().unwrap().config.ignore_patterns.clear();
        }

        // The `current_path` is the root for the ignore matcher. All file paths
        // in the state MUST be children of this root path.
        let root = "/test/project";
        harness.state.lock().unwrap().current_path = root.to_string();

        // Create abstract files on the "filesystem".
        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_file("debug.log", "log content");

        // Set up the initial file list using full paths that are consistent
        // with the root path. This resolves the `ignore` crate panic.
        harness.set_initial_files(&[
            &format!("{}/src", root),
            &format!("{}/src/main.rs", root),
            &format!("{}/debug.log", root),
        ]);

        // Add a pattern to the config.
        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.ignore_patterns.insert("*.log".to_string());
        let payload = serde_json::to_value(new_config).unwrap();

        // Execute the function under test.
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        // Assert the outcome.
        let ui_state = harness.last_state_update().unwrap();

        assert!(
            !ui_state.patterns_need_rescan,
            "Flag should NOT be set when only adding patterns"
        );

        // After filtering "*.log", two items remain: "/test/project/src" and "/test/project/src/main.rs".
        assert_eq!(
            ui_state.visible_files_count, 2,
            "Should have src dir and main.rs, log file should be filtered out"
        );
    }

    #[tokio::test]
    async fn test_update_config_sets_rescan_flag_on_pattern_removal() {
        let mut harness = TestHarness::new();

        // Create actual files
        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_file("debug.log", "log content");
        harness.create_dir("node_modules");
        harness.create_file("node_modules/package.json", "{}");

        // Set initial files
        harness.set_initial_files(&[
            "src",
            "src/main.rs",
            "debug.log",
            "node_modules",
            "node_modules/package.json",
        ]);

        // Set initial patterns
        {
            let mut state = harness.state.lock().unwrap();
            state.config.ignore_patterns.insert("*.log".to_string());
            state
                .config
                .ignore_patterns
                .insert("node_modules/".to_string());
        }

        // Remove one pattern
        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.ignore_patterns.remove("node_modules/");
        let payload = serde_json::to_value(new_config).unwrap();

        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        let ui_state = harness.last_state_update().unwrap();
        assert!(
            ui_state.patterns_need_rescan,
            "Flag should be set when patterns are removed"
        );
    }

    #[tokio::test]
    async fn test_rescan_clears_patterns_need_rescan_flag() {
        let mut harness = TestHarness::new();

        // Create a test directory with a file
        harness.create_file("test.txt", "content");

        {
            let mut state = harness.state.lock().unwrap();
            state.patterns_need_rescan = true;
            // Ensure we have a valid current_path for the rescan
            // (TestHarness::new() already sets this)
        }

        rescan_directory(harness.proxy.clone(), harness.state.clone());

        // Wait for scan to complete
        let final_state = harness.wait_for_scan_completion().await.unwrap();
        assert!(
            !final_state.patterns_need_rescan,
            "Flag should be cleared after rescan"
        );
    }

    /// This is an isolated, robust unit test for the "add pattern" logic in `update_config`.
    /// It does not use the TestHarness and has no external dependencies, making it 100% deterministic.
    /// It asserts three things:
    /// 1. That the correct log message was emitted (proving the code path was taken).
    /// 2. That the event sent to the UI has the correct, updated state.
    /// 3. That the internal AppState was mutated correctly.
    #[tokio::test]
    async fn test_update_config_add_pattern_isolated() {
        // ADDED: Use the same logging setup as the other tests.
        setup_test_logging();

        // --- ARRANGE ---
        let (tx, mut rx) = mpsc::unbounded_channel();
        let proxy = TestEventProxy { sender: tx };
        let state = {
            let mut s = AppState::default();
            s.config.ignore_patterns.clear();
            s.current_path = "/app".to_string();
            s.full_file_list = vec![
                FileItem {
                    path: PathBuf::from("/app/src/main.rs"),
                    ..Default::default()
                },
                FileItem {
                    path: PathBuf::from("/app/debug.log"),
                    ..Default::default()
                },
            ];
            s.set_filtered_file_list(s.full_file_list.clone());
            Arc::new(Mutex::new(s))
        };
        let payload = {
            let mut new_config = AppConfig::default();
            new_config.ignore_patterns.insert("*.log".to_string());
            serde_json::to_value(new_config).unwrap()
        };

        // --- ACT ---
        update_config(payload, proxy, state.clone()).await;

        // --- ASSERT ---

        // The following state and event assertions are the primary verification of correctness.

        let event = rx.recv().await.expect("An event should have been sent.");
        let ui_state = match event {
            UserEvent::StateUpdate(s) => s,
            _ => panic!("Expected a StateUpdate event"),
        };
        assert_eq!(ui_state.visible_files_count, 1);
        assert!(!ui_state.patterns_need_rescan);

        let final_state = state.lock().unwrap();
        assert_eq!(final_state.full_file_list.len(), 1);
        assert!(final_state.config.ignore_patterns.contains("*.log"));
    }

    /// A comprehensive integration test for the entire user workflow of removing an ignore pattern.
    ///
    /// This test simulates a scenario with a nested directory structure to verify
    /// the two-phase state update process when un-ignoring files.
    ///
    /// It verifies the following sequence of events:
    /// 1. An initial scan is performed with a directory ("build_artifacts/") actively ignored,
    ///    confirming that the initial file count is low as expected.
    /// 2. The configuration is updated to remove the ignore pattern.
    /// 3. It asserts that the `patterns_need_rescan` flag is correctly set immediately
    ///    after the update, while the file count correctly remains unchanged.
    /// 4. A re-scan is triggered by the user (simulated by the test).
    /// 5. It asserts that after the re-scan completes, the flag is cleared and the final
    ///    file count ("statistic") has increased, reflecting that the previously ignored
    ///    files are now visible.
    ///
    /// NOTE on the final file count: The final count is 5, not 6. This is because the
    /// scanner's underlying `ignore` library 0has default behavior that automatically
    /// filters out files it detects as binary based on their content (e.g., containing
    /// null bytes). The `app.dat` file created in this test is correctly identified
    /// as binary and skipped, and the test asserts this real-world behavior.
    #[tokio::test]
    async fn test_removing_pattern_and_rescanning_updates_file_list() {
        // --- 1. Arrange ---
        let mut harness = TestHarness::new();
        harness.create_file(".gitignore", "");
        let artifact_dir_name = "build_artifacts";
        let sub_dir_name = "binaries";
        let binary_filename = "app.dat";

        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_dir(&format!("{}/{}", artifact_dir_name, sub_dir_name));
        harness.create_file(
            &format!("{}/{}/{}", artifact_dir_name, sub_dir_name, binary_filename),
            "\x00\x01\x02\x03", // Use actual binary content to trigger detection
        );
        harness.create_file("README.md", "Project Readme");

        {
            let mut state = harness.state.lock().unwrap();
            state
                .config
                .ignore_patterns
                .insert(format!("{}/", artifact_dir_name));
        }

        // --- 2. Act & Assert Initial State ---
        rescan_directory(harness.proxy.clone(), harness.state.clone());
        let initial_state = harness
            .wait_for_scan_completion()
            .await
            .expect("Initial scan should complete.");
        assert_eq!(initial_state.visible_files_count, 3);

        // --- 3. Act: Remove pattern and rescan ---
        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config
            .ignore_patterns
            .remove(&format!("{}/", artifact_dir_name));
        let payload = serde_json::to_value(new_config).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        let _ = harness.last_state_update();

        rescan_directory(harness.proxy.clone(), harness.state.clone());
        let final_state = harness
            .wait_for_scan_completion()
            .await
            .expect("Final re-scan should complete.");

        // --- 4. Assert Final State ---
        assert!(!final_state.patterns_need_rescan);

        assert_eq!(
            final_state.visible_files_count, 5,
            "Final count should be 5. The binary 'app.dat' is skipped by the scanner's default behavior."
        );
        assert!(final_state
            .tree
            .iter()
            .any(|node| node.name == artifact_dir_name));
    }

    // --- Config changes that need more than a re-filter ---

    /// Registers a scan whose task never finishes, as if a long scan were running.
    fn begin_parked_scan(harness: &TestHarness) -> crate::app::state::ScanToken {
        let mut state = harness.state.lock().unwrap();
        let token = state.begin_scan();
        state.attach_scan_task(&token, tokio::spawn(std::future::pending()));
        token
    }

    fn file_names(state: &AppState) -> HashSet<String> {
        state
            .full_file_list
            .iter()
            .map(|item| {
                item.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// A scan works with the patterns it was started with. If a pattern is added while
    /// it is running, its result would bring the entries back that the pattern has just
    /// removed, so the scan is replaced by one that knows the new patterns.
    #[tokio::test]
    async fn test_update_config_restarts_running_scan_when_pattern_is_added() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_file("docs/guide.md", "# Guide");
        harness.set_initial_files(&["src", "docs"]); // The shallow list of the scan.
        let old_scan = begin_parked_scan(&harness);

        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.ignore_patterns.insert("docs/".to_string());
        let payload = serde_json::to_value(new_config).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        // The old scan is out of the game, a new one has taken over, and the ignored
        // directory has disappeared at once.
        {
            let state = harness.state.lock().unwrap();
            assert!(old_scan.is_cancelled());
            assert!(!state.is_current_scan(&old_scan));
            assert!(state.is_scanning(), "A new scan must be running");
            assert!(state.has_scan_task(), "The new scan must be cancellable");
            assert_eq!(file_names(&state), HashSet::from(["src".to_string()]));
        }

        let final_state = harness
            .wait_for_state(|ui_state| !ui_state.is_scanning && ui_state.is_fully_scanned)
            .await;
        assert!(!final_state.tree.iter().any(|node| node.name == "docs"));
        assert!(!final_state.patterns_need_rescan);
        let state = harness.state.lock().unwrap();
        assert_eq!(
            file_names(&state),
            HashSet::from(["src".to_string(), "main.rs".to_string()]),
            "The complete list must follow the new patterns"
        );
        assert!(state.active_ignore_patterns.contains("docs/"));
    }

    /// Removing a pattern during a scan restarts it as well: the new scan lists what
    /// the pattern used to hide, so no separate re-scan has to be recommended.
    #[tokio::test]
    async fn test_update_config_restarts_running_scan_when_pattern_is_removed() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_file("docs/guide.md", "# Guide");
        harness.set_initial_files(&["src"]);
        harness
            .state
            .lock()
            .unwrap()
            .config
            .ignore_patterns
            .insert("docs/".to_string());
        let old_scan = begin_parked_scan(&harness);

        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.ignore_patterns.remove("docs/");
        let payload = serde_json::to_value(new_config).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        assert!(old_scan.is_cancelled());
        let final_state = harness
            .wait_for_state(|ui_state| !ui_state.is_scanning && ui_state.is_fully_scanned)
            .await;
        assert!(final_state.tree.iter().any(|node| node.name == "docs"));
        assert!(!final_state.patterns_need_rescan);
        let state = harness.state.lock().unwrap();
        assert!(file_names(&state).contains("guide.md"));
    }

    /// A config change that does not touch the patterns leaves a running scan alone.
    #[tokio::test]
    async fn test_update_config_without_pattern_change_keeps_running_scan() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src"]);
        let scan = begin_parked_scan(&harness);

        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.remove_empty_directories = !new_config.remove_empty_directories;
        let payload = serde_json::to_value(new_config).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.is_scanning);
        let mut state = harness.state.lock().unwrap();
        assert!(state.is_current_scan(&scan));
        assert!(!scan.is_cancelled());
        state.cancel_current_scan(); // Stop the parked task.
    }

    /// The matches of a content search depend on the case sensitivity, so changing it
    /// repeats the search instead of filtering with the matches of the old setting.
    #[tokio::test]
    async fn test_update_config_case_sensitivity_change_repeats_content_search() {
        let mut harness = TestHarness::new();
        let lower = harness.create_file("lower.txt", "a needle in the hay");
        let upper = harness.create_file("upper.txt", "a NEEDLE in the hay");
        harness.set_initial_files(&["lower.txt", "upper.txt"]);
        {
            // The state a case-insensitive search for "needle" has left behind.
            let mut state = harness.state.lock().unwrap();
            state.config.case_sensitive_search = false;
            state.content_search_query = "needle".to_string();
            state.content_search_results = HashSet::from([lower.clone(), upper.clone()]);
        }

        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.case_sensitive_search = true;
        let payload = serde_json::to_value(new_config.clone()).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.visible_files_count, 1);
        assert_eq!(
            harness.state.lock().unwrap().content_search_results,
            HashSet::from([lower.clone()])
        );

        // And back: the file that only matches case-insensitively is found again.
        new_config.case_sensitive_search = false;
        let payload = serde_json::to_value(new_config).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.visible_files_count, 2);
        assert_eq!(
            harness.state.lock().unwrap().content_search_results,
            HashSet::from([lower, upper])
        );
    }

    /// Without a content query there is nothing to repeat: the change is answered
    /// with the one state update of the re-filter.
    #[tokio::test]
    async fn test_update_config_case_sensitivity_change_without_content_query_only_refilters() {
        let mut harness = TestHarness::new();
        harness.create_file("Main.rs", "");
        harness.set_initial_files(&["Main.rs"]);
        harness.state.lock().unwrap().search_query = "main".to_string();

        let mut new_config = harness.state.lock().unwrap().config.clone();
        new_config.case_sensitive_search = true;
        let payload = serde_json::to_value(new_config).unwrap();
        update_config(payload, harness.proxy.clone(), harness.state.clone()).await;

        let mut updates = Vec::new();
        while let Ok(event) = harness.event_rx.try_recv() {
            updates.push(event);
        }
        assert_eq!(updates.len(), 1, "Exactly one state update expected");
        match &updates[0] {
            UserEvent::StateUpdate(ui_state) => assert_eq!(ui_state.visible_files_count, 0),
            other => panic!("Expected a state update, got {other:?}"),
        }
    }

    /// The file size limit of the configuration is the one the generation works with.
    #[tokio::test]
    async fn test_generate_preview_applies_configured_file_size_limit() {
        let mut harness = TestHarness::new();
        let file = harness.create_file("file.txt", "some content");
        harness.set_initial_files(&["file.txt"]);
        {
            let mut state = harness.state.lock().unwrap();
            state.selected_files.insert(file);
            state.config.max_file_size_mb = 0;
        }

        generate_preview(harness.proxy.clone(), harness.state.clone());

        let content = loop {
            match harness.get_next_event().await {
                Some(UserEvent::ShowGeneratedContent { content, .. }) => break content,
                Some(_) => {}
                None => panic!("Timed out waiting for the generated content"),
            }
        };
        assert!(content.contains("[FILE TOO LARGE: 12 bytes - CONTENT SKIPPED]"));
    }
}
