#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod core;
mod utils;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::WindowBuilder,
};
use wry::{WebView, WebViewBuilder};

use crate::core::{DirectoryScanner, FileHandler, FileItem};

// --- KORREKTUR HIER ---
#[derive(Debug, Serialize, Clone)]
struct TreeNode {
    name: String,
    path: PathBuf,
    is_binary: bool,
    children: Vec<TreeNode>,
}

#[derive(Debug, Clone)]
enum UserEvent {
    UpdateFileTree(Result<Vec<TreeNode>, String>),
    UpdateGeneratedContent(Result<String, String>),
    FileSaveStatus(bool, String),
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
        .with_inner_size(tao::dpi::LogicalSize::new(1200, 800))
        .build(&event_loop)
        .expect("Failed to build Window");
    let proxy = event_loop.create_proxy();
    let html_content = include_str!("ui/index.html");
    let css_content = include_str!("ui/style.css");
    let js_content = include_str!("ui/script.js");
    let final_html = html_content
        .replace("/*INJECT_CSS*/", css_content)
        .replace("/*INJECT_JS*/", js_content);
    let webview = WebViewBuilder::new(&window)
        .with_html(final_html)
        .with_ipc_handler(move |message: String| handle_ipc_message(message, proxy.clone()))
        .with_devtools(true)
        .build()
        .expect("Failed to build WebView");
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit
        }
        if let Event::UserEvent(user_event) = event {
            handle_user_event(user_event, &webview);
        }
    });
}

fn handle_ipc_message(message: String, proxy: EventLoopProxy<UserEvent>) {
    if let Ok(msg) = serde_json::from_str::<IpcMessage>(&message) {
        match msg.command.as_str() {
            "selectDirectory" => {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    // Send path to JS to trigger the scan
                    if let Ok(path_str) = serde_json::to_string(&path.to_string_lossy()) {
                        let _ = proxy.send_event(UserEvent::UpdateFileTree(Err(path_str)));
                    }
                }
            }
            "scanDirectory" => {
                if let Some(path_str) = msg.payload.as_str() {
                    let path = PathBuf::from(path_str);
                    tokio::spawn(async move {
                        let config = config::AppConfig::load().unwrap_or_default();
                        let scanner = DirectoryScanner::new(config.ignore_patterns);
                        let result = scanner.scan_directory_basic(&path).await;
                        let tree = result.map(build_file_tree);
                        let _ = proxy
                            .send_event(UserEvent::UpdateFileTree(tree.map_err(|e| e.to_string())));
                    });
                }
            }
            "generateContent" => {
                if let Ok(paths) = serde_json::from_value::<Vec<String>>(msg.payload) {
                    let file_paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                    let root_path = find_common_root(&file_paths).unwrap_or_default();
                    tokio::spawn(async move {
                        let result = FileHandler::generate_concatenated_content_simple(
                            &file_paths,
                            &root_path,
                        )
                        .await;
                        proxy
                            .send_event(UserEvent::UpdateGeneratedContent(
                                result.map_err(|e| e.to_string()),
                            ))
                            .unwrap();
                    });
                }
            }
            "saveFile" => {
                if let Some(content) = msg.payload.as_str() {
                    let content_clone = content.to_string();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Text", &["txt"])
                        .set_file_name("cfc-output.txt")
                        .save_file()
                    {
                        tokio::spawn(async move {
                            let (success, path_str) = match std::fs::write(&path, content_clone) {
                                Ok(_) => (true, path.to_string_lossy().to_string()),
                                Err(_) => (false, "".to_string()),
                            };
                            proxy
                                .send_event(UserEvent::FileSaveStatus(success, path_str))
                                .unwrap();
                        });
                    } else {
                        proxy
                            .send_event(UserEvent::FileSaveStatus(false, "cancelled".to_string()))
                            .unwrap();
                    }
                }
            }
            _ => tracing::warn!("Unknown IPC command: {}", msg.command),
        }
    }
}

