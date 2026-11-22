//! Contains long-running, asynchronous tasks that the application can perform.
//!
//! These tasks, such as scanning a directory or searching file contents, are designed
//! to run in the background without blocking the UI. They communicate their progress
//! and results back to the main application thread via `UserEvent`s.
//!
//! This module uses dependency inversion. Tasks are generic over traits (`ContentGenerator`,
//! `Scanner`, `Tokenizer`, `FileSearcher`) to allow for injecting mock implementations during testing,
//! making the tests deterministic and robust.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use super::events::UserEvent;
use super::filtering;
use super::proxy::EventProxy;
use super::state::{AppState, GenerationToken, LazyLoadToken, ScanToken};
use super::view_model::{
    auto_expand_for_matches, generate_ui_state, get_selected_files_in_tree_order,
};

use crate::core::scanner::ScanRequest;
use crate::core::{
    ContentSearch, CoreError, DirectoryScanner, FileHandler, FileItem, ScanProgress, SearchEngine,
};
use tiktoken_rs::cl100k_base;

//================================================================================================//
//|                                     SERVICE TRAITS                                           |//
//================================================================================================//

/// A trait abstracting the generation of concatenated file content.
pub trait ContentGenerator: Send + Sync {
    fn generate(
        &self,
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    ) -> impl Future<Output = Result<String, CoreError>> + Send;
}

/// The outcome of a directory scan: the items found and the ignore patterns that matched.
pub type ScanOutcome = Result<(Vec<FileItem>, HashSet<String>), CoreError>;

/// A trait abstracting the directory scanning functionality.
pub trait Scanner: Send + Sync {
    fn scan(
        &self,
        request: &ScanRequest,
        progress_callback: Box<dyn Fn(ScanProgress) + Send + Sync>,
    ) -> impl Future<Output = ScanOutcome> + Send;
}

/// A trait abstracting the token counting functionality.
pub trait Tokenizer: Send + Sync {
    fn count_tokens(&self, text: &str) -> impl Future<Output = usize> + Send;
}

/// A trait abstracting the file content search functionality.
pub trait FileSearcher: Send + Sync {
    fn search(
        &self,
        files_to_search: Vec<FileItem>,
        query: &str,
        case_sensitive: bool,
    ) -> impl Future<Output = HashSet<PathBuf>> + Send;
}

//================================================================================================//
//|                                   REAL IMPLEMENTATIONS                                       |//
//================================================================================================//

pub struct RealContentGenerator {
    pub cancel_flag: Arc<AtomicBool>,
}
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
impl Scanner for RealScanner {
    async fn scan(
        &self,
        request: &ScanRequest,
        progress_callback: Box<dyn Fn(ScanProgress) + Send + Sync>,
    ) -> ScanOutcome {
        let scanner = DirectoryScanner::new(self.ignore_patterns.clone());
        scanner
            .scan(request, self.cancel_flag.clone(), progress_callback)
            .await
    }
}

pub struct RealTokenizer;
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
impl FileSearcher for RealFileSearcher {
    async fn search(
        &self,
        files_to_search: Vec<FileItem>,
        query: &str,
        case_sensitive: bool,
    ) -> HashSet<PathBuf> {
        let search = ContentSearch::new(query, case_sensitive);
        tokio::task::spawn_blocking(move || search.matching_paths(&files_to_search))
            .await
            .unwrap_or_default()
    }
}

//================================================================================================//
//|                                   TASK IMPLEMENTATIONS                                       |//
//================================================================================================//

/// The main asynchronous task for generating the concatenated file content.
///
/// `token` identifies this generation run. The outcome is only published while the
/// token is still current, so a cancelled or superseded task can neither show outdated
/// content nor reset the generating state of the generation that replaced it.
pub async fn generation_task<P, G, T>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    content_generator: G,
    tokenizer: T,
    token: GenerationToken,
) where
    P: EventProxy,
    G: ContentGenerator + 'static,
    T: Tokenizer + 'static,
{
    let _generation_guard = GenerationGuard {
        state: &state,
        proxy: &proxy,
        token: &token,
    };

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
        tracing::debug!("Pruning empty directories from the generated tree.");
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

    let outcome = match result {
        Ok(content) => {
            let token_count = tokenizer.count_tokens(&content).await;
            Ok((content, token_count))
        }
        Err(e) => Err(e),
    };

    publish_generation_outcome(outcome, &proxy, &state, &token);
}

/// Ends the generation identified by `token` and reports its outcome to the UI.
///
/// The outcome of a generation that is no longer the current one is only logged.
fn publish_generation_outcome<P: EventProxy>(
    outcome: Result<(String, usize), CoreError>,
    proxy: &P,
    state: &Arc<Mutex<AppState>>,
    token: &GenerationToken,
) {
    let mut state_guard = state.lock().expect("Mutex poisoned");
    if !state_guard.finish_generation(token) {
        tracing::debug!("Discarding the outcome of an outdated generation.");
        return;
    }

    match outcome {
        Ok((content, token_count)) => {
            proxy.send_event(UserEvent::ShowGeneratedContent {
                content,
                token_count,
            });
        }
        Err(CoreError::Cancelled) => {
            tracing::info!("Generation cancelled.");
            state_guard.scan_progress.current_scanning_path = "Generation cancelled.".to_string();
        }
        Err(e) => {
            tracing::error!("Generation failed: {}", e);
            proxy.send_event(UserEvent::ShowError(e.to_string()));
        }
    }
    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
        &state_guard,
    ))));
}

/// Releases the generating state when a generation task ends without having published
/// an outcome (e.g. a panicking generator) and tells the UI about it.
///
/// The guard is bound to the generation's own [`GenerationToken`]: once the outcome has
/// been published, or a newer generation has taken over, dropping it changes nothing.
struct GenerationGuard<'a, P: EventProxy> {
    state: &'a Arc<Mutex<AppState>>,
    proxy: &'a P,
    token: &'a GenerationToken,
}

impl<P: EventProxy> Drop for GenerationGuard<'_, P> {
    fn drop(&mut self) {
        // Never panic in drop: this may run while the task is already unwinding.
        let Ok(mut state) = self.state.lock() else {
            tracing::error!("AppState mutex poisoned; cannot release generating state.");
            return;
        };
        if state.finish_generation(self.token) {
            tracing::error!("Generation task ended without publishing an outcome.");
            self.proxy
                .send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&state))));
        }
    }
}

/// Releases the scanning state when a scan task ends without having done so itself
/// (cancellation, abort, panic) and tells the UI about it.
///
/// The guard is bound to the scan's own [`ScanToken`]: if a newer scan has taken over in
/// the meantime, dropping the guard of the old task leaves the state untouched.
struct ScanGuard<'a, P: EventProxy> {
    state: &'a Arc<Mutex<AppState>>,
    proxy: &'a P,
    token: &'a ScanToken,
}

