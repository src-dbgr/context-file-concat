//! Provides the functionality for recursively scanning directories.

use super::{CoreError, FileItem};
use crate::utils::file_detection::is_text_file;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Represents the progress of a directory scan.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScanProgress {
    /// The number of files and directories processed so far.
    pub files_scanned: usize,
    /// The number of files skipped because they exceeded the size limit.
    pub large_files_skipped: usize,
    /// The path of the item currently being processed.
    pub current_scanning_path: String,
}

const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;
const PROGRESS_UPDATE_THROTTLE: Duration = Duration::from_millis(100);

/// Scans a directory for files and subdirectories, respecting ignore patterns.
pub struct DirectoryScanner {
    ignore_patterns: HashSet<String>,
}

impl DirectoryScanner {
    /// Creates a new `DirectoryScanner` with a given set of ignore patterns.
    pub fn new(ignore_patterns: HashSet<String>) -> Self {
        Self { ignore_patterns }
    }

    /// Scans a directory asynchronously, providing progress updates via a callback.
    ///
    /// This function performs the scan in a blocking thread to avoid blocking the async runtime,
    /// while allowing for cancellation and progress reporting. It uses the `ignore` crate
    /// for high-performance, gitignore-aware directory traversal.
    pub async fn scan_directory_with_progress<F>(
        &self,
        root_path: &Path,
        max_depth: Option<usize>,
        cancel_flag: Arc<AtomicBool>,
        progress_callback: F,
    ) -> Result<(Vec<FileItem>, HashSet<String>), CoreError>
    where
        F: Fn(ScanProgress) + Send + Sync + 'static,
    {
        tracing::info!("LOG: SCANNER::scan_directory_with_progress called.");
        let root_path_buf = root_path.to_path_buf();
        let ignore_patterns_clone = self.ignore_patterns.clone();

        let blocking_task_handle = tokio::task::spawn_blocking(move || {
            let mut final_files = Vec::new();
            let active_patterns = Arc::new(Mutex::new(HashSet::<String>::new()));
            let large_files_skipped_counter = AtomicUsize::new(0);
            let files_scanned_counter = AtomicUsize::new(0);
            let mut last_update = Instant::now();

            // Create a list of individual matchers, one for each user-defined pattern.
            // This allows us to know exactly which pattern matched.
            let custom_matchers: Vec<(String, ignore::gitignore::Gitignore)> =
                ignore_patterns_clone
                    .iter()
                    .filter_map(|pattern| {
                        let mut builder = ignore::gitignore::GitignoreBuilder::new(&root_path_buf);
                        builder.add_line(None, pattern).ok()?;
                        builder
                            .build()
                            .ok()
                            .map(|matcher| (pattern.clone(), matcher))
                    })
                    .collect();

            let mut walker_builder = WalkBuilder::new(&root_path_buf);
            if let Some(depth) = max_depth {
                walker_builder.max_depth(Some(depth));
            }
            walker_builder
                .hidden(false)
                .parents(false)
                .git_global(true)
                .git_ignore(true)
                .git_exclude(true)
                .follow_links(false);

            let active_patterns_clone = active_patterns.clone();
            walker_builder.filter_entry(move |entry| {
                let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());

                // Check against our custom patterns first.
                for (pattern, matcher) in &custom_matchers {
                    if matcher.matched(entry.path(), is_dir).is_ignore() {
                        active_patterns_clone
                            .lock()
                            .unwrap()
                            .insert(pattern.clone());
                        return false; // Exclude this entry from the walk.
                    }
                }

                // If our custom patterns didn't match, let the walker proceed with its own
                // standard gitignore/hidden file filtering.
                true
            });

            let walker = walker_builder.build();

            for result in walker {
                if cancel_flag.load(Ordering::SeqCst) {
                    tracing::warn!("LOG: BLOCKING-TASK:: Cancellation detected, stopping walk.");
                    break;
                }

                let entry = match result {
                    Ok(e) => e,
                    Err(err) => {
                        tracing::warn!("Error walking directory: {}", err);
                        continue;
                    }
                };

                let count = files_scanned_counter.fetch_add(1, Ordering::Relaxed) + 1;
                if Instant::now().duration_since(last_update) > PROGRESS_UPDATE_THROTTLE {
                    let path_str = entry.path().to_string_lossy().into_owned();
                    progress_callback(ScanProgress {
                        files_scanned: count,
                        large_files_skipped: large_files_skipped_counter.load(Ordering::Relaxed),
                        current_scanning_path: path_str,
                    });
                    last_update = Instant::now();
                }

                if entry.depth() == 0 {
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
                    !is_text_file(entry.path()).unwrap_or(false)
                } else {
                    false
                };

                final_files.push(FileItem {
                    path: entry.path().to_path_buf(),
                    is_directory: metadata.is_dir(),
                    is_binary,
                    size: metadata.len(),
                    depth: entry.depth(),
                    parent: entry.path().parent().map(|p| p.to_path_buf()),
                });
            }

            tracing::info!("LOG: BLOCKING-TASK:: Processing loop finished.");
            let final_active_patterns = active_patterns.lock().unwrap().clone();

            Ok((final_files, final_active_patterns))
        });

        tracing::info!("LOG: SCANNER:: Waiting for spawn_blocking result...");
        blocking_task_handle.await?
    }
}