fn handle_user_event(event: UserEvent, webview: &WebView) {
    match event {
        UserEvent::UpdateFileTree(result) => {
            let script = match result {
                // Ein "Err" wird hier zweckentfremdet, um den initialen Pfad an JS zu senden
                Err(path_json) => format!("window.setScannedPath({});", path_json),
                Ok(tree) => format!(
                    "window.updateFileTree({});",
                    serde_json::to_string(&tree).unwrap()
                ),
            };
            let _ = webview.evaluate_script(&script);
        }
        UserEvent::UpdateGeneratedContent(result) => {
            let script = match result {
                Ok(content) => format!(
                    "window.showGeneratedContent({});",
                    serde_json::to_string(&content).unwrap()
                ),
                Err(e) => format!(
                    "window.showError('Generation failed: {}');",
                    serde_json::to_string(&e).unwrap()
                ),
            };
            let _ = webview.evaluate_script(&script);
        }
        UserEvent::FileSaveStatus(success, path) => {
            let script = format!(
                "window.fileSaveStatus({}, {});",
                success,
                serde_json::to_string(&path).unwrap()
            );
            let _ = webview.evaluate_script(&script);
        }
    }
}

fn build_file_tree(items: Vec<FileItem>) -> Vec<TreeNode> {
    let mut nodes: HashMap<PathBuf, TreeNode> = HashMap::new();
    // Create all file nodes first
    for item in &items {
        if item.is_directory {
            continue;
        }
        let node = TreeNode {
            name: item.path.file_name().unwrap().to_string_lossy().to_string(),
            path: item.path.clone(),
            is_binary: item.is_binary,
            children: Vec::new(),
        };
        nodes.insert(item.path.clone(), node);
    }

    let mut parent_map: HashMap<PathBuf, Vec<TreeNode>> = HashMap::new();
    for item in items {
        if item.is_directory {
            continue;
        }
        if let Some(parent) = item.parent {
            if let Some(node) = nodes.remove(&item.path) {
                parent_map.entry(parent).or_default().push(node);
            }
        }
    }

    let all_paths: Vec<PathBuf> = parent_map.keys().cloned().collect();
    let root = find_common_root(&all_paths).unwrap_or_default();

    build_level(&root, &mut parent_map)
}

fn build_level(
    parent_path: &Path,
    parent_map: &mut HashMap<PathBuf, Vec<TreeNode>>,
) -> Vec<TreeNode> {
    let mut children = Vec::new();

    // Find all directories that are direct children of parent_path
    let subdirs: Vec<PathBuf> = parent_map
        .keys()
        .filter(|p| p.parent() == Some(parent_path))
        .cloned()
        .collect();

    for subdir in subdirs {
        let subdir_children = build_level(&subdir, parent_map);
        if !subdir_children.is_empty() {
            children.push(TreeNode {
                name: subdir.file_name().unwrap().to_string_lossy().to_string(),
                path: subdir,
                is_binary: true, // not relevant for dirs
                children: subdir_children,
            });
        }
    }

    // Add files at this level
    if let Some(mut files) = parent_map.remove(parent_path) {
        children.append(&mut files);
    }

    children.sort_by(|a, b| {
        let a_is_dir = !a.children.is_empty();
        let b_is_dir = !b.children.is_empty();
        if a_is_dir != b_is_dir {
            b_is_dir.cmp(&a_is_dir) // true (dir) comes first
        } else {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        }
    });

    children
}

fn find_common_root(paths: &[PathBuf]) -> Option<PathBuf> {
    if paths.is_empty() {
        return None;
    }
    let mut common_path = paths[0].clone();
    if common_path.is_file() {
        if let Some(parent) = common_path.parent() {
            common_path = parent.to_path_buf();
        }
    }
    loop {
        if paths.iter().all(|p| p.starts_with(&common_path)) {
            return Some(common_path);
        }
        if !common_path.pop() {
            break;
        }
    }
    None
}