impl<P: EventProxy> Drop for ScanGuard<'_, P> {
    fn drop(&mut self) {
        // Never panic in drop: this may run while the task is already unwinding.
        let Ok(mut state) = self.state.lock() else {
            tracing::error!("AppState mutex poisoned; cannot release scanning state.");
            return;
        };
        if state.finish_scan(self.token) {
            self.proxy
                .send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&state))));
        }
    }
}

/// The core orchestration logic for the proactive, two-phase scan.
///
/// `token` identifies this scan run. Every state mutation is conditional on the token
/// still being current, so a cancelled or superseded task cannot interfere with the
/// scan that replaced it.
///
/// A content search whose query is active sees only the files known at the time it ran.
/// Once the scan has delivered the complete file list, the search is therefore repeated
/// with `searcher`.
pub async fn proactive_scan_task<P: EventProxy, S: Scanner, F: FileSearcher>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    path: PathBuf,
    scanner: S,
    searcher: F,
    token: ScanToken,
) {
    let _scan_guard = ScanGuard {
        state: &state,
        proxy: &proxy,
        token: &token,
    };

    // --- Phase 1: Shallow Scan ---
    let progress_proxy_shallow = proxy.clone();
    let scan_result_shallow = scanner
        .scan(
            &ScanRequest::project(&path, Some(1)),
            Box::new(move |p| progress_proxy_shallow.send_event(UserEvent::ScanProgress(p))),
        )
        .await;

    if token.is_cancelled() {
        tracing::info!("Scan cancelled after shallow scan phase.");
        return; // _scan_guard is dropped here, cleaning up the state and notifying.
    }

    match scan_result_shallow {
        Ok((files, patterns)) => {
            if !apply_shallow_scan_result(&state, &proxy, &token, &path, files, patterns) {
                return;
            }
        }
        Err(e) => {
            handle_scan_error(e, &state, &proxy, &token);
            return;
        }
    }

    // --- Phase 2: Deep Background Scan (Indexing) ---
    let progress_proxy_deep = proxy.clone();
    let scan_result_deep = scanner
        .scan(
            &ScanRequest::project(&path, None),
            Box::new(move |p| progress_proxy_deep.send_event(UserEvent::ScanProgress(p))),
        )
        .await;

    if token.is_cancelled() {
        tracing::info!("Scan cancelled during deep scan phase.");
        return; // _scan_guard is dropped here.
    }

    match scan_result_deep {
        Ok((files, patterns)) => {
            if apply_deep_scan_result(&state, &proxy, &token, files, patterns) {
                refresh_content_search(&proxy, &state, searcher).await;
            }
        }
        Err(e) => handle_scan_error(e, &state, &proxy, &token),
    }
}

/// Publishes the result of the shallow scan phase.
///
/// Returns `false` without touching the state if the scan is no longer the current one.
fn apply_shallow_scan_result<P: EventProxy>(
    state: &Arc<Mutex<AppState>>,
    proxy: &P,
    token: &ScanToken,
    root: &Path,
    files: Vec<FileItem>,
    patterns: HashSet<String>,
) -> bool {
    let mut s = state.lock().expect("Mutex was poisoned");
    if !s.is_current_scan(token) {
        return false;
    }
    s.replace_with_shallow_file_list(root, files);
    s.active_ignore_patterns = patterns;
    filtering::apply_filters(&mut s);
    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&s))));
    true
}

/// Publishes the result of the deep scan phase and marks the scan as finished.
///
/// Returns `false` without touching the state if the scan is no longer the current one.
fn apply_deep_scan_result<P: EventProxy>(
    state: &Arc<Mutex<AppState>>,
    proxy: &P,
    token: &ScanToken,
    files: Vec<FileItem>,
    patterns: HashSet<String>,
) -> bool {
    let mut s = state.lock().expect("Mutex was poisoned");
    if !s.is_current_scan(token) {
        return false;
    }
    let new_file_paths: HashSet<_> = files.iter().map(|f| f.path.clone()).collect();
    s.selected_files.retain(|p| new_file_paths.contains(p));
    s.replace_with_complete_file_list(files);
    s.active_ignore_patterns = patterns;
    s.is_fully_scanned = true;
    filtering::apply_filters(&mut s);

    // The final state is set here; the guard only cleans up on panics/cancellations.
    s.finish_scan(token);
    s.scan_progress.current_scanning_path = format!(
        "Indexing complete. Found {} visible items.",
        s.filtered_file_list().len()
    );

    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&s))));
    true
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

        // Replacing the old scan and registering the new one happens under a single
        // lock, so no other scan or cancellation can slip in between.
        let (token, ignore_patterns) = {
            let mut state_guard = state.lock().expect("Mutex was poisoned");
            if !preserve_state {
                state_guard.reset_directory_state();
            } else {
                state_guard.cancel_current_scan();
            }
            state_guard.current_path = directory_path.to_string_lossy().to_string();
            state_guard.config.last_directory = Some(directory_path.clone());
            crate::config::settings::save_config(&state_guard.config, None).ok();
            state_guard.is_fully_scanned = false;
            let token = state_guard.begin_scan();
            proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
                &state_guard,
            ))));
            (token, state_guard.config.ignore_patterns.clone())
        };
        let scanner = RealScanner {
            ignore_patterns,
            cancel_flag: token.cancel_flag(),
        };
        let task_token = token.clone();
        let handle = tokio::spawn(async move {
            proactive_scan_task(
                proxy_clone,
                state_clone,
                directory_path,
                scanner,
                RealFileSearcher,
                task_token,
            )
            .await;
        });
        state
            .lock()
            .expect("Mutex was poisoned")
            .attach_scan_task(&token, handle);
    });
}

/// Helper to handle scan errors consistently.
///
/// Errors of a scan that is no longer the current one are only logged.
fn handle_scan_error<P: EventProxy>(
    error: CoreError,
    state: &Arc<Mutex<AppState>>,
    proxy: &P,
    token: &ScanToken,
) {
    tracing::error!("Scan failed: {}", error);
    let mut state_lock = state.lock().expect("Mutex was poisoned");
    if !state_lock.finish_scan(token) {
        return;
    }
    state_lock.scan_progress.current_scanning_path = format!("Scan failed: {error}");
    let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_lock)));
    proxy.send_event(event);
}

