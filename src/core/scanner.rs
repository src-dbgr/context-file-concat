//! Provides the functionality for recursively scanning directories.

use super::{CoreError, FileItem};
use crate::utils::file_detection::is_text_file;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Represents the progress of a directory scan.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScanProgress {
    pub files_scanned: usize,
    pub large_files_skipped: usize,
    pub current_scanning_path: String,
}

const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;
const PROGRESS_UPDATE_THROTTLE: Duration = Duration::from_millis(100);

/// Scans a directory for files and subdirectories, respecting ignore patterns.
pub struct DirectoryScanner {
    ignore_patterns: HashSet<String>,
    // This field only exists in test builds to allow deterministic testing of progress updates.
    #[cfg(test)]
    progress_throttle_override: Option<Duration>,
}

/// Private helper function with the core walker loop.
/// This allows the throttling logic to be tested deterministically without
/// polluting the public API signature.
fn process_walker_results<F, H>(
    walker: ignore::Walk,
    cancel_flag: Arc<AtomicBool>,
    progress_callback: F,
    progress_throttle: Duration,
    mut test_hook: H,
) -> Vec<FileItem>
where
    F: Fn(ScanProgress) + Send + Sync + 'static,
    H: FnMut(&ignore::DirEntry) + Send + 'static,
{
    let mut final_files = Vec::new();
    let large_files_skipped_counter = AtomicUsize::new(0);
    let files_scanned_counter = AtomicUsize::new(0);
    let mut last_update = Instant::now();

    for result in walker {
        if cancel_flag.load(Ordering::SeqCst) {
            break;
        }

        let entry = match result {
            Ok(e) => e,
            // An error here means the walker couldn't process an entry, e.g., due to
            // permissions on a directory it was trying to read. We log it and continue.
            Err(e) => {
                tracing::warn!("Skipping entry due to walker error: {}", e);
                continue;
            }
        };

        // This hook is a no-op in production, but can inject delays and code during tests.
        test_hook(&entry);

        // Check for the root directory and skip it *before* incrementing the counter.
        // This prevents inflating the scanned count with the root directory itself.
        if entry.depth() == 0 {
            continue;
        }

        // All logic related to counting and progress now happens only for valid entries.
        let count = files_scanned_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if Instant::now().duration_since(last_update) > progress_throttle {
            tracing::debug!(
                "[SCANNER] Throttling condition met. Invoking progress callback for {} files.",
                count
            );
            let path_str = entry.path().to_string_lossy().into_owned();
            progress_callback(ScanProgress {
                files_scanned: count,
                large_files_skipped: large_files_skipped_counter.load(Ordering::Relaxed),
                current_scanning_path: path_str,
            });
            last_update = Instant::now();
        }

        let metadata = match entry.metadata() {
            Ok(md) => md,
            // An error here could be a race condition (file deleted during scan) or
            // permissions on a specific file. We log it and continue.
            Err(e) => {
                tracing::warn!(
                    "Skipping entry {} due to metadata error: {}",
                    entry.path().display(),
                    e
                );
                continue;
            }
        };

        if !metadata.is_dir() && metadata.len() > MAX_FILE_SIZE {
            large_files_skipped_counter.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        let is_binary = if metadata.is_file() {
            // If file content cannot be read for is_text_file check, it returns Err.
            // We treat such files as binary for safety, covering the unwrap_or(false) path.
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
            parent: entry.path().parent().map(PathBuf::from),
        });
    }
    final_files
}

impl DirectoryScanner {
    pub fn new(ignore_patterns: HashSet<String>) -> Self {
        Self {
            ignore_patterns,
            #[cfg(test)]
            progress_throttle_override: None,
        }
    }

    #[cfg(test)]
    fn new_with_throttle(ignore_patterns: HashSet<String>, throttle: Duration) -> Self {
        Self {
            ignore_patterns,
            progress_throttle_override: Some(throttle),
        }
    }

    /// Scans a directory asynchronously, providing progress updates via a callback.
    ///
    /// This function performs the scan in a blocking thread to avoid blocking the async runtime,
    /// while allowing for cancellation and progress reporting. It uses the `ignore` crate
    /// for high-performance, gitignore-aware directory traversal. It also manually checks
    /// custom ignore patterns to report which ones were actively used.
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
        let root_path_buf = root_path.to_path_buf();
        let ignore_patterns_clone = self.ignore_patterns.clone();

        #[cfg(test)]
        let throttle = self
            .progress_throttle_override
            .unwrap_or(PROGRESS_UPDATE_THROTTLE);
        #[cfg(not(test))]
        let throttle = PROGRESS_UPDATE_THROTTLE;

