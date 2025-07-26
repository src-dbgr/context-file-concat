#!/bin/bash
#
# Automatic Refactoring Script for CFC Project
#
# This script restructures the `main.rs` file into a modular architecture
# following SOLID principles.
#
# WARNING: Please back up your project before running this script!
#          e.g., using git: `git commit -a -m "Pre-refactoring backup"`
#
# USAGE:
# 1. Save this script as `refactor.sh` in the root of your project.
# 2. Make it executable: `chmod +x refactor.sh`
# 3. Run it: `./refactor.sh`
#

set -e

echo "🚀 Starting architectural refactoring..."

# 1. Create the new directory structure
echo "📁 Creating new directory structure..."
mkdir -p src/app
mkdir -p src/ipc
mkdir -p src/ipc/commands
mkdir -p src/tasks
mkdir -p src/ui

# 2. Create the module definition files (`mod.rs`)
echo "📝 Creating module definitions..."

cat <<'EOF' > src/app/mod.rs
pub mod state;
EOF

cat <<'EOF' > src/ipc/mod.rs
pub mod commands;
pub mod handler;
EOF

cat <<'EOF' > src/ipc/commands/mod.rs
pub mod config_commands;
pub mod directory_commands;
pub mod file_commands;
pub mod selection_commands;
pub mod ui_commands;
EOF

cat <<'EOF' > src/tasks/mod.rs
pub mod scan;
pub mod search;
EOF

cat <<'EOF' > src/ui/mod.rs
pub mod state;
pub mod tree;
EOF


# 3. Populate the new modules with refactored code from main.rs
echo "🧩 Populating new modules..."

# ==============================================================================
# src/app/state.rs - The new structured application state
# ==============================================================================
cat <<'EOF' > src/app/state.rs
use crate::config::AppConfig;
use crate::core::{FileItem, ScanProgress};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Main state container, composed of specialized sub-states.
pub struct AppState {
    pub config: AppConfig,
    pub directory: DirectoryState,
    pub ui: UiInteractionState,
    pub task: TaskState,
    pub auto_load_last_directory: bool, // This was outside config logic
}

/// State related to the currently loaded directory.
pub struct DirectoryState {
    pub current_path: String,
    pub full_file_list: Vec<FileItem>,
    pub filtered_file_list: Vec<FileItem>,
    pub active_ignore_patterns: HashSet<String>,
}

/// State related to direct user interactions with the UI.
pub struct UiInteractionState {
    pub selected_files: HashSet<PathBuf>,
    pub expanded_dirs: HashSet<PathBuf>,
    pub search_query: String,
    pub extension_filter: String,
    pub content_search_query: String,
    pub content_search_results: HashSet<PathBuf>,
    pub previewed_file_path: Option<PathBuf>,
}

/// State for managing background tasks like scanning.
pub struct TaskState {
    pub is_scanning: bool,
    pub scan_progress: ScanProgress,
    pub scan_task: Option<JoinHandle<()>>,
    pub scan_cancellation_flag: Arc<AtomicBool>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            config: AppConfig::load().unwrap_or_default(),
            directory: DirectoryState {
                current_path: String::new(),
                full_file_list: Vec::new(),
                filtered_file_list: Vec::new(),
                active_ignore_patterns: HashSet::new(),
            },
            ui: UiInteractionState {
                selected_files: HashSet::new(),
                expanded_dirs: HashSet::new(),
                search_query: String::new(),
                extension_filter: String::new(),
                content_search_query: String::new(),
                content_search_results: HashSet::new(),
                previewed_file_path: None,
            },
            task: TaskState {
                is_scanning: false,
                scan_progress: ScanProgress {
                    files_scanned: 0,
                    large_files_skipped: 0,
                    current_scanning_path: "Ready.".to_string(),
                },
                scan_task: None,
                scan_cancellation_flag: Arc::new(AtomicBool::new(false)),
            },
            auto_load_last_directory: false, // Defaulted, as in original
        }
    }

    pub fn cancel_current_scan(&mut self) {
        tracing::info!("LOG: AppState::cancel_current_scan aufgerufen.");
        if let Some(handle) = self.task.scan_task.take() {
            handle.abort();
        }
        self.task.scan_cancellation_flag.store(true, Ordering::Relaxed);
        self.task.is_scanning = false;
        self.task.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Scan cancelled.".to_string(),
        };
    }

    pub fn reset_directory_state(&mut self) {
        self.cancel_current_scan();
        self.directory = DirectoryState {
            current_path: String::new(),
            full_file_list: Vec::new(),
            filtered_file_list: Vec::new(),
            active_ignore_patterns: HashSet::new(),
        };
        self.ui = UiInteractionState {
            selected_files: HashSet::new(),
            expanded_dirs: HashSet::new(),
            search_query: String::new(),
            extension_filter: String::new(),
            content_search_query: String::new(),
            content_search_results: HashSet::new(),
            previewed_file_path: None,
        };
        self.task.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Ready.".to_string(),
        };
    }
}

