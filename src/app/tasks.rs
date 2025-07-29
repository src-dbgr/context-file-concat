//! Contains long-running, asynchronous tasks that the application can perform.
//!
//! These tasks, such as scanning a directory or searching file contents, are designed
//! to run in the background without blocking the UI. They communicate their progress
//! and results back to the main application thread via `UserEvent`s.

use rayon::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::events::UserEvent;
use super::proxy::EventProxy;
use super::state::AppState;
use super::view_model::{
    apply_filters, auto_expand_for_matches, generate_ui_state, get_selected_files_in_tree_order,
};

use crate::app::view_model::TreeNode;
use crate::core::{CoreError, DirectoryScanner, FileHandler, FileItem, ScanProgress};

/// Initiates a directory scan for a given path.
///
/// This function sets up the application state for the scan and spawns the `scan_directory_task`.
pub fn start_scan_on_path<P: EventProxy>(path: PathBuf, proxy: P, state: Arc<Mutex<AppState>>) {
    tokio::spawn(async move {
        let directory_path = if path.is_dir() {
            path
        } else {
            path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
        };

        if !directory_path.is_dir() {
            let event = UserEvent::ShowError("Dropped item is not a valid directory.".to_string());
            proxy.send_event(event);
            return;
        }

        let mut state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        state_guard.cancel_current_scan();
        state_guard.active_ignore_patterns.clear();

        state_guard.current_path = directory_path.to_string_lossy().to_string();
        state_guard.config.last_directory = Some(directory_path);
        crate::config::settings::save_config(&state_guard.config).ok();

        state_guard.is_scanning = true;
        state_guard.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Initializing scan...".to_string(),
        };

        let new_cancel_flag = Arc::new(AtomicBool::new(false));
        state_guard.scan_cancellation_flag = new_cancel_flag.clone();

        let proxy_clone = proxy.clone();
        let state_clone = state.clone();

        tracing::info!("LOG: Spawning new scan_directory_task.");
        let handle = tokio::spawn(async move {
            scan_directory_task(proxy_clone, state_clone, new_cancel_flag).await;
        });
        state_guard.scan_task = Some(handle);

        let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
        proxy.send_event(event);
    });
}

struct PostProcessingResult {
    full_file_list: Vec<FileItem>,
    filtered_file_list: Vec<FileItem>,
    active_ignore_patterns: HashSet<String>,
    tree: Vec<TreeNode>, // Wir geben nur den berechneten Baum zurück
}

/// The main asynchronous task for scanning a directory.
///
/// This function performs the core scanning logic and updates the application state
/// with the results or any errors that occur.
async fn scan_directory_task<P: EventProxy>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    cancel_flag: Arc<AtomicBool>,
) {
    tracing::info!("LOG: TASK:: scan_directory_task started.");
    let (path_str, ignore_patterns) = {
        let state_lock = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        (
            state_lock.current_path.clone(),
            state_lock.config.ignore_patterns.clone(),
        )
    };

    let path = PathBuf::from(&path_str);
    if !path.is_dir() {
        let event = UserEvent::ShowError("Selected path is not a valid directory.".to_string());
        proxy.send_event(event);

        let mut state_lock = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        state_lock.cancel_current_scan();
        let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_lock)));
        proxy.send_event(event);
        return;
    }

    let scanner = DirectoryScanner::new(ignore_patterns);
    let progress_proxy = proxy.clone();
    let progress_callback = move |progress: ScanProgress| {
        progress_proxy.send_event(UserEvent::ScanProgress(progress));
    };

    tracing::info!("LOG: TASK:: Calling scanner.scan_directory_with_progress...");
    let scan_result = scanner
        .scan_directory_with_progress(&path, cancel_flag, progress_callback)
        .await;
    tracing::info!("LOG: TASK:: scanner.scan_directory_with_progress has returned.");

    if let Ok((scan_files, scan_patterns)) = scan_result {
        tracing::info!(
            "LOG: TASK:: Scan successful. {} files found. Offloading post-processing.",
            scan_files.len()
        );

        let (
            config,
            current_path,
            search_query,
            ext_filter,
            content_query,
            content_results,
            selected,
            expanded,
            previewed,
        ) = {
            let s = state.lock().expect("Mutex was poisoned.");
            (
                s.config.clone(),
                s.current_path.clone(),
                s.search_query.clone(),
                s.extension_filter.clone(),
                s.content_search_query.clone(),
                s.content_search_results.clone(),
                s.selected_files.clone(),
                s.expanded_dirs.clone(),
                s.previewed_file_path.clone(),
            )
        };

        let post_processing_task = tokio::task::spawn_blocking(move || {
            let mut state_for_processing = AppState {
                full_file_list: scan_files,
                config,
                current_path,
                search_query,
                extension_filter: ext_filter,
                content_search_query: content_query,
                content_search_results: content_results,
                selected_files: selected,
                expanded_dirs: expanded,
                previewed_file_path: previewed,
                ..Default::default()
            };

            apply_filters(&mut state_for_processing);

            // Berechne nur den Baum, nicht das ganze UiState
            let tree = generate_ui_state(&state_for_processing).tree;

            PostProcessingResult {
                full_file_list: state_for_processing.full_file_list,
                filtered_file_list: state_for_processing.filtered_file_list,
                active_ignore_patterns: scan_patterns,
                tree,
            }
        });

        match post_processing_task.await {
            Ok(result) => {
                let mut s = state.lock().expect("Mutex was poisoned.");
                if !s.is_scanning {
                    tracing::warn!("LOG: TASK:: Scan was cancelled during post-processing. Discarding results.");
                    return;
                }

                s.full_file_list = result.full_file_list;
                s.filtered_file_list = result.filtered_file_list;
                s.active_ignore_patterns = result.active_ignore_patterns;
                s.is_scanning = false;
                s.scan_task = None;
                s.scan_progress.current_scanning_path = format!(
                    "Scan complete. Found {} visible items.",
                    s.filtered_file_list.len()
                );

                // KORREKTUR: Erstelle das finale UiState HIER aus dem finalen AppState
                let mut final_ui_state = generate_ui_state(&s);
                final_ui_state.tree = result.tree; // Setze den bereits berechneten Baum ein

                proxy.send_event(UserEvent::StateUpdate(Box::new(final_ui_state)));
                tracing::info!("LOG: TASK:: Post-processing complete. Final state sent to UI.");
            }
            Err(e) => {
                tracing::error!("Post-processing task failed: {}", e);
            }
        }
    } else if let Err(e) = scan_result {
        tracing::error!("LOG: TASK:: Scan finished with error: {e}");
        let mut state_lock = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        if !state_lock.is_scanning {
            return;
        }
        state_lock.scan_progress.current_scanning_path = format!("Scan failed: {e}");
        state_lock.is_scanning = false;
        state_lock.scan_task = None;
        let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_lock)));
        proxy.send_event(event);
    }
}