        let blocking_task_handle = tokio::task::spawn_blocking(move || {
            let mut walker_builder = ignore::WalkBuilder::new(&root_path_buf);

            let active_patterns =
                std::sync::Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));

            // Create a list of individual matchers for each user-defined pattern.
            let custom_matchers: Vec<(String, ignore::gitignore::Gitignore)> =
                ignore_patterns_clone
                    .iter()
                    .filter_map(|pattern| {
                        let mut builder = ignore::gitignore::GitignoreBuilder::new(&root_path_buf);
                        builder.add_line(None, pattern).ok()?;
                        builder.build().ok().map(|m| (pattern.clone(), m))
                    })
                    .collect();

            if let Some(depth) = max_depth {
                walker_builder.max_depth(Some(depth));
            }

            walker_builder
                .hidden(false)
                .parents(false)
                .git_global(true)
                .git_ignore(true)
                .git_exclude(true)
                .require_git(false) // CRITICAL: Don't require a .git repo to exist.
                .follow_links(false);

            // Add a filter to check our custom patterns and collect the active ones.
            let active_patterns_clone = active_patterns.clone();
            walker_builder.filter_entry(move |entry| {
                let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
                for (pattern, matcher) in &custom_matchers {
                    if matcher.matched(entry.path(), is_dir).is_ignore() {
                        active_patterns_clone
                            .lock()
                            .unwrap()
                            .insert(pattern.clone());
                        return false; // Exclude this entry.
                    }
                }
                true // Keep this entry.
            });

            let walker = walker_builder.build();

            // Call the internal helper with a no-op closure for the test hook.
            let final_files =
                process_walker_results(walker, cancel_flag, progress_callback, throttle, |_| {});

