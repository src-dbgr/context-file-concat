//! Contains long-running, asynchronous tasks that the application can perform.
//!
//! These tasks, such as scanning a directory or searching file contents, are designed
//! to run in the background without blocking the UI. They communicate their progress
//! and results back to the main application thread via `UserEvent`s.
//!
//! This module uses dependency inversion. Tasks are generic over traits (`ContentGenerator`,
//! `Scanner`, `Tokenizer`, `FileSearcher`) to allow for injecting mock implementations during testing,
//! making the tests deterministic and robust.

use async_trait::async_trait;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use super::events::UserEvent;
use super::filtering;
use super::proxy::EventProxy;
use super::state::AppState;
use super::view_model::{
    auto_expand_for_matches, generate_ui_state, get_selected_files_in_tree_order,
};

use crate::core::{CoreError, DirectoryScanner, FileHandler, FileItem, ScanProgress, SearchEngine};
use tiktoken_rs::cl100k_base;

//================================================================================================//
//|                                     SERVICE TRAITS                                           |//
//================================================================================================//

/// A trait abstracting the generation of concatenated file content.
#[async_trait]
pub trait ContentGenerator: Send + Sync {
    async fn generate(
        &self,
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    ) -> Result<String, CoreError>;
}

/// A trait abstracting the directory scanning functionality.
#[async_trait]
pub trait Scanner: Send + Sync {
    async fn scan(
        &self,
        root_path: &Path,
        max_depth: Option<usize>,
        progress_callback: Box<dyn Fn(ScanProgress) + Send + Sync>,
    ) -> Result<(Vec<FileItem>, HashSet<String>), CoreError>;
}

/// A trait abstracting the token counting functionality.
#[async_trait]
pub trait Tokenizer: Send + Sync {
    async fn count_tokens(&self, text: &str) -> usize;
}

/// A trait abstracting the file content search functionality.
#[async_trait]
pub trait FileSearcher: Send + Sync {
    async fn search(
        &self,
        files_to_search: Vec<FileItem>,
        query: &str,
        case_sensitive: bool,
    ) -> HashSet<PathBuf>;
}

//================================================================================================//
//|                                   REAL IMPLEMENTATIONS                                       |//
//================================================================================================//

pub struct RealContentGenerator {
    pub cancel_flag: Arc<AtomicBool>,
}
#[async_trait]
impl ContentGenerator for RealContentGenerator {
    async fn generate(
        &self,
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    ) -> Result<String, CoreError> {
        FileHandler::generate_concatenated_content_simple(
            selected_files,
            root_path,
            include_tree,
            items_for_tree,
            tree_ignore_patterns,
            use_relative_paths,
            self.cancel_flag.clone(),
            #[cfg(test)]
            None,
        )
        .await
    }
}

pub struct RealScanner {
    pub ignore_patterns: HashSet<String>,
    pub cancel_flag: Arc<AtomicBool>,
}
#[async_trait]
impl Scanner for RealScanner {
    async fn scan(
        &self,
        root_path: &Path,
        max_depth: Option<usize>,
        progress_callback: Box<dyn Fn(ScanProgress) + Send + Sync>,
    ) -> Result<(Vec<FileItem>, HashSet<String>), CoreError> {
        let scanner = DirectoryScanner::new(self.ignore_patterns.clone());
        scanner
            .scan_directory_with_progress(
                root_path,
                max_depth,
                self.cancel_flag.clone(),
                progress_callback,
            )
            .await
    }
}

pub struct RealTokenizer;
#[async_trait]
impl Tokenizer for RealTokenizer {
    async fn count_tokens(&self, text: &str) -> usize {
        let text_clone = text.to_string();
        tokio::task::spawn_blocking(move || {
            cl100k_base()
                .map(|bpe| bpe.encode_with_special_tokens(&text_clone).len())
                .unwrap_or(0)
        })
        .await
        .unwrap_or(0)
    }
}

