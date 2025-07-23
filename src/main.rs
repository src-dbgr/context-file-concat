#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod core;
mod utils;

use crate::config::AppConfig;
use crate::core::{
    build_globset_from_patterns, DirectoryScanner, FileHandler, FileItem, SearchEngine,
    SearchFilter,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::WindowBuilder,
};
use wry::{WebView, WebViewBuilder};

#[derive(Serialize, Clone, Debug)]
struct UiState {
    config: AppConfig,
    current_path: String,
    tree: Vec<TreeNode>,
    total_files_found: usize,
    visible_files_count: usize,
    selected_files_count: usize,
    is_scanning: bool,
    status_message: String,
    search_query: String,
    extension_filter: String,
    content_search_query: String,
}

#[derive(Serialize, Clone, Debug)]
struct TreeNode {
    name: String,
    path: PathBuf,
    is_directory: bool,
    is_binary: bool,
    size: u64,
    children: Vec<TreeNode>,
    // "none", "partial", "full"
    selection_state: String,
    is_expanded: bool,
    is_match: bool,
}

struct AppState {
    config: AppConfig,
    current_path: String,
    full_file_list: Vec<FileItem>,
    filtered_file_list: Vec<FileItem>,
    selected_files: HashSet<PathBuf>,
    expanded_dirs: HashSet<PathBuf>,
    is_scanning: bool,
    // Filter state
    search_query: String,
    extension_filter: String,
    content_search_query: String,
    content_search_results: HashSet<PathBuf>,
}

impl AppState {
    fn new() -> Self {
        Self {
            config: AppConfig::load().unwrap_or_default(),
            current_path: String::new(),
            full_file_list: Vec::new(),
            filtered_file_list: Vec::new(),
            selected_files: HashSet::new(),
            expanded_dirs: HashSet::new(),
            is_scanning: false,
            search_query: String::new(),
            extension_filter: String::new(),
            content_search_query: String::new(),
            content_search_results: HashSet::new(),
        }
    }
}

#[derive(Debug)]
enum UserEvent {
    StateUpdate(UiState),
    ShowFilePreview {
        content: String,
        language: String,
        search_term: Option<String>,
        path: PathBuf, // HINZUGEFÜGT
    },
    ShowGeneratedContent(String),
    ShowError(String),
    SaveComplete(bool, String),
    ConfigExported(bool),
}