            let final_active_patterns = active_patterns.lock().unwrap().clone();
            (final_files, final_active_patterns)
        });

        // Await the result. If the task panicked, spawn_blocking returns a JoinError,
        // which we convert into our custom CoreError::Join.
        match blocking_task_handle.await {
            Ok(result) => Ok(result),
            Err(join_error) => Err(join_error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::panic;
    use std::path::PathBuf;
    use std::sync::{atomic::AtomicBool, Mutex, Once};
    use tempfile::TempDir;

    static LOGGING_INIT: Once = Once::new();

    /// Initializes tracing for tests. Safe to call multiple times.
    fn setup_logging() {
        LOGGING_INIT.call_once(|| {
            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
                .with_test_writer()
                .init();
        });
    }

    /// Creates a temporary, realistic file system structure for robust testing.
    fn setup_test_filesystem() -> (TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let root = temp_dir.path().to_path_buf();
        fs::create_dir_all(root.join("src/core")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join(".gitignore"), b"/target/\n*.log\nnode_modules/").unwrap();
        fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        fs::write(root.join("src/core/scanner.rs"), b"// ...").unwrap();
        fs::write(root.join("debug.log"), b"log data").unwrap();
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/some_dep"), b"{}").unwrap();
        // Add a large file to test the size limit skip
        let large_file_path = root.join("large_file.bin");
        let large_file = fs::File::create(&large_file_path).unwrap();
        large_file.set_len(MAX_FILE_SIZE + 1).unwrap();
        (temp_dir, root)
    }

    /// Verifies the main success path: respecting .gitignore, custom ignores, and large file skips.
    #[tokio::test]
    async fn test_scan_respects_all_ignore_mechanisms() {
        setup_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let mut custom_ignores = HashSet::new();
        custom_ignores.insert("src/core/".to_string()); // Custom rule

        let scanner = DirectoryScanner::new(custom_ignores);

        let (files, _) = scanner
            .scan_directory_with_progress(&root, None, Arc::new(AtomicBool::new(false)), |_| {})
            .await
            .expect("Scan should succeed");

        let paths: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();

        // Check file that should be present
        assert!(paths.contains(&root.join("src/main.rs")));
        // Check custom ignore rule
        assert!(!paths.contains(&root.join("src/core/scanner.rs")));
        // Check .gitignore rules
        assert!(!paths.iter().any(|p| p.starts_with(&root.join("target"))));
        assert!(!paths.contains(&root.join("debug.log")));
        // Check large file skip
        assert!(!paths.contains(&root.join("large_file.bin")));
    }

    /// Verifies that the `max_depth` parameter is correctly honored.
    #[tokio::test]
    async fn test_max_depth_is_honored() {
        setup_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let scanner = DirectoryScanner::new(HashSet::new());

        let (files, _) = scanner
            .scan_directory_with_progress(&root, Some(1), Arc::new(AtomicBool::new(false)), |_| {})
            .await
            .expect("Scan should succeed");

        let paths: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();

        // Check for expected items at depth 1 that are NOT ignored by .gitignore or size
        assert!(paths.contains(&root.join("src")));
        assert!(paths.contains(&root.join(".gitignore")));

        // Check for items that should be IGNORED and thus absent
        assert!(!paths.contains(&root.join("target"))); // Ignored by .gitignore
        assert!(!paths.contains(&root.join("debug.log"))); // Ignored by .gitignore
        assert!(!paths.contains(&root.join("node_modules"))); // Ignored by .gitignore
        assert!(!paths.contains(&root.join("large_file.bin"))); // Skipped due to size

        // Verify that no items have a depth greater than 1
        for file in files {
            assert!(
                file.depth <= 1,
                "Found item with depth > 1: {:?}",
                file.path
            );
        }
    }

    /// Verifies that the scan stops promptly upon cancellation.
    #[tokio::test]
    async fn test_cancellation_stops_scan() {
        setup_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        for i in 0..200 {
            fs::write(root.join(format!("file_{i}.txt")), "data").unwrap();
        }

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let sender = Arc::new(Mutex::new(Some(tx)));
        let task_cancel_flag = cancel_flag.clone();
        let task_sender = sender.clone();
        let walker = ignore::WalkBuilder::new(&root).require_git(false).build();

        let handle = tokio::task::spawn_blocking(move || {
            process_walker_results(
                walker,
                task_cancel_flag,
                move |progress| {
                    if progress.files_scanned > 20 {
                        if let Some(s) = task_sender.lock().unwrap().take() {
                            let _ = s.send(());
                        }
                    }
                },
                PROGRESS_UPDATE_THROTTLE, // Use standard throttle for this test
                move |_| {
                    // Introduce a small delay to make cancellation more likely to happen mid-scan.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                },
            )
        });

        // Wait for the signal that the scan is well underway.
        tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("Test timed out waiting for signal")
            .expect("Sender was dropped without sending");

        // Now, cancel the operation.
        cancel_flag.store(true, Ordering::SeqCst);
        let files = handle.await.expect("Scan task panicked");

        assert!(!files.is_empty());
        assert!(
            files.len() < 200,
            "Scan should have stopped early, but found {} files",
            files.len()
        );
    }

    /// Verifies that the progress callback is invoked deterministically.
    #[tokio::test]
    async fn test_progress_callback_is_invoked_deterministically() {
        setup_logging();
        // Create an isolated, clean environment for this test.
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("file1.txt"), "data1").unwrap();
        fs::write(root.join("file2.txt"), "data2").unwrap();

        let walker = ignore::WalkBuilder::new(&root).build();
        let progress_updates = Arc::new(Mutex::new(Vec::new()));
        let updates_clone = progress_updates.clone();

        let mut hook_call_count = 0;

        process_walker_results(
            walker,
            Arc::new(AtomicBool::new(false)),
            move |progress| {
                updates_clone.lock().unwrap().push(progress);
            },
            PROGRESS_UPDATE_THROTTLE, // Use standard throttle
            // The deterministic test hook.
            move |entry| {
                hook_call_count += 1;
                // The hook is called for the root dir first (depth 0), then for file1.txt.
                // We sleep after the *first valid file* is processed by the hook.
                if entry.depth() > 0 && hook_call_count == 2 {
                    std::thread::sleep(PROGRESS_UPDATE_THROTTLE + Duration::from_millis(10));
                }
            },
        );

        let updates = progress_updates.lock().unwrap();
        assert_eq!(
            updates.len(),
            1,
            "Progress callback should have been called exactly once."
        );
        assert_eq!(updates[0].files_scanned, 1);
    }

    /// Verifies that custom ignore patterns that are used are reported back.
    #[tokio::test]
    async fn test_active_ignore_patterns_are_reported() {
        setup_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let mut custom_ignores = HashSet::new();
        // Use a pattern that is NOT in the .gitignore to isolate the test's purpose.
        let pattern_to_match = "src/main.rs".to_string();
        let pattern_not_to_match = "*.tmp".to_string();
        custom_ignores.insert(pattern_to_match.clone());
        custom_ignores.insert(pattern_not_to_match.clone());

        let scanner = DirectoryScanner::new(custom_ignores);
        let (files, active_patterns) = scanner
            .scan_directory_with_progress(&root, None, Arc::new(AtomicBool::new(false)), |_| {})
            .await
            .expect("Scan should succeed");

        // 1. Check that the pattern was correctly reported as active.
        assert!(
            active_patterns.contains(&pattern_to_match),
            "Expected 'src/main.rs' to be an active pattern."
        );
        assert!(
            !active_patterns.contains(&pattern_not_to_match),
            "Expected '*.tmp' not to be an active pattern."
        );
        assert_eq!(active_patterns.len(), 1);

        // 2. Check that the file matching the active pattern was actually excluded.
        let paths: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        assert!(!paths.contains(&root.join("src/main.rs")));
    }

    /// Verifies that paths with special characters are handled correctly.
    #[tokio::test]
    async fn test_scan_with_special_characters_in_paths() {
        setup_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let special_dir = root.join("ein Ordner mit Leerzeichen");
        fs::create_dir(&special_dir).unwrap();
        let special_file = special_dir.join("Lösung.rs");
        fs::write(&special_file, "fn solution() {}").unwrap();

        let scanner = DirectoryScanner::new(HashSet::new());
        let (files, _) = scanner
            .scan_directory_with_progress(root, None, Arc::new(AtomicBool::new(false)), |_| {})
            .await
            .unwrap();

        let paths: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        assert!(paths.contains(&special_file));
        assert!(paths.contains(&special_dir));
    }

    /// Verifies that the scanner runs correctly when no custom ignores are provided.
    #[tokio::test]
    async fn test_scan_handles_empty_custom_ignores() {
        setup_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let scanner = DirectoryScanner::new(HashSet::new());

        let result = scanner
            .scan_directory_with_progress(&root, None, Arc::new(AtomicBool::new(false)), |_| {})
            .await;

        assert!(result.is_ok());
        let (files, active_patterns) = result.unwrap();
        assert!(!files.is_empty());
        assert!(active_patterns.is_empty());
    }

    /// Verifies that an invalid ignore pattern does not crash the scanner.
    #[tokio::test]
    async fn test_scan_with_invalid_ignore_pattern() {
        setup_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let mut custom_ignores = HashSet::new();
        custom_ignores.insert("[".to_string());

        let scanner = DirectoryScanner::new(custom_ignores);
        let result = scanner
            .scan_directory_with_progress(&root, None, Arc::new(AtomicBool::new(false)), |_| {})
            .await;

        assert!(result.is_ok());
        let (files, active_patterns) = result.unwrap();
        assert!(active_patterns.is_empty());
        assert!(!files.is_empty());
    }

    // =========================================================================================
    // SECTION: ROBUSTNESS AND EDGE CASE TESTS (NEW AND CORRECTED)
    // =========================================================================================

    /// This test covers an error during directory iteration. By making a subdirectory
    /// unreadable, we simulate an I/O error that the `ignore::Walk` iterator will yield.
    /// The scanner should gracefully skip this directory and continue scanning others.
    #[tokio::test]
    #[cfg(unix)] // Relies on Unix-style permissions.
    async fn test_scan_unreadable_directory() {
        setup_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let unreadable_dir = root.join("unreadable");
        fs::create_dir(&unreadable_dir).unwrap();
        fs::write(unreadable_dir.join("secret.txt"), "cant see me").unwrap();

        let mut perms = fs::metadata(&unreadable_dir).unwrap().permissions();
        perms.set_mode(0o300); // Write/execute only for owner, no read.
        fs::set_permissions(&unreadable_dir, perms.clone()).unwrap();

        let scanner = DirectoryScanner::new(HashSet::new());
        let (files, _) = scanner
            .scan_directory_with_progress(root, None, Arc::new(AtomicBool::new(false)), |_| {})
            .await
            .unwrap();

        let paths: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        assert!(paths.contains(&unreadable_dir));
        assert!(!paths.contains(&unreadable_dir.join("secret.txt")));

        // Cleanup: restore permissions so the temp dir can be deleted.
        perms.set_mode(0o755);
        fs::set_permissions(&unreadable_dir, perms).unwrap();
    }

    /// Tests that the scanner continues gracefully if it fails to get metadata for a single entry.
    /// This is simulated by deleting a file *during* the scan, a realistic race condition.
    #[tokio::test]
    async fn test_scanner_skips_entry_on_metadata_error() {
        setup_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let file_ok = root.join("file_ok.txt");
        let file_to_delete = root.join("file_to_delete.txt");
        fs::write(&file_ok, "good data").unwrap();
        fs::write(&file_to_delete, "bad data").unwrap();

        let walker = ignore::WalkBuilder::new(&root).build();

        let hook_file_to_delete = file_to_delete.clone();
        let test_hook = move |entry: &ignore::DirEntry| {
            if entry.path() == hook_file_to_delete {
                // Deleting the file here causes the subsequent `entry.metadata()` call to fail.
                fs::remove_file(&hook_file_to_delete).unwrap();
            }
        };

        // We test the private helper directly to use the deterministic test hook.
        let files = process_walker_results(
            walker,
            Arc::new(AtomicBool::new(false)),
            |_| {},
            PROGRESS_UPDATE_THROTTLE,
            test_hook,
        );

        let paths: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();

        assert!(
            paths.contains(&file_ok),
            "The readable file should be included."
        );
        assert!(
            !paths.contains(&file_to_delete),
            "The file that was deleted should be skipped."
        );
    }

    /// Provides a targeted test of the cancellation flag, ensuring the walker loop
    /// terminates promptly after the flag is set.
    #[tokio::test]
    async fn test_cancellation_is_near_immediate() {
        setup_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        for i in 0..100 {
            fs::write(root.join(format!("file_{}.txt", i)), "data").unwrap();
        }

        let walker = ignore::WalkBuilder::new(&root).build();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let files_processed_count = Arc::new(AtomicUsize::new(0));

        let hook_cancel_flag = cancel_flag.clone();
        let hook_files_processed = files_processed_count.clone();

        let test_hook = move |entry: &ignore::DirEntry| {
            if entry.depth() > 0 {
                // Skip counting the root directory itself.
                let count = hook_files_processed.fetch_add(1, Ordering::SeqCst);
                if count == 5 {
                    hook_cancel_flag.store(true, Ordering::SeqCst);
                }
            }
        };

        let files = process_walker_results(
            walker,
            cancel_flag,
            |_| {},
            PROGRESS_UPDATE_THROTTLE,
            test_hook,
        );

        // The ignore::Walk iterator might have a small internal buffer, so the stop
        // might not be *exactly* at 5, but it should be very close. A value like 20
        // provides a safe upper bound that is much lower than the total count (100).
        assert!(
            files.len() < 20,
            "Scan should have stopped almost immediately. Processed: {}",
            files.len()
        );
    }

    /// Tests that if the code running inside `spawn_blocking` panics, the main async function
    /// catches it and returns a `CoreError::Join` instead of panicking the test runner.
    /// This test is now deterministic by overriding the progress throttle.
    #[tokio::test]
    async fn test_scan_returns_join_error_on_panic() {
        setup_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        // Create a scanner with a zero-duration throttle to guarantee the callback is called.
        let scanner = DirectoryScanner::new_with_throttle(HashSet::new(), Duration::ZERO);

        // This callback will panic on its first invocation.
        let panicking_callback = |_progress: ScanProgress| {
            panic!("Deliberate panic in callback");
        };

        let result = scanner
            .scan_directory_with_progress(
                &root,
                None,
                Arc::new(AtomicBool::new(false)),
                panicking_callback,
            )
            .await;

        assert!(result.is_err(), "Expected an error result, but got Ok.");

        let err = result.unwrap_err();
        match err {
            CoreError::Join(msg) => {
                // Tokio's JoinError includes the panic message.
                assert!(msg.contains("Deliberate panic in callback"));
            }
            _ => panic!("Expected a CoreError::Join, but got {:?}", err),
        }
    }

    /// Tests how the scanner handles a file being replaced by a directory mid-scan.
    /// This is a plausible race condition. The scanner should correctly identify the
    /// item as a directory at the time of processing.
    #[tokio::test]
    async fn test_scanner_handles_file_being_replaced_by_directory() {
        setup_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let path_to_replace = root.join("race_condition.data");
        fs::write(&path_to_replace, "i will be replaced").unwrap();

        let walker = ignore::WalkBuilder::new(&root).build();

        let hook_path_to_replace = path_to_replace.clone();
        let test_hook = move |entry: &ignore::DirEntry| {
            if entry.path() == hook_path_to_replace {
                // This simulates a race condition where the file is replaced by a directory
                // after being discovered by the directory walker.
                fs::remove_file(&hook_path_to_replace).unwrap();
                fs::create_dir(&hook_path_to_replace).unwrap();
            }
        };

        let files = process_walker_results(
            walker,
            Arc::new(AtomicBool::new(false)),
            |_| {},
            PROGRESS_UPDATE_THROTTLE,
            test_hook,
        );

        // The scanner should process the item based on what it is when metadata is read.
        assert_eq!(files.len(), 1, "The item should have been processed.");
        let item = &files[0];

        // Assert the *correct* behavior: the scanner identified it as a directory.
        assert!(
            item.is_directory,
            "Item should be classified as a directory."
        );
        assert!(
            !item.is_binary,
            "A directory should not be classified as binary."
        );
    }
}
