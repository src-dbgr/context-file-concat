use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

use super::events::UserEvent;
use super::state::AppState;
use super::tasks::{search_in_files, start_scan_on_path};
use super::view_model::{
    apply_filters, auto_expand_for_matches, generate_ui_state, get_language_from_path,
    get_selected_files_in_tree_order,
};
use crate::config;
use crate::core::FileHandler;

// Jede Funktion hier entspricht einem IpcMessage-Befehl.

pub fn select_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        start_scan_on_path(path, proxy, state);
    } else {
        tracing::info!("LOG: Benutzer hat Verzeichnisauswahl abgebrochen.");
        let mut state_guard = state.lock().unwrap();
        state_guard.is_scanning = false;
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub fn clear_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    state_guard.reset_directory_state();
    state_guard.config.last_directory = None;
    config::settings::save_config(&state_guard.config).ok();
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub fn rescan_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let current_path_str = { state.lock().unwrap().current_path.clone() };
    if !current_path_str.is_empty() {
        start_scan_on_path(PathBuf::from(current_path_str), proxy, state);
    }
}

pub fn cancel_scan(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    tracing::info!("LOG: IPC 'cancelScan' erhalten.");
    let mut state_guard = state.lock().unwrap();
    state_guard.cancel_current_scan();
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub fn update_config(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(new_config) = serde_json::from_value(payload) {
        let (should_restart_scan, path_if_needed) = {
            let mut state_guard = state.lock().unwrap();
            let old_ignore_patterns = state_guard.config.ignore_patterns.clone();
            state_guard.config = new_config;
            config::settings::save_config(&state_guard.config).ok();

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
            let mut state_guard = state.lock().unwrap();
            apply_filters(&mut state_guard);
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                .unwrap();
        }
    }
}

// Ersetze diese Funktion in src/app/commands.rs

pub fn initialize(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    // Das ursprüngliche, "immer deaktivierte" Verhalten wird hier wiederhergestellt.
    // Wir scannen nicht automatisch und senden nur den initialen UI-Zustand.
    let state_guard = state.lock().unwrap();
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub async fn update_filters(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(filters) = serde_json::from_value::<HashMap<String, String>>(payload) {
        let should_search_content = {
            let mut state_guard = state.lock().unwrap();
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
            let mut state_guard = state.lock().unwrap();
            apply_filters(&mut state_guard);
            if !state_guard.search_query.is_empty() {
                auto_expand_for_matches(&mut state_guard);
            }
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                .unwrap();
        }
    }
}

pub fn load_file_preview(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path = PathBuf::from(path_str);
        let search_term = {
            let state_guard = state.lock().unwrap();
            if state_guard.content_search_query.is_empty() {
                None
            } else {
                Some(state_guard.content_search_query.clone())
            }
        };

        {
            state.lock().unwrap().previewed_file_path = Some(path.clone());
        }

        match FileHandler::get_file_preview(&path, 1500) {
            Ok(content) => {
                proxy
                    .send_event(UserEvent::ShowFilePreview {
                        content,
                        language: get_language_from_path(&path),
                        search_term,
                        path: path.clone(),
                    })
                    .unwrap();
            }
            Err(e) => proxy
                .send_event(UserEvent::ShowError(e.to_string()))
                .unwrap(),
        }

        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(
                &state.lock().unwrap(),
            )))
            .unwrap();
    }
}

pub fn add_ignore_path(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path_to_ignore = PathBuf::from(path_str);
        let mut state_guard = state.lock().unwrap();
        let root_path = PathBuf::from(&state_guard.current_path);

        if let Ok(relative_path) = path_to_ignore.strip_prefix(&root_path) {
            let mut pattern = relative_path.to_string_lossy().to_string();
            if path_to_ignore.is_dir() {
                pattern.push('/');
            }
            state_guard
                .selected_files
                .retain(|p| !p.starts_with(&path_to_ignore));
            state_guard.config.ignore_patterns.insert(pattern);
            config::settings::save_config(&state_guard.config).ok();
            apply_filters(&mut state_guard);
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                .unwrap();
        }
    }
}

