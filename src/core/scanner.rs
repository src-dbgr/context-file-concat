use super::{build_globset_from_patterns, CoreError, FileItem};
use crate::utils::file_detection::is_text_file;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use walkdir::{DirEntry, WalkDir};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScanProgress {
    pub files_scanned: usize,
    pub large_files_skipped: usize,
    pub current_scanning_path: String,
}

const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;
const PROGRESS_UPDATE_THROTTLE: Duration = Duration::from_millis(100);

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
        cancel_flag: Arc<AtomicBool>,
        progress_callback: F,
    ) -> Result<(Vec<FileItem>, HashSet<String>), CoreError>
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
            let (ignore_glob_set, glob_patterns) =
                build_globset_from_patterns(&ignore_patterns_clone);
            let mut active_patterns: HashSet<String> = HashSet::new();

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
                    return (final_files, active_patterns);
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

                let mut is_ignored = false;
                if path.components().any(|c| c.as_os_str() == ".git") {
                    is_ignored = true;
                } else {
                    let matches_indices: Vec<usize> =
                        ignore_glob_set.matches(path).into_iter().collect();
                    if !matches_indices.is_empty() {
                        for &match_index in &matches_indices {
                            if let Some(pattern) = glob_patterns.get(match_index) {
                                active_patterns.insert(pattern.clone());
                            }
                        }
                        is_ignored = true;
                    }
                }

                if is_ignored {
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
            (final_files, active_patterns)
        });

        tracing::info!("LOG: SCANNER:: Warte auf Ergebnis von spawn_blocking...");
        let result = blocking_task_handle.await?; // This converts JoinError into CoreError
        tracing::info!("LOG: SCANNER:: spawn_blocking erfolgreich beendet.");
        Ok(result)
    }
}
