//! Responsible for transforming the `AppState` into a `UiState` view model.
//!
//! This module acts as a presentation layer, preparing data specifically for consumption
//! by the UI. It builds the file tree structure, applies filters, and computes various
//! display-related properties.

use crate::config::AppConfig;
use crate::core::{FileItem, SearchEngine, SearchFilter};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::state::AppState;

/// A serializable representation of the application state for the UI.
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
    pub current_config_filename: Option<String>,
    pub scan_progress: crate::core::ScanProgress,
    pub active_ignore_patterns: HashSet<String>,
}

/// A serializable representation of a single node in the file tree for the UI.
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

/// Creates the complete `UiState` from the current `AppState`.
pub fn generate_ui_state(state: &AppState) -> UiState {
    let root = PathBuf::from(&state.current_path);
    let search_matches = if !state.content_search_query.is_empty() {
        state.content_search_results.clone()
    } else {
        HashSet::new()
    };
    let tree = if state.is_scanning {
        Vec::new()
    } else {
        build_tree_nodes(
            &state.filtered_file_list,
            &root,
            &state.selected_files,
            &state.expanded_dirs,
            &search_matches,
            &state.search_query,
            state.config.case_sensitive_search,
            &state.previewed_file_path,
        )
    };
    let status_message = if state.is_scanning {
        format!(
            "Scanning... {} files processed. {} large files skipped ({})",
            state.scan_progress.files_scanned,
            state.scan_progress.large_files_skipped,
            state.scan_progress.current_scanning_path
        )
    } else {
        state.scan_progress.current_scanning_path.clone()
    };

    UiState {
        config: state.config.clone(),
        current_path: state.current_path.clone(),
        tree,
        total_files_found: state.full_file_list.len(),
        visible_files_count: state.filtered_file_list.len(),
        selected_files_count: state.selected_files.len(),
        is_scanning: state.is_scanning,
        status_message,
        search_query: state.search_query.clone(),
        extension_filter: state.extension_filter.clone(),
        content_search_query: state.content_search_query.clone(),
        current_config_filename: state.current_config_filename.clone(),
        scan_progress: state.scan_progress.clone(),
        active_ignore_patterns: state.active_ignore_patterns.clone(),
    }
}

/// Applies all current filters to the full file list to generate the visible list.
pub fn apply_filters(state: &mut AppState) {
    let filtered_list = apply_filters_on_data(
        &state.full_file_list,
        &PathBuf::from(&state.current_path),
        &state.config,
        &state.search_query,
        &state.extension_filter,
        &state.content_search_results,
    );
    state.filtered_file_list = filtered_list;
}

/// A "pure" function that takes data and returns a filtered list of `FileItem`s.
fn apply_filters_on_data(
    full_file_list: &[FileItem],
    root_path: &Path,
    config: &AppConfig,
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

/// Expands the parent directories of files that match the current search criteria.
pub fn auto_expand_for_matches(state: &mut AppState) {
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

/// Recursively builds the `TreeNode` structure for the UI from a flat list of `FileItem`s.
fn build_tree_nodes(
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

/// Determines the selection state of a directory ('none', 'partial', 'full').
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

/// Returns a list of the selected file paths in natural tree order.
pub fn get_selected_files_in_tree_order(state: &AppState) -> Vec<PathBuf> {
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

/// Determines the programming language from a file path for syntax highlighting.
pub fn get_language_from_path(path: &Path) -> String {
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