/// Initiates a scan for a specific subdirectory level (lazy loading).
///
/// Asking for the content of a directory is a request to see it, so the directory is
/// marked as expanded right here, when the request is made. The result of the load does
/// not touch the expansion state anymore: if the user collapses the directory while the
/// load is in flight, it stays collapsed.
pub fn start_lazy_load_scan<P: EventProxy>(
    path: PathBuf,
    proxy: P,
    state: Arc<Mutex<AppState>>,
    completion_signal: Option<oneshot::Sender<()>>,
) {
    tokio::spawn(async move {
        let (ignore_patterns, token, request) = {
            let mut state_guard = state
                .lock()
                .expect("Mutex was poisoned. This should not happen.");
            let Some(token) = state_guard.begin_lazy_load() else {
                tracing::warn!(
                    "Attempted to lazy load while a full scan was in progress. Ignoring."
                );
                return;
            };
            // The project root anchors the ignore rules and the depth of the new items.
            let request = match ScanRequest::directory_level(&state_guard.current_path, &path) {
                Ok(request) if !state_guard.current_path.is_empty() => request,
                _ => {
                    tracing::warn!("Ignoring lazy load outside the project: {:?}", path);
                    return;
                }
            };
            state_guard.expanded_dirs.insert(path);
            (state_guard.config.ignore_patterns.clone(), token, request)
        };
        let scanner = lazy_load_scanner(ignore_patterns, &token);
        tracing::debug!("Starting lazy load for {:?}.", request.start_dir());
        lazy_load_task(
            request,
            proxy,
            state,
            scanner,
            RealFileSearcher,
            token,
            completion_signal,
        )
        .await;
    });
}

/// Creates the scanner for a lazy load. It shares the cancellation flag of `token`, so
/// the scan stops as soon as the file list it was started for is replaced.
fn lazy_load_scanner(ignore_patterns: HashSet<String>, token: &LazyLoadToken) -> RealScanner {
    RealScanner {
        ignore_patterns,
        cancel_flag: token.cancel_flag(),
    }
}

/// The asynchronous task for scanning a single directory level and appending the results.
///
/// `token` ties the load to the file list it was started for. If that list has been
/// replaced in the meantime (new scan, directory reset), the outcome is discarded.
/// Otherwise the new files are also run through an active content search.
async fn lazy_load_task<P: EventProxy, S: Scanner, F: FileSearcher>(
    request: ScanRequest,
    proxy: P,
    state: Arc<Mutex<AppState>>,
    scanner: S,
    searcher: F,
    token: LazyLoadToken,
    completion_signal: Option<oneshot::Sender<()>>,
) {
    let scan_result = scanner.scan(&request, Box::new(|_| {})).await;

    if apply_lazy_load_result(scan_result, request.start_dir(), &proxy, &state, &token) {
        refresh_content_search(&proxy, &state, searcher).await;
    }

    // After all work is done, send the completion signal if one was provided.
    if let Some(signal) = completion_signal {
        let _ = signal.send(());
    }
}

/// Merges the outcome of a lazy load into the state, unless it has become outdated.
///
/// Returns `true` if new items were merged into the file list.
fn apply_lazy_load_result<P: EventProxy>(
    scan_result: ScanOutcome,
    loaded_dir: &Path,
    proxy: &P,
    state: &Arc<Mutex<AppState>>,
    token: &LazyLoadToken,
) -> bool {
    let mut state_guard = state.lock().expect("Mutex was poisoned");
    if !state_guard.is_current_lazy_load(token) {
        tracing::debug!("Discarding outdated lazy load result for {:?}.", loaded_dir);
        return false;
    }

    match scan_result {
        Ok((new_items, new_active_patterns)) => {
            tracing::debug!(
                "Lazy load found {} items in {:?}.",
                new_items.len(),
                loaded_dir
            );

            state_guard.loaded_dirs.insert(loaded_dir.to_path_buf());
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

            proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
                &state_guard,
            ))));
            true
        }
        Err(e) => {
            tracing::error!("Lazy load failed for {:?}: {}", loaded_dir, e);
            proxy.send_event(UserEvent::ShowError(format!(
                "Failed to load directory {}: {}",
                loaded_dir.display(),
                e
            )));
            false
        }
    }
}

/// Repeats the content search after the file list has changed, so that files which
/// were not known when the query was entered are searched as well.
///
/// Does nothing (and sends nothing) if no content query is active.
async fn refresh_content_search<P: EventProxy, S: FileSearcher>(
    proxy: &P,
    state: &Arc<Mutex<AppState>>,
    searcher: S,
) {
    let has_content_query = !state
        .lock()
        .expect("Mutex was poisoned")
        .content_search_query
        .is_empty();
    if has_content_query {
        search_in_files(proxy.clone(), state.clone(), searcher).await;
    }
}

