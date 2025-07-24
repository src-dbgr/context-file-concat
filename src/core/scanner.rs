use super::{build_globset_from_patterns, FileItem};
use crate::utils::file_detection::is_text_file;
use anyhow::Result;
use globset::GlobSet;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use walkdir::WalkDir;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScanProgress {
    pub files_scanned: usize,
    pub large_files_skipped: usize,
    pub current_scanning_path: String,
}

const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024; // 20MB
const PROGRESS_UPDATE_INTERVAL: usize = 25; // Update every 25 files (more frequent)
const CANCELLATION_CHECK_INTERVAL: usize = 10; // Check cancellation every 10 files

pub struct DirectoryScanner {
    ignore_patterns: HashSet<String>,
}

impl DirectoryScanner {
    pub fn new(ignore_patterns: HashSet<String>) -> Self {
        Self { ignore_patterns }
    }

    pub async fn scan_directory_basic(&self, root_path: &Path) -> Result<Vec<FileItem>> {
        // Fallback for legacy API
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.scan_directory_with_progress(root_path, cancel_flag, |_| {})
            .await
    }

    /// ENHANCED: Scan with frequent progress updates and responsive cancellation
    pub async fn scan_directory_with_progress<F>(
        &self,
        root_path: &Path,
        cancel_flag: Arc<AtomicBool>,
        progress_callback: F,
    ) -> Result<Vec<FileItem>>
    where
        F: Fn(ScanProgress) + Send + Sync + 'static,
    {
        let ignore_glob_set = build_globset_from_patterns(&self.ignore_patterns);
        let files = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let files_scanned = Arc::new(AtomicUsize::new(0));
        let large_files_skipped = Arc::new(AtomicUsize::new(0));

        let progress_callback = Arc::new(progress_callback);

        // Initial progress update
        progress_callback(ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: "Starting directory scan...".to_string(),
        });