pub fn toggle_selection(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path = PathBuf::from(path_str);
        let mut state_guard = state.lock().unwrap();
        if state_guard.selected_files.contains(&path) {
            state_guard.selected_files.remove(&path);
        } else {
            state_guard.selected_files.insert(path);
        }
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub fn toggle_directory_selection(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let dir_path = PathBuf::from(path_str);
        let mut state_guard = state.lock().unwrap();

        let selection_state = super::view_model::get_directory_selection_state(
            &dir_path,
            &state_guard.filtered_file_list,
            &state_guard.selected_files,
        );

        let files_in_dir: Vec<PathBuf> = state_guard
            .filtered_file_list
            .iter()
            .filter(|item| !item.is_directory && item.path.starts_with(&dir_path))
            .map(|item| item.path.clone())
            .collect();

        if selection_state == "full" {
            for file in files_in_dir {
                state_guard.selected_files.remove(&file);
            }
        } else {
            for file in files_in_dir {
                state_guard.selected_files.insert(file);
            }
        }
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub fn toggle_expansion(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path = PathBuf::from(path_str);
        let mut state_guard = state.lock().unwrap();
        if state_guard.expanded_dirs.contains(&path) {
            state_guard.expanded_dirs.remove(&path);
        } else {
            state_guard.expanded_dirs.insert(path);
        }
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub fn expand_collapse_all(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(expand) = serde_json::from_value::<bool>(payload) {
        let mut state_guard = state.lock().unwrap();
        if expand {
            state_guard.expanded_dirs = state_guard
                .filtered_file_list
                .iter()
                .filter(|i| i.is_directory)
                .map(|i| i.path.clone())
                .collect();
        } else {
            state_guard.expanded_dirs.clear();
        }
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub fn select_all(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    let paths_to_select: Vec<PathBuf> = state_guard
        .filtered_file_list
        .iter()
        .filter(|item| !item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    state_guard.selected_files.extend(paths_to_select);
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub fn deselect_all(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    state_guard.selected_files.clear();
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub async fn generate_preview(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let (selected, root, config, visible_files) = {
        let mut state_guard = state.lock().unwrap();
        state_guard.previewed_file_path = None;
        (
            get_selected_files_in_tree_order(&state_guard),
            PathBuf::from(&state_guard.current_path),
            state_guard.config.clone(),
            state_guard.filtered_file_list.clone(),
        )
    };

    let result = FileHandler::generate_concatenated_content_simple(
        &selected,
        &root,
        config.include_tree_by_default,
        visible_files,
        config.tree_ignore_patterns,
        config.use_relative_paths,
    )
    .await;

    match result {
        Ok(content) => {
            proxy
                .send_event(UserEvent::ShowGeneratedContent(content))
                .unwrap();
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(
                    &state.lock().unwrap(),
                )))
                .unwrap();
        }
        Err(e) => proxy
            .send_event(UserEvent::ShowError(e.to_string()))
            .unwrap(),
    }
}

pub fn clear_preview_state(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    state_guard.previewed_file_path = None;
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub fn save_file(
    payload: serde_json::Value,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Some(content) = payload.as_str() {
        let content_clone = content.to_string();
        let (output_dir, filename) = {
            let state_guard = state.lock().unwrap();
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
                Ok(_) => proxy
                    .send_event(UserEvent::SaveComplete(
                        true,
                        path.to_string_lossy().to_string(),
                    ))
                    .unwrap(),
                Err(e) => proxy
                    .send_event(UserEvent::SaveComplete(false, e.to_string()))
                    .unwrap(),
            };
        } else {
            proxy
                .send_event(UserEvent::SaveComplete(false, "cancelled".to_string()))
                .unwrap();
        }
    }
}

pub fn pick_output_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        let mut state_guard = state.lock().unwrap();
        state_guard.config.output_directory = Some(path);
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub fn import_config(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()
    {
        match config::settings::import_config(&path) {
            Ok(new_config) => {
                let filename = path.file_name().and_then(|n| n.to_str()).map(String::from);
                let dir_to_scan = {
                    let mut state_guard = state.lock().unwrap();
                    state_guard.cancel_current_scan();
                    state_guard.config = new_config;
                    state_guard.current_config_filename = filename;
                    config::settings::save_config(&state_guard.config).ok();
                    state_guard.config.last_directory.clone()
                };

                if let Some(dir) = dir_to_scan {
                    if dir.exists() {
                        start_scan_on_path(dir, proxy, state);
                    }
                } else {
                    proxy
                        .send_event(UserEvent::StateUpdate(generate_ui_state(
                            &state.lock().unwrap(),
                        )))
                        .unwrap();
                }
            }
            Err(e) => {
                proxy
                    .send_event(UserEvent::ShowError(format!(
                        "Failed to import config: {}",
                        e
                    )))
                    .unwrap();
            }
        }
    }
}

pub fn export_config(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name("cfc-config.json")
        .save_file()
    {
        let state_guard = state.lock().unwrap();
        let result = config::settings::export_config(&state_guard.config, &path).is_ok();
        proxy.send_event(UserEvent::ConfigExported(result)).unwrap();
    }
}