/// Performs a content search across all non-binary files.
///
/// This function runs in parallel using Rayon for performance.
pub async fn search_in_files<P: EventProxy>(proxy: P, state: Arc<Mutex<AppState>>) {
    let (files_to_search, query, case_sensitive) = {
        let mut state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        if state_guard.content_search_query.is_empty() {
            state_guard.content_search_results.clear();
            apply_filters(&mut state_guard);
            let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
            proxy.send_event(event);
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

    let mut state_guard = state
        .lock()
        .expect("Mutex was poisoned. This should not happen.");
    state_guard.content_search_results = matching_paths;
    apply_filters(&mut state_guard);
    auto_expand_for_matches(&mut state_guard);
    let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
    proxy.send_event(event);
}

/// The main asynchronous task for generating the concatenated file content.
///
/// This function performs the core file reading and concatenation logic and updates
/// the application state with the results or any errors that occur. It is cancellable.
pub async fn generation_task<P: EventProxy>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    cancel_flag: Arc<AtomicBool>,
) {
    // Get the necessary data for the task from the main state.
    let (selected, root, config, all_scanned_files) = {
        let state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        (
            get_selected_files_in_tree_order(&state_guard),
            PathBuf::from(&state_guard.current_path),
            state_guard.config.clone(),
            state_guard.full_file_list.clone(),
        )
    };

    // Perform the potentially long-running file I/O operations.
    let result = FileHandler::generate_concatenated_content_simple(
        &selected,
        &root,
        config.include_tree_by_default,
        all_scanned_files,
        config.tree_ignore_patterns,
        config.use_relative_paths,
        cancel_flag,
    )
    .await;

    // Lock the state again to update it after the task is done.
    let mut state_guard = state
        .lock()
        .expect("Mutex was poisoned. This should not happen.");

    // The task is finished, so we can clear the handle.
    state_guard.generation_task = None;

    // Check if the operation was cancelled. If so, we don't show an error.
    // The UI state will be updated in any case.
    match result {
        Ok(content) => {
            proxy.send_event(UserEvent::ShowGeneratedContent(content));
        }
        Err(CoreError::Cancelled) => {
            state_guard.scan_progress.current_scanning_path = "Generation cancelled.".to_string();
        }
        Err(e) => {
            proxy.send_event(UserEvent::ShowError(e.to_string()));
        }
    }

    // This is now the single point of truth for resetting the generating state.
    state_guard.is_generating = false;
    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
        &state_guard,
    ))));
}
