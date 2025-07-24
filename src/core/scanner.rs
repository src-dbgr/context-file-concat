// src/core/scanner.rs

use super::{build_globset_from_patterns, FileItem};
use crate::utils::file_detection::is_text_file;
use anyhow::Result;
use globset::GlobSet;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant}; // NEU: Import für Zeitmessung
use walkdir::{DirEntry, WalkDir};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScanProgress {
    pub files_scanned: usize,
    pub large_files_skipped: usize,
    pub current_scanning_path: String,
}

const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;
const PROGRESS_UPDATE_THROTTLE: Duration = Duration::from_millis(100); // UI-Update max. alle 100ms

pub struct DirectoryScanner {
    ignore_patterns: HashSet<String>,
}

impl DirectoryScanner {
    pub fn new(ignore_patterns: HashSet<String>) -> Self {
        Self { ignore_patterns }
    }

    pub async fn scan_directory_with_progress<F>(
        &self,
        root_path: &Path,
        cancel_flag: Arc<AtomicBool>, // Das Stopp-Signal aus main.rs
        progress_callback: F,
    ) -> Result<Vec<FileItem>>
    where
        F: Fn(ScanProgress) + Send + Sync + 'static,
    {
        tracing::info!("LOG: SCANNER::scan_directory_with_progress aufgerufen.");
        let root_path_buf = root_path.to_path_buf();
        let ignore_patterns_clone = self.ignore_patterns.clone();
        let progress_callback = Arc::new(progress_callback);

        tracing::info!("LOG: SCANNER:: Rufe tokio::task::spawn_blocking auf.");
        let blocking_task_handle = tokio::task::spawn_blocking(move || {
            tracing::info!("LOG: BLOCKING-TASK:: Gestartet.");
            let ignore_glob_set = build_globset_from_patterns(&ignore_patterns_clone);

            tracing::info!("LOG: BLOCKING-TASK:: Starte WalkDir...");
            let entries = WalkDir::new(root_path_buf)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .collect::<Vec<DirEntry>>();
            tracing::info!(
                "LOG: BLOCKING-TASK:: WalkDir beendet. {} Einträge gefunden.",
                entries.len()
            );

            let total_to_process = entries.len();
            let mut final_files = Vec::with_capacity(total_to_process);
            let large_files_skipped_counter = AtomicUsize::new(0);
            let mut last_update = Instant::now();

            for (i, entry) in entries.into_iter().enumerate() {
                if cancel_flag.load(Ordering::Relaxed) {
                    tracing::warn!("LOG: BLOCKING-TASK:: Stopp-Signal erkannt! Breche Schleife ab bei Index {}.", i);
                    return final_files;
                }

                if i > 0 && i % 2000 == 0 {
                    tracing::info!("LOG: BLOCKING-TASK:: Verarbeite... Index {}", i);
                }

                let now = Instant::now();
                if now.duration_since(last_update) > PROGRESS_UPDATE_THROTTLE
                    || i == total_to_process - 1
                {
                    tracing::info!(
                        "LOG: BLOCKING-TASK:: Sende Fortschritts-Update an UI (Index {}).",
                        i + 1
                    );
                    let path_str = entry.path().to_string_lossy().into_owned();
                    progress_callback(ScanProgress {
                        files_scanned: i + 1,
                        large_files_skipped: large_files_skipped_counter.load(Ordering::Relaxed),
                        current_scanning_path: path_str,
                    });
                    last_update = now;
                }

                let path = entry.path();
                if Self::should_ignore(path, &ignore_glob_set) {
                    continue;
                }

                let metadata = match entry.metadata() {
                    Ok(md) => md,
                    Err(_) => continue,
                };
                if !metadata.is_dir() && metadata.len() > MAX_FILE_SIZE {
                    large_files_skipped_counter.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                let is_binary = if metadata.is_file() {
                    !is_text_file(path).unwrap_or(false)
                } else {
                    false
                };

                final_files.push(FileItem {
                    path: path.to_path_buf(),
                    is_directory: metadata.is_dir(),
                    is_binary,
                    size: metadata.len(),
                    depth: entry.depth(),
                    parent: path.parent().map(|p| p.to_path_buf()),
                });
            }
            tracing::info!("LOG: BLOCKING-TASK:: Verarbeitungsschleife beendet.");
            final_files
        });

        tracing::info!("LOG: SCANNER:: Warte auf Ergebnis von spawn_blocking...");
        match blocking_task_handle.await {
            Ok(files) => {
                tracing::info!("LOG: SCANNER:: spawn_blocking erfolgreich beendet.");
                Ok(files)
            }
            Err(e) => {
                tracing::error!("LOG: SCANNER:: spawn_blocking mit Fehler beendet (wahrscheinlich abgebrochen): {}", e);
                Err(anyhow::anyhow!("Scan task was cancelled or failed: {}", e))
            }
        }
    }

    fn should_ignore(path: &Path, ignore_glob_set: &GlobSet) -> bool {
        if path.components().any(|c| c.as_os_str() == ".git") {
            return true;
        }
        ignore_glob_set.is_match(path)
    }
}
