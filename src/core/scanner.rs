//! Provides the functionality for recursively scanning directories.

use super::{CoreError, FileItem, MAX_FILE_SIZE};
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

const PROGRESS_UPDATE_THROTTLE: Duration = Duration::from_millis(100);

/// Describes which part of a project a scan covers.
///
/// A scan always happens *in the context of a project*: ignore rules (the user's custom
/// patterns as well as the chain of `.gitignore` files) are anchored at the project root,
/// and [`FileItem::depth`] is counted from there. Where the walk starts is a separate
/// matter: a full scan starts at the project root itself, a lazy load starts at one of
/// its subdirectories. Keeping the two apart is what makes a lazily loaded level
/// identical to the same level of a full scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRequest {
    project_root: PathBuf,
    start_dir: PathBuf,
    /// How many levels below `start_dir` are listed; `None` means all of them.
    max_depth: Option<usize>,
    /// The depth of `start_dir` relative to `project_root`.
    start_depth: usize,
}

impl ScanRequest {
    /// A scan of the whole project, optionally limited to `max_depth` levels.
    pub fn project(project_root: impl Into<PathBuf>, max_depth: Option<usize>) -> Self {
        let project_root = project_root.into();
        Self {
            start_dir: project_root.clone(),
            project_root,
            max_depth,
            start_depth: 0,
        }
    }

    /// A scan of the direct children of `directory`, which must lie inside
    /// `project_root` (or be the project root itself).
    pub fn directory_level(
        project_root: impl Into<PathBuf>,
        directory: impl Into<PathBuf>,
    ) -> Result<Self, CoreError> {
        let project_root = project_root.into();
        let start_dir = directory.into();
        let start_depth = start_dir.strip_prefix(&project_root)?.components().count();
        Ok(Self {
            project_root,
            start_dir,
            max_depth: Some(1),
            start_depth,
        })
    }

    /// The directory that anchors ignore rules and item depths.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// The directory whose content is listed.
    pub fn start_dir(&self) -> &Path {
        &self.start_dir
    }

    /// The number of levels listed below the start directory (`None`: unlimited).
    pub fn max_depth(&self) -> Option<usize> {
        self.max_depth
    }

    /// The depth limit as the walker needs it: counted from the project root.
    fn max_depth_from_project_root(&self) -> Option<usize> {
        self.max_depth.map(|depth| self.start_depth + depth)
    }

