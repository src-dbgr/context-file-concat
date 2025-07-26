use crate::config::AppConfig;
use crate::core::{FileItem, ScanProgress};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Hält den kompletten, veränderbaren Zustand der Anwendung.
pub struct AppState {
    pub config: AppConfig,
    pub current_path: String,
    pub full_file_list: Vec<FileItem>,
    pub filtered_file_list: Vec<FileItem>,
    pub selected_files: HashSet<PathBuf>,
    pub expanded_dirs: HashSet<PathBuf>,
    pub is_scanning: bool,
    pub search_query: String,
    pub extension_filter: String,
    pub content_search_query: String,
    pub content_search_results: HashSet<PathBuf>,
    pub current_config_filename: Option<String>,
    pub scan_progress: ScanProgress,
    // pub auto_load_last_directory: bool, // <-- DIESE ZEILE LÖSCHEN
    pub previewed_file_path: Option<PathBuf>,
    pub scan_task: Option<JoinHandle<()>>,
    pub scan_cancellation_flag: Arc<AtomicBool>,
    pub active_ignore_patterns: HashSet<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            config: AppConfig::load().unwrap_or_default(),
            // auto_load_last_directory, // <-- DIESE ZEILE LÖSCHEN
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
            current_config_filename: None,
            scan_progress: ScanProgress {
                files_scanned: 0,
                large_files_skipped: 0,
                current_scanning_path: "Ready.".to_string(),
            },
            previewed_file_path: None,
            scan_task: None,
            scan_cancellation_flag: Arc::new(AtomicBool::new(false)),
            active_ignore_patterns: HashSet::new(),
        }
    }

    /// Bricht den aktuellen Scan-Task ab, falls vorhanden, und setzt den Scan-Zustand zurück.
    pub fn cancel_current_scan(&mut self) {
        tracing::info!("LOG: AppState::cancel_current_scan aufgerufen.");
        if let Some(handle) = self.scan_task.take() {
            tracing::info!("LOG: Aktiver Scan-Task gefunden. Rufe handle.abort() auf...");
            handle.abort();
            tracing::info!("LOG: handle.abort() wurde aufgerufen.");
        } else {
            tracing::warn!(
                "LOG: cancel_current_scan aufgerufen, aber kein aktiver Scan-Task gefunden."
            );
        }

        tracing::info!("LOG: Setze Stopp-Signal (AtomicBool) auf true.");
        self.scan_cancellation_flag.store(true, Ordering::Relaxed);

        self.is_scanning = false;
        self.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Scan cancelled.".to_string(),
        };
        tracing::info!("LOG: AppState wurde auf 'cancelled' zurückgesetzt.");
    }

    /// Setzt den gesamten Zustand zurück, der sich auf ein geladenes Verzeichnis bezieht.
    pub fn reset_directory_state(&mut self) {
        self.cancel_current_scan();

        self.current_path = String::new();
        self.full_file_list.clear();
        self.filtered_file_list.clear();
        self.selected_files.clear();
        self.expanded_dirs.clear();
        self.search_query.clear();
        self.extension_filter.clear();
        self.content_search_query.clear();
        self.content_search_results.clear();
        self.previewed_file_path = None;
        self.active_ignore_patterns.clear();

        self.scan_progress = ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Ready.".to_string(),
        };
    }
}