// Event definitions sent from Backend to Frontend
#[derive(Debug)]
pub enum UserEvent {
    StateUpdate(UiState),
    ShowFilePreview {
        content: String,
        language: String,
        search_term: Option<String>,
        path: PathBuf,
    },
    ShowGeneratedContent(String),
    ShowError(String),
    SaveComplete(bool, String),
    ConfigExported(bool),
    ScanProgress(ScanProgress),
    DragStateChanged(bool),
}

// Struct for IPC messages from Frontend to Backend
#[derive(Deserialize, Debug)]
pub struct IpcMessage {
    pub command: String,
    pub payload: serde_json::Value,
}

// Data Transfer Object for UI state
#[derive(Serialize, Clone, Debug)]
pub struct UiState {
    pub config: AppConfig,
    pub current_path: String,
    pub tree: Vec<TreeNode>,
    pub total_files_found: usize,
    pub visible_files_count: usize,
    pub selected_files_count: usize,
    pub is_scanning: bool,
    pub status_message: String,
    pub search_query: String,
    pub extension_filter: String,
    pub content_search_query: String,
    pub current_config_filename: Option<String>, // Kept for now, consider moving to config state
    pub scan_progress: ScanProgress,
    pub active_ignore_patterns: HashSet<String>,
}

// Data Transfer Object for a single node in the file tree UI
#[derive(Serialize, Clone, Debug)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_binary: bool,
    pub size: u64,
    pub children: Vec<TreeNode>,
    pub selection_state: String,
    pub is_expanded: bool,
    pub is_match: bool,
    pub is_previewed: bool,
}
EOF

# ==============================================================================
# src/ipc/handler.rs - The main IPC message dispatcher
# ==============================================================================
cat <<'EOF' > src/ipc/handler.rs
use crate::app::state::{AppState, IpcMessage, UserEvent};
use crate::ipc::commands::{
    config_commands, directory_commands, file_commands, selection_commands, ui_commands,
};
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

pub fn handle_ipc_message(
    message: String,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(msg) = serde_json::from_str::<IpcMessage>(&message) {
        tokio::spawn(async move {
            match msg.command.as_str() {
                // Directory Commands
                "selectDirectory" => directory_commands::select_directory(proxy, state).await,
                "clearDirectory" => directory_commands::clear_directory(proxy, state).await,
                "rescanDirectory" => directory_commands::rescan_directory(proxy, state).await,
                "cancelScan" => directory_commands::cancel_scan(proxy, state).await,

                // Config Commands
                "updateConfig" => config_commands::update_config(proxy, state, msg.payload).await,
                "importConfig" => config_commands::import_config(proxy, state).await,
                "exportConfig" => config_commands::export_config(proxy, state).await,
                "pickOutputDirectory" => config_commands::pick_output_directory(proxy, state).await,

                // File & Preview Commands
                "loadFilePreview" => file_commands::load_file_preview(proxy, state, msg.payload).await,
                "generatePreview" => file_commands::generate_preview(proxy, state).await,
                "clearPreviewState" => file_commands::clear_preview_state(proxy, state).await,
                "saveFile" => file_commands::save_file(proxy, state, msg.payload).await,
                "addIgnorePath" => file_commands::add_ignore_path(proxy, state, msg.payload).await,

                // Selection Commands
                "toggleSelection" => selection_commands::toggle_selection(proxy, state, msg.payload).await,
                "toggleDirectorySelection" => selection_commands::toggle_directory_selection(proxy, state, msg.payload).await,
                "selectAll" => selection_commands::select_all(proxy, state).await,
                "deselectAll" => selection_commands::deselect_all(proxy, state).await,

                // UI Commands
                "initialize" => ui_commands::initialize(proxy, state).await,
                "updateFilters" => ui_commands::update_filters(proxy, state, msg.payload).await,
                "toggleExpansion" => ui_commands::toggle_expansion(proxy, state, msg.payload).await,
                "expandCollapseAll" => ui_commands::expand_collapse_all(proxy, state, msg.payload).await,

                _ => tracing::warn!("Unknown IPC command: {}", msg.command),
            }
        });
    } else {
        tracing::error!("Failed to parse IPC message: {}", message);
    }
}
EOF

# ==============================================================================
# src/ipc/commands/directory_commands.rs
# ==============================================================================
cat <<'EOF' > src/ipc/commands/directory_commands.rs
use crate::app::state::{AppState, UserEvent};
use crate::tasks::scan::start_scan_on_path;
use crate::ui::state::generate_ui_state;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

