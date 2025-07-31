//! Integration tests for the CFC (Context File Concatenator) application.
//!
//! These tests use an async-aware MPSC channel from `tokio::sync` to avoid
//! deadlocks between the test thread and the application's async tasks.

use context_file_concat::app::{self, events::UserEvent, proxy::EventProxy, state::AppState};
use context_file_concat::config::AppConfig;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
        pub state: Arc<Mutex<AppState>>,
        pub proxy: TestEventProxy,
        pub event_rx: mpsc::UnboundedReceiver<UserEvent>,
        pub root_path: PathBuf,
        _temp_dir: TempDir,
    }

    impl TestHarness {
        /// Creates a new test harness with a clean configuration.
        pub fn new() -> Self {
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            let root_path = temp_dir.path().to_path_buf();
            let (event_tx, event_rx) = mpsc::unbounded_channel();

            let config = Self::create_clean_test_config(root_path.clone());
            let mut state = AppState::default();
            state.config = config;
            state.current_path = root_path.to_string_lossy().to_string();

            Self {
                state: Arc::new(Mutex::new(state)),
                proxy: TestEventProxy { sender: event_tx },
                event_rx,
                root_path,
                _temp_dir: temp_dir,
            }
        }

        /// Creates a clean test configuration without production ignore patterns.
        ///
        /// This ensures that test files are not filtered out by default patterns
        /// and allows tests to verify specific filtering behavior in isolation.
        fn create_clean_test_config(root_path: PathBuf) -> AppConfig {
            AppConfig {
                last_directory: Some(root_path),
                ignore_patterns: HashSet::new(), // Start with empty patterns for clean tests
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

        /// Waits for the background scan to complete by listening for the final event.
        pub async fn wait_for_scan_completion(&mut self) {
            loop {
                match tokio::time::timeout(Duration::from_secs(5), self.event_rx.recv()).await {
                    Ok(Some(UserEvent::StateUpdate(ui_state))) => {
                        if !ui_state.is_scanning {
                            return; // Scan is complete
                        }
                    }
                    Ok(Some(_)) => { /* Ignore other events like ScanProgress */ }
                    _ => panic!("Scan did not complete within timeout or channel closed"),
                }
            }
        }

        /// Waits for any StateUpdate event. Useful for lazy loading.
        pub async fn wait_for_state_update(&mut self) {
            loop {
                match tokio::time::timeout(Duration::from_secs(5), self.event_rx.recv()).await {
                    Ok(Some(UserEvent::StateUpdate(_))) => {
                        return; // Got the update
                    }
                    Ok(Some(_)) => { /* Ignore other events */ }
                    _ => panic!("Did not receive StateUpdate within timeout or channel closed"),
                }
            }
        }
    }
}

#[tokio::test]
async fn test_scan_and_initial_state_is_lazy() {
    // --- ARRANGE ---
    let mut harness = helpers::TestHarness::new();
    harness.setup_basic_project(); // Creates src/main.rs, src/lib.rs, README.md, etc.

    // --- ACT ---
    app::tasks::start_scan_on_path(
        harness.root_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    harness.wait_for_scan_completion().await;

    // --- ASSERT ---
    let state = harness.state.lock().unwrap();
    assert!(!state.is_scanning, "Scan should be finished");

    let visible_paths: HashSet<_> = state
        .full_file_list // Check the full list, as filters aren't the focus here.
        .iter()
        .map(|item| {
            item.path
                .strip_prefix(&harness.root_path)
                .unwrap()
                .to_path_buf()
        })
        .collect();

    // Only top-level items should be present.
    assert!(
        visible_paths.contains(&PathBuf::from("src")),
        "Top-level 'src' directory should be present"
    );
    assert!(
        visible_paths.contains(&PathBuf::from("docs")),
        "Top-level 'docs' directory should be present"
    );
    assert!(
        visible_paths.contains(&PathBuf::from("README.md")),
        "Top-level 'README.md' file should be present"
    );
    assert!(
        visible_paths.contains(&PathBuf::from("Cargo.toml")),
        "Top-level 'Cargo.toml' file should be present"
    );

    // Nested items should NOT be present.
    assert!(
        !visible_paths.contains(&PathBuf::from("src/main.rs")),
        "'src/main.rs' should not be loaded initially"
    );
    assert!(
        !visible_paths.contains(&PathBuf::from("docs/guide.txt")),
        "'docs/guide.txt' should not be loaded initially"
    );

    assert_eq!(visible_paths.len(), 4, "Should only have 4 top-level items");

    // Also check the loaded_dirs set
    assert!(state.loaded_dirs.contains(&harness.root_path));
}

#[tokio::test]
async fn test_lazy_load_directory_level() {
    // --- ARRANGE ---
    let mut harness = helpers::TestHarness::new();
    harness.setup_basic_project();

    // Perform initial scan
    app::tasks::start_scan_on_path(
        harness.root_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    harness.wait_for_scan_completion().await;

    // --- ACT ---
    // Simulate user clicking on the 'src' directory to lazy load it.
    let src_dir_path = harness.root_path.join("src");
    app::commands::load_directory_level(
        serde_json::to_value(src_dir_path.to_string_lossy()).unwrap(),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    harness.wait_for_state_update().await;

    // --- ASSERT ---
    let state = harness.state.lock().unwrap();

    let full_list_paths: HashSet<_> = state
        .full_file_list
        .iter()
        .map(|item| {
            item.path
                .strip_prefix(&harness.root_path)
                .unwrap()
                .to_path_buf()
        })
        .collect();

    // Check that the original top-level items are still there.
    assert!(full_list_paths.contains(&PathBuf::from("src")));
    assert!(full_list_paths.contains(&PathBuf::from("README.md")));

    // Check that the newly loaded items are now present.
    assert!(
        full_list_paths.contains(&PathBuf::from("src/main.rs")),
        "'src/main.rs' should now be loaded"
    );
    assert!(
        full_list_paths.contains(&PathBuf::from("src/lib.rs")),
        "'src/lib.rs' should now be loaded"
    );

    // Check that items from other unloaded directories are still not present.
    assert!(
        !full_list_paths.contains(&PathBuf::from("docs/guide.txt")),
        "'docs/guide.txt' should still not be loaded"
    );

    // Check that the 'src' directory is now marked as loaded.
    assert!(state.loaded_dirs.contains(&src_dir_path));
}

#[tokio::test]
async fn test_ignore_patterns_are_applied_on_scan() {
    // --- ARRANGE ---
    let mut harness = helpers::TestHarness::new();
    harness.setup_basic_project(); // has README.md, Cargo.toml

    {
        let mut state = harness.state.lock().unwrap();
        // Ignore a top-level directory and a top-level file pattern
        state.config.ignore_patterns.insert("src/".to_string());
        state.config.ignore_patterns.insert("*.md".to_string());
    }

    // --- ACT ---
    app::tasks::start_scan_on_path(
        harness.root_path.clone(),
        harness.proxy.clone(),
        harness.state.clone(),
    );
    harness.wait_for_scan_completion().await;

    // --- ASSERT ---
    let state = harness.state.lock().unwrap();

    let visible_paths: HashSet<_> = state
        .full_file_list // test against full list
        .iter()
        .map(|item| {
            item.path
                .strip_prefix(&harness.root_path)
                .unwrap()
                .to_path_buf()
        })
        .collect();

    // Verify that ignored top-level patterns are not present
    assert!(
        !visible_paths.contains(&PathBuf::from("src")),
        "src directory should be filtered out by src/ pattern"
    );
    assert!(
        !visible_paths.contains(&PathBuf::from("README.md")),
        "README.md should be filtered out by *.md pattern"
    );

    // Verify that non-ignored top-level files are still present
    assert!(
        visible_paths.contains(&PathBuf::from("Cargo.toml")),
        "Cargo.toml should not be filtered out"
    );
    assert!(
        visible_paths.contains(&PathBuf::from("docs")),
        "docs directory should not be filtered out"
    );

    // Verify that ignore patterns were activated
    assert!(
        state.active_ignore_patterns.contains("src/"),
        "src/ ignore pattern should be active"
    );
    assert!(
        state.active_ignore_patterns.contains("*.md"),
        "*.md ignore pattern should be active"
    );
}