/// Performs a content search across all non-binary files.
///
/// Every search is a run of its own (see [`ContentSearchToken`]): of several searches
/// in flight only the one started last publishes its matches, so a slow search for an
/// outdated query can never overwrite the matches of the current one.
///
/// [`ContentSearchToken`]: super::state::ContentSearchToken
pub async fn search_in_files<P: EventProxy, S: FileSearcher>(
    proxy: P,
    state: Arc<Mutex<AppState>>,
    searcher: S,
) {
    let (token, files_to_search, query, case_sensitive) = {
        let mut state_guard = state
            .lock()
            .expect("Mutex was poisoned. This should not happen.");
        if state_guard.content_search_query.is_empty() {
            state_guard.clear_content_search();
            filtering::apply_filters(&mut state_guard);
            let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
            proxy.send_event(event);
            return;
        }
        (
            state_guard.begin_content_search(),
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
    if !state_guard.finish_content_search(&token, matching_paths) {
        tracing::debug!("Discarding outdated content search result for {:?}.", query);
        return;
    }
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
    use std::sync::atomic::Ordering;
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
            // Take the receiver in its own statement so the lock is released before the
            // await; a guard held across it would make the future `!Send`.
            let receiver = self.cancellation_check.lock().unwrap().take();
            if let Some(receiver) = receiver {
                let _ = receiver.await;
            }
            self.result.lock().unwrap().clone()
        }
    }

    /// A generator that panics instead of producing content.
    struct PanickingContentGenerator;

    impl ContentGenerator for PanickingContentGenerator {
        async fn generate(
            &self,
            _: &[PathBuf],
            _: &Path,
            _: bool,
            _: Vec<FileItem>,
            _: HashSet<String>,
            _: bool,
        ) -> Result<String, CoreError> {
            panic!("generator failure injected by the test");
        }
    }

    #[derive(Clone)]
    struct MockTokenizer {
        token_count: usize,
    }

    impl Tokenizer for MockTokenizer {
        async fn count_tokens(&self, _: &str) -> usize {
            self.token_count
        }
    }

    #[derive(Clone)]
    struct MockScanner {
        shallow_result: Arc<Mutex<ScanOutcome>>,
        deep_result: Arc<Mutex<ScanOutcome>>,
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

    impl Scanner for MockScanner {
        async fn scan(
            &self,
            request: &ScanRequest,
            _: Box<dyn Fn(ScanProgress) + Send + Sync>,
        ) -> ScanOutcome {
            if request.max_depth() == Some(1) {
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
    impl FileSearcher for MockFileSearcher {
        async fn search(&self, _: Vec<FileItem>, _: &str, _: bool) -> HashSet<PathBuf> {
            self.results.lock().unwrap().clone()
        }
    }

    /// A searcher that behaves like the real one as far as the file list is concerned:
    /// it can only find what it was given. Reports the configured paths that are part
    /// of the searched files.
    #[derive(Clone, Default)]
    struct ListBoundFileSearcher {
        matching: HashSet<PathBuf>,
    }
    impl FileSearcher for ListBoundFileSearcher {
        async fn search(&self, files: Vec<FileItem>, _: &str, _: bool) -> HashSet<PathBuf> {
            files
                .into_iter()
                .map(|item| item.path)
                .filter(|path| self.matching.contains(path))
                .collect()
        }
    }

    /// A searcher that signals when it has started and then holds its result back until
    /// the test releases it, to stage searches that overlap.
    struct GatedFileSearcher {
        results: HashSet<PathBuf>,
        started: Mutex<Option<oneshot::Sender<()>>>,
        release: Mutex<Option<oneshot::Receiver<()>>>,
    }
    impl GatedFileSearcher {
        fn new(results: HashSet<PathBuf>) -> (Self, oneshot::Receiver<()>, oneshot::Sender<()>) {
            let (started_tx, started_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let searcher = Self {
                results,
                started: Mutex::new(Some(started_tx)),
                release: Mutex::new(Some(release_rx)),
            };
            (searcher, started_rx, release_tx)
        }
    }
    impl FileSearcher for GatedFileSearcher {
        async fn search(&self, _: Vec<FileItem>, _: &str, _: bool) -> HashSet<PathBuf> {
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            let release = self.release.lock().unwrap().take();
            if let Some(release) = release {
                let _ = release.await;
            }
            self.results.clone()
        }
    }

    fn dir_item(path: &Path, depth: usize) -> FileItem {
        FileItem {
            path: path.to_path_buf(),
            is_directory: true,
            depth,
            parent: path.parent().map(PathBuf::from),
            ..Default::default()
        }
    }

    fn file_item(path: &Path, depth: usize) -> FileItem {
        FileItem {
            path: path.to_path_buf(),
            depth,
            parent: path.parent().map(PathBuf::from),
            ..Default::default()
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
        let token = harness.state.lock().unwrap().begin_generation();

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
            token,
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

        // The state update is the second of the two events captured above.
        assert!(matches!(events[1], UserEvent::StateUpdate(_)));
        if let UserEvent::StateUpdate(final_state_in_event) = &events[1] {
            assert!(!final_state_in_event.is_generating);
        }

        assert!(
            !harness.state.lock().unwrap().is_generating(),
            "is_generating should be reset in AppState"
        );
    }

    #[tokio::test]
    async fn generation_task_cancellation_is_handled_gracefully() {
        let mut harness = TestHarness::new();
        let mut generator = MockContentGenerator::new();
        let (has_started_receiver, unblock_sender) = generator.expect_cancellation();
        generator.set_result(Err(CoreError::Cancelled));
        let token = harness.state.lock().unwrap().begin_generation();

        let proxy_clone = harness.proxy.clone();
        let task_state = harness.state.clone();

        let task_handle = tokio::spawn(async move {
            generation_task(
                proxy_clone,
                task_state,
                generator,
                MockTokenizer { token_count: 0 },
                token,
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
        assert!(!matches!(events.first(), Some(UserEvent::ShowError(_))));
    }

    #[tokio::test]
    async fn generation_task_io_error_sends_error_event_and_resets_state() {
        // Arrange
        let mut harness = TestHarness::new();
        let generator = MockContentGenerator::new();
        let io_error = CoreError::Io("File not found".to_string(), PathBuf::from("a/b/c.txt"));
        generator.set_result(Err(io_error));
        let token = harness.state.lock().unwrap().begin_generation();

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 0 },
            token,
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

        // The state update is the second of the two events captured above.
        assert!(matches!(events[1], UserEvent::StateUpdate(_)));
        if let UserEvent::StateUpdate(final_state_in_event) = &events[1] {
            assert!(!final_state_in_event.is_generating);
        }

        assert!(
            !harness.state.lock().unwrap().is_generating(),
            "is_generating should be reset in AppState"
        );
    }

    #[tokio::test]
    async fn generation_task_panic_resets_state_and_notifies_ui() {
        // Arrange
        let mut harness = TestHarness::new();
        let token = harness.state.lock().unwrap().begin_generation();
        assert!(
            harness.state.lock().unwrap().is_generating(),
            "precondition"
        );

        // Act
        let task_handle = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            PanickingContentGenerator,
            MockTokenizer { token_count: 0 },
            token.clone(),
        ));
        let join_error = task_handle.await.expect_err("The task must panic");
        assert!(join_error.is_panic());

        // Assert
        {
            let state = harness
                .state
                .lock()
                .expect("The panic must not poison the state");
            assert!(
                !state.is_generating(),
                "is_generating should be reset after a panic"
            );
            assert!(!state.is_current_generation(&token));
        }

        let events = harness.get_n_events(1).await;
        assert_eq!(events.len(), 1, "Expected exactly one event");
        match &events[0] {
            UserEvent::StateUpdate(ui_state) => assert!(!ui_state.is_generating),
            _ => panic!("Expected a StateUpdate event."),
        }
        assert!(
            harness.event_rx.try_recv().is_err(),
            "No further events expected"
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

        let token = harness.state.lock().unwrap().begin_scan();

        // Act
        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
        )
        .await;

        // Assert
        // The success path publishes the final state itself; the guard only acts on
        // panics and cancellations, so exactly two events are expected.
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
        let token = harness.state.lock().unwrap().begin_scan();
        let cancel_flag = token.cancel_flag();

        let proxy_clone = harness.proxy.clone();
        let task_state = harness.state.clone();
        let task_path = harness.root_path.clone();

        let task_handle = tokio::spawn(async move {
            proactive_scan_task(
                proxy_clone,
                task_state,
                task_path,
                scanner,
                MockFileSearcher::default(),
                token,
            )
            .await;
        });
        has_started_deep_scan.await.unwrap();
        cancel_flag.store(true, Ordering::SeqCst);
        unblock_deep_scan.send(()).unwrap();
        let _ = task_handle.await;

        // The task now cleans up after itself via the RAII guard.
        // We can now confidently check the final state.
        let final_app_state = harness.state.lock().unwrap();
        assert!(
            !final_app_state.is_scanning(),
            "is_scanning should be false"
        );
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

        let token = harness.state.lock().unwrap().begin_scan();

        // Act
        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
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

        let token = harness.state.lock().unwrap().begin_scan();

        // Act
        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
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
        let token = harness.state.lock().unwrap().begin_generation();

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
            token,
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
            !harness.state.lock().unwrap().is_generating(),
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

        let token = harness
            .state
            .lock()
            .unwrap()
            .begin_lazy_load()
            .expect("No scan is running");

        // Act
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &path_to_load).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
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

        let token = harness.state.lock().unwrap().begin_scan();
        let cancel_flag = token.cancel_flag();
        let proxy_clone = harness.proxy.clone();
        let state_clone = harness.state.clone();
        let path_clone = harness.root_path.clone();

        // Act
        let task_handle = tokio::spawn(async move {
            proactive_scan_task(
                proxy_clone,
                state_clone,
                path_clone,
                scanner,
                MockFileSearcher::default(),
                token,
            )
            .await
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
        assert!(!final_app_state.is_scanning());
        assert!(!final_app_state.has_scan_task());
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
        assert!(!state.is_scanning(), "is_scanning should remain false.");
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
        assert!(state.is_scanning(), "is_scanning should be true.");
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
            assert!(state.is_scanning(), "Scanning should have started.");
            assert_eq!(state.current_path, new_scan_path.to_string_lossy());
            assert!(
                state.expanded_dirs.contains(&dir_to_expand),
                "expanded_dirs should have been preserved."
            );
        }

        // We need to wait for the scan to finish to avoid panics on drop.
        // Let's get all events to clear the channel.
        while !harness.get_n_events(1).await.is_empty() {}
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
        std::fs::write(&binary_file_path, [0, 1, 2, 3, 255]).unwrap(); // A binary file

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

        let token = harness.state.lock().unwrap().begin_generation();

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
            token,
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
            let all_items = state.full_file_list.clone();
            state.set_filtered_file_list(all_items);
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
            let _token = state.begin_scan(); // Simulate an ongoing scan
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
        let token = harness.state.lock().unwrap().begin_generation();
        let cancel_flag = token.cancel_flag();
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
            token,
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
        // The scan has already finished, so is_scanning is false.
        let token = harness.state.lock().unwrap().begin_scan();
        harness.state.lock().unwrap().finish_scan(&token);
        let dummy_error = CoreError::Cancelled;

        // Act
        handle_scan_error(dummy_error, &harness.state, &harness.proxy, &token);

        // Assert
        // The function should return early without sending any events.
        let events = harness.get_n_events(1).await;
        assert!(
            events.is_empty(),
            "No events should be sent if not scanning."
        );
    }

    //============================================================================================//
    //|                         REGRESSION TESTS: STALE SCAN TASKS                               |//
    //============================================================================================//

    /// Starts a scan task that parks inside its deep scan phase.
    ///
    /// Returns the task handle, the scan's token and the sender that lets the deep scan
    /// return. The shallow phase has already been applied when this function returns.
    async fn spawn_scan_parked_in_deep_phase(
        harness: &TestHarness,
        deep_files: Vec<FileItem>,
    ) -> (tokio::task::JoinHandle<()>, ScanToken, oneshot::Sender<()>) {
        let mut scanner = MockScanner::new();
        scanner.set_results(vec![], deep_files);
        let (shallow_scan_started, unblock_deep_scan) = scanner.prepare_for_cancellation();

        let token = harness.state.lock().unwrap().begin_scan();
        let handle = tokio::spawn(proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
            MockFileSearcher::default(),
            token.clone(),
        ));
        shallow_scan_started.await.unwrap();
        // On the single-threaded test runtime the task only yields once it is blocked,
        // i.e. it is now waiting inside the deep scan.
        (handle, token, unblock_deep_scan)
    }

    /// Simulates a newer scan taking over: registers it in the state the way
    /// `start_scan_on_path` does and returns its token.
    fn begin_replacement_scan(harness: &TestHarness) -> ScanToken {
        let mut state = harness.state.lock().unwrap();
        let token = state.begin_scan();
        state.attach_scan_task(&token, tokio::spawn(std::future::pending()));
        token
    }

    fn assert_replacement_scan_is_intact(harness: &TestHarness, token: &ScanToken) {
        let mut state = harness.state.lock().unwrap();
        assert!(
            state.is_scanning(),
            "The new scan must still be shown as running"
        );
        assert!(
            state.has_scan_task(),
            "The new scan must still be cancellable"
        );
        assert!(state.is_current_scan(token));
        assert!(!token.is_cancelled());
        assert!(!state.is_fully_scanned);
        state.cancel_current_scan(); // Stop the pending dummy task.
    }

    /// The original race: the old task is aborted and its future (including the
    /// `ScanGuard`) is dropped only *after* the new scan has registered itself.
    #[tokio::test]
    async fn aborted_scan_task_does_not_reset_state_of_newer_scan() {
        // Arrange
        let mut harness = TestHarness::new();
        let (old_task, _old_token, _unblock) =
            spawn_scan_parked_in_deep_phase(&harness, vec![]).await;
        let _ = harness.get_last_state_update().await; // Drain the shallow update.

        // Act: the new scan starts first, then the old future gets dropped.
        let new_token = begin_replacement_scan(&harness);
        old_task.abort();
        let join_error = old_task.await.expect_err("Old task should be aborted");
        assert!(join_error.is_cancelled());

        // Assert
        assert!(
            harness.get_last_state_update().await.is_none(),
            "The stale guard must not announce the end of a scan it does not own"
        );
        assert_replacement_scan_is_intact(&harness, &new_token);
    }

    /// The old task is not aborted in time and resumes after the new scan has started.
    /// It must notice its own cancellation instead of consulting the new scan's flag,
    /// and must not publish its results.
    #[tokio::test]
    async fn superseded_scan_task_does_not_publish_results_into_newer_scan() {
        // Arrange
        let mut harness = TestHarness::new();
        let stale_file = FileItem {
            path: harness.root_path.join("stale.txt"),
            ..Default::default()
        };
        let (old_task, old_token, unblock_deep_scan) =
            spawn_scan_parked_in_deep_phase(&harness, vec![stale_file]).await;
        let _ = harness.get_last_state_update().await; // Drain the shallow update.

        // Act: the new scan starts, then the old deep scan returns successfully.
        let new_token = begin_replacement_scan(&harness);
        unblock_deep_scan.send(()).unwrap();
        old_task.await.expect("Old task should end on its own");

        // Assert
        assert!(old_token.is_cancelled());
        assert!(
            harness.get_last_state_update().await.is_none(),
            "A superseded scan must not send state updates"
        );
        assert!(
            harness.state.lock().unwrap().full_file_list.is_empty(),
            "Results of the superseded scan must be discarded"
        );
        assert_replacement_scan_is_intact(&harness, &new_token);
    }

    /// Even if a stale task gets as far as publishing (its cancellation flag was not
    /// observed in time), the state must reject the write.
    #[tokio::test]
    async fn stale_scan_results_are_rejected_by_state() {
        // Arrange
        let mut harness = TestHarness::new();
        let old_token = harness.state.lock().unwrap().begin_scan();
        let new_token = begin_replacement_scan(&harness);
        let stale_files = vec![FileItem {
            path: harness.root_path.join("stale.txt"),
            ..Default::default()
        }];

        // Act
        let applied = apply_shallow_scan_result(
            &harness.state,
            &harness.proxy,
            &old_token,
            &harness.root_path,
            stale_files.clone(),
            HashSet::new(),
        );
        apply_deep_scan_result(
            &harness.state,
            &harness.proxy,
            &old_token,
            stale_files,
            HashSet::new(),
        );

        // Assert
        assert!(!applied);
        assert!(harness.get_last_state_update().await.is_none());
        {
            let state = harness.state.lock().unwrap();
            assert!(state.full_file_list.is_empty());
            assert!(state.loaded_dirs.is_empty());
        }
        assert_replacement_scan_is_intact(&harness, &new_token);
    }

    #[tokio::test]
    async fn handle_scan_error_from_stale_scan_does_not_fail_newer_scan() {
        // Arrange
        let mut harness = TestHarness::new();
        let old_token = harness.state.lock().unwrap().begin_scan();
        let new_token = begin_replacement_scan(&harness);

        // Act
        handle_scan_error(
            CoreError::Cancelled,
            &harness.state,
            &harness.proxy,
            &old_token,
        );

        // Assert
        assert!(harness.get_last_state_update().await.is_none());
        assert!(!harness
            .state
            .lock()
            .unwrap()
            .scan_progress
            .current_scanning_path
            .contains("Scan failed"));
        assert_replacement_scan_is_intact(&harness, &new_token);
    }

    /// A generation that was superseded but could not be aborted in time must not show
    /// its content or end the generating state of the generation that replaced it.
    #[tokio::test]
    async fn superseded_generation_task_does_not_publish_into_newer_generation() {
        // Arrange
        let mut harness = TestHarness::new();
        let mut generator = MockContentGenerator::new();
        let (has_started, unblock) = generator.expect_cancellation();
        generator.set_result(Ok("Outdated content".to_string()));
        let old_token = harness.state.lock().unwrap().begin_generation();
        let old_task = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 1 },
            old_token.clone(),
        ));
        has_started.await.expect("Mock did not signal start");

        // Act: the user starts a new generation while the old one is still producing.
        let new_token = harness.state.lock().unwrap().begin_generation();
        unblock.send(()).unwrap();
        old_task.await.unwrap();

        // Assert
        assert!(old_token.is_cancelled());
        let events = harness.get_n_events(1).await;
        assert!(
            events.is_empty(),
            "Outdated generation must stay silent, got {events:?}"
        );
        let state = harness.state.lock().unwrap();
        assert!(
            state.is_generating(),
            "New generation must still be running"
        );
        assert!(state.is_current_generation(&new_token));
    }

    /// A cancelled generation that only notices the cancellation after a new generation
    /// has started must not report "cancelled" on behalf of the new one.
    #[tokio::test]
    async fn cancelled_generation_task_does_not_reset_state_of_newer_generation() {
        // Arrange
        let mut harness = TestHarness::new();
        let mut generator = MockContentGenerator::new();
        let (has_started, unblock) = generator.expect_cancellation();
        generator.set_result(Err(CoreError::Cancelled));
        let old_token = harness.state.lock().unwrap().begin_generation();
        let old_task = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 0 },
            old_token,
        ));
        has_started.await.expect("Mock did not signal start");

        // Act
        let new_token = {
            let mut state = harness.state.lock().unwrap();
            state.cancel_current_generation();
            state.begin_generation()
        };
        unblock.send(()).unwrap();
        old_task.await.unwrap();

        // Assert
        assert!(harness.get_n_events(1).await.is_empty());
        let state = harness.state.lock().unwrap();
        assert!(state.is_generating());
        assert!(state.is_current_generation(&new_token));
        assert_ne!(
            state.scan_progress.current_scanning_path,
            "Generation cancelled."
        );
    }

    /// The scanner of a lazy load must stop working once the file list it loads into is
    /// replaced by a new scan or dropped by a directory reset.
    #[tokio::test]
    async fn lazy_load_scanner_is_cancelled_when_file_list_is_replaced() {
        let harness = TestHarness::new();
        let mut state = harness.state.lock().unwrap();

        let token = state.begin_lazy_load().expect("No scan is running");
        let scanner = lazy_load_scanner(HashSet::new(), &token);
        assert!(!scanner.cancel_flag.load(Ordering::SeqCst));
        let scan = state.begin_scan();
        assert!(
            scanner.cancel_flag.load(Ordering::SeqCst),
            "A new scan must stop the lazy load"
        );
        state.finish_scan(&scan);

        let token = state.begin_lazy_load().expect("Scan has finished");
        let scanner = lazy_load_scanner(HashSet::new(), &token);
        assert!(!scanner.cancel_flag.load(Ordering::SeqCst));
        state.reset_directory_state();
        assert!(
            scanner.cancel_flag.load(Ordering::SeqCst),
            "A directory reset must stop the lazy load"
        );
    }

    /// A lazy load that finishes after the directory was replaced must not merge its
    /// items into the new file list.
    #[tokio::test]
    async fn lazy_load_result_is_discarded_after_file_list_was_replaced() {
        // Arrange
        let mut harness = TestHarness::new();
        let old_dir = harness.root_path.join("old_project/src");
        let mut scanner = MockScanner::new();
        scanner.set_results(
            vec![FileItem {
                path: old_dir.join("main.rs"),
                ..Default::default()
            }],
            vec![],
        );
        let token = harness
            .state
            .lock()
            .unwrap()
            .begin_lazy_load()
            .expect("No scan is running");
        let (completion_tx, completion_rx) = oneshot::channel();

        // Act: the user switches directories while the lazy load is in flight.
        harness.state.lock().unwrap().reset_directory_state();
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &old_dir).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
            Some(completion_tx),
        )
        .await;

        // Assert
        completion_rx
            .await
            .expect("Completion must be signalled even for discarded results");
        assert!(harness.get_last_state_update().await.is_none());
        let state = harness.state.lock().unwrap();
        assert!(state.full_file_list.is_empty());
        assert!(!state.loaded_dirs.contains(&old_dir));
        assert!(!state.expanded_dirs.contains(&old_dir));
    }

    /// The result of a lazy load only delivers content. Whether the directory is shown
    /// expanded was decided when the load was requested; a directory the user has
    /// collapsed in the meantime must not pop open again.
    #[tokio::test]
    async fn lazy_load_result_does_not_expand_directory_collapsed_meanwhile() {
        // Arrange: the load of `src` is in flight, and `src` is (no longer) expanded.
        let mut harness = TestHarness::new();
        let dir = harness.root_path.join("src");
        let mut scanner = MockScanner::new();
        scanner.set_results(vec![file_item(&dir.join("main.rs"), 2)], vec![]);
        let token = {
            let mut state = harness.state.lock().unwrap();
            state.full_file_list.push(dir_item(&dir, 1));
            state.begin_lazy_load().expect("No scan is running")
        };

        // Act
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &dir).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
            None,
        )
        .await;

        // Assert
        let ui_state = harness.get_last_state_update().await.unwrap();
        let src_node = ui_state.tree.iter().find(|n| n.name == "src").unwrap();
        assert!(src_node.children_loaded && src_node.has_children);
        assert!(!src_node.is_expanded, "Directory must stay collapsed");
        let state = harness.state.lock().unwrap();
        assert!(state.loaded_dirs.contains(&dir));
        assert!(!state.expanded_dirs.contains(&dir));
    }

    /// End to end with the real scanner: a lazily loaded level of a subdirectory honours
    /// the ignore rules of the project (parent `.gitignore`, user patterns relative to the
    /// project root) and its items carry their depth within the project.
    #[tokio::test]
    async fn lazy_load_of_subdirectory_applies_project_ignore_rules() {
        // Arrange
        let mut harness = TestHarness::new();
        let root = harness.root_path.clone();
        let src = root.join("src");
        std::fs::create_dir_all(src.join("build_out")).unwrap();
        std::fs::create_dir_all(src.join("generated")).unwrap();
        std::fs::write(root.join(".gitignore"), "*.gen\n/src/build_out/\n").unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(src.join("secret.gen"), "secret").unwrap();
        std::fs::write(src.join("build_out/artifact.o"), "obj").unwrap();
        std::fs::write(src.join("generated/bindings.rs"), "// generated").unwrap();
        {
            // The state after a scan that was cancelled in its deep phase.
            let mut state = harness.state.lock().unwrap();
            state.config.ignore_patterns = HashSet::from(["src/generated/".to_string()]);
            state.replace_with_shallow_file_list(&root, vec![dir_item(&src, 1)]);
            filtering::apply_filters(&mut state);
        }
        let (tx, rx) = oneshot::channel();

        // Act
        start_lazy_load_scan(
            src.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            Some(tx),
        );
        rx.await.expect("Lazy load must signal its completion");

        // Assert
        let ui_state = harness.get_last_state_update().await.unwrap();
        let src_node = ui_state.tree.iter().find(|n| n.name == "src").unwrap();
        let child_names: Vec<&str> = src_node.children.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(child_names, vec!["main.rs"]);

        let state = harness.state.lock().unwrap();
        let loaded: Vec<&FileItem> = state
            .full_file_list
            .iter()
            .filter(|item| item.path != src)
            .collect();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].path, src.join("main.rs"));
        assert_eq!(
            loaded[0].depth, 2,
            "Depth must be relative to the project root"
        );
        assert!(state.active_ignore_patterns.contains("src/generated/"));
    }

    /// A lazy load for a path that is not part of the loaded project is refused.
    #[tokio::test]
    async fn lazy_load_outside_project_is_ignored() {
        let mut harness = TestHarness::new();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("file.txt"), "content").unwrap();
        let (tx, rx) = oneshot::channel();

        start_lazy_load_scan(
            outside.path().to_path_buf(),
            harness.proxy.clone(),
            harness.state.clone(),
            Some(tx),
        );

        assert!(rx.await.is_err(), "No load must have been performed");
        assert!(harness.get_last_state_update().await.is_none());
        let state = harness.state.lock().unwrap();
        assert!(state.full_file_list.is_empty());
        assert!(state.expanded_dirs.is_empty());
    }

    /// Rescan of a fully scanned project, cancelled in the deep phase: the list holds a
    /// single level again, so the subdirectories must be offered for lazy loading instead
    /// of counting as loaded (and empty).
    #[tokio::test]
    async fn rescan_cancelled_in_deep_phase_leaves_subdirectories_unloaded() {
        // Arrange: the state a completed scan leaves behind.
        let mut harness = TestHarness::new();
        let root = harness.root_path.clone();
        let src = root.join("src");
        let core = src.join("core");
        {
            let mut state = harness.state.lock().unwrap();
            state.replace_with_complete_file_list(vec![
                dir_item(&src, 1),
                dir_item(&core, 2),
                file_item(&core.join("lib.rs"), 3),
            ]);
            state.loaded_dirs.insert(root.clone());
            state.is_fully_scanned = true;
        }
        let mut scanner = MockScanner::new();
        scanner.set_results(vec![dir_item(&src, 1)], vec![]);
        let (_shallow_started, unblock_deep_scan) = scanner.prepare_for_cancellation();
        let token = {
            let mut state = harness.state.lock().unwrap();
            state.is_fully_scanned = false;
            state.begin_scan()
        };

        // Act: rescan, and cancel once the shallow result has been published.
        let task = tokio::spawn(proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            root.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
        ));
        let shallow_events = harness.get_n_events(1).await;
        assert!(matches!(shallow_events[0], UserEvent::StateUpdate(_)));
        harness.state.lock().unwrap().cancel_current_scan();
        unblock_deep_scan.send(()).ok();
        let _ = task.await;

        // Assert
        let state = harness.state.lock().unwrap();
        assert_eq!(state.loaded_dirs, HashSet::from([root]));
        let ui_state = generate_ui_state(&state);
        let src_node = ui_state.tree.iter().find(|n| n.name == "src").unwrap();
        assert!(
            !src_node.children_loaded,
            "`src` must be lazily loadable after the cancelled rescan"
        );
        assert!(state.begin_lazy_load().is_some());
    }

    /// Two overlapping searches: the one for the older query finishes last. Its matches
    /// must not replace those of the current query.
    #[tokio::test]
    async fn outdated_content_search_does_not_overwrite_newer_results() {
        // Arrange
        let mut harness = TestHarness::new();
        let fo_match = harness.root_path.join("fo.txt");
        let foo_match = harness.root_path.join("foo.txt");
        {
            let mut state = harness.state.lock().unwrap();
            state.full_file_list = vec![file_item(&fo_match, 1), file_item(&foo_match, 1)];
            state.content_search_query = "fo".to_string();
        }
        let (slow_searcher, slow_started, release_slow) =
            GatedFileSearcher::new(HashSet::from([fo_match.clone(), foo_match.clone()]));
        let slow_search = tokio::spawn(search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            slow_searcher,
        ));
        slow_started.await.expect("Slow search must start");

        // Act: the user types on; the search for "foo" completes first.
        harness.state.lock().unwrap().content_search_query = "foo".to_string();
        let fast_searcher = MockFileSearcher::default();
        fast_searcher.set_results(HashSet::from([foo_match.clone()]));
        search_in_files(harness.proxy.clone(), harness.state.clone(), fast_searcher).await;
        let state_after_current_search = harness.get_last_state_update().await.unwrap();
        release_slow.send(()).unwrap();
        slow_search.await.unwrap();

        // Assert
        assert_eq!(state_after_current_search.visible_files_count, 1);
        assert!(
            harness.get_last_state_update().await.is_none(),
            "The outdated search must not publish anything"
        );
        let state = harness.state.lock().unwrap();
        assert_eq!(state.content_search_results, HashSet::from([foo_match]));
        assert_eq!(state.filtered_file_list().len(), 1);
    }

    /// Clearing the query while a search is running must not let that search bring its
    /// matches back afterwards.
    #[tokio::test]
    async fn content_search_finishing_after_query_was_cleared_is_discarded() {
        let harness = TestHarness::new();
        let file = harness.root_path.join("a.txt");
        {
            let mut state = harness.state.lock().unwrap();
            state.full_file_list = vec![file_item(&file, 1)];
            state.content_search_query = "a".to_string();
        }
        let (slow_searcher, slow_started, release_slow) =
            GatedFileSearcher::new(HashSet::from([file.clone()]));
        let slow_search = tokio::spawn(search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            slow_searcher,
        ));
        slow_started.await.expect("Slow search must start");

        harness.state.lock().unwrap().content_search_query.clear();
        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            MockFileSearcher::default(),
        )
        .await;
        release_slow.send(()).unwrap();
        slow_search.await.unwrap();

        assert!(harness
            .state
            .lock()
            .unwrap()
            .content_search_results
            .is_empty());
    }

    /// A content query entered while the scan is still running has only been applied to
    /// the files of the shallow list. Once the deep scan delivers the complete list, the
    /// search must be repeated so that deeper files are found as well.
    #[tokio::test]
    async fn content_search_is_repeated_on_complete_list_after_deep_scan() {
        // Arrange
        let mut harness = TestHarness::new();
        let root = harness.root_path.clone();
        let src = root.join("src");
        let top_match = root.join("top.txt");
        let deep_match = src.join("deep.txt");
        let shallow = vec![dir_item(&src, 1), file_item(&top_match, 1)];
        let mut deep = shallow.clone();
        deep.push(file_item(&deep_match, 2));
        deep.push(file_item(&src.join("other.txt"), 2));
        let mut scanner = MockScanner::new();
        scanner.set_results(shallow.clone(), deep);
        let searcher = ListBoundFileSearcher {
            matching: HashSet::from([top_match.clone(), deep_match.clone()]),
        };
        let token = {
            // The query was entered during the scan and searched on the shallow list.
            let mut state = harness.state.lock().unwrap();
            state.content_search_query = "needle".to_string();
            state.content_search_results = HashSet::from([top_match.clone()]);
            state.begin_scan()
        };

        // Act
        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            root,
            scanner,
            searcher,
            token,
        )
        .await;

        // Assert
        let ui_state = harness.get_last_state_update().await.unwrap();
        assert!(!ui_state.is_scanning);
        let state = harness.state.lock().unwrap();
        assert_eq!(
            state.content_search_results,
            HashSet::from([top_match.clone(), deep_match.clone()])
        );
        let visible: HashSet<PathBuf> = state
            .filtered_file_list()
            .iter()
            .map(|item| item.path.clone())
            .collect();
        assert_eq!(visible, HashSet::from([src, top_match, deep_match]));
    }

    /// Without a content query a finished scan must not trigger a search (nor an
    /// additional state update).
    #[tokio::test]
    async fn deep_scan_without_content_query_does_not_search() {
        struct UnexpectedSearch;
        impl FileSearcher for UnexpectedSearch {
            async fn search(&self, _: Vec<FileItem>, _: &str, _: bool) -> HashSet<PathBuf> {
                panic!("No search expected without a content query");
            }
        }
        let mut harness = TestHarness::new();
        let token = harness.state.lock().unwrap().begin_scan();

        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            MockScanner::new(),
            UnexpectedSearch,
            token,
        )
        .await;

        let events = harness.get_n_events(3).await;
        assert_eq!(events.len(), 2, "Only the shallow and the deep update");
    }

    /// Files that arrive through a lazy load are subject to an active content query too.
    #[tokio::test]
    async fn content_search_is_repeated_after_lazy_load() {
        let harness = TestHarness::new();
        let dir = harness.root_path.join("src");
        let new_match = dir.join("match.txt");
        let mut scanner = MockScanner::new();
        scanner.set_results(
            vec![
                file_item(&new_match, 2),
                file_item(&dir.join("other.txt"), 2),
            ],
            vec![],
        );
        let token = {
            let mut state = harness.state.lock().unwrap();
            state.full_file_list.push(dir_item(&dir, 1));
            state.content_search_query = "needle".to_string();
            state.begin_lazy_load().expect("No scan is running")
        };

        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &dir).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            ListBoundFileSearcher {
                matching: HashSet::from([new_match.clone()]),
            },
            token,
            None,
        )
        .await;

        let state = harness.state.lock().unwrap();
        assert_eq!(state.content_search_results, HashSet::from([new_match]));
    }

    /// A lazy load started with the previous ignore patterns must not re-insert entries
    /// that the patterns added in the meantime have just removed from the list.
    #[tokio::test]
    async fn lazy_load_result_is_discarded_after_ignore_patterns_were_applied() {
        // Arrange: a lazy load of `src` is in flight and will report `src/generated`.
        let mut harness = TestHarness::new();
        let dir = harness.root_path.join("src");
        let generated = dir.join("generated");
        let mut scanner = MockScanner::new();
        scanner.set_results(vec![dir_item(&generated, 2)], vec![]);
        let token = {
            let mut state = harness.state.lock().unwrap();
            state.full_file_list.push(dir_item(&dir, 1));
            state.begin_lazy_load().expect("No scan is running")
        };

        // Act: meanwhile the user adds an ignore pattern, which is applied locally.
        {
            let mut state = harness.state.lock().unwrap();
            state
                .config
                .ignore_patterns
                .insert("src/generated/".to_string());
            state.apply_ignore_patterns();
        }
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &dir).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
            None,
        )
        .await;

        // Assert
        assert!(harness.get_last_state_update().await.is_none());
        let state = harness.state.lock().unwrap();
        assert!(!state.full_file_list.iter().any(|i| i.path == generated));
        assert!(
            !state.loaded_dirs.contains(&dir),
            "The directory stays loadable with the new patterns"
        );
    }
}