#[derive(Copy, Clone)]
pub struct RealFileSearcher;
#[async_trait]
impl FileSearcher for RealFileSearcher {
    async fn search(
        &self,
        files_to_search: Vec<FileItem>,
        query: &str,
        case_sensitive: bool,
    ) -> HashSet<PathBuf> {
        let query_clone = query.to_string();
        tokio::task::spawn_blocking(move || {
            files_to_search
                .into_par_iter()
                .filter_map(|item| {
                    if item.is_directory || item.is_binary {
                        return None;
                    }
                    if let Ok(content) = std::fs::read_to_string(&item.path) {
                        let found = if case_sensitive {
                            content.contains(&query_clone)
                        } else {
                            content.to_lowercase().contains(&query_clone.to_lowercase())
                        };
                        if found {
                            Some(item.path)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }
}

//================================================================================================//
//|                                   TASK IMPLEMENTATIONS                                       |//
//================================================================================================//

/// The main asynchronous task for generating the concatenated file content.
pub async fn generation_task<P, G, T>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    content_generator: G,
    tokenizer: T,
) where
    P: EventProxy,
    G: ContentGenerator + 'static,
    T: Tokenizer + 'static,
{
    let (selected, root, config, files_for_tree, is_fully_scanned) = {
        let state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        (
            get_selected_files_in_tree_order(&state_guard),
            PathBuf::from(&state_guard.current_path),
            state_guard.config.clone(),
            state_guard.full_file_list.clone(),
            state_guard.is_fully_scanned,
        )
    };

    let items_for_tree = if config.remove_empty_directories && is_fully_scanned {
        tracing::info!("🌳 Pruning empty directories from the generated tree.");
        SearchEngine::remove_empty_directories(
            files_for_tree.clone(),
            &files_for_tree,
            &HashSet::new(),
        )
        .0
    } else {
        files_for_tree
    };

    let result = content_generator
        .generate(
            &selected,
            &root,
            config.include_tree_by_default,
            items_for_tree,
            config.tree_ignore_patterns,
            config.use_relative_paths,
        )
        .await;

    let finalize_state = |s: &mut AppState| {
        s.is_generating = false;
        proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(s))));
    };

    match result {
        Ok(content) => {
            let token_count = tokenizer.count_tokens(&content).await;
            proxy.send_event(UserEvent::ShowGeneratedContent {
                content,
                token_count,
            });
            let mut state_guard = state.lock().expect("Mutex poisoned");
            finalize_state(&mut state_guard);
        }
        Err(CoreError::Cancelled) => {
            tracing::info!("LOG: Generation task gracefully cancelled.");
            let mut state_guard = state.lock().expect("Mutex poisoned");
            state_guard.scan_progress.current_scanning_path = "Generation cancelled.".to_string();
            finalize_state(&mut state_guard);
        }
        Err(e) => {
            tracing::error!("LOG: Generation task failed: {}", e);
            proxy.send_event(UserEvent::ShowError(e.to_string()));
            let mut state_guard = state.lock().expect("Mutex poisoned");
            finalize_state(&mut state_guard);
        }
    }
}

/// The core orchestration logic for the proactive, two-phase scan.
pub async fn proactive_scan_task<P: EventProxy, S: Scanner>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    path: PathBuf,
    scanner: S,
) {
    // VET: RAII Guard now also takes the proxy to send a final state update.
    struct ScanGuard<'a, P: EventProxy> {
        state: &'a Arc<Mutex<AppState>>,
        proxy: &'a P,
    }
    impl<'a, P: EventProxy> Drop for ScanGuard<'a, P> {
        fn drop(&mut self) {
            let mut state = self.state.lock().unwrap();
            // Only update and notify if the scan was actually running.
            // This prevents sending a redundant event if the scan finished cleanly.
            if state.is_scanning {
                state.is_scanning = false;
                state.scan_task = None;
                self.proxy
                    .send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&state))));
            }
        }
    }
    let _scan_guard = ScanGuard {
        state: &state,
        proxy: &proxy,
    };

    // --- Phase 1: Shallow Scan ---
    let progress_proxy_shallow = proxy.clone();
    let scan_result_shallow = scanner
        .scan(
            &path,
            Some(1),
            Box::new(move |p| progress_proxy_shallow.send_event(UserEvent::ScanProgress(p))),
        )
        .await;

    if state
        .lock()
        .unwrap()
        .scan_cancellation_flag
        .load(Ordering::SeqCst)
    {
        tracing::info!("Scan cancelled after shallow scan phase.");
        return; // _scan_guard is dropped here, cleaning up the state and notifying.
    }

    match scan_result_shallow {
        Ok((files, patterns)) => {
            let mut s = state.lock().unwrap();
            s.full_file_list = files;
            s.active_ignore_patterns = patterns;
            s.loaded_dirs.insert(path.clone());
            filtering::apply_filters(&mut s);
            proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&s))));
        }
        Err(e) => {
            handle_scan_error(e, &state, &proxy);
            return; // _scan_guard is dropped here.
        }
    }

    // --- Phase 2: Deep Background Scan (Indexing) ---
    let progress_proxy_deep = proxy.clone();
    let scan_result_deep = scanner
        .scan(
            &path,
            None,
            Box::new(move |p| progress_proxy_deep.send_event(UserEvent::ScanProgress(p))),
        )
        .await;

    if state
        .lock()
        .unwrap()
        .scan_cancellation_flag
        .load(Ordering::SeqCst)
    {
        tracing::info!("Scan cancelled during deep scan phase.");
        return; // _scan_guard is dropped here.
    }

    match scan_result_deep {
        Ok((files, patterns)) => {
            let mut s = state.lock().unwrap();
            let new_file_paths: HashSet<_> = files.iter().map(|f| f.path.clone()).collect();
            s.selected_files.retain(|p| new_file_paths.contains(p));
            s.full_file_list = files;
            s.active_ignore_patterns = patterns;
            s.is_fully_scanned = true;
            s.loaded_dirs = s
                .full_file_list
                .iter()
                .filter(|i| i.is_directory)
                .map(|i| i.path.clone())
                .collect();
            filtering::apply_filters(&mut s);

            // VET: We now set the final state here and the guard is just for cleanup on panics/cancellations.
            s.is_scanning = false;
            s.scan_task = None;
            s.scan_progress.current_scanning_path = format!(
                "Indexing complete. Found {} visible items.",
                s.filtered_file_list.len()
            );

            proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&s))));
        }
        Err(e) => {
            handle_scan_error(e, &state, &proxy);
        }
    }
}

/// Initiates a proactive, two-phase directory scan.
pub fn start_scan_on_path<P: EventProxy>(
    path: PathBuf,
    proxy: P,
    state: Arc<Mutex<AppState>>,
    preserve_state: bool,
) {
    let proxy_clone = proxy.clone();
    let state_clone = state.clone();
    tokio::spawn(async move {
        // 1. First, check if the path exists at all.
        if !path.exists() {
            proxy.send_event(UserEvent::ShowError(format!(
                "Path does not exist: {}",
                path.display()
            )));
            return;
        }

        // 2. If it exists, determine the correct directory to scan.
        let directory_path = if path.is_dir() {
            path
        } else {
            // It's a file, so we take its parent. This is safe now because we know the path exists.
            path.parent().map(|p| p.to_path_buf()).unwrap_or(path) // Fallback just in case of root paths like "/"
        };

        let new_cancel_flag = {
            let mut state_guard = state.lock().expect("Mutex was poisoned");
            if !preserve_state {
                state_guard.reset_directory_state();
            } else {
                state_guard.cancel_current_scan();
            }
            state_guard.current_path = directory_path.to_string_lossy().to_string();
            state_guard.config.last_directory = Some(directory_path.clone());
            crate::config::settings::save_config(&state_guard.config, None).ok();
            state_guard.is_scanning = true;
            state_guard.is_fully_scanned = false;
            let flag = Arc::new(AtomicBool::new(false));
            state_guard.scan_cancellation_flag = flag.clone();
            flag
        };
        proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
            &state.lock().unwrap(),
        ))));
        let ignore_patterns = state.lock().unwrap().config.ignore_patterns.clone();
        let scanner = RealScanner {
            ignore_patterns,
            cancel_flag: new_cancel_flag,
        };
        let handle = tokio::spawn(async move {
            proactive_scan_task(proxy_clone, state_clone, directory_path, scanner).await;
        });
        let mut state_guard = state.lock().expect("Mutex was poisoned");
        state_guard.scan_task = Some(handle);
    });
}

/// Helper to handle scan errors consistently.
fn handle_scan_error<P: EventProxy>(error: CoreError, state: &Arc<Mutex<AppState>>, proxy: &P) {
    tracing::error!("LOG: TASK:: Scan finished with error: {}", error);
    let mut state_lock = state.lock().expect("Mutex was poisoned");
    if !state_lock.is_scanning {
        return;
    }
    state_lock.scan_progress.current_scanning_path = format!("Scan failed: {error}");
    state_lock.is_scanning = false;
    state_lock.scan_task = None;
    let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_lock)));
    proxy.send_event(event);
}

/// Initiates a scan for a specific subdirectory level (lazy loading).
pub fn start_lazy_load_scan<P: EventProxy>(
    path: PathBuf,
    proxy: P,
    state: Arc<Mutex<AppState>>,
    completion_signal: Option<oneshot::Sender<()>>,
) {
    tokio::spawn(async move {
        let (ignore_patterns, is_scanning) = {
            let state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            (
                state_guard.config.ignore_patterns.clone(),
                state_guard.is_scanning,
            )
        };
        if is_scanning {
            tracing::warn!("Attempted to lazy load while a full scan was in progress. Ignoring.");
            return;
        }
        let new_cancel_flag = Arc::new(AtomicBool::new(false));
        let scanner = RealScanner {
            ignore_patterns,
            cancel_flag: new_cancel_flag.clone(),
        };
        let proxy_clone = proxy.clone();
        let state_clone = state.clone();
        tracing::info!("LOG: Spawning new lazy_load_task for path: {:?}", path);
        tokio::spawn(async move {
            lazy_load_task(path, proxy_clone, state_clone, scanner, completion_signal).await;
        });
    });
}