    /// `true` for the entries the walk has to visit: the directories leading from the
    /// project root to the start directory, and everything below the start directory.
    fn is_on_route(&self, path: &Path) -> bool {
        path.starts_with(&self.start_dir) || self.start_dir.starts_with(path)
    }
}

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
///
/// Entries up to and including `start_depth` (the start directory of the scan and the
/// directories leading to it) are walked but neither counted nor reported.
fn process_walker_results<F, H>(
    walker: ignore::Walk,
    start_depth: usize,
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

        // Check for the start directory (and its ancestors) and skip it *before*
        // incrementing the counter. This prevents inflating the scanned count with
        // directories that are not part of the result.
        if entry.depth() <= start_depth {
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

    /// Performs the scan described by `request` asynchronously, providing progress
    /// updates via a callback.
    ///
    /// This function performs the scan in a blocking thread to avoid blocking the async runtime,
    /// while allowing for cancellation and progress reporting. It uses the `ignore` crate
    /// for high-performance, gitignore-aware directory traversal. It also manually checks
    /// custom ignore patterns to report which ones were actively used.
    ///
    /// The walk always begins at the project root, even when only a subdirectory is
    /// requested: on its way down it visits nothing but the directories leading to the
    /// start directory. That way the `.gitignore` files of all ancestors inside the
    /// project, the anchoring of the custom patterns and the depth of the items are the
    /// same as in a full scan by construction. A start directory that is itself ignored
    /// consequently yields nothing.
    pub async fn scan<F>(
        &self,
        request: &ScanRequest,
        cancel_flag: Arc<AtomicBool>,
        progress_callback: F,
    ) -> Result<(Vec<FileItem>, HashSet<String>), CoreError>
    where
        F: Fn(ScanProgress) + Send + Sync + 'static,
    {
        let request = request.clone();
        let ignore_patterns_clone = self.ignore_patterns.clone();

        #[cfg(test)]
        let throttle = self
            .progress_throttle_override
            .unwrap_or(PROGRESS_UPDATE_THROTTLE);
        #[cfg(not(test))]
        let throttle = PROGRESS_UPDATE_THROTTLE;

        let blocking_task_handle = tokio::task::spawn_blocking(move || {
            let mut walker_builder = ignore::WalkBuilder::new(request.project_root());

            let active_patterns =
                std::sync::Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));

            // Create a list of individual matchers for each user-defined pattern.
            let custom_matchers: Vec<(String, ignore::gitignore::Gitignore)> =
                ignore_patterns_clone
                    .iter()
                    .filter_map(|pattern| {
                        let mut builder =
                            ignore::gitignore::GitignoreBuilder::new(request.project_root());
                        builder.add_line(None, pattern).ok()?;
                        builder.build().ok().map(|m| (pattern.clone(), m))
                    })
                    .collect();

            walker_builder
                .max_depth(request.max_depth_from_project_root())
                .hidden(false)
                .parents(false)
                .git_global(true)
                .git_ignore(true)
                .git_exclude(true)
                .require_git(false) // CRITICAL: Don't require a .git repo to exist.
                .follow_links(false);

            // Add a filter to check our custom patterns and collect the active ones.
            let active_patterns_clone = active_patterns.clone();
            let start_depth = request.start_depth;
            walker_builder.filter_entry(move |entry| {
                // Stay on the route to the start directory. Entries beside it are not
                // part of this scan, so they must not activate a pattern either.
                if !request.is_on_route(entry.path()) {
                    return false;
                }
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
            let final_files = process_walker_results(
                walker,
                start_depth,
                cancel_flag,
                progress_callback,
                throttle,
                |_| {},
            );

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
    use crate::utils::test_helpers::running_as_root;
    use crate::utils::test_helpers::setup_test_logging;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::panic;
    use std::path::PathBuf;
    use std::sync::{atomic::AtomicBool, Mutex};
    use tempfile::TempDir;

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
        setup_test_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let mut custom_ignores = HashSet::new();
        custom_ignores.insert("src/core/".to_string()); // Custom rule

        let scanner = DirectoryScanner::new(custom_ignores);

        let (files, _) = scanner
            .scan(
                &ScanRequest::project(&root, None),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await
            .expect("Scan should succeed");

        let paths: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();

        // Check file that should be present
        assert!(paths.contains(&root.join("src/main.rs")));
        // Check custom ignore rule
        assert!(!paths.contains(&root.join("src/core/scanner.rs")));
        // Check .gitignore rules
        assert!(!paths.iter().any(|p| p.starts_with(root.join("target"))));
        assert!(!paths.contains(&root.join("debug.log")));
        // Check large file skip
        assert!(!paths.contains(&root.join("large_file.bin")));
    }

    /// Verifies that the `max_depth` parameter is correctly honored.
    #[tokio::test]
    async fn test_max_depth_is_honored() {
        setup_test_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let scanner = DirectoryScanner::new(HashSet::new());

        let (files, _) = scanner
            .scan(
                &ScanRequest::project(&root, Some(1)),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
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
        setup_test_logging();
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
                0,
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
        setup_test_logging();
        // Create an isolated, clean environment for this test.
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        fs::write(root.join("file1.txt"), "data1").unwrap();
        fs::write(root.join("file2.txt"), "data2").unwrap();

        let walker = ignore::WalkBuilder::new(root).build();
        let progress_updates = Arc::new(Mutex::new(Vec::new()));
        let updates_clone = progress_updates.clone();

        let mut hook_call_count = 0;

        process_walker_results(
            walker,
            0,
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
        setup_test_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let mut custom_ignores = HashSet::new();
        // Use a pattern that is NOT in the .gitignore to isolate the test's purpose.
        let pattern_to_match = "src/main.rs".to_string();
        let pattern_not_to_match = "*.tmp".to_string();
        custom_ignores.insert(pattern_to_match.clone());
        custom_ignores.insert(pattern_not_to_match.clone());

        let scanner = DirectoryScanner::new(custom_ignores);
        let (files, active_patterns) = scanner
            .scan(
                &ScanRequest::project(&root, None),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
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
        setup_test_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let special_dir = root.join("ein Ordner mit Leerzeichen");
        fs::create_dir(&special_dir).unwrap();
        let special_file = special_dir.join("Lösung.rs");
        fs::write(&special_file, "fn solution() {}").unwrap();

        let scanner = DirectoryScanner::new(HashSet::new());
        let (files, _) = scanner
            .scan(
                &ScanRequest::project(root, None),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await
            .unwrap();

        let paths: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        assert!(paths.contains(&special_file));
        assert!(paths.contains(&special_dir));
    }

    /// Verifies that the scanner runs correctly when no custom ignores are provided.
    #[tokio::test]
    async fn test_scan_handles_empty_custom_ignores() {
        setup_test_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let scanner = DirectoryScanner::new(HashSet::new());

        let result = scanner
            .scan(
                &ScanRequest::project(&root, None),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await;

        assert!(result.is_ok());
        let (files, active_patterns) = result.unwrap();
        assert!(!files.is_empty());
        assert!(active_patterns.is_empty());
    }

    /// Verifies that an invalid ignore pattern does not crash the scanner.
    #[tokio::test]
    async fn test_scan_with_invalid_ignore_pattern() {
        setup_test_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        let mut custom_ignores = HashSet::new();
        custom_ignores.insert("[".to_string());

        let scanner = DirectoryScanner::new(custom_ignores);
        let result = scanner
            .scan(
                &ScanRequest::project(&root, None),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
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
        if running_as_root() {
            eprintln!("Skipping permission-based test because process runs as root (Docker/act).");
            return;
        }
        setup_test_logging();
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
            .scan(
                &ScanRequest::project(root, None),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
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
        setup_test_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let file_ok = root.join("file_ok.txt");
        let file_to_delete = root.join("file_to_delete.txt");
        fs::write(&file_ok, "good data").unwrap();
        fs::write(&file_to_delete, "bad data").unwrap();

        let walker = ignore::WalkBuilder::new(root).build();

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
            0,
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
        setup_test_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        for i in 0..100 {
            fs::write(root.join(format!("file_{}.txt", i)), "data").unwrap();
        }

        let walker = ignore::WalkBuilder::new(root).build();
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
            0,
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
        setup_test_logging();
        let (_temp_dir, root) = setup_test_filesystem();
        // Create a scanner with a zero-duration throttle to guarantee the callback is called.
        let scanner = DirectoryScanner::new_with_throttle(HashSet::new(), Duration::ZERO);

        // This callback will panic on its first invocation.
        let panicking_callback = |_progress: ScanProgress| {
            panic!("Deliberate panic in callback");
        };

        let result = scanner
            .scan(
                &ScanRequest::project(&root, None),
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
        setup_test_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let path_to_replace = root.join("race_condition.data");
        fs::write(&path_to_replace, "i will be replaced").unwrap();

        let walker = ignore::WalkBuilder::new(root).build();

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
            0,
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

    /// A project in which ignore rules of every kind apply *inside* a subdirectory:
    /// the root `.gitignore` (glob and root-anchored rule), a nested `.gitignore` with a
    /// whitelist rule, and a user pattern in the form `add_ignore_path` produces.
    fn setup_project_with_ignored_entries_in_subdirectory() -> (TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let root = temp_dir.path().to_path_buf();
        fs::create_dir_all(root.join("src/build_out")).unwrap();
        fs::create_dir_all(root.join("src/generated")).unwrap();
        fs::create_dir_all(root.join("src/core/nested")).unwrap();
        fs::write(root.join(".gitignore"), "*.gen\n/src/build_out/\n").unwrap();
        fs::write(root.join("src/.gitignore"), "!keep.gen\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/secret.gen"), "secret").unwrap();
        fs::write(root.join("src/keep.gen"), "kept").unwrap();
        fs::write(root.join("src/build_out/artifact.o"), "obj").unwrap();
        fs::write(root.join("src/generated/bindings.rs"), "// generated").unwrap();
        fs::write(root.join("src/core/lib.rs"), "// lib").unwrap();
        fs::write(root.join("src/core/cache.gen"), "cache").unwrap();
        fs::write(root.join("src/core/nested/deep.rs"), "// deep").unwrap();
        fs::write(root.join("top.gen"), "top").unwrap();
        (temp_dir, root)
    }

    /// The comparable essence of scan results: what was found, as what, and how deep.
    fn summarize(items: impl IntoIterator<Item = FileItem>) -> Vec<(PathBuf, bool, usize)> {
        let mut summary: Vec<_> = items
            .into_iter()
            .map(|item| (item.path, item.is_directory, item.depth))
            .collect();
        summary.sort();
        summary
    }

    /// A lazily loaded directory level must be indistinguishable from the same level of
    /// a full scan: same entries (ignore rules anchored at the project root, parent
    /// `.gitignore` files honoured) and same depth values.
    #[tokio::test]
    async fn lazy_level_of_subdirectory_equals_same_level_of_full_scan() {
        setup_test_logging();
        let (_temp_dir, root) = setup_project_with_ignored_entries_in_subdirectory();
        let scanner = DirectoryScanner::new(HashSet::from(["src/generated/".to_string()]));
        let no_cancel = || Arc::new(AtomicBool::new(false));

        let (full, _) = scanner
            .scan(&ScanRequest::project(&root, None), no_cancel(), |_| {})
            .await
            .expect("Full scan should succeed");

        for (directory, expected_depth) in [(root.join("src"), 2), (root.join("src/core"), 3)] {
            let request = ScanRequest::directory_level(&root, &directory).unwrap();
            let (lazy, _) = scanner
                .scan(&request, no_cancel(), |_| {})
                .await
                .expect("Lazy scan should succeed");

            let expected: Vec<FileItem> = full
                .iter()
                .filter(|item| item.parent.as_deref() == Some(directory.as_path()))
                .cloned()
                .collect();
            assert!(
                !expected.is_empty(),
                "precondition: level must not be empty"
            );
            assert!(lazy.iter().all(|item| item.depth == expected_depth));
            assert_eq!(
                summarize(lazy),
                summarize(expected),
                "lazy level of {directory:?} differs from the full scan"
            );
        }
    }

    /// Spells out the user-visible part of the comparison above for the reported scenario.
    #[tokio::test]
    async fn lazy_level_hides_entries_ignored_relative_to_project_root() {
        setup_test_logging();
        let (_temp_dir, root) = setup_project_with_ignored_entries_in_subdirectory();
        let pattern = "src/generated/".to_string();
        let scanner = DirectoryScanner::new(HashSet::from([pattern.clone()]));

        let (items, active_patterns) = scanner
            .scan(
                &ScanRequest::directory_level(&root, root.join("src")).unwrap(),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await
            .expect("Lazy scan should succeed");

        let names: HashSet<String> = items
            .iter()
            .map(|item| {
                item.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let expected_names: HashSet<String> = [".gitignore", "main.rs", "keep.gen", "core"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(names, expected_names);
        assert_eq!(active_patterns, HashSet::from([pattern]));
    }

    /// Entries beside the route to the start directory are not part of a lazy load, so
    /// they neither show up nor mark a custom pattern as active.
    #[tokio::test]
    async fn lazy_level_ignores_everything_beside_its_route() {
        setup_test_logging();
        let (_temp_dir, root) = setup_project_with_ignored_entries_in_subdirectory();
        fs::create_dir(root.join("docs")).unwrap();
        fs::write(root.join("docs/readme.md"), "# docs").unwrap();
        let scanner = DirectoryScanner::new(HashSet::from(["docs/".to_string()]));

        let (items, active_patterns) = scanner
            .scan(
                &ScanRequest::directory_level(&root, root.join("src/core")).unwrap(),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await
            .expect("Lazy scan should succeed");

        assert!(items
            .iter()
            .all(|item| item.parent.as_deref() == Some(root.join("src/core").as_path())));
        assert!(active_patterns.is_empty());
    }

    /// A directory that is ignored itself has no visible content, whichever way it is scanned.
    #[tokio::test]
    async fn lazy_level_of_ignored_directory_is_empty() {
        setup_test_logging();
        let (_temp_dir, root) = setup_project_with_ignored_entries_in_subdirectory();
        let scanner = DirectoryScanner::new(HashSet::new());

        let (items, _) = scanner
            .scan(
                &ScanRequest::directory_level(&root, root.join("src/build_out")).unwrap(),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await
            .expect("Lazy scan should succeed");

        assert!(items.is_empty());
    }

    #[test]
    fn directory_level_request_rejects_directory_outside_project() {
        let result = ScanRequest::directory_level("/project", "/elsewhere/src");

        assert!(matches!(result, Err(CoreError::PathStrip(_))));
    }

    #[test]
    fn project_request_starts_at_the_project_root() {
        let request = ScanRequest::project("/project", Some(1));

        assert_eq!(request.project_root(), Path::new("/project"));
        assert_eq!(request.start_dir(), Path::new("/project"));
        assert_eq!(request.max_depth(), Some(1));
        assert_eq!(
            ScanRequest::directory_level("/project", "/project").unwrap(),
            request
        );
    }
}