        // KRITISCH: Früher Cancel-Check vor schweren Operationen
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("Scan cancelled"));
        }

        // ENHANCED: Collect entries with SEHR HÄUFIGEN cancellation checks
        let mut entries = Vec::new();
        let mut entry_count = 0;

        for entry in WalkDir::new(root_path)
            .follow_links(false)
            .max_open(5) // REDUZIERT: Weniger File-Handles
            .into_iter()
            .filter_map(Result::ok)
        {
            // KRITISCH: Cancel-Check bei JEDEM EINZELNEN Entry
            if cancel_flag.load(Ordering::Relaxed) {
                tracing::info!(
                    "🛑 Scan cancelled during entry collection after {} entries",
                    entry_count
                );
                return Err(anyhow::anyhow!("Scan cancelled"));
            }

            entries.push(entry);
            entry_count += 1;

            // HÄUFIGERE Progress-Updates und Yields
            if entry_count % 50 == 0 {
                // Alle 50 statt 250
                progress_callback(ScanProgress {
                    files_scanned: 0,
                    large_files_skipped: 0,
                    current_scanning_path: format!("Collecting entries... {} found", entry_count),
                });

                // KRITISCH: Häufigere Yields für UI-Responsiveness
                tokio::task::yield_now().await;
            }
        }

        tracing::info!("📂 Collected {} entries for processing", entries.len());

        // KRITISCH: Cancel-Check nach Entry-Collection
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("Scan cancelled"));
        }

        // Update progress with total entries found
        progress_callback(ScanProgress {
            files_scanned: 0,
            large_files_skipped: 0,
            current_scanning_path: format!("Processing {} entries...", entries.len()),
        });

        // ENHANCED: KLEINERE Chunks für bessere Responsiveness
        let chunk_size = 25.min(entries.len().max(5)); // REDUZIERT: 25 statt 50
        let chunks: Vec<_> = entries.chunks(chunk_size).collect();
        let total_chunks = chunks.len();

        for (chunk_idx, chunk) in chunks.into_iter().enumerate() {
            // KRITISCH: Cancel-Check vor JEDEM Chunk
            if cancel_flag.load(Ordering::Relaxed) {
                tracing::info!("🛑 Scan cancelled before chunk {}", chunk_idx + 1);
                return Err(anyhow::anyhow!("Scan cancelled"));
            }

            // Process chunk with SEHR HÄUFIGEN cancellation checks
            let chunk_results: Vec<Option<FileItem>> = chunk
                .iter()
                .enumerate()
                .map(|(idx, entry)| {
                    // KRITISCH: Cancel-Check bei JEDEM File in Chunk
                    if cancel_flag.load(Ordering::Relaxed) {
                        return None;
                    }

                    let path = entry.path();

                    // Update current path being processed HÄUFIGER
                    if idx % 5 == 0 {
                        // Alle 5 statt 10
                        let current_scanned = files_scanned.load(Ordering::Relaxed);
                        let current_skipped = large_files_skipped.load(Ordering::Relaxed);

                        progress_callback(ScanProgress {
                            files_scanned: current_scanned,
                            large_files_skipped: current_skipped,
                            current_scanning_path: format!(
                                "Processing: {}",
                                path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("...")
                                    .chars()
                                    .take(30) // KÜRZER für bessere Performance
                                    .collect::<String>()
                            ),
                        });
                    }

                    // Skip ignored paths (schnell)
                    if Self::should_ignore(path, &ignore_glob_set) {
                        return None;
                    }

                    // KRITISCH: Nochmal Cancel-Check vor I/O
                    if cancel_flag.load(Ordering::Relaxed) {
                        return None;
                    }

                    let metadata = match entry.metadata() {
                        Ok(md) => md,
                        Err(_) => return None,
                    };

                    // Check file size before processing (schnell)
                    if metadata.len() > MAX_FILE_SIZE {
                        large_files_skipped.fetch_add(1, Ordering::Relaxed);
                        return None;
                    }

                    // LEICHTERE Binary-Check - nur für Files < 1MB
                    let is_binary = if metadata.is_file() && metadata.len() < 1024 * 1024 {
                        !is_text_file(path).unwrap_or(false)
                    } else if metadata.is_file() {
                        // Große Files: nur Extension-Check (viel schneller)
                        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                            let ext_lower = ext.to_lowercase();
                            !matches!(
                                ext_lower.as_str(),
                                "txt"
                                    | "rs"
                                    | "py"
                                    | "js"
                                    | "ts"
                                    | "json"
                                    | "md"
                                    | "html"
                                    | "css"
                                    | "yaml"
                                    | "toml"
                                    | "sh"
                                    | "log"
                            )
                        } else {
                            true // Keine Extension = wahrscheinlich binär
                        }
                    } else {
                        false
                    };

                    files_scanned.fetch_add(1, Ordering::Relaxed);

                    Some(FileItem {
                        path: path.to_path_buf(),
                        is_directory: metadata.is_dir(),
                        is_binary,
                        size: metadata.len(),
                        depth: entry.depth(),
                        parent: path.parent().map(|p| p.to_path_buf()),
                    })
                })
                .collect();

            // KRITISCH: Cancel-Check nach Chunk-Processing
            if cancel_flag.load(Ordering::Relaxed) {
                tracing::info!("🛑 Scan cancelled after processing chunk {}", chunk_idx + 1);
                return Err(anyhow::anyhow!("Scan cancelled"));
            }

            // Add results to files
            {
                let mut files_guard = files.lock().await;
                for item in chunk_results.into_iter().flatten() {
                    files_guard.push(item);
                }
            }

            // Progress update after each chunk
            let current_scanned = files_scanned.load(Ordering::Relaxed);
            let current_skipped = large_files_skipped.load(Ordering::Relaxed);

            progress_callback(ScanProgress {
                files_scanned: current_scanned,
                large_files_skipped: current_skipped,
                current_scanning_path: format!(
                    "Chunk {} of {} completed ({:.1}%)",
                    chunk_idx + 1,
                    total_chunks,
                    ((chunk_idx + 1) as f32 / total_chunks as f32) * 100.0
                ),
            });

            // KRITISCH: Yield nach JEDEM Chunk für UI-Responsiveness
            tokio::task::yield_now().await;

            // KRITISCH: Final cancel check nach yield
            if cancel_flag.load(Ordering::Relaxed) {
                tracing::info!("🛑 Scan cancelled after yield in chunk {}", chunk_idx + 1);
                return Err(anyhow::anyhow!("Scan cancelled"));
            }
        }

        // Final cancellation check
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("Scan cancelled"));
        }

        let final_files = files.lock().await.clone();
        let final_scanned = files_scanned.load(Ordering::Relaxed);
        let final_skipped = large_files_skipped.load(Ordering::Relaxed);

        tracing::info!(
            "✅ Scan completed successfully: {} files processed, {} large files skipped",
            final_scanned,
            final_skipped
        );

        // Final completion progress update
        progress_callback(ScanProgress {
            files_scanned: final_scanned,
            large_files_skipped: final_skipped,
            current_scanning_path: format!("Scan completed! {} files found", final_files.len()),
        });

        Ok(final_files)
    }

    fn should_ignore(path: &Path, ignore_glob_set: &GlobSet) -> bool {
        // Fast path for .git directories
        if path.components().any(|c| c.as_os_str() == ".git") {
            return true;
        }

        // Check against ignore patterns
        ignore_glob_set.is_match(path)
    }
}