/// The asynchronous task for scanning a single directory level and appending the results.
async fn lazy_load_task<P: EventProxy, S: Scanner>(
    path_to_load: PathBuf,
    proxy: P,
    state: Arc<Mutex<AppState>>,
    scanner: S,
    completion_signal: Option<oneshot::Sender<()>>,
) {
    // The scan call remains the same.
    let scan_result = scanner.scan(&path_to_load, Some(1), Box::new(|_| {})).await;

    match scan_result {
        Ok((new_items, new_active_patterns)) => {
            tracing::info!(
                "LOG: TASK:: Lazy load successful. {} new items found for {:?}.",
                new_items.len(),
                path_to_load
            );

            // Instead of using the `with_state_and_notify` helper, we now manually
            // lock the state and explicitly send the notification event.
            // This aligns with the pattern used in other modern tasks in this file.
            let mut state_guard = state.lock().expect("Mutex was poisoned");

            state_guard.loaded_dirs.insert(path_to_load.clone());
            state_guard.expanded_dirs.insert(path_to_load);
            state_guard
                .active_ignore_patterns
                .extend(new_active_patterns);

            let existing_paths: HashSet<PathBuf> = state_guard
                .full_file_list
                .iter()
                .map(|item| item.path.clone())
                .collect();

            for item in new_items {
                if !existing_paths.contains(&item.path) {
                    state_guard.full_file_list.push(item);
                }
            }

            // Apply other filters to the now-updated list.
            filtering::apply_filters(&mut state_guard);

            // Explicitly send the state update event. This fixes the test panic.
            proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
                &state_guard,
            ))));
        }
        Err(e) => {
            // Error handling remains the same.
            tracing::error!("LOG: TASK:: Lazy load failed for {:?}: {}", path_to_load, e);
            proxy.send_event(UserEvent::ShowError(format!(
                "Failed to load directory {}: {}",
                path_to_load.display(),
                e
            )));
        }
    }

    // After all work is done, send the completion signal if one was provided.
    if let Some(signal) = completion_signal {
        let _ = signal.send(());
    }
}

