use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;

use super::state::AppState;
use super::view_model::{apply_filters, auto_expand_for_matches, generate_ui_state};
use super::events::UserEvent;

use crate::core::{DirectoryScanner, ScanProgress};

/// Initiert einen Verzeichnisscan für einen gegebenen Pfad.
/// Diese Funktion bereitet den Zustand für den Scan vor und startet den `scan_directory_task`.
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

        tracing::info!("LOG: Spawne neuen scan_directory_task.");
        let handle = tokio::spawn(async move {
            scan_directory_task(proxy_clone, state_clone, new_cancel_flag).await;
        });
        state_guard.scan_task = Some(handle);

        proxy
            .send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard)))
            .unwrap();
    });
}

/// Der asynchrone Haupt-Task zum Scannen eines Verzeichnisses.
async fn scan_directory_task(
    proxy: EventLoopProxy<UserEvent>,
    state: Arc<Mutex<AppState>>,
    cancel_flag: Arc<AtomicBool>,
) {
    tracing::info!("LOG: TASK:: scan_directory_task gestartet.");
    let (path_str, ignore_patterns) = {
        let state_lock = state.lock().unwrap();
        (
            state_lock.current_path.clone(),
            state_lock.config.ignore_patterns.clone(),
        )
    };

    let path = PathBuf::from(&path_str);
    if !path.is_dir() {
        proxy.send_event(UserEvent::ShowError("Selected path is not a valid directory.".to_string())).unwrap();
        let mut state_lock = state.lock().unwrap();
        state_lock.cancel_current_scan();
        proxy.send_event(UserEvent::StateUpdate(generate_ui_state(&state_lock))).unwrap();
        return;
    }

    let scanner = DirectoryScanner::new(ignore_patterns);
    let progress_proxy = proxy.clone();
    let progress_callback = move |progress: ScanProgress| {
        let _ = progress_proxy.send_event(UserEvent::ScanProgress(progress));
    };

    tracing::info!("LOG: TASK:: Rufe scanner.scan_directory_with_progress auf...");
    let scan_result = scanner
        .scan_directory_with_progress(&path, cancel_flag, progress_callback)
        .await;
    tracing::info!("LOG: TASK:: scanner.scan_directory_with_progress ist zurückgekehrt.");

    let (final_files, active_patterns) = match scan_result {
        Ok(files) => files,
        Err(e) => {
            tracing::error!("LOG: TASK:: Scan mit Fehler beendet: {}", e);
            let mut state_lock = state.lock().unwrap();
            if !state_lock.is_scanning { return; }
            state_lock.scan_progress.current_scanning_path = format!("Scan failed: {}", e);
            state_lock.is_scanning = false;
            state_lock.scan_task = None;
            proxy.send_event(UserEvent::StateUpdate(generate_ui_state(&state_lock))).unwrap();
            return;
        }
    };
    
    tracing::info!("LOG: TASK:: Scan erfolgreich. {} Dateien gefunden.", final_files.len());
    
    let mut state_lock = state.lock().unwrap();
    if !state_lock.is_scanning {
        tracing::warn!("LOG: TASK:: Scan wurde zwischenzeitlich abgebrochen. Verwerfe Ergebnisse.");
        return;
    }

    state_lock.full_file_list = final_files;
    apply_filters(&mut state_lock);
    state_lock.active_ignore_patterns = active_patterns;
    state_lock.is_scanning = false;
    state_lock.scan_progress.current_scanning_path = format!(
        "Scan complete. Found {} visible items.",
        state_lock.filtered_file_list.len()
    );
    state_lock.scan_task = None;
    proxy.send_event(UserEvent::StateUpdate(generate_ui_state(&state_lock))).unwrap();
    tracing::info!("LOG: TASK:: Finaler Zustand wurde aktualisiert und an die UI gesendet.");
}


/// Führt eine Inhaltssuche in allen nicht-binären Dateien durch.
pub async fn search_in_files(proxy: EventLoopProxy<UserEvent>, state: Arc<Mutex<AppState>>) {
    let (files_to_search, query, case_sensitive) = {
        let mut state_guard = state.lock().unwrap();
        if state_guard.content_search_query.is_empty() {
            state_guard.content_search_results.clear();
            apply_filters(&mut state_guard);
            proxy.send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard))).unwrap();
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
            if item.is_directory || item.is_binary { return None; }
            if let Ok(content) = std::fs::read_to_string(&item.path) {
                let found = if case_sensitive {
                    content.contains(&query)
                } else {
                    content.to_lowercase().contains(&query.to_lowercase())
                };
                if found { Some(item.path) } else { None }
            } else {
                None
            }
        })
        .collect();

    let mut state_guard = state.lock().unwrap();
    state_guard.content_search_results = matching_paths;
    apply_filters(&mut state_guard);
    auto_expand_for_matches(&mut state_guard);
    proxy.send_event(UserEvent::StateUpdate(generate_ui_state(&state_guard))).unwrap();
}
