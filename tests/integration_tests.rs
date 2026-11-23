//! Integration tests for the CFC (Context File Concatenator) application.
//!
//! These tests use an async-aware MPSC channel from `tokio::sync` to avoid
//! deadlocks between the test thread and the application's async tasks.

use context_file_concat::app::{
    self, events::UserEvent, proxy::EventProxy, shared_state::SharedState, state::AppState,
};
use context_file_concat::config::AppConfig;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Contains the test infrastructure.
mod helpers {
    use super::*;
    use std::fs;

    /// A test double for the `EventLoopProxy` using a tokio MPSC channel.
    #[derive(Clone)]
    pub struct TestEventProxy {
        pub sender: mpsc::UnboundedSender<UserEvent>,
    }

    impl EventProxy for TestEventProxy {
        fn send_event(&self, event: UserEvent) {
            if let Err(e) = self.sender.send(event) {
                // Panic in a test if the receiver is dropped, as it indicates a test setup error.
                panic!("Test receiver dropped: {}", e);
            }
        }
    }

    /// `TestHarness` sets up a complete, isolated environment for each test case.
    pub struct TestHarness {
        pub state: SharedState,
        pub proxy: TestEventProxy,
        pub event_rx: mpsc::UnboundedReceiver<UserEvent>,
        pub root_path: PathBuf,
        _temp_dir: TempDir,
    }

    impl TestHarness {
        /// Creates a new test harness with a clean configuration.
        ///
        /// The state is a default `AppState` with another configuration: it is not
        /// bound to a config file, so the tests neither read nor write the developer's
        /// real configuration (see `test_state_of_the_tests_is_not_bound_to_a_config_file`).
        pub fn new() -> Self {
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            let root_path = temp_dir.path().to_path_buf();
            let (event_tx, event_rx) = mpsc::unbounded_channel();

            let config = Self::create_clean_test_config(root_path.clone());
            let mut state = AppState::default();
            state.config = config;
            // current_path is set by start_scan_on_path, so we leave it empty here.

            Self {
                state: SharedState::new(state),
                proxy: TestEventProxy { sender: event_tx },
                event_rx,
                root_path,
                _temp_dir: temp_dir,
            }
        }

        /// Creates a clean test configuration without production ignore patterns.
        fn create_clean_test_config(root_path: PathBuf) -> AppConfig {
            AppConfig {
                last_directory: Some(root_path),
                ignore_patterns: HashSet::new(), // Start with empty patterns
                case_sensitive_search: false,
                remove_empty_directories: true,
                ..Default::default()
            }
        }

        /// Creates a file inside the temporary test directory.
        pub fn create_file(&self, path: &str, content: &str) {
            let file_path = self.root_path.join(path);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).expect("Failed to create parent dir");
            }
            fs::write(file_path, content).expect("Failed to write file");
        }

        /// Sets up a standard project structure for testing.
        pub fn setup_basic_project(&self) {
            self.create_file("src/main.rs", "fn main() {}");
            self.create_file("src/lib.rs", "// Library code");
            self.create_file("README.md", "# My Project");
            self.create_file("Cargo.toml", "[package]\nname = \"test\"");
            self.create_file("docs/guide.txt", "User guide content");
        }

        /// Waits for the shallow scan phase to complete.
        pub async fn wait_for_shallow_scan_completion(&mut self) {
            loop {
                match tokio::time::timeout(Duration::from_secs(5), self.event_rx.recv()).await {
                    Ok(Some(UserEvent::StateUpdate(ui_state))) => {
                        // The shallow scan is done when we get a state update
                        // where scanning is still true, but the file list is not empty.
                        if ui_state.is_scanning && ui_state.rows_total > 0 {
                            return;
                        }
                    }
                    Ok(Some(_)) => { /* Ignore other events like ScanProgress */ }
                    _ => panic!("Shallow scan did not complete within timeout or channel closed"),
                }
            }
        }

        /// Waits for the entire background scan to complete.
        pub async fn wait_for_full_scan_completion(&mut self) {
            loop {
                match tokio::time::timeout(Duration::from_secs(10), self.event_rx.recv()).await {
                    Ok(Some(UserEvent::StateUpdate(ui_state))) => {
                        if !ui_state.is_scanning {
                            return; // Full scan is complete
                        }
                    }
                    Ok(Some(_)) => { /* Ignore other events */ }
                    _ => panic!("Full scan did not complete within timeout or channel closed"),
                }
            }
        }
    }
}

/// No test may ever read or write the developer's real configuration: the state the
/// tests work with is an `AppState::default()`, which is bound to no config file (see
/// `test_default_state_has_the_default_config_and_is_not_bound_to_a_file` in
/// `state.rs`), so "saving" its configuration succeeds and writes nothing.
#[test]
fn test_state_of_the_tests_is_not_bound_to_a_config_file() {
    let harness = helpers::TestHarness::new();
    let state = harness.state.lock();

    state
        .config_snapshot()
        .save()
        .expect("saving a detached snapshot is a no-op");
}

