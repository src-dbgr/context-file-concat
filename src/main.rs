#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod core;
mod utils;

use crate::config::AppConfig;
use crate::core::{
    build_globset_from_patterns, DirectoryScanner, FileHandler, FileItem, SearchEngine,
    SearchFilter,
}; // TreeGenerator entfernt
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
}

#[derive(Serialize, Clone, Debug)]
struct TreeNode {
    name: String,
    path: PathBuf,
    is_directory: bool,
    is_binary: bool,
    size: u64,
    children: Vec<TreeNode>,
    is_selected: bool,
    is_expanded: bool,
}

struct AppState {
    config: AppConfig,
    current_path: String,
    full_file_list: Vec<FileItem>,
    filtered_file_list: Vec<FileItem>,
    selected_files: HashSet<PathBuf>,
    expanded_dirs: HashSet<PathBuf>,
    is_scanning: bool,
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
        }
    }
}

#[derive(Debug)]
enum UserEvent {
    StateUpdate(UiState),
    ShowFilePreview(String),
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
                "initialize" => {
                    let should_scan = {
                        let mut state_guard = state.lock().unwrap();
                        if let Some(last_dir) = state_guard.config.last_directory.clone() {
                            if last_dir.exists() {
                                state_guard.current_path = last_dir.to_string_lossy().to_string();
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    };

                    if should_scan {
                        scan_directory(proxy.clone(), state.clone()).await;
                    } else {
                        let state_guard = state.lock().unwrap();
                        proxy
                            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                            .unwrap();
                    }
                }
                "selectDirectory" => {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        {
                            let mut state_guard = state.lock().unwrap();
                            state_guard.current_path = path.to_string_lossy().to_string();
                            state_guard.config.last_directory = Some(path);
                            config::settings::save_config(&state_guard.config).ok();
                        }
                        scan_directory(proxy, state).await;
                    }
                }
                "updateConfig" => {
                    if let Ok(new_config) = serde_json::from_value(msg.payload) {
                        let mut state_guard = state.lock().unwrap();
                        state_guard.config = new_config;
                        config::settings::save_config(&state_guard.config).ok();
                        apply_filters(&mut state_guard);
                        proxy
                            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
                            .unwrap();
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
                        let all_selected = files_in_dir
                            .iter()
                            .all(|f| state_guard.selected_files.contains(f));

                        if all_selected && !files_in_dir.is_empty() {
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
                    // *** KORREKTUR HIER ***
                    // 1. Sammle die Pfade, die hinzugefügt werden sollen.
                    let paths_to_select: Vec<PathBuf> = state_guard
                        .filtered_file_list
                        .iter()
                        .filter(|item| !item.is_directory)
                        .map(|item| item.path.clone())
                        .collect();

                    // 2. Füge die gesammelten Pfade hinzu, nachdem die Iteration beendet ist.
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
                "loadFilePreview" => {
                    if let Ok(path_str) = serde_json::from_value::<String>(msg.payload) {
                        match FileHandler::get_file_preview(&PathBuf::from(path_str), 1500) {
                            Ok(content) => proxy
                                .send_event(UserEvent::ShowFilePreview(content))
                                .unwrap(),
                            Err(e) => proxy
                                .send_event(UserEvent::ShowError(e.to_string()))
                                .unwrap(),
                        }
                    }
                }
                "generatePreview" => {
                    let (
                        selected_files_ordered,
                        root_path,
                        include_tree,
                        items_for_tree,
                        tree_ignore,
                    ) = {
                        let state_guard = state.lock().unwrap();
                        let ignore_set =
                            build_globset_from_patterns(&state_guard.config.ignore_patterns);
                        let items = state_guard
                            .full_file_list
                            .iter()
                            .filter(|item| !ignore_set.is_match(&item.path))
                            .cloned()
                            .collect();
                        (
                            get_selected_files_in_tree_order(&state_guard),
                            PathBuf::from(&state_guard.current_path),
                            state_guard.config.include_tree_by_default,
                            items,
                            state_guard.config.ignore_patterns.clone(),
                        )
                    };

                    let result = FileHandler::generate_concatenated_content_simple(
                        &selected_files_ordered,
                        &root_path,
                        include_tree,
                        items_for_tree,
                        tree_ignore,
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
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Text File", &["txt"])
                            .set_file_name("cfc_output.txt")
                            .save_file()
                        {
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
        UserEvent::ShowFilePreview(content) => format!(
            "window.showPreviewContent({});",
            serde_json::to_string(&content).unwrap_or_default()
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
        query: String::new(),     // Placeholder for future UI implementation
        extension: String::new(), // Placeholder for future UI implementation
        case_sensitive: state.config.case_sensitive_search,
        ignore_patterns: state.config.ignore_patterns.clone(),
    };
    state.filtered_file_list = SearchEngine::filter_files(&state.full_file_list, &filter);
}

fn generate_ui_state(state: &AppState) -> UiState {
    let root = PathBuf::from(&state.current_path);
    let tree = build_tree_nodes(
        &state.filtered_file_list,
        &root,
        &state.selected_files,
        &state.expanded_dirs,
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
    }
}

fn build_tree_nodes(
    items: &[FileItem],
    root_path: &Path,
    selected: &HashSet<PathBuf>,
    expanded: &HashSet<PathBuf>,
) -> Vec<TreeNode> {
    let mut nodes: HashMap<PathBuf, TreeNode> = HashMap::new();
    let mut children_map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    for item in items {
        nodes.insert(
            item.path.clone(),
            TreeNode {
                name: item
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                path: item.path.clone(),
                is_directory: item.is_directory,
                is_binary: item.is_binary,
                size: item.size,
                children: Vec::new(),
                is_selected: if item.is_directory {
                    false
                } else {
                    selected.contains(&item.path)
                },
                is_expanded: expanded.contains(&item.path),
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