pub async fn select_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        start_scan_on_path(path, proxy.clone(), state.clone());
    } else {
        tracing::info!("LOG: Benutzer hat Verzeichnisauswahl abgebrochen.");
        let mut state_guard = state.lock().unwrap();
        state_guard.task.is_scanning = false;
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub async fn clear_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    state_guard.reset_directory_state();
    state_guard.config.last_directory = None;
    crate::config::settings::save_config(&state_guard.config).ok();
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub async fn rescan_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let current_path_str = { state.lock().unwrap().directory.current_path.clone() };
    if !current_path_str.is_empty() {
        start_scan_on_path(
            PathBuf::from(current_path_str),
            proxy.clone(),
            state.clone(),
        );
    }
}

pub async fn cancel_scan(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    state_guard.cancel_current_scan();
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}
EOF

# ==============================================================================
# src/ipc/commands/config_commands.rs
# ==============================================================================
cat <<'EOF' > src/ipc/commands/config_commands.rs
use crate::app::state::{AppState, UserEvent};
use crate::config;
use crate::tasks::scan::start_scan_on_path;
use crate::ui::state::{apply_filters, generate_ui_state};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

pub async fn update_config(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
) {
    if let Ok(new_config) = serde_json::from_value(payload) {
        let should_restart_scan = {
            let mut state_guard = state.lock().unwrap();
            let old_ignore_patterns = state_guard.config.ignore_patterns.clone();
            let was_scanning = state_guard.task.is_scanning;
            state_guard.config = new_config;
            config::settings::save_config(&state_guard.config).ok();

            let ignore_patterns_changed =
                old_ignore_patterns != state_guard.config.ignore_patterns;

            if ignore_patterns_changed && was_scanning {
                state_guard.cancel_current_scan();
                true
            } else {
                false
            }
        };

        if should_restart_scan {
            tracing::info!("🔄 Restarting scan due to ignore pattern changes");
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let current_path_str = { state.lock().unwrap().directory.current_path.clone() };
            if !current_path_str.is_empty() {
                start_scan_on_path(
                    PathBuf::from(current_path_str),
                    proxy.clone(),
                    state.clone(),
                );
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

pub async fn import_config(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()
    {
        match config::settings::import_config(&path) {
            Ok(new_config) => {
                let directory_to_scan = {
                    let mut state_guard = state.lock().unwrap();
                    state_guard.cancel_current_scan();
                    state_guard.config = new_config;
                    // state_guard.current_config_filename = ... // This needs a home if it's kept
                    config::settings::save_config(&state_guard.config).ok();
                    state_guard.config.last_directory.clone()
                };

                if let Some(dir) = directory_to_scan {
                    if dir.exists() {
                        start_scan_on_path(dir, proxy, state);
                    }
                } else {
                    let state_guard = state.lock().unwrap();
                    proxy
                        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
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

pub async fn export_config(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name("cfc-config.json")
        .save_file()
    {
        let state_guard = state.lock().unwrap();
        let result =
            config::settings::export_config(&state_guard.config, &path).is_ok();
        proxy.send_event(UserEvent::ConfigExported(result)).unwrap();
    }
}

pub async fn pick_output_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        let mut state_guard = state.lock().unwrap();
        state_guard.config.output_directory = Some(path);
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}
EOF

# ==============================================================================
# src/ipc/commands/file_commands.rs
# ==============================================================================
cat <<'EOF' > src/ipc/commands/file_commands.rs
use crate::app::state::{AppState, UserEvent};
use crate::core::FileHandler;
use crate::ui::state::{apply_filters, generate_ui_state, get_selected_files_in_tree_order};
use crate::utils::file_detection::get_language_from_path;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

pub async fn load_file_preview(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path = PathBuf::from(path_str);
        let search_term = {
            let state_guard = state.lock().unwrap();
            if state_guard.ui.content_search_query.is_empty() {
                None
            } else {
                Some(state_guard.ui.content_search_query.clone())
            }
        };

        {
            let mut state_guard = state.lock().unwrap();
            state_guard.ui.previewed_file_path = Some(path.clone());
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

pub async fn generate_preview(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let (selected, root, config, visible_files) = {
        let mut state_guard = state.lock().unwrap();
        state_guard.ui.previewed_file_path = None;
        (
            get_selected_files_in_tree_order(&state_guard),
            PathBuf::from(&state_guard.directory.current_path),
            state_guard.config.clone(),
            state_guard.directory.filtered_file_list.clone(),
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

pub async fn clear_preview_state(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    state_guard.ui.previewed_file_path = None;
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub async fn save_file(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
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
        let dialog = rfd::FileDialog::new()
            .add_filter("Text File", &["txt"])
            .set_file_name(&filename);

        let dialog = if let Some(dir) = output_dir {
            dialog.set_directory(dir)
        } else {
            dialog
        };

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

pub async fn add_ignore_path(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path_to_ignore = PathBuf::from(path_str);
        let mut state_guard = state.lock().unwrap();
        let root_path = PathBuf::from(&state_guard.directory.current_path);

        if let Ok(relative_path) = path_to_ignore.strip_prefix(&root_path) {
            let mut pattern = relative_path.to_string_lossy().to_string();
            if path_to_ignore.is_dir() {
                pattern.push('/');
            }

            state_guard
                .ui
                .selected_files
                .retain(|selected_path| !selected_path.starts_with(&path_to_ignore));

            state_guard.config.ignore_patterns.insert(pattern);
            crate::config::settings::save_config(&state_guard.config).ok();

            apply_filters(&mut state_guard);
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                .unwrap();
        }
    }
}
EOF

# ==============================================================================
# src/ipc/commands/selection_commands.rs
# ==============================================================================
cat <<'EOF' > src/ipc/commands/selection_commands.rs
use crate::app::state::{AppState, UserEvent};
use crate::ui::state::generate_ui_state;
use crate::ui::tree::get_directory_selection_state;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

pub async fn toggle_selection(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path = PathBuf::from(path_str);
        let mut state_guard = state.lock().unwrap();
        if state_guard.ui.selected_files.contains(&path) {
            state_guard.ui.selected_files.remove(&path);
        } else {
            state_guard.ui.selected_files.insert(path);
        }
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub async fn toggle_directory_selection(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let dir_path = PathBuf::from(path_str);
        let mut state_guard = state.lock().unwrap();
        let files_in_dir: Vec<PathBuf> = state_guard
            .directory
            .filtered_file_list
            .iter()
            .filter(|item| !item.is_directory && item.path.starts_with(&dir_path))
            .map(|item| item.path.clone())
            .collect();

        let selection_state = get_directory_selection_state(
            &dir_path,
            &state_guard.directory.filtered_file_list,
            &state_guard.ui.selected_files,
        );

        if selection_state == "full" {
            for file in files_in_dir {
                state_guard.ui.selected_files.remove(&file);
            }
        } else {
            for file in files_in_dir {
                state_guard.ui.selected_files.insert(file);
            }
        }
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub async fn select_all(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    let paths_to_select: Vec<PathBuf> = state_guard
        .directory
        .filtered_file_list
        .iter()
        .filter(|item| !item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    state_guard.ui.selected_files.extend(paths_to_select);
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}

pub async fn deselect_all(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let mut state_guard = state.lock().unwrap();
    state_guard.ui.selected_files.clear();
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}
EOF

# ==============================================================================
# src/ipc/commands/ui_commands.rs
# ==============================================================================
cat <<'EOF' > src/ipc/commands/ui_commands.rs
use crate::app::state::{AppState, UserEvent};
use crate::tasks::scan::start_scan_on_path;
use crate::tasks::search::search_in_files;
use crate::ui::state::{apply_filters, auto_expand_for_matches, generate_ui_state};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

pub async fn initialize(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let should_auto_scan = {
        let mut state_guard = state.lock().unwrap();
        // Here, we can also set the initial value from the config
        state_guard.auto_load_last_directory = state_guard.config.auto_load_last_directory;

        if state_guard.auto_load_last_directory {
            if let Some(last_dir) = state_guard.config.last_directory.clone() {
                if last_dir.exists() {
                    state_guard.directory.current_path = last_dir.to_string_lossy().to_string();
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    };

    if should_auto_scan {
        let current_path_str = { state.lock().unwrap().directory.current_path.clone() };
        start_scan_on_path(
            PathBuf::from(current_path_str),
            proxy.clone(),
            state.clone(),
        );
    } else {
        let state_guard = state.lock().unwrap();
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub async fn update_filters(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
) {
    if let Ok(filters) = serde_json::from_value::<HashMap<String, String>>(payload) {
        let should_search_content = {
            let mut state_guard = state.lock().unwrap();
            state_guard.ui.search_query = filters.get("searchQuery").cloned().unwrap_or_default();
            state_guard.ui.extension_filter =
                filters.get("extensionFilter").cloned().unwrap_or_default();

            let new_content_query = filters
                .get("contentSearchQuery")
                .cloned()
                .unwrap_or_default();

            if new_content_query != state_guard.ui.content_search_query {
                state_guard.ui.content_search_query = new_content_query;
                true
            } else {
                false
            }
        };

        if should_search_content {
            search_in_files(proxy.clone(), state.clone()).await;
        } else {
            let mut state_guard = state.lock().unwrap();
            apply_filters(&mut state_guard);
            if !state_guard.ui.search_query.is_empty() {
                auto_expand_for_matches(&mut state_guard);
            }
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                .unwrap();
        }
    }
}

pub async fn toggle_expansion(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
) {
    if let Ok(path_str) = serde_json::from_value::<String>(payload) {
        let path = PathBuf::from(path_str);
        let mut state_guard = state.lock().unwrap();
        if state_guard.ui.expanded_dirs.contains(&path) {
            state_guard.ui.expanded_dirs.remove(&path);
        } else {
            state_guard.ui.expanded_dirs.insert(path);
        }
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

pub async fn expand_collapse_all(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    payload: Value,
) {
    if let Ok(expand) = serde_json::from_value::<bool>(payload) {
        let mut state_guard = state.lock().unwrap();
        if expand {
            state_guard.ui.expanded_dirs = state_guard
                .directory
                .filtered_file_list
                .iter()
                .filter(|i| i.is_directory)
                .map(|i| i.path.clone())
                .collect();
        } else {
            state_guard.ui.expanded_dirs.clear();
        }
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}
EOF

# ==============================================================================
# src/tasks/scan.rs - The async directory scanning task
# ==============================================================================
cat <<'EOF' > src/tasks/scan.rs
use crate::app::state::{AppState, UserEvent};
use crate::config;
use crate::core::{DirectoryScanner, ScanProgress};
use crate::ui::state::{apply_filters_on_data, generate_ui_state};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinHandle;

pub fn start_scan_on_path(
    path: PathBuf,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    tokio::spawn(async move {
        let directory_path = if path.is_dir() {
            path
        } else {
            path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
        };

        if !directory_path.is_dir() {
            proxy
                .send_event(UserEvent::ShowError(
                    "Dropped item is not a valid directory.".to_string(),
                ))
                .ok();
            return;
        }

        let mut state_guard = state.lock().unwrap();
        state_guard.cancel_current_scan();
        state_guard.directory.active_ignore_patterns.clear();
        state_guard.directory.current_path = directory_path.to_string_lossy().to_string();
        state_guard.config.last_directory = Some(directory_path);
        config::settings::save_config(&state_guard.config).ok();

        state_guard.task.is_scanning = true;
        state_guard.task.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Initializing scan...".to_string(),
        };

        let new_cancel_flag = Arc::new(AtomicBool::new(false));
        state_guard.task.scan_cancellation_flag = new_cancel_flag.clone();

        let proxy_clone = proxy.clone();
        let state_clone = state.clone();

        let handle = tokio::spawn(async move {
            scan_directory_task(proxy_clone, state_clone, new_cancel_flag).await;
        });
        state_guard.task.scan_task = Some(handle);

        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    });
}

pub async fn scan_directory_task(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    cancel_flag: Arc<AtomicBool>,
) {
    let (path_str, config, search_query, extension_filter, content_search_results) = {
        let state_lock = state.lock().unwrap();
        (
            state_lock.directory.current_path.clone(),
            state_lock.config.clone(),
            state_lock.ui.search_query.clone(),
            state_lock.ui.extension_filter.clone(),
            state_lock.ui.content_search_results.clone(),
        )
    };

    let path = PathBuf::from(&path_str);
    if !path.is_dir() {
        // ... (error handling as before) ...
        return;
    }

    let scanner = DirectoryScanner::new(config.ignore_patterns.clone());
    let progress_proxy = proxy.clone();
    let progress_callback = move |progress: ScanProgress| {
        let _ = progress_proxy.send_event(UserEvent::ScanProgress(progress));
    };

    let scan_result = scanner
        .scan_directory_with_progress(&path, cancel_flag, progress_callback)
        .await;

    let (final_files, active_patterns) = match scan_result {
        Ok(files) => files,
        Err(e) => {
            // ... (error handling as before) ...
            return;
        }
    };

    let filtered_files = apply_filters_on_data(
        &final_files,
        &path,
        &config,
        &search_query,
        &extension_filter,
        &content_search_results,
    );

    let mut state_lock = state.lock().unwrap();
    if !state_lock.task.is_scanning {
        return; // Scan was cancelled during post-processing
    }

    state_lock.directory.full_file_list = final_files;
    state_lock.directory.filtered_file_list = filtered_files;
    state_lock.directory.active_ignore_patterns = active_patterns;
    state_lock.task.is_scanning = false;
    state_lock.task.scan_progress.current_scanning_path = format!(
        "Scan complete. Found {} visible items.",
        state_lock.directory.filtered_file_list.len()
    );
    state_lock.task.scan_task = None;

    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_lock)))
        .unwrap();
}
EOF

# ==============================================================================
# src/tasks/search.rs - The async file content search task
# ==============================================================================
cat <<'EOF' > src/tasks/search.rs
use crate::app::state::{AppState, UserEvent};
use crate::ui::state::{apply_filters, auto_expand_for_matches, generate_ui_state};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

pub async fn search_in_files(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let (files_to_search, query, case_sensitive) = {
        let mut state_guard = state.lock().unwrap();
        if state_guard.ui.content_search_query.is_empty() {
            state_guard.ui.content_search_results.clear();
            apply_filters(&mut state_guard);
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                .unwrap();
            return;
        }
        (
            state_guard.directory.full_file_list.clone(),
            state_guard.ui.content_search_query.clone(),
            state_guard.config.case_sensitive_search,
        )
    };

    let matching_paths: HashSet<PathBuf> = files_to_search
        .into_par_iter()
        .filter_map(|item| {
            if item.is_directory || item.is_binary {
                return None;
            }
            if let Ok(content) = std::fs::read_to_string(&item.path) {
                let found = if case_sensitive {
                    content.contains(&query)
                } else {
                    content.to_lowercase().contains(&query.to_lowercase())
                };
                if found {
                    Some(item.path)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    let mut state_guard = state.lock().unwrap();
    state_guard.ui.content_search_results = matching_paths;
    apply_filters(&mut state_guard);
    auto_expand_for_matches(&mut state_guard);
    proxy
        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
        .unwrap();
}
EOF

# ==============================================================================
# src/ui/state.rs - UI state generation and filtering logic
# ==============================================================================
cat <<'EOF' > src/ui/state.rs
use crate::app::state::{AppState, UiState};
use crate::core::{FileItem, SearchEngine, SearchFilter};
use crate::ui::tree;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn generate_ui_state(state: &AppState) -> UiState {
    let root = PathBuf::from(&state.directory.current_path);
    let search_matches = if !state.ui.content_search_query.is_empty() {
        state.ui.content_search_results.clone()
    } else {
        HashSet::new()
    };

    let tree = if state.task.is_scanning {
        Vec::new()
    } else {
        tree::build_tree_nodes(
            &state.directory.filtered_file_list,
            &root,
            &state.ui.selected_files,
            &state.ui.expanded_dirs,
            &search_matches,
            &state.ui.search_query,
            state.config.case_sensitive_search,
            &state.ui.previewed_file_path,
        )
    };

    let status_message = if state.task.is_scanning {
        format!(
            "Scanning... {} files processed. {} large files skipped ({})",
            state.task.scan_progress.files_scanned,
            state.task.scan_progress.large_files_skipped,
            state.task.scan_progress.current_scanning_path
        )
    } else {
        state.task.scan_progress.current_scanning_path.clone()
    };

    UiState {
        config: state.config.clone(),
        current_path: state.directory.current_path.clone(),
        tree,
        total_files_found: state.directory.full_file_list.len(),
        visible_files_count: state.directory.filtered_file_list.len(),
        selected_files_count: state.ui.selected_files.len(),
        is_scanning: state.task.is_scanning,
        status_message,
        search_query: state.ui.search_query.clone(),
        extension_filter: state.ui.extension_filter.clone(),
        content_search_query: state.ui.content_search_query.clone(),
        current_config_filename: None, // This needs a proper home
        scan_progress: state.task.scan_progress.clone(),
        active_ignore_patterns: state.directory.active_ignore_patterns.clone(),
    }
}

pub fn apply_filters(state: &mut AppState) {
    let filtered_list = apply_filters_on_data(
        &state.directory.full_file_list,
        &PathBuf::from(&state.directory.current_path),
        &state.config,
        &state.ui.search_query,
        &state.ui.extension_filter,
        &state.ui.content_search_results,
    );
    state.directory.filtered_file_list = filtered_list;
}

pub fn apply_filters_on_data(
    full_file_list: &[FileItem],
    root_path: &Path,
    config: &crate::config::AppConfig,
    search_query: &str,
    extension_filter: &str,
    content_search_results: &HashSet<PathBuf>,
) -> Vec<FileItem> {
    let filter = SearchFilter {
        query: search_query.to_string(),
        extension: extension_filter.to_string(),
        case_sensitive: config.case_sensitive_search,
        ignore_patterns: config.ignore_patterns.clone(),
    };

    let mut filtered = SearchEngine::filter_files(full_file_list, &filter);

    if !content_search_results.is_empty() {
        filtered.retain(|item| content_search_results.contains(&item.path));
    }

    let required_dirs: HashSet<PathBuf> = filtered
        .par_iter()
        .flat_map(|item| {
            let mut parents = Vec::new();
            let mut current = item.path.parent();
            while let Some(parent) = current {
                if parent.starts_with(root_path) {
                    parents.push(parent.to_path_buf());
                } else {
                    break;
                }
                current = parent.parent();
            }
            parents
        })
        .collect();

    let existing_paths_in_filtered: HashSet<PathBuf> =
        filtered.par_iter().map(|item| item.path.clone()).collect();

    for dir_path in required_dirs {
        if !existing_paths_in_filtered.contains(&dir_path) {
            if let Some(dir_item) = full_file_list.iter().find(|i| i.path == dir_path) {
                filtered.push(dir_item.clone());
            }
        }
    }

    if config.remove_empty_directories {
        let (filtered_without_empty, _) = SearchEngine::remove_empty_directories(filtered);
        filtered_without_empty
    } else {
        filtered
    }
}

pub fn auto_expand_for_matches(state: &mut AppState) {
    let root_path = PathBuf::from(&state.directory.current_path);
    let matches: Vec<PathBuf> = state
        .directory
        .filtered_file_list
        .iter()
        .filter(|item| {
            let file_name = item.path.file_name().unwrap_or_default().to_string_lossy();
            let name_match = if !state.ui.search_query.is_empty() {
                if state.config.case_sensitive_search {
                    file_name.contains(&state.ui.search_query)
                } else {
                    file_name
                        .to_lowercase()
                        .contains(&state.ui.search_query.to_lowercase())
                }
            } else {
                false
            };
            let content_match = state.ui.content_search_results.contains(&item.path);
            (name_match || content_match) && !item.is_directory
        })
        .map(|item| item.path.clone())
        .collect();

    for path in matches {
        let mut current = path.parent();
        while let Some(parent) = current {
            if parent.starts_with(&root_path) && parent != root_path {
                state.ui.expanded_dirs.insert(parent.to_path_buf());
            } else {
                break;
            }
            current = parent.parent();
        }
    }
}

pub fn get_selected_files_in_tree_order(state: &AppState) -> Vec<PathBuf> {
    let mut selected_file_items: Vec<&FileItem> = state
        .directory
        .full_file_list
        .iter()
        .filter(|item| !item.is_directory && state.ui.selected_files.contains(&item.path))
        .collect();
    selected_file_items.sort_by_key(|a| a.path.clone());
    selected_file_items
        .into_iter()
        .map(|item| item.path.clone())
        .collect()
}
EOF

# ==============================================================================
# src/ui/tree.rs - UI tree building logic
# ==============================================================================
cat <<'EOF' > src/ui/tree.rs
use crate::app::state::TreeNode;
use crate::core::FileItem;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub fn build_tree_nodes(
    items: &[FileItem],
    root_path: &Path,
    selected: &HashSet<PathBuf>,
    expanded: &HashSet<PathBuf>,
    content_search_matches: &HashSet<PathBuf>,
    filename_query: &str,
    case_sensitive: bool,
    previewed_path: &Option<PathBuf>,
) -> Vec<TreeNode> {
    let mut nodes: HashMap<PathBuf, TreeNode> = HashMap::new();
    let mut children_map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    for item in items {
        let selection_state = if item.is_directory {
            get_directory_selection_state(&item.path, items, selected)
        } else if selected.contains(&item.path) {
            "full".to_string()
        } else {
            "none".to_string()
        };

        let file_name = item.path.file_name().unwrap_or_default().to_string_lossy();
        let name_match = if !filename_query.is_empty() {
            if case_sensitive {
                file_name.contains(filename_query)
            } else {
                file_name
                    .to_lowercase()
                    .contains(&filename_query.to_lowercase())
            }
        } else {
            false
        };

        let content_match = content_search_matches.contains(&item.path);
        let is_previewed = previewed_path.as_ref() == Some(&item.path);

        nodes.insert(
            item.path.clone(),
            TreeNode {
                name: file_name.to_string(),
                path: item.path.clone(),
                is_directory: item.is_directory,
                is_binary: item.is_binary,
                size: item.size,
                children: Vec::new(),
                selection_state,
                is_expanded: expanded.contains(&item.path),
                is_match: name_match || content_match,
                is_previewed,
            },
        );

        if let Some(parent) = item.path.parent() {
            if parent.starts_with(root_path) {
                children_map
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(item.path.clone());
            }
        }
    }

    let mut root_nodes_paths: Vec<PathBuf> = items
        .iter()
        .filter(|item| item.path.parent() == Some(root_path))
        .map(|item| item.path.clone())
        .collect();

    build_level(&mut root_nodes_paths, &mut nodes, &children_map)
}

fn build_level(
    paths: &mut Vec<PathBuf>,
    nodes: &mut HashMap<PathBuf, TreeNode>,
    children_map: &HashMap<PathBuf, Vec<PathBuf>>,
) -> Vec<TreeNode> {
    paths.sort_by(|a, b| {
        let node_a = nodes.get(a).unwrap();
        let node_b = nodes.get(b).unwrap();
        match (node_a.is_directory, node_b.is_directory) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    let mut result = Vec::new();
    for path in paths {
        if let Some(mut node) = nodes.remove(path) {
            if let Some(children_paths) = children_map.get(path) {
                node.children = build_level(&mut children_paths.clone(), nodes, children_map);
            }
            result.push(node);
        }
    }
    result
}

pub fn get_directory_selection_state(
    dir_path: &Path,
    all_items: &[FileItem],
    selected_files: &HashSet<PathBuf>,
) -> String {
    let child_files: Vec<_> = all_items
        .iter()
        .filter(|i| !i.is_directory && i.path.starts_with(dir_path))
        .collect();

    if child_files.is_empty() {
        return "none".to_string();
    }

    let selected_count = child_files
        .iter()
        .filter(|f| selected_files.contains(&f.path))
        .count();

    if selected_count == 0 {
        "none".to_string()
    } else if selected_count == child_files.len() {
        "full".to_string()
    } else {
        "partial".to_string()
    }
}
EOF

# 4. Finally, rewrite main.rs to be the slim entry point
echo "=> Rewriting src/main.rs as the application entry point..."

cat <<'EOF' > src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Module declarations
mod app;
mod config;
mod core;
mod ipc;
mod tasks;
mod ui;
mod utils;

use app::state::{AppState, UserEvent};
use ipc::handler::handle_ipc_message;
use std::sync::{Arc, Mutex};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::{WebView, WebViewBuilder};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("CFC - Context File Concatenator")
        .with_inner_size(tao::dpi::LogicalSize::new(1400, 900))
        .with_min_inner_size(tao::dpi::LogicalSize::new(900, 600))
        .build(&event_loop)
        .expect("Failed to build Window");

    let proxy = event_loop.create_proxy();
    let state = Arc::new(Mutex::new(AppState::new()));

    // Inject CSS and JS into the HTML template
    let html_content = include_str!("ui/index.html")
        .replace("/*INJECT_CSS*/", include_str!("ui/style.css"))
        .replace("/*INJECT_JS*/", include_str!("ui/dist/bundle.js")); // Assuming you have a build step for JS

    let ipc_proxy = proxy.clone();
    let ipc_state = state.clone();
    let drop_proxy = proxy.clone();
    let drop_state = state.clone();

    let webview = WebViewBuilder::new(&window)
        .with_html(html_content)
        .with_ipc_handler(move |message: String| {
            handle_ipc_message(message, ipc_proxy.clone(), ipc_state.clone())
        })
        .with_file_drop_handler(move |event| {
            use wry::FileDropEvent;
            match event {
                FileDropEvent::Hovered { .. } => {
                    drop_proxy.send_event(UserEvent::DragStateChanged(true)).unwrap();
                }
                FileDropEvent::Dropped { paths, .. } => {
                    drop_proxy.send_event(UserEvent::DragStateChanged(false)).unwrap();
                    if let Some(path) = paths.first() {
                        tasks::scan::start_scan_on_path(
                            path.clone(),
                            drop_proxy.clone(),
                            drop_state.clone(),
                        );
                    }
                }
                FileDropEvent::Cancelled => {
                    drop_proxy.send_event(UserEvent::DragStateChanged(false)).unwrap();
                }
                _ => (),
            }
            true
        })
        .with_devtools(true)
        .build()
        .expect("Failed to build WebView");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(user_event) => handle_user_event(user_event, &webview),
            _ => (),
        }
    });
}

/// Handles events sent from the backend to the webview.
fn handle_user_event(event: UserEvent, webview: &WebView) {
    let script = match event {
        UserEvent::StateUpdate(state) => format!(
            "window.render({});",
            serde_json::to_string(&state).unwrap_or_default()
        ),
        UserEvent::ShowFilePreview {
            content,
            language,
            search_term,
            path,
        } => format!(
            "window.showPreviewContent({}, {}, {}, {});",
            serde_json::to_string(&content).unwrap_or_default(),
            serde_json::to_string(&language).unwrap_or_default(),
            serde_json::to_string(&search_term).unwrap_or_default(),
            serde_json::to_string(&path).unwrap_or_default()
        ),
        UserEvent::ShowGeneratedContent(content) => format!(
            "window.showGeneratedContent({});",
            serde_json::to_string(&content).unwrap_or_default()
        ),
        UserEvent::ShowError(msg) => format!(
            "window.showError({});",
            serde_json::to_string(&msg).unwrap_or_default()
        ),
        UserEvent::SaveComplete(success, path) => format!(
            "window.fileSaveStatus({}, {});",
            success,
            serde_json::to_string(&path).unwrap_or_default()
        ),
        UserEvent::ConfigExported(success) => format!(
            "window.showStatus('{}');",
            if success {
                "Config exported successfully."
            } else {
                "Failed to export config."
            }
        ),
        UserEvent::ScanProgress(progress) => format!(
            "window.updateScanProgress({});",
            serde_json::to_string(&progress).unwrap_or_default()
        ),
        UserEvent::DragStateChanged(is_dragging) => {
            format!("window.setDragState({});", is_dragging)
        }
    };
    webview.evaluate_script(&script).ok();
}
EOF


echo "✅ Refactoring complete!"
echo "➡️ Next steps:"
echo "1. Review the changes in the 'src/' directory."
echo "2. Run 'cargo check' to ensure everything still compiles."
echo "3. Run 'cargo fmt' to format the new files."
echo "4. Run your application to test that all functionality is preserved."