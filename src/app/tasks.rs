//! Contains long-running, asynchronous tasks that the application can perform.
//!
//! These tasks, such as scanning a directory or searching file contents, are designed
//! to run in the background without blocking the UI. They communicate their progress
//! and results back to the main application thread via `UserEvent`s.

use rayon::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::events::UserEvent;
use super::proxy::EventProxy;
use super::state::AppState;
use super::view_model::{
    apply_filters, auto_expand_for_matches, generate_ui_state, get_selected_files_in_tree_order,
};

use crate::core::{CoreError, DirectoryScanner, FileHandler, FileItem, ScanProgress};

/// Initiates a proactive, two-phase directory scan.
///
/// Phase 1: An immediate, shallow scan (depth=1) to quickly populate the UI.
/// Phase 2: A full, recursive scan in the background to index the entire directory for global actions.
pub fn start_scan_on_path<P: EventProxy>(path: PathBuf, proxy: P, state: Arc<Mutex<AppState>>) {
    let proxy_clone = proxy.clone();
    let state_clone = state.clone();

    tokio::spawn(async move {
        let directory_path = if path.is_dir() {
            path
        } else {
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| path.clone())
        };

        if !directory_path.is_dir() {
            let event = UserEvent::ShowError("Dropped item is not a valid directory.".to_string());
            proxy.send_event(event);
            return;
        }

        // --- Setup State for the entire scanning process ---
        let new_cancel_flag = {
            let mut state_guard = state.lock().expect("Mutex was poisoned");

            // This resets everything, including scan tasks.
            state_guard.reset_directory_state();

            state_guard.current_path = directory_path.to_string_lossy().to_string();
            state_guard.config.last_directory = Some(directory_path.clone());
            crate::config::settings::save_config(&state_guard.config).ok();

            state_guard.is_scanning = true; // The overall process is now scanning
            state_guard.is_fully_scanned = false; // Reset the full scan flag

            let flag = Arc::new(AtomicBool::new(false));
            state_guard.scan_cancellation_flag = flag.clone();
            flag
        };

        // Send an initial UI update to show the "Scanning..." state immediately.
        proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
            &state.lock().unwrap(),
        ))));

        // --- Spawn the main orchestration task ---
        let handle = tokio::spawn(async move {
            proactive_scan_task(proxy_clone, state_clone, directory_path, new_cancel_flag).await;
        });

        // Store the handle in the state so the entire process can be cancelled.
        let mut state_guard = state.lock().expect("Mutex was poisoned");
        state_guard.scan_task = Some(handle);
    });
}

/// The core orchestration logic for the proactive, two-phase scan.
/// This task runs sequentially to avoid race conditions but provides a responsive UI
/// by updating after the initial shallow scan.
async fn proactive_scan_task<P: EventProxy>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    path: PathBuf,
    cancel_flag: Arc<AtomicBool>,
) {
    let (ignore_patterns, _) = {
        let state_lock = state.lock().unwrap();
        (
            state_lock.config.ignore_patterns.clone(),
            state_lock.config.clone(),
        )
    };

    let scanner = DirectoryScanner::new(ignore_patterns);

    // --- Phase 1: Shallow Scan ---
    let progress_proxy_shallow = proxy.clone();
    let scan_result_shallow = scanner
        .scan_directory_with_progress(&path, Some(1), cancel_flag.clone(), move |p| {
            progress_proxy_shallow.send_event(UserEvent::ScanProgress(p))
        })
        .await;

    if cancel_flag.load(Ordering::Relaxed) {
        tracing::info!("Scan cancelled after shallow scan phase.");
        return;
    }

    // Process shallow scan results to provide immediate UI feedback
    match scan_result_shallow {
        Ok((files, patterns)) => {
            let mut s = state.lock().unwrap();
            s.full_file_list = files;
            s.active_ignore_patterns = patterns;
            s.loaded_dirs.insert(path.clone()); // Mark root as loaded
            apply_filters(&mut s);
            proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&s))));
        }
        Err(e) => {
            handle_scan_error(e, &state, &proxy);
            return; // Abort on shallow scan failure
        }
    }

    if cancel_flag.load(Ordering::Relaxed) {
        tracing::info!("Scan cancelled before deep scan phase.");
        return;
    }

    // --- Phase 2: Deep Background Scan (Indexing) ---
    // The UI is now responsive. We continue with the full scan in the background.
    let progress_proxy_deep = proxy.clone();
    let scan_result_deep = scanner
        .scan_directory_with_progress(&path, None, cancel_flag.clone(), move |p| {
            progress_proxy_deep.send_event(UserEvent::ScanProgress(p))
        })
        .await;

    if cancel_flag.load(Ordering::Relaxed) {
        tracing::info!("Scan cancelled during deep scan phase.");
        return;
    }

    // Process deep scan results to finalize the state
    match scan_result_deep {
        Ok((files, patterns)) => {
            let mut s = state.lock().unwrap();
            if cancel_flag.load(Ordering::Relaxed) {
                return;
            }

            s.full_file_list = files; // Replace shallow list with the full one
            s.active_ignore_patterns.extend(patterns);
            s.is_fully_scanned = true;

            // Mark all found directories as loaded
            s.loaded_dirs = s
                .full_file_list
                .iter()
                .filter(|i| i.is_directory)
                .map(|i| i.path.clone())
                .collect();

            apply_filters(&mut s);

            // Finalize state
            s.is_scanning = false;
            s.scan_task = None;
            s.scan_progress.current_scanning_path = format!(
                "Indexing complete. Found {} visible items.",
                s.filtered_file_list.len()
            );

            proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&s))));
        }
        Err(e) => {
            handle_scan_error(e, &state, &proxy);
        }
    }
}

/// Helper to handle scan errors consistently.
fn handle_scan_error<P: EventProxy>(error: CoreError, state: &Arc<Mutex<AppState>>, proxy: &P) {
    tracing::error!("LOG: TASK:: Scan finished with error: {}", error);
    let mut state_lock = state.lock().expect("Mutex was poisoned");

    // Ensure we don't act on an already cancelled scan
    if !state_lock.is_scanning {
        return;
    }

    state_lock.scan_progress.current_scanning_path = format!("Scan failed: {}", error);
    state_lock.is_scanning = false;
    state_lock.scan_task = None;
    let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_lock)));
    proxy.send_event(event);
}

/// Initiates a scan for a specific subdirectory level (lazy loading).
pub fn start_lazy_load_scan<P: EventProxy>(path: PathBuf, proxy: P, state: Arc<Mutex<AppState>>) {
    tokio::spawn(async move {
        let (ignore_patterns, is_scanning) = {
            let state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            (
                state_guard.config.ignore_patterns.clone(),
                state_guard.is_scanning,
            )
        };

        if is_scanning {
            tracing::warn!("Attempted to lazy load while a full scan was in progress. Ignoring.");
            return;
        }

        let new_cancel_flag = Arc::new(AtomicBool::new(false));
        let proxy_clone = proxy.clone();
        let state_clone = state.clone();

        tracing::info!("LOG: Spawning new lazy_load_task for path: {:?}", path);

        tokio::spawn(async move {
            lazy_load_task(
                path,
                ignore_patterns,
                proxy_clone,
                state_clone,
                new_cancel_flag,
            )
            .await;
        });
    });
}

/// The asynchronous task for scanning a single directory level and appending the results.
async fn lazy_load_task<P: EventProxy>(
    path_to_load: PathBuf,
    ignore_patterns: HashSet<String>,
    proxy: P,
    state: Arc<Mutex<AppState>>,
    cancel_flag: Arc<AtomicBool>,
) {
    let scanner = DirectoryScanner::new(ignore_patterns);

    let scan_result = scanner
        .scan_directory_with_progress(&path_to_load, Some(1), cancel_flag, |_| {})
        .await;

    match scan_result {
        Ok((new_items, new_active_patterns)) => {
            tracing::info!(
                "LOG: TASK:: Lazy load successful. {} new items found for {:?}.",
                new_items.len(),
                path_to_load
            );

            super::helpers::with_state_and_notify(&state, &proxy, |s| {
                // *** FIX STARTS HERE ***
                // 1. Mark as loaded to prevent re-loading.
                s.loaded_dirs.insert(path_to_load.clone());
                // 2. Automatically expand the directory to solve the double-click issue.
                s.expanded_dirs.insert(path_to_load);
                // *** FIX ENDS HERE ***

                s.active_ignore_patterns.extend(new_active_patterns);

                let existing_paths: HashSet<PathBuf> = s
                    .full_file_list
                    .iter()
                    .map(|item| item.path.clone())
                    .collect();
                for item in new_items {
                    if !existing_paths.contains(&item.path) {
                        s.full_file_list.push(item);
                    }
                }

                apply_filters(s);
            });
        }
        Err(e) => {
            tracing::error!("LOG: TASK:: Lazy load failed for {:?}: {}", path_to_load, e);
            proxy.send_event(UserEvent::ShowError(format!(
                "Failed to load directory {}: {}",
                path_to_load.display(),
                e
            )));
        }
    }
}

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