/// The location of the real configuration file can be redirected with an environment
/// variable; the application resolves it in exactly one place.
#[test]
fn test_default_config_path_honours_the_environment_override() {
    use context_file_concat::config::settings::{self, CONFIG_DIR_ENV};

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cfc-integration-config");
    // This is the only test of this binary that reads or writes the variable.
    std::env::set_var(CONFIG_DIR_ENV, &dir);
    let path = settings::default_config_path();
    std::env::remove_var(CONFIG_DIR_ENV);

    assert_eq!(path, Some(dir.join("config.json")));
}

#[tokio::test]
async fn test_proactive_scan_loads_shallow_then_deep() {
    // --- ARRANGE ---
    let mut harness = helpers::TestHarness::new();
    harness.setup_basic_project(); // Creates src/main.rs, docs/guide.txt, etc.

    // --- ACT ---
    // 1. Start the proactive scan
    app::tasks::start_scan_on_path(
        harness.root_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        false,
    );

    // 2. Wait for the shallow scan to finish and update the UI
    harness.wait_for_shallow_scan_completion().await;

    // --- ASSERT (Phase 1: Shallow Scan) ---
    {
        let state = harness.state.lock();
        assert!(
            state.is_scanning(),
            "Scanning should still be active in the background"
        );
        assert!(
            !state.is_fully_scanned,
            "is_fully_scanned should be false after shallow scan"
        );

        let visible_paths: HashSet<_> = state
            .full_file_list
            .iter()
            .map(|item| {
                item.path
                    .strip_prefix(&harness.root_path)
                    .unwrap()
                    .to_path_buf()
            })
            .collect();

        // Only top-level items should be present
        assert!(visible_paths.contains(&PathBuf::from("src")));
        assert!(visible_paths.contains(&PathBuf::from("docs")));
        assert!(visible_paths.contains(&PathBuf::from("README.md")));
        assert!(visible_paths.contains(&PathBuf::from("Cargo.toml")));

        // Nested items should NOT be present yet
        assert!(!visible_paths.contains(&PathBuf::from("src/main.rs")));
        assert!(!visible_paths.contains(&PathBuf::from("docs/guide.txt")));

        assert_eq!(
            visible_paths.len(),
            4,
            "Should only have 4 top-level items after shallow scan"
        );
    }

    // 3. Wait for the deep "indexing" scan to finish
    harness.wait_for_full_scan_completion().await;

    // --- ASSERT (Phase 2: Deep Scan) ---
    {
        let state = harness.state.lock();
        assert!(!state.is_scanning(), "Scanning should be finished");
        assert!(
            state.is_fully_scanned,
            "is_fully_scanned should be true after deep scan"
        );

        let all_paths: HashSet<_> = state
            .full_file_list
            .iter()
            .map(|item| {
                item.path
                    .strip_prefix(&harness.root_path)
                    .unwrap()
                    .to_path_buf()
            })
            .collect();

        // All items, including nested ones, should now be present
        assert!(all_paths.contains(&PathBuf::from("src/main.rs")));
        assert!(all_paths.contains(&PathBuf::from("src/lib.rs")));
        assert!(all_paths.contains(&PathBuf::from("docs/guide.txt")));

        // Total items: src, docs, README.md, Cargo.toml, src/main.rs, src/lib.rs, docs/guide.txt
        assert_eq!(
            all_paths.len(),
            7,
            "Should have all 7 project items after deep scan"
        );
    }
}

#[tokio::test]
async fn test_expand_all_after_scan() {
    // --- ARRANGE ---
    let mut harness = helpers::TestHarness::new();
    harness.setup_basic_project();

    // Run the full proactive scan and wait for it to complete
    app::tasks::start_scan_on_path(
        harness.root_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
        false,
    );
    harness.wait_for_full_scan_completion().await;

    // --- ACT ---
    // Now that the scan is complete, expand everything it has found
    app::commands::expand_collapse_all(
        serde_json::json!(true),
        harness.proxy.clone(),
        harness.state.clone(),
    );

    // The command is synchronous, so we can check the state immediately.

    // --- ASSERT ---
    let state = harness.state.lock();
    assert!(state.is_fully_scanned);

    // All directories ("src" and "docs") should be in the expanded set
    let src_path = harness.root_path.join("src");
    let docs_path = harness.root_path.join("docs");

    assert!(
        state.expanded_dirs.contains(&src_path),
        "src directory should be expanded"
    );
    assert!(
        state.expanded_dirs.contains(&docs_path),
        "docs directory should be expanded"
    );
    assert_eq!(
        state.expanded_dirs.len(),
        2,
        "There should be exactly 2 expanded directories"
    );
}