#[derive(Deserialize, Debug)]
struct IpcMessage {
    command: String,
    payload: serde_json::Value,
}

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

    let html_content = include_str!("ui/index.html")
        .replace("/*INJECT_CSS*/", include_str!("ui/style.css"))
        .replace("/*INJECT_JS*/", include_str!("ui/script.js"));

    let webview = WebViewBuilder::new(&window)
        .with_html(html_content)
        .with_ipc_handler(move |message: String| {
            handle_ipc_message(message, proxy.clone(), state.clone())
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

fn handle_ipc_message(
    message: String,
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
) {
    if let Ok(msg) = serde_json::from_str::<IpcMessage>(&message) {
        tokio::spawn(async move {
            match msg.command.as_str() {
                "updateConfig" => {
                    if let Ok(new_config) = serde_json::from_value(msg.payload) {
                        // KORREKTUR: Prüfen ob eine neue Inhaltssuche nötig ist
                        let should_research_content = {
                            let mut state_guard = state.lock().unwrap();
                            let old_case_sensitive = state_guard.config.case_sensitive_search;
                            state_guard.config = new_config;
                            config::settings::save_config(&state_guard.config).ok();

                            old_case_sensitive != state_guard.config.case_sensitive_search
                                && !state_guard.content_search_query.is_empty()
                        };

                        if should_research_content {
                            search_in_files(proxy.clone(), state.clone()).await;
                        } else {
                            let mut state_guard = state.lock().unwrap();
                            apply_filters(&mut state_guard);
                            proxy
                                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                                .unwrap();
                        }
                    }
                }
                // ... andere IPC Befehle ...
                "initialize" => {
                    let mut should_scan = false;
                    {
                        let mut state_guard = state.lock().unwrap();
                        if let Some(last_dir) = state_guard.config.last_directory.clone() {
                            if last_dir.exists() {
                                state_guard.current_path = last_dir.to_string_lossy().to_string();
                                should_scan = true;
                            }
                        }
                    }

                    if should_scan {
                        scan_directory(proxy, state).await;
                    } else {
                        let state_guard = state.lock().unwrap();
                        proxy
                            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                            .unwrap();
                    }
                }
                "selectDirectory" | "rescanDirectory" => {
                    if msg.command == "selectDirectory" {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            let mut state_guard = state.lock().unwrap();
                            state_guard.current_path = path.to_string_lossy().to_string();
                            state_guard.config.last_directory = Some(path);
                            config::settings::save_config(&state_guard.config).ok();
                        } else {
                            return;
                        }
                    }
                    scan_directory(proxy, state).await;
                }
                "updateFilters" => {
                    if let Ok(filters) =
                        serde_json::from_value::<HashMap<String, String>>(msg.payload)
                    {
                        let should_search_content = {
                            let mut state_guard = state.lock().unwrap();
                            state_guard.search_query =
                                filters.get("searchQuery").cloned().unwrap_or_default();
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
                            search_in_files(proxy.clone(), state.clone()).await;
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
                "loadFilePreview" => {
                    if let Ok(path_str) = serde_json::from_value::<String>(msg.payload) {
                        let path = PathBuf::from(path_str);
                        let search_term = {
                            let state_guard = state.lock().unwrap();
                            if state_guard.content_search_query.is_empty() {
                                None
                            } else {
                                Some(state_guard.content_search_query.clone())
                            }
                        };

                        match FileHandler::get_file_preview(&path, 1500) {
                            Ok(content) => {
                                let language = get_language_from_path(&path);
                                proxy
                                    .send_event(UserEvent::ShowFilePreview {
                                        content,
                                        language,
                                        search_term,
                                        path, // HINZUGEFÜGT
                                    })
                                    .unwrap();
                            }
                            Err(e) => proxy
                                .send_event(UserEvent::ShowError(e.to_string()))
                                .unwrap(),
                        }
                    }
                }
                "addIgnorePath" => {
                    if let Ok(path_str) = serde_json::from_value::<String>(msg.payload) {
                        let path_to_ignore = PathBuf::from(path_str);
                        let mut state_guard = state.lock().unwrap();
                        let root_path = PathBuf::from(&state_guard.current_path);

                        if let Ok(relative_path) = path_to_ignore.strip_prefix(&root_path) {
                            let mut pattern = relative_path.to_string_lossy().to_string();
                            if path_to_ignore.is_dir() {
                                pattern.push('/');
                            }
                            state_guard.config.ignore_patterns.insert(pattern);
                            config::settings::save_config(&state_guard.config).ok();
                            apply_filters(&mut state_guard);
                            proxy
                                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                                .unwrap();
                        }
                    }
                }
                "toggleSelection" => {
                    if let Ok(path_str) = serde_json::from_value::<String>(msg.payload) {
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
                "toggleDirectorySelection" => {
                    if let Ok(path_str) = serde_json::from_value::<String>(msg.payload) {
                        let dir_path = PathBuf::from(path_str);
                        let mut state_guard = state.lock().unwrap();
                        let files_in_dir: Vec<PathBuf> = state_guard
                            .filtered_file_list
                            .iter()
                            .filter(|item| !item.is_directory && item.path.starts_with(&dir_path))
                            .map(|item| item.path.clone())
                            .collect();

                        let selection_state = get_directory_selection_state(
                            &dir_path,
                            &state_guard.filtered_file_list,
                            &state_guard.selected_files,
                        );

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
                "toggleExpansion" => {
                    if let Ok(path_str) = serde_json::from_value::<String>(msg.payload) {
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
                "expandCollapseAll" => {
                    if let Ok(expand) = serde_json::from_value::<bool>(msg.payload) {
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
                "selectAll" => {
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
                "deselectAll" => {
                    let mut state_guard = state.lock().unwrap();
                    state_guard.selected_files.clear();
                    proxy
                        .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                        .unwrap();
                }
                "generatePreview" => {
                    let (selected, root, config, all_files) = {
                        let state_guard = state.lock().unwrap();
                        (
                            get_selected_files_in_tree_order(&state_guard),
                            PathBuf::from(&state_guard.current_path),
                            state_guard.config.clone(),
                            state_guard.full_file_list.clone(),
                        )
                    };

                    let result = FileHandler::generate_concatenated_content_simple(
                        &selected,
                        &root,
                        config.include_tree_by_default,
                        all_files,
                        config.tree_ignore_patterns,
                        config.use_relative_paths,
                    )
                    .await;

                    match result {
                        Ok(content) => proxy
                            .send_event(UserEvent::ShowGeneratedContent(content))
                            .unwrap(),
                        Err(e) => proxy
                            .send_event(UserEvent::ShowError(e.to_string()))
                            .unwrap(),
                    }
                }
                "saveFile" => {
                    if let Some(content) = msg.payload.as_str() {
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
                "pickOutputDirectory" => {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        let mut state_guard = state.lock().unwrap();
                        state_guard.config.output_directory = Some(path);
                        proxy
                            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                            .unwrap();
                    }
                }
                "importConfig" => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("JSON", &["json"])
                        .pick_file()
                    {
                        if let Ok(config) = config::settings::import_config(&path) {
                            {
                                let mut state_guard = state.lock().unwrap();
                                state_guard.config = config;
                                config::settings::save_config(&state_guard.config).ok();
                            }
                            scan_directory(proxy, state).await;
                        } else {
                            proxy
                                .send_event(UserEvent::ShowError(
                                    "Failed to import config.".to_string(),
                                ))
                                .unwrap();
                        }
                    }
                }
                "exportConfig" => {
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
                _ => tracing::warn!("Unknown IPC command: {}", msg.command),
            }
        });
    }
}

fn handle_user_event(event: UserEvent, webview: &WebView) {
    let script = match event {
        UserEvent::StateUpdate(state) => {
            format!(
                "window.render({});",
                serde_json::to_string(&state).unwrap_or_default()
            )
        }
        UserEvent::ShowFilePreview {
            content,
            language,
            search_term,
            path, // HINZUGEFÜGT
        } => format!(
            "window.showPreviewContent({}, {}, {}, {});", // 4. Parameter hinzugefügt
            serde_json::to_string(&content).unwrap_or_default(),
            serde_json::to_string(&language).unwrap_or_default(),
            serde_json::to_string(&search_term).unwrap_or_default(),
            serde_json::to_string(&path).unwrap_or_default() // HINZUGEFÜGT
        ),
        UserEvent::ShowGeneratedContent(content) => format!(
            "window.showGeneratedContent({});",
            serde_json::to_string(&content).unwrap_or_default()
        ),
        UserEvent::ShowError(msg) => {
            format!(
                "window.showError({});",
                serde_json::to_string(&msg).unwrap_or_default()
            )
        }
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
    };
    webview.evaluate_script(&script).ok();
}

async fn scan_directory(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    {
        let mut state_lock = state.lock().unwrap();
        state_lock.is_scanning = true;
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_lock)))
            .unwrap();
    }

    let (path_str, ignore_patterns) = {
        let state_lock = state.lock().unwrap();
        (
            state_lock.current_path.clone(),
            state_lock.config.ignore_patterns.clone(),
        )
    };

    let scanner = DirectoryScanner::new(ignore_patterns);
    match scanner.scan_directory_basic(&PathBuf::from(path_str)).await {
        Ok(files) => {
            let mut state_lock = state.lock().unwrap();
            state_lock.full_file_list = files;
            apply_filters(&mut state_lock);
            state_lock.is_scanning = false;
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_lock)))
                .unwrap();
        }
        Err(e) => {
            let mut state_lock = state.lock().unwrap();
            state_lock.is_scanning = false;
            proxy
                .send_event(UserEvent::ShowError(e.to_string()))
                .unwrap();
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_lock)))
                .unwrap();
        }
    };
}

fn apply_filters(state: &mut AppState) {
    let filter = SearchFilter {
        query: state.search_query.clone(),
        extension: state.extension_filter.clone(),
        case_sensitive: state.config.case_sensitive_search,
        ignore_patterns: state.config.ignore_patterns.clone(),
    };

    let mut filtered = SearchEngine::filter_files(&state.full_file_list, &filter);

    if !state.content_search_query.is_empty() {
        filtered.retain(|item| state.content_search_results.contains(&item.path));
    }

    let root_path = PathBuf::from(&state.current_path);
    let required_dirs: HashSet<PathBuf> = filtered
        .par_iter()
        .flat_map(|item| {
            let mut parents = Vec::new();
            let mut current = item.path.parent();
            while let Some(parent) = current {
                if parent.starts_with(&root_path) {
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
            if let Some(dir_item) = state.full_file_list.iter().find(|i| i.path == dir_path) {
                filtered.push(dir_item.clone());
            }
        }
    }

    state.filtered_file_list = filtered;
}

fn auto_expand_for_matches(state: &mut AppState) {
    let root_path = PathBuf::from(&state.current_path);
    let matches: Vec<PathBuf> = state
        .filtered_file_list
        .iter()
        .filter(|item| {
            let file_name = item.path.file_name().unwrap_or_default().to_string_lossy();
            let name_match = if !state.search_query.is_empty() {
                if state.config.case_sensitive_search {
                    file_name.contains(&state.search_query)
                } else {
                    file_name
                        .to_lowercase()
                        .contains(&state.search_query.to_lowercase())
                }
            } else {
                false
            };
            let content_match = state.content_search_results.contains(&item.path);
            (name_match || content_match) && !item.is_directory
        })
        .map(|item| item.path.clone())
        .collect();

    for path in matches {
        let mut current = path.parent();
        while let Some(parent) = current {
            if parent.starts_with(&root_path) && parent != root_path {
                state.expanded_dirs.insert(parent.to_path_buf());
            } else {
                break;
            }
            current = parent.parent();
        }
    }
}

async fn search_in_files(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let (files_to_search, query, case_sensitive) = {
        let mut state_guard = state.lock().unwrap();
        if state_guard.content_search_query.is_empty() {
            state_guard.content_search_results.clear();
            apply_filters(&mut state_guard);
            proxy
                .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                .unwrap();
            return;
        }
        (
            state_guard.full_file_list.clone(),
            state_guard.content_search_query.clone(),
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

    {
        let mut state_guard = state.lock().unwrap();
        state_guard.content_search_results = matching_paths;
        apply_filters(&mut state_guard);
        auto_expand_for_matches(&mut state_guard);
        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    }
}

fn generate_ui_state(state: &AppState) -> UiState {
    let root = PathBuf::from(&state.current_path);

    let search_matches = if !state.content_search_query.is_empty() {
        state.content_search_results.clone()
    } else {
        HashSet::new()
    };

    let tree = build_tree_nodes(
        &state.filtered_file_list,
        &root,
        &state.selected_files,
        &state.expanded_dirs,
        &search_matches,
        &state.search_query,
        state.config.case_sensitive_search,
    );
    UiState {
        config: state.config.clone(),
        current_path: state.current_path.clone(),
        tree,
        total_files_found: state.full_file_list.len(),
        visible_files_count: state.filtered_file_list.len(),
        selected_files_count: state.selected_files.len(),
        is_scanning: state.is_scanning,
        status_message: if state.is_scanning {
            "Scanning...".to_string()
        } else {
            "Ready.".to_string()
        },
        search_query: state.search_query.clone(),
        extension_filter: state.extension_filter.clone(),
        content_search_query: state.content_search_query.clone(),
    }
}

fn get_directory_selection_state(
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

fn build_tree_nodes(
    items: &[FileItem],
    root_path: &Path,
    selected: &HashSet<PathBuf>,
    expanded: &HashSet<PathBuf>,
    content_search_matches: &HashSet<PathBuf>,
    filename_query: &str,
    case_sensitive: bool,
) -> Vec<TreeNode> {
    let mut nodes: HashMap<PathBuf, TreeNode> = HashMap::new();
    let mut children_map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    for item in items {
        let selection_state = if item.is_directory {
            get_directory_selection_state(&item.path, items, selected)
        } else {
            if selected.contains(&item.path) {
                "full".to_string()
            } else {
                "none".to_string()
            }
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

    build_level(&mut root_nodes_paths, &mut nodes, &children_map)
}

fn get_selected_files_in_tree_order(state: &AppState) -> Vec<PathBuf> {
    let mut selected_file_items: Vec<&FileItem> = state
        .full_file_list
        .iter()
        .filter(|item| !item.is_directory && state.selected_files.contains(&item.path))
        .collect();
    selected_file_items.sort_by_key(|a| a.path.clone());
    selected_file_items
        .into_iter()
        .map(|item| item.path.clone())
        .collect()
}

fn get_language_from_path(path: &Path) -> String {
    match path.extension().and_then(|s| s.to_str()) {
        Some("rs") => "rust",
        Some("js") | Some("mjs") | Some("cjs") => "javascript",
        Some("ts") | Some("tsx") => "typescript",
        Some("py") => "python",
        Some("html") | Some("htm") => "html",
        Some("css") => "css",
        Some("json") => "json",
        Some("md") => "markdown",
        Some("toml") => "toml",
        Some("yaml") | Some("yml") => "yaml",
        Some("sh") => "shell",
        Some("go") => "go",
        Some("java") => "java",
        Some("c") | Some("h") => "c",
        Some("cpp") | Some("hpp") | Some("cxx") | Some("hxx") => "cpp",
        _ => "plaintext",
    }
    .to_string()
}