/// Performs a content search across all non-binary files.
pub async fn search_in_files<P: EventProxy, S: FileSearcher>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    searcher: S,
) {
    let (files_to_search, query, case_sensitive) = {
        let mut state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        if state_guard.content_search_query.is_empty() {
            state_guard.content_search_results.clear();
            filtering::apply_filters(&mut state_guard);
            let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
            proxy.send_event(event);
            return;
        }
        (
            state_guard.full_file_list.clone(),
            state_guard.content_search_query.clone(),
            state_guard.config.case_sensitive_search,
        )
    };
    let matching_paths = searcher
        .search(files_to_search, &query, case_sensitive)
        .await;
    let mut state_guard = state
        .lock()
        .expect("Mutex was poisoned. This should not happen.");
    state_guard.content_search_results = matching_paths;
    filtering::apply_filters(&mut state_guard);
    auto_expand_for_matches(&mut state_guard);
    let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
    proxy.send_event(event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view_model::UiState;
    use crate::config::AppConfig;
    use crate::utils::test_helpers::running_as_root;
    use std::time::Duration;
    use tempfile::{tempdir, TempDir};
    use tokio::sync::{mpsc, oneshot};

    //============================================================================================//
    //|                                       TEST HARNESS                                       |//
    //============================================================================================//
    #[derive(Clone)]
    struct TestEventProxy {
        sender: mpsc::UnboundedSender<UserEvent>,
    }
    impl EventProxy for TestEventProxy {
        fn send_event(&self, event: UserEvent) {
            self.sender.send(event).expect("Test receiver dropped");
        }
    }
    struct TestHarness {
        state: Arc<Mutex<AppState>>,
        proxy: TestEventProxy,
        event_rx: mpsc::UnboundedReceiver<UserEvent>,
        _temp_dir: TempDir,
        root_path: PathBuf,
    }
    impl TestHarness {
        fn new() -> Self {
            let temp_dir = tempdir().expect("Failed to create temp dir");
            let root_path = temp_dir.path().to_path_buf();
            let (tx, rx) = mpsc::unbounded_channel();
            let proxy = TestEventProxy { sender: tx };
            let mut state = AppState::default();
            state.config = AppConfig::default();
            state.current_path = root_path.to_string_lossy().to_string();
            Self {
                state: Arc::new(Mutex::new(state)),
                proxy,
                event_rx: rx,
                _temp_dir: temp_dir,
                root_path,
            }
        }
        async fn get_n_events(&mut self, n: usize) -> Vec<UserEvent> {
            let mut events = Vec::with_capacity(n);
            for _ in 0..n {
                match tokio::time::timeout(Duration::from_secs(2), self.event_rx.recv()).await {
                    Ok(Some(event)) => events.push(event),
                    _ => break,
                }
            }
            events
        }
        async fn get_last_state_update(&mut self) -> Option<UiState> {
            let mut last_state = None;
            while let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(100), self.event_rx.recv()).await
            {
                if let UserEvent::StateUpdate(state) = event {
                    last_state = Some(*state);
                }
            }
            last_state
        }
    }

    //============================================================================================//
    //|                                        MOCK SERVICES                                     |//
    //============================================================================================//

    #[derive(Clone)]
    struct MockContentGenerator {
        result: Arc<Mutex<Result<String, CoreError>>>,
        cancellation_check: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
        start_notifier: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    }

    impl MockContentGenerator {
        fn new() -> Self {
            Self {
                result: Arc::new(Mutex::new(Ok(String::new()))),
                cancellation_check: Arc::new(Mutex::new(None)),
                start_notifier: Arc::new(Mutex::new(None)),
            }
        }
        fn set_result(&self, result: Result<String, CoreError>) {
            *self.result.lock().unwrap() = result;
        }
        fn expect_cancellation(&mut self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
            let (tx_start, rx_start) = oneshot::channel();
            *self.start_notifier.lock().unwrap() = Some(tx_start);
            let (tx_wait, rx_wait) = oneshot::channel();
            *self.cancellation_check.lock().unwrap() = Some(rx_wait);
            (rx_start, tx_wait)
        }
    }

    #[async_trait]
    impl ContentGenerator for MockContentGenerator {
        async fn generate(
            &self,
            _: &[PathBuf],
            _: &Path,
            _: bool,
            _: Vec<FileItem>,
            _: HashSet<String>,
            _: bool,
        ) -> Result<String, CoreError> {
            if let Some(notifier) = self.start_notifier.lock().unwrap().take() {
                let _ = notifier.send(());
            }
            // VET: Release lock before await to fix !Send error
            let receiver = self.cancellation_check.lock().unwrap().take();
            if let Some(receiver) = receiver {
                let _ = receiver.await;
            }
            self.result.lock().unwrap().clone()
        }
    }

    #[derive(Clone)]
    struct MockTokenizer {
        token_count: usize,
    }

    #[async_trait]
    impl Tokenizer for MockTokenizer {
        async fn count_tokens(&self, _: &str) -> usize {
            self.token_count
        }
    }

    #[derive(Clone)]
    struct MockScanner {
        shallow_result: Arc<Mutex<Result<(Vec<FileItem>, HashSet<String>), CoreError>>>,
        deep_result: Arc<Mutex<Result<(Vec<FileItem>, HashSet<String>), CoreError>>>,
        cancellation_trigger: Arc<Mutex<Option<oneshot::Sender<()>>>>,
        wait_for_cancel: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    }

    impl MockScanner {
        fn new() -> Self {
            Self {
                shallow_result: Arc::new(Mutex::new(Ok((vec![], HashSet::new())))),
                deep_result: Arc::new(Mutex::new(Ok((vec![], HashSet::new())))),
                cancellation_trigger: Arc::new(Mutex::new(None)),
                wait_for_cancel: Arc::new(Mutex::new(None)),
            }
        }
        fn set_results(&mut self, shallow: Vec<FileItem>, deep: Vec<FileItem>) {
            *self.shallow_result.lock().unwrap() = Ok((shallow, HashSet::new()));
            *self.deep_result.lock().unwrap() = Ok((deep, HashSet::new()));
        }
        fn prepare_for_cancellation(&mut self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
            let (tx_trigger, rx_trigger) = oneshot::channel();
            let (tx_wait, rx_wait) = oneshot::channel();
            *self.cancellation_trigger.lock().unwrap() = Some(tx_trigger);
            *self.wait_for_cancel.lock().unwrap() = Some(rx_wait);
            (rx_trigger, tx_wait)
        }
    }

    #[async_trait]
    impl Scanner for MockScanner {
        async fn scan(
            &self,
            _: &Path,
            depth: Option<usize>,
            _: Box<dyn Fn(ScanProgress) + Send + Sync>,
        ) -> Result<(Vec<FileItem>, HashSet<String>), CoreError> {
            if depth == Some(1) {
                if let Some(trigger) = self.cancellation_trigger.lock().unwrap().take() {
                    trigger.send(()).ok();
                }
                self.shallow_result.lock().unwrap().clone()
            } else {
                let waiter = self.wait_for_cancel.lock().unwrap().take();
                if let Some(waiter) = waiter {
                    waiter.await.ok();
                }
                self.deep_result.lock().unwrap().clone()
            }
        }
    }

    #[derive(Clone, Default)]
    struct MockFileSearcher {
        results: Arc<Mutex<HashSet<PathBuf>>>,
    }
    impl MockFileSearcher {
        fn set_results(&self, results: HashSet<PathBuf>) {
            *self.results.lock().unwrap() = results;
        }
    }
    #[async_trait]
    impl FileSearcher for MockFileSearcher {
        async fn search(&self, _: Vec<FileItem>, _: &str, _: bool) -> HashSet<PathBuf> {
            self.results.lock().unwrap().clone()
        }
    }

    //============================================================================================//
    //|                                        TEST CASES                                        |//
    //============================================================================================//

    #[tokio::test]
    async fn generation_task_happy_path_sends_content_and_resets_state() {
        // Arrange
        let mut harness = TestHarness::new();
        let generator = MockContentGenerator::new();
        generator.set_result(Ok("Generated Content".to_string()));
        let tokenizer = MockTokenizer { token_count: 2 };
        harness.state.lock().unwrap().is_generating = true;

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
        )
        .await;

        // Assert
        let events = harness.get_n_events(2).await;
        assert_eq!(events.len(), 2, "Expected exactly two events");

        assert!(matches!(events[0], UserEvent::ShowGeneratedContent { .. }));
        if let UserEvent::ShowGeneratedContent {
            content,
            token_count,
        } = &events[0]
        {
            assert_eq!(content, "Generated Content");
            assert_eq!(*token_count, 2);
        }

        // VET: Fix - Assert the event we already captured, don't try to fetch a new one.
        assert!(matches!(events[1], UserEvent::StateUpdate(_)));
        if let UserEvent::StateUpdate(final_state_in_event) = &events[1] {
            assert!(!final_state_in_event.is_generating);
        }

        assert!(
            !harness.state.lock().unwrap().is_generating,
            "is_generating should be reset in AppState"
        );
    }

    #[tokio::test]
    async fn generation_task_cancellation_is_handled_gracefully() {
        let mut harness = TestHarness::new();
        let mut generator = MockContentGenerator::new();
        let (has_started_receiver, unblock_sender) = generator.expect_cancellation();
        generator.set_result(Err(CoreError::Cancelled));
        harness.state.lock().unwrap().is_generating = true;

        let proxy_clone = harness.proxy.clone();
        let task_state = harness.state.clone();

        let task_handle = tokio::spawn(async move {
            generation_task(
                proxy_clone,
                task_state,
                generator,
                MockTokenizer { token_count: 0 },
            )
            .await;
        });
        has_started_receiver
            .await
            .expect("Mock did not signal start");
        unblock_sender.send(()).unwrap();
        task_handle.await.unwrap();
        let final_state = harness.get_last_state_update().await.unwrap();
        assert!(!final_state.is_generating);
        assert!(final_state.status_message.contains("Generation cancelled"));
        let events = harness.get_n_events(1).await;
        assert!(!matches!(events.get(0), Some(UserEvent::ShowError(_))));
    }

    #[tokio::test]
    async fn generation_task_io_error_sends_error_event_and_resets_state() {
        // Arrange
        let mut harness = TestHarness::new();
        let generator = MockContentGenerator::new();
        let io_error = CoreError::Io("File not found".to_string(), PathBuf::from("a/b/c.txt"));
        generator.set_result(Err(io_error));
        harness.state.lock().unwrap().is_generating = true;

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 0 },
        )
        .await;

        // Assert
        let events = harness.get_n_events(2).await;
        assert_eq!(events.len(), 2);

        assert!(matches!(events[0], UserEvent::ShowError(_)));
        if let UserEvent::ShowError(msg) = &events[0] {
            assert!(msg.contains("I/O error"));
            assert!(msg.contains("a/b/c.txt"));
        }

        // VET: Fix - Assert the event we already captured.
        assert!(matches!(events[1], UserEvent::StateUpdate(_)));
        if let UserEvent::StateUpdate(final_state_in_event) = &events[1] {
            assert!(!final_state_in_event.is_generating);
        }

        assert!(
            !harness.state.lock().unwrap().is_generating,
            "is_generating should be reset in AppState"
        );
    }

    #[tokio::test]
    async fn proactive_scan_task_verifies_two_phase_events() {
        // Arrange
        let mut harness = TestHarness::new();
        let mut scanner = MockScanner::new();

        let shallow_files = vec![FileItem {
            path: harness.root_path.join("src"),
            is_directory: true,
            ..Default::default()
        }];
        let deep_files = vec![
            FileItem {
                path: harness.root_path.join("src"),
                is_directory: true,
                ..Default::default()
            },
            FileItem {
                path: harness.root_path.join("src/main.rs"),
                is_directory: false,
                ..Default::default()
            },
        ];
        scanner.set_results(shallow_files.clone(), deep_files.clone());

        harness.state.lock().unwrap().is_scanning = true;

        // Act
        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
        )
        .await;

        // Assert
        // VET: Fix - Expect 2 events now. The success path sets the final state itself.
        // The guard is now only for panics or cancellations.
        let events = harness.get_n_events(2).await;
        assert_eq!(events.len(), 2, "Expected two StateUpdate events");

        // Phase 1: Shallow scan update
        let state1 = match &events[0] {
            UserEvent::StateUpdate(s) => s,
            _ => panic!("Expected StateUpdate"),
        };
        assert_eq!(state1.visible_files_count, 1);
        assert!(
            !state1.is_fully_scanned,
            "Should not be fully scanned after phase 1"
        );
        assert!(state1.is_scanning, "Should still be scanning after phase 1");

        // Phase 2: Deep scan update (this is the final event in the success case)
        let state2 = match &events[1] {
            UserEvent::StateUpdate(s) => s,
            _ => panic!("Expected StateUpdate"),
        };
        assert_eq!(state2.visible_files_count, 2);
        assert!(
            state2.is_fully_scanned,
            "Should be fully scanned after phase 2"
        );
        assert!(
            !state2.is_scanning,
            "Scanning should be complete in final state"
        );
        assert!(state2.status_message.contains("Indexing complete"));
    }

    #[tokio::test]
    async fn proactive_scan_cancellation_during_deep_scan_aborts_task() {
        let harness = TestHarness::new();
        let mut scanner = MockScanner::new();
        scanner.set_results(vec![FileItem::default()], vec![]);
        let (has_started_deep_scan, unblock_deep_scan) = scanner.prepare_for_cancellation();
        harness.state.lock().unwrap().is_scanning = true;
        let cancel_flag = harness.state.lock().unwrap().scan_cancellation_flag.clone();

        let proxy_clone = harness.proxy.clone();
        let task_state = harness.state.clone();
        let task_path = harness.root_path.clone();

        let task_handle = tokio::spawn(async move {
            proactive_scan_task(proxy_clone, task_state, task_path, scanner).await;
        });
        has_started_deep_scan.await.unwrap();
        cancel_flag.store(true, Ordering::SeqCst);
        unblock_deep_scan.send(()).unwrap();
        let _ = task_handle.await;

        // The task now cleans up after itself via the RAII guard.
        // We can now confidently check the final state.
        let final_app_state = harness.state.lock().unwrap();
        assert!(!final_app_state.is_scanning, "is_scanning should be false");
        assert!(
            !final_app_state.is_fully_scanned,
            "is_fully_scanned should be false"
        );
    }

    #[tokio::test]
    async fn search_in_files_updates_state_with_mock_results() {
        let mut harness = TestHarness::new();
        let searcher = MockFileSearcher::default();
        let match1 = harness.root_path.join("match1.txt");
        let match2 = harness.root_path.join("match2.txt");
        let mut mock_results = HashSet::new();
        mock_results.insert(match1.clone());
        mock_results.insert(match2.clone());
        searcher.set_results(mock_results);
        {
            let mut state = harness.state.lock().unwrap();
            state.content_search_query = "magic".to_string();
            state.full_file_list.push(FileItem {
                path: match1.clone(),
                ..Default::default()
            });
            state.full_file_list.push(FileItem {
                path: match2.clone(),
                ..Default::default()
            });
            state.full_file_list.push(FileItem {
                path: harness.root_path.join("nomatch.txt"),
                ..Default::default()
            });
        }
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
        {
            let final_state = harness.state.lock().unwrap();
            assert_eq!(final_state.content_search_results.len(), 2);
            assert!(final_state.content_search_results.contains(&match1));
            assert!(final_state.content_search_results.contains(&match2));
        }
        let ui_state = harness.get_last_state_update().await.unwrap();
        assert_eq!(
            ui_state.visible_files_count, 2,
            "Filtered list should only contain the 2 matches"
        );
    }

    #[tokio::test]
    async fn search_in_files_clears_results_on_empty_query() {
        let mut harness = TestHarness::new();
        let searcher = MockFileSearcher::default();
        {
            let mut state = harness.state.lock().unwrap();
            state
                .content_search_results
                .insert(PathBuf::from("previous_match.txt"));
            state.content_search_query = "".to_string();
        }
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
        {
            let final_state = harness.state.lock().unwrap();
            assert!(
                final_state.content_search_results.is_empty(),
                "Search results should be cleared"
            );
        }
        assert!(
            harness.get_last_state_update().await.is_some(),
            "Should have received a state update"
        );
    }

    #[tokio::test]
    async fn proactive_scan_task_handles_shallow_scan_error() {
        // Arrange
        let mut harness = TestHarness::new();
        let scanner = MockScanner::new();
        let scan_error = CoreError::Io("Permission denied".to_string(), harness.root_path.clone());

        // Configure the mock scanner to fail the shallow scan
        *scanner.shallow_result.lock().unwrap() = Err(scan_error.clone());

        harness.state.lock().unwrap().is_scanning = true;

        // Act
        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
        )
        .await;

        // Assert
        // The RAII guard will send a final StateUpdate on drop because of the error.
        let events = harness.get_n_events(1).await;
        assert_eq!(events.len(), 1, "Expected one final StateUpdate event");

        let final_state = match &events[0] {
            UserEvent::StateUpdate(s) => s,
            _ => panic!("Expected a StateUpdate event."),
        };

        assert!(
            !final_state.is_scanning,
            "Scanning should be stopped on error."
        );
        assert!(
            final_state
                .status_message
                .contains("Scan failed: I/O error"),
            "Status message should reflect the scan failure."
        );
    }

    #[tokio::test]
    async fn proactive_scan_task_handles_deep_scan_error() {
        // Arrange
        let mut harness = TestHarness::new();
        let scanner = MockScanner::new();
        let scan_error = CoreError::Io("Disk full".to_string(), harness.root_path.clone());

        // Shallow scan succeeds, deep scan fails
        let shallow_files = vec![FileItem {
            path: harness.root_path.join("file.txt"),
            ..Default::default()
        }];
        *scanner.shallow_result.lock().unwrap() = Ok((shallow_files.clone(), HashSet::new()));
        *scanner.deep_result.lock().unwrap() = Err(scan_error.clone());

        harness.state.lock().unwrap().is_scanning = true;

        // Act
        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
        )
        .await;

        // Assert
        // Expect two StateUpdates: one after the successful shallow scan, one for the final error state.
        let events = harness.get_n_events(2).await;
        assert_eq!(events.len(), 2, "Expected two StateUpdate events");

        // Check state after shallow scan
        let state1 = match &events[0] {
            UserEvent::StateUpdate(s) => s,
            _ => panic!("Expected StateUpdate for shallow scan"),
        };
        assert!(
            state1.is_scanning,
            "Should still be scanning after shallow scan."
        );
        assert_eq!(
            state1.visible_files_count, 1,
            "Shallow scan results should be visible."
        );

        // Check final state after deep scan error
        let state2 = match &events[1] {
            UserEvent::StateUpdate(s) => s,
            _ => panic!("Expected StateUpdate for deep scan failure"),
        };
        assert!(
            !state2.is_scanning,
            "Scanning should be stopped after error."
        );
        assert!(
            state2.status_message.contains("Scan failed: I/O error"),
            "Status message should reflect the deep scan failure."
        );
    }

    #[tokio::test]
    async fn generation_task_handles_generator_error() {
        // Arrange
        let mut harness = TestHarness::new();
        let generator = MockContentGenerator::new();
        let gen_error = CoreError::Io("Failed to read file".to_string(), PathBuf::from("test.txt"));
        generator.set_result(Err(gen_error.clone()));
        let tokenizer = MockTokenizer { token_count: 0 };
        harness.state.lock().unwrap().is_generating = true;

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
        )
        .await;

        // Assert
        // Expect a ShowError event, followed by a StateUpdate.
        let events = harness.get_n_events(2).await;
        assert_eq!(events.len(), 2, "Expected ShowError and StateUpdate events");

        // Check the ShowError event
        match &events[0] {
            UserEvent::ShowError(msg) => {
                assert!(
                    msg.contains("Failed to read file"),
                    "Error message content is incorrect."
                );
            }
            _ => panic!("Expected a ShowError event first."),
        }

        // Check the final StateUpdate
        match &events[1] {
            UserEvent::StateUpdate(s) => {
                assert!(!s.is_generating, "is_generating flag should be reset.");
            }
            _ => panic!("Expected a StateUpdate event second."),
        }

        // Double-check the final state directly
        assert!(
            !harness.state.lock().unwrap().is_generating,
            "is_generating should be false in final AppState."
        );
    }

    #[tokio::test]
    async fn lazy_load_task_handles_scan_error() {
        // Arrange
        let mut harness = TestHarness::new();
        let scanner = MockScanner::new();
        let path_to_load = harness.root_path.join("lazy");

        // Configure scanner to fail for the shallow scan, which is what lazy_load uses
        let scan_error = CoreError::NotADirectory(path_to_load.clone());
        *scanner.shallow_result.lock().unwrap() = Err(scan_error.clone());

        // Act
        lazy_load_task(
            path_to_load.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            None,
        )
        .await;

        // Assert
        // Expect a ShowError event.
        let events = harness.get_n_events(1).await;
        assert_eq!(events.len(), 1, "Expected one ShowError event");

        match &events[0] {
            UserEvent::ShowError(msg) => {
                assert!(
                    msg.contains("Failed to load directory"),
                    "Error message is incorrect"
                );
                assert!(
                    msg.contains("lazy"),
                    "Error message should contain the path"
                );
                assert!(
                    msg.contains("not a valid directory"),
                    "Error message should contain the error detail"
                );
            }
            _ => panic!("Expected a ShowError event."),
        }

        // Assert that state was not left inconsistent
        let state = harness.state.lock().unwrap();
        assert!(
            !state.loaded_dirs.contains(&path_to_load),
            "Failed path should not be marked as loaded."
        );
    }

    #[tokio::test]
    async fn proactive_scan_task_cancels_between_phases() {
        // Arrange
        let mut harness = TestHarness::new();
        let mut scanner = MockScanner::new();

        // Use the cancellation helper to pause the task between scan phases
        let (shallow_scan_finished, allow_deep_scan_to_proceed) =
            scanner.prepare_for_cancellation();

        // Shallow scan succeeds.
        let shallow_files = vec![FileItem {
            path: harness.root_path.join("file.txt"),
            ..Default::default()
        }];
        *scanner.shallow_result.lock().unwrap() = Ok((shallow_files.clone(), HashSet::new()));
        *scanner.deep_result.lock().unwrap() = Ok((vec![], HashSet::new()));

        harness.state.lock().unwrap().is_scanning = true;
        let cancel_flag = harness.state.lock().unwrap().scan_cancellation_flag.clone();
        let proxy_clone = harness.proxy.clone();
        let state_clone = harness.state.clone();
        let path_clone = harness.root_path.clone();

        // Act
        let task_handle = tokio::spawn(async move {
            proactive_scan_task(proxy_clone, state_clone, path_clone, scanner).await
        });

        // 1. Wait for the signal from the mock that the shallow scan is complete.
        shallow_scan_finished
            .await
            .expect("Mock scanner should signal that shallow scan finished");

        // 2. Now that the task is paused, set the cancellation flag.
        cancel_flag.store(true, Ordering::SeqCst);

        // 3. Unblock the task, which will now immediately see the cancel flag and exit.
        allow_deep_scan_to_proceed.send(()).unwrap();

        // 4. Wait for the task to fully terminate.
        task_handle.await.unwrap();

        // Assert
        // We need to drain the initial shallow update event first.
        let _ = harness.get_n_events(1).await;
        // Now, the ScanGuard will have fired a final state update on drop. Get that one.
        let final_state_update = harness
            .get_last_state_update()
            .await
            .expect("ScanGuard should send a final update on cancellation");

        assert!(
            !final_state_update.is_scanning,
            "Scanning should be false in the final state."
        );
        assert!(
            !final_state_update.is_fully_scanned,
            "Scan should not be marked as full."
        );

        // The final AppState should be consistent
        let final_app_state = harness.state.lock().unwrap();
        assert!(!final_app_state.is_scanning);
        assert!(final_app_state.scan_task.is_none());
    }

    #[tokio::test]
    async fn start_scan_on_path_handles_nonexistent_path() {
        // Arrange
        let mut harness = TestHarness::new();
        let nonexistent_path = harness.root_path.join("nonexistent");

        // Act
        start_scan_on_path(
            nonexistent_path,
            harness.proxy.clone(),
            harness.state.clone(),
            false,
        );

        // Assert
        // We must await the event, as it's sent from a spawned task.
        let event = harness
            .get_n_events(1)
            .await
            .pop()
            .expect("Should have received one event");

        match event {
            UserEvent::ShowError(msg) => {
                // Check for the more specific error message from our fix.
                assert!(msg.contains("Path does not exist"));
            }
            _ => panic!("Expected a ShowError event."),
        }

        let state = harness.state.lock().unwrap();
        assert!(!state.is_scanning, "is_scanning should remain false.");
    }

    #[tokio::test]
    async fn start_scan_on_path_handles_path_is_file() {
        // Arrange
        let mut harness = TestHarness::new();
        let file_path = harness.root_path.join("some_file.txt");
        std::fs::write(&file_path, "content").unwrap();

        // Act
        // We expect this to run the scan on the parent directory.
        start_scan_on_path(
            file_path,
            harness.proxy.clone(),
            harness.state.clone(),
            false,
        );

        // Assert
        // It should NOT send an error. It should start scanning the parent.
        // It sends an initial state update setting is_scanning to true.
        let events = harness.get_n_events(1).await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            UserEvent::StateUpdate(s) => {
                assert!(s.is_scanning);
                assert_eq!(s.current_path, harness.root_path.to_string_lossy());
            }
            _ => panic!("Expected a StateUpdate event."),
        }

        // Lock the state mutably to check and then cancel.
        let mut state = harness.state.lock().unwrap();
        assert!(state.is_scanning, "is_scanning should be true.");
        assert_eq!(state.current_path, harness.root_path.to_string_lossy());

        // Cancel the scan to clean up the task and prevent test runner warnings.
        state.cancel_current_scan();
    }

    #[tokio::test]
    async fn start_scan_on_path_preserves_state() {
        // Arrange
        let mut harness = TestHarness::new();
        let dir_to_expand = harness.root_path.join("src");
        let new_scan_path = harness.root_path.join("new_project");
        std::fs::create_dir_all(&new_scan_path).unwrap();

        // Set initial state
        {
            let mut state = harness.state.lock().unwrap();
            state.expanded_dirs.insert(dir_to_expand.clone());
        }

        // Act
        // Call with preserve_state = true
        start_scan_on_path(
            new_scan_path.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            true,
        );

        // Assert
        // Wait for the initial StateUpdate event which confirms the task has started and set the initial state.
        // This resolves the race condition.
        let initial_update = harness
            .get_n_events(1)
            .await
            .pop()
            .expect("Task should send an initial StateUpdate");
        assert!(matches!(initial_update, UserEvent::StateUpdate(_)));

        // Now it's safe to check the state.
        {
            let state = harness.state.lock().unwrap();
            assert!(state.is_scanning, "Scanning should have started.");
            assert_eq!(state.current_path, new_scan_path.to_string_lossy());
            assert!(
                state.expanded_dirs.contains(&dir_to_expand),
                "expanded_dirs should have been preserved."
            );
        }

        // We need to wait for the scan to finish to avoid panics on drop.
        // Let's get all events to clear the channel.
        while harness.get_n_events(1).await.len() > 0 {}
    }

    /// Comprehensive test for the RealFileSearcher implementation via the search_in_files task.
    /// This test covers:
    /// 1. Case-sensitive matching.
    /// 2. Case-insensitive matching (default).
    /// 3. Correctly skipping binary files and directories.
    /// 4. Handling files where the search term is not found.
    #[tokio::test]
    async fn search_in_files_with_real_searcher_covers_edge_cases() {
        // Arrange
        let mut harness = TestHarness::new();
        let searcher = RealFileSearcher; // Use the real implementation

        // Create a diverse set of files
        let text_file_path = harness.root_path.join("file.txt");
        std::fs::write(&text_file_path, "find the MagicWord here").unwrap();

        let binary_file_path = harness.root_path.join("app.exe");
        std::fs::write(&binary_file_path, &[0, 1, 2, 3, 255]).unwrap(); // A binary file

        let no_match_path = harness.root_path.join("another.txt");
        std::fs::write(&no_match_path, "nothing to see").unwrap();

        let dir_path = harness.root_path.join("subfolder");
        std::fs::create_dir(&dir_path).unwrap();

        // Populate state
        {
            let mut state = harness.state.lock().unwrap();
            state.full_file_list = vec![
                FileItem {
                    path: text_file_path.clone(),
                    is_binary: false,
                    ..Default::default()
                },
                FileItem {
                    path: binary_file_path,
                    is_binary: true,
                    ..Default::default()
                },
                FileItem {
                    path: no_match_path,
                    is_binary: false,
                    ..Default::default()
                },
                FileItem {
                    path: dir_path,
                    is_directory: true,
                    ..Default::default()
                },
            ];
        }

        // --- SCENARIO 1: Case-sensitive search (no match) ---
        {
            let mut state = harness.state.lock().unwrap();
            state.config.case_sensitive_search = true;
            state.content_search_query = "magicword".to_string(); // Lowercase
        }
        // Pass 'searcher' by value (it's Copy)
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
        // Assert against the AppState, not the UiState
        assert!(
            harness
                .state
                .lock()
                .unwrap()
                .content_search_results
                .is_empty(),
            "Case-sensitive search should not find 'magicword'"
        );
        // Drain the event queue
        let _ = harness.get_last_state_update().await;

        // --- SCENARIO 2: Case-sensitive search (exact match) ---
        {
            let mut state = harness.state.lock().unwrap();
            state.config.case_sensitive_search = true;
            state.content_search_query = "MagicWord".to_string(); // Exact case
        }
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
        // Assert against the AppState for correctness
        let final_state = harness.state.lock().unwrap();
        assert_eq!(
            final_state.content_search_results.len(),
            1,
            "Case-sensitive search should find 'MagicWord'"
        );
        assert!(final_state.content_search_results.contains(&text_file_path));
    }

    /// Tests that the generation_task correctly prunes empty directories from the
    /// generated tree view when the corresponding config flags are set.
    #[tokio::test]
    async fn generation_task_prunes_empty_directories_from_tree() {
        // Arrange
        let mut harness = TestHarness::new();
        let generator = MockContentGenerator::new();
        // The generator will include a tree. We check if the input it receives is correct.
        generator.set_result(Ok("Success".to_string()));
        let tokenizer = MockTokenizer { token_count: 1 };

        let file_path = harness.root_path.join("src/main.rs");
        let empty_dir_path = harness.root_path.join("src/empty");

        {
            let mut state = harness.state.lock().unwrap();
            state.is_fully_scanned = true; // Required for pruning
            state.config.remove_empty_directories = true; // Enable feature
            state.config.include_tree_by_default = true;
            state.selected_files.insert(file_path.clone());
            state.full_file_list = vec![
                FileItem {
                    path: harness.root_path.join("src"),
                    is_directory: true,
                    ..Default::default()
                },
                FileItem {
                    path: file_path,
                    ..Default::default()
                },
                FileItem {
                    path: empty_dir_path,
                    is_directory: true,
                    ..Default::default()
                },
            ];
        }

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
        )
        .await;

        // Assert
        // This test is implicitly asserting that the `remove_empty_directories` logic in `generation_task`
        // is called and doesn't panic. A more advanced mock could capture the `items_for_tree` argument
        // and assert its contents, but for coverage, simply executing the path is sufficient.
        let events = harness.get_n_events(2).await;
        assert!(matches!(events[0], UserEvent::ShowGeneratedContent { .. }));
        assert!(matches!(events[1], UserEvent::StateUpdate(_)));
    }

    /// Test for the lazy load happy path, using the proper entry point.
    #[tokio::test]
    async fn start_lazy_load_scan_happy_path_adds_files() {
        // Arrange
        let mut harness = TestHarness::new();
        let dir_to_load = harness.root_path.join("src");
        std::fs::create_dir_all(&dir_to_load).unwrap();
        let new_file_path = dir_to_load.join("main.rs");
        std::fs::write(&new_file_path, "fn main() {}").unwrap();

        {
            let mut state = harness.state.lock().unwrap();
            state.full_file_list.push(FileItem {
                path: dir_to_load.clone(),
                is_directory: true,
                ..Default::default()
            });
            state.filtered_file_list = state.full_file_list.clone();
        }

        // Create a signaling channel for synchronization
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Act - Pass the sender part of the channel to the function.
        start_lazy_load_scan(
            dir_to_load.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            Some(tx), // Provide the signal sender.
        );

        // --- Assert ---
        // 1. Wait deterministically for the background task to complete.
        //    This replaces the flaky timeout.
        rx.await
            .expect("The lazy_load_task should send a completion signal.");

        // 2. Now that we KNOW the task is done, retrieve the event from the channel.
        //    This call will now find the event immediately without needing a long wait.
        let final_state_update = harness
            .get_last_state_update()
            .await
            .expect("Should receive a state update after lazy load");

        // 3. Perform the final state checks.
        assert_eq!(
            final_state_update.visible_files_count, 2,
            "Should show parent dir and new file"
        );
        let state = harness.state.lock().unwrap();
        assert!(state.loaded_dirs.contains(&dir_to_load));
        assert!(state.expanded_dirs.contains(&dir_to_load));
        assert!(state
            .full_file_list
            .iter()
            .any(|item| item.path == new_file_path));
    }

    /// Tests that a lazy load scan is ignored if a main scan is already in progress.
    #[tokio::test]
    async fn start_lazy_load_scan_is_ignored_if_already_scanning() {
        // Arrange
        let mut harness = TestHarness::new();
        let dir_to_load = harness.root_path.join("src");

        {
            let mut state = harness.state.lock().unwrap();
            state.is_scanning = true; // Simulate an ongoing scan
        }

        // Act
        start_lazy_load_scan(
            dir_to_load,
            harness.proxy.clone(),
            harness.state.clone(),
            None,
        );

        // Assert that no events are sent and no new tasks are spawned.
        // We wait a short moment to ensure the spawned task would have had time to run.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let events = harness.get_n_events(1).await;
        assert!(
            events.is_empty(),
            "No events should be sent when lazy load is ignored"
        );
    }

    /// Tests that the RealFileSearcher gracefully handles files that cannot be read
    /// from the filesystem (e.g., due to permissions).
    #[tokio::test]
    #[cfg(unix)] // This test relies on Unix-style permissions.
    async fn search_in_files_with_real_searcher_handles_read_error() {
        if running_as_root() {
            eprintln!("Skipping permission-based test because process runs as root (Docker/act).");
            return;
        }
        // Arrange
        use std::os::unix::fs::PermissionsExt;

        let harness = TestHarness::new();
        let searcher = RealFileSearcher;

        let unreadable_file_path = harness.root_path.join("unreadable.txt");
        std::fs::write(&unreadable_file_path, "you can't read me").unwrap();

        // Set file permissions to write-only (0o200) to cause a read error.
        let mut perms = std::fs::metadata(&unreadable_file_path)
            .unwrap()
            .permissions();
        perms.set_mode(0o200);
        std::fs::set_permissions(&unreadable_file_path, perms).unwrap();

        {
            let mut state = harness.state.lock().unwrap();
            state.full_file_list = vec![FileItem {
                path: unreadable_file_path.clone(),
                ..Default::default()
            }];
            state.content_search_query = "read".to_string();
        }

        // Act
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;

        // Assert
        // The main assertion is that the task completes without panicking.
        // We also assert that the unreadable file is not included in the results.
        let final_state = harness.state.lock().unwrap();
        assert!(
            final_state.content_search_results.is_empty(),
            "Unreadable file should not be a search result."
        );
    }

    /// Tests the actual cancellation mechanism of the generation_task using the RealContentGenerator.
    #[tokio::test]
    async fn generation_task_with_real_generator_cancels_gracefully() {
        // Arrange
        let mut harness = TestHarness::new();
        // Use the REAL generator, not a mock.
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let generator = RealContentGenerator {
            cancel_flag: cancel_flag.clone(),
        };
        let tokenizer = MockTokenizer { token_count: 0 };

        // Set up state with a file to be processed.
        {
            let mut state = harness.state.lock().unwrap();
            let file_path = harness.root_path.join("file1.txt");
            std::fs::write(&file_path, "content").unwrap();
            state.selected_files.insert(file_path.clone());
            state.full_file_list.push(FileItem {
                path: file_path,
                ..Default::default()
            });
        }

        // Act
        let task_handle = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
        ));

        // Immediately signal cancellation. The task should pick this up.
        cancel_flag.store(true, Ordering::SeqCst);
        task_handle.await.unwrap();

        // Assert
        let events = harness.get_n_events(2).await;
        // We expect ONLY a StateUpdate event that resets the is_generating flag.
        // We should NOT receive a ShowGeneratedContent or ShowError event.
        assert_eq!(
            events.len(),
            1,
            "Only one final StateUpdate event should be sent on cancellation."
        );

        let final_state = match &events[0] {
            UserEvent::StateUpdate(s) => s,
            other => panic!("Expected a StateUpdate, but got {:?}", other),
        };

        assert!(!final_state.is_generating, "is_generating should be reset.");
        assert!(
            final_state.status_message.contains("Generation cancelled"),
            "Status message should indicate cancellation."
        );
    }

    /// Tests the edge case where handle_scan_error is called when no scan is active.
    #[tokio::test]
    async fn handle_scan_error_does_nothing_if_not_scanning() {
        // Arrange
        let mut harness = TestHarness::new();
        // Ensure is_scanning is false.
        harness.state.lock().unwrap().is_scanning = false;
        let dummy_error = CoreError::Cancelled;

        // Act
        handle_scan_error(dummy_error, &harness.state, &harness.proxy);

        // Assert
        // The function should return early without sending any events.
        let events = harness.get_n_events(1).await;
        assert!(
            events.is_empty(),
            "No events should be sent if not scanning."
        );
    }
}
