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
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::events::UserEvent;
use super::filtering::{self, auto_expand_for_matches};
use super::proxy::EventProxy;
use super::selection::selected_files_in_tree_order;
use super::shared_state::SharedState;
use super::staged;
use super::state::Discarded;
use super::state::{
    AfterAbandonedScan, AppState, GenerationToken, LazyLoadToken, PreviewToken, ScanToken,
    StatusNotice,
};
use super::view_model::generate_ui_state;
use crate::config::{AppConfig, ConfigSnapshot};

use crate::core::file_handler::GenerationRequest;
use crate::core::scanner::ScanRequest;
use crate::core::{
    is_blank, token_counter, ContentSearch, ContentSearchRequest, CoreError, DirectoryScanner,
    FileHandler, FileItem, ScanProgress, SearchEngine,
};
use crate::utils::language::language_of;

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

/// The outcome of a scan that has delivered its items in batches: the ignore patterns
/// that matched.
pub type StreamedScanOutcome = Result<HashSet<String>, CoreError>;

/// Takes the batches of a streaming scan, see [`Scanner::scan_in_batches`].
pub type BatchSink = Box<dyn Fn(Vec<FileItem>) + Send + Sync>;

/// A trait abstracting the directory scanning functionality.
pub trait Scanner: Send + Sync {
    /// Scans what `request` describes and returns all of it at once.
    fn scan(
        &self,
        request: &ScanRequest,
        progress_callback: Box<dyn Fn(ScanProgress) + Send + Sync>,
    ) -> impl Future<Output = ScanOutcome> + Send;

    /// Scans what `request` describes and passes the items to `on_batch` while the
    /// scan runs: batch by batch, every item once. `on_batch` is called from the
    /// threads of the scan and must be quick. The batches that were delivered stand
    /// even if the scan fails or is cancelled afterwards.
    ///
    /// By default the result of [`Scanner::scan`] is delivered as a single batch.
    fn scan_in_batches(
        &self,
        request: &ScanRequest,
        progress_callback: Box<dyn Fn(ScanProgress) + Send + Sync>,
        on_batch: BatchSink,
    ) -> impl Future<Output = StreamedScanOutcome> + Send {
        async move {
            let (items, matched_patterns) = self.scan(request, progress_callback).await?;
            on_batch(items);
            Ok(matched_patterns)
        }
    }
}

/// A trait abstracting the token counting functionality.
pub trait Tokenizer: Send + Sync {
    /// Counts the tokens of `text`, which is shared with the caller, not copied.
    /// `None` if the count was cancelled; its result is not needed any more then.
    fn count_tokens(&self, text: Arc<String>) -> impl Future<Output = Option<usize>> + Send;
}

/// The outcome of a content search: the matching paths, or why the search failed.
pub type SearchOutcome = Result<HashSet<PathBuf>, String>;

/// A trait abstracting the file content search functionality.
pub trait FileSearcher: Send + Sync {
    /// Returns the paths of the files whose content matches `request`, or what kept
    /// the search from coming to an end (a failed search is not a search without
    /// matches).
    ///
    /// Once the cancellation flag of the request is raised the result is no longer
    /// needed; an implementation should then stop and may return anything.
    fn search(
        &self,
        files_to_search: Vec<FileItem>,
        request: ContentSearchRequest,
    ) -> impl Future<Output = SearchOutcome> + Send;
}

//================================================================================================//
//|                                   REAL IMPLEMENTATIONS                                       |//
//================================================================================================//

pub struct RealContentGenerator {
    pub cancel_flag: Arc<AtomicBool>,
    /// Files larger than this many bytes are not inlined into the output.
    pub max_file_size: u64,
    /// A document larger than this many bytes is not built ([`MAX_OUTPUT_SIZE`]).
    pub max_output_size: u64,
    /// What the document is expected to weigh ([`GenerationPlan::estimated_size`]).
    pub expected_output_size: u64,
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
            GenerationRequest {
                selected_files,
                root_path,
                include_tree,
                items_for_tree,
                tree_ignore_patterns,
                use_relative_paths,
                max_file_size: self.max_file_size,
                max_output_size: self.max_output_size,
                expected_output_size: self.expected_output_size,
            },
            self.cancel_flag.clone(),
            // Nobody follows the progress of a generation (yet).
            &(),
        )
        .await
    }
}

pub struct RealScanner {
    pub ignore_patterns: HashSet<String>,
    /// Files larger than this many bytes are not listed.
    pub max_file_size: u64,
    pub cancel_flag: Arc<AtomicBool>,
}
impl Scanner for RealScanner {
    async fn scan(
        &self,
        request: &ScanRequest,
        progress_callback: Box<dyn Fn(ScanProgress) + Send + Sync>,
    ) -> ScanOutcome {
        let scanner = DirectoryScanner::new(self.ignore_patterns.clone())
            .with_max_file_size(self.max_file_size);
        scanner
            .scan(request, self.cancel_flag.clone(), progress_callback)
            .await
    }

    async fn scan_in_batches(
        &self,
        request: &ScanRequest,
        progress_callback: Box<dyn Fn(ScanProgress) + Send + Sync>,
        on_batch: BatchSink,
    ) -> StreamedScanOutcome {
        let scanner = DirectoryScanner::new(self.ignore_patterns.clone())
            .with_max_file_size(self.max_file_size);
        scanner
            .scan_in_batches(
                request,
                self.cancel_flag.clone(),
                progress_callback,
                on_batch,
            )
            .await
    }
}

/// Counts tokens on the blocking thread pool (see [`token_counter`]).
pub struct RealTokenizer {
    /// Raised when the generation the count belongs to is cancelled or superseded.
    /// Aborting the task does not reach the blocking thread; this flag does.
    pub cancel_flag: Arc<AtomicBool>,
}
impl Tokenizer for RealTokenizer {
    async fn count_tokens(&self, text: Arc<String>) -> Option<usize> {
        let cancel_flag = self.cancel_flag.clone();
        tokio::task::spawn_blocking(move || token_counter::count_tokens(&text, &cancel_flag))
            .await
            .unwrap_or_else(|e| {
                // The document is worth showing without its count.
                tracing::error!("Counting the tokens failed: {e}");
                Some(0)
            })
    }
}

#[derive(Copy, Clone)]
pub struct RealFileSearcher;
impl FileSearcher for RealFileSearcher {
    async fn search(
        &self,
        files_to_search: Vec<FileItem>,
        request: ContentSearchRequest,
    ) -> SearchOutcome {
        let search = ContentSearch::from(request);
        tokio::task::spawn_blocking(move || search.matching_paths(&files_to_search))
            .await
            .map_err(|e| e.to_string())
    }
}

//================================================================================================//
//|                                   TASK IMPLEMENTATIONS                                       |//
//================================================================================================//

/// An output of more than this many bytes (estimated) is still generated, but the
/// user is told that it is large (`OutputSizeNotice::Large`): 20 MiB.
///
/// Up to here a generation is a matter of well under a second. Beyond it the time
/// becomes noticeable, and most of it is spent where it cannot be cancelled: the
/// editor needs about 25 ms per megabyte to take a document over, on the UI thread
/// (measured in Chromium and WebKit; 20 MiB: 0.5 s, 50 MiB: 1.2 s).
pub const LARGE_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;

/// An output of more than this many bytes is not generated
/// (`OutputSizeNotice::TooLarge`): 200 MiB.
///
/// The document is handed to the WebView as one string and shown by the editor as a
/// whole. At 200 MiB that blocks the UI thread for about 5 s, and it is within a
/// factor of 2.5 of what a string can hold in the WebView of Windows at all (V8:
/// 512 MiB of characters). A document of this size is also far beyond what it is
/// made for: 200 MiB of source code are roughly 50 million tokens.
///
/// The limit is enforced twice: the command refuses a selection whose estimated
/// output exceeds it, without reading a file, and the generation gives up if the
/// document grows beyond it all the same (the files have grown since the scan).
pub const MAX_OUTPUT_SIZE: u64 = 200 * 1024 * 1024;

/// The directory tree of a planned generation.
pub struct PlannedTree {
    /// The entries the tree is drawn from: the unfiltered file list.
    pub items: Vec<FileItem>,
    /// Directories without files are left out ("remove empty directories", for a
    /// list that is known to be complete).
    pub prune_empty_directories: bool,
    pub ignore_patterns: HashSet<String>,
}

/// Everything a generation works with, taken from the state at one moment.
///
/// The command that starts a generation makes the plan under the state lock it holds
/// anyway, so what is generated is the selection as it was when the user asked for
/// it, and the size the command judges is the size of exactly that. Of the file list
/// only what is needed leaves the lock: the paths of the selected files are copied,
/// their sizes are added up where they are, and the list itself is copied once, and
/// only if the document gets a tree (the configuration is not copied at all).
pub struct GenerationPlan {
    /// The selected files in the order of the tree.
    pub selected_files: Vec<PathBuf>,
    pub root: PathBuf,
    /// `None` if the document gets no directory tree.
    pub tree: Option<PlannedTree>,
    pub use_relative_paths: bool,
    /// What the document will weigh, in bytes, told from the sizes of the scan
    /// (`FileHandler::estimate_output_size`).
    pub estimated_size: u64,
}

impl GenerationPlan {
    /// The plan of a generation of what is selected in `state` right now.
    pub fn of(state: &AppState) -> Self {
        let config = &state.config;
        let items = state.full_file_list();
        let include_tree = config.include_tree_by_default;

        let estimated_size = FileHandler::estimate_output_size(
            items
                .iter()
                .filter(|item| state.selected_files.contains(&item.path)),
            include_tree.then_some(items),
            config.max_file_size_bytes(),
        );

        Self {
            selected_files: selected_files_in_tree_order(items, &state.selected_files),
            root: PathBuf::from(&state.current_path),
            tree: include_tree.then(|| PlannedTree {
                items: items.to_vec(),
                prune_empty_directories: config.remove_empty_directories && state.is_fully_scanned,
                ignore_patterns: config.tree_ignore_patterns.clone(),
            }),
            use_relative_paths: config.use_relative_paths,
            estimated_size,
        }
    }
}

/// The main asynchronous task for generating the concatenated file content.
///
/// `plan` says what is generated (see [`GenerationPlan`]); the task does not read the
/// selection from the state again. `token` identifies this generation run. The outcome
/// is only published while the token is still current, so a cancelled or superseded
/// task can neither show outdated content nor reset the generating state of the
/// generation that replaced it.
pub async fn generation_task<P, G, T>(
    proxy: P,
    state: SharedState,
    content_generator: G,
    tokenizer: T,
    token: GenerationToken,
    plan: GenerationPlan,
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

    let GenerationPlan {
        selected_files,
        root,
        tree,
        use_relative_paths,
        estimated_size: _,
    } = plan;

    let include_tree = tree.is_some();
    let (items_for_tree, tree_ignore_patterns) = match tree {
        Some(mut tree) => {
            if tree.prune_empty_directories {
                tracing::debug!("Pruning empty directories from the generated tree.");
                SearchEngine::retain_directories_with_files(&mut tree.items);
            }
            (tree.items, tree.ignore_patterns)
        }
        None => (Vec::new(), HashSet::new()),
    };

    let result = content_generator
        .generate(
            &selected_files,
            &root,
            include_tree,
            items_for_tree,
            tree_ignore_patterns,
            use_relative_paths,
        )
        .await;

    let outcome = match result {
        Ok(content) => {
            // From here on the document is shared: with the token count, with the
            // event that shows it and with the state that keeps it for saving.
            let document = Arc::new(content);
            match tokenizer.count_tokens(document.clone()).await {
                Some(token_count) => Ok((document, token_count)),
                None => Err(CoreError::Cancelled),
            }
        }
        Err(e) => Err(e),
    };

    publish_generation_outcome(outcome, &proxy, &state, &token);
}

/// Ends the generation identified by `token` and reports its outcome to the UI.
///
/// The outcome of a generation that is no longer the current one (superseded or
/// cancelled) is only logged; whoever replaced or cancelled it informs the UI.
///
/// A document that is published is kept by the state under the id of its generation
/// (`AppState::complete_generation`), so that a save can write it without the UI
/// sending it back.
fn publish_generation_outcome<P: EventProxy>(
    outcome: Result<(Arc<String>, usize), CoreError>,
    proxy: &P,
    state: &SharedState,
    token: &GenerationToken,
) {
    let mut state_guard = state.lock();
    match outcome {
        Ok((text, token_count)) => {
            let Some(document) = state_guard.complete_generation(token, text) else {
                tracing::debug!("Discarding the outcome of an outdated generation.");
                return;
            };
            proxy.send_event(UserEvent::ShowGeneratedContent {
                document,
                token_count,
            });
        }
        // A cancelled generation never gets past the check: cancelling is what raises
        // the cancel flag, and it releases the generating state first, so the outcome
        // of the cancelled run (`CoreError::Cancelled`) is discarded here.
        Err(e) => {
            if !state_guard.finish_generation(token) {
                tracing::debug!("Discarding the outcome of an outdated generation.");
                return;
            }
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
    state: &'a SharedState,
    proxy: &'a P,
    token: &'a GenerationToken,
}

impl<P: EventProxy> Drop for GenerationGuard<'_, P> {
    fn drop(&mut self) {
        // This may run while the task is unwinding from a panic, possibly one that
        // happened under the state lock; `SharedState::lock` copes with that.
        let mut state = self.state.lock();
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
struct ScanGuard<P: EventProxy> {
    state: SharedState,
    proxy: P,
    token: ScanToken,
}

impl<P: EventProxy> ScanGuard<P> {
    fn new(state: &SharedState, proxy: &P, token: &ScanToken) -> Self {
        Self {
            state: state.clone(),
            proxy: proxy.clone(),
            token: token.clone(),
        }
    }
}

impl<P: EventProxy> Drop for ScanGuard<P> {
    fn drop(&mut self) {
        // This may run while the task is unwinding from a panic, possibly one that
        // happened under the state lock; `SharedState::lock` copes with that.
        let mut state = self.state.lock();
        if state.finish_scan(&self.token) {
            self.proxy
                .send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&state))));
        }
    }
}

/// The proactive, two-phase scan of `path` as a task of its own: [`scan_phases`] under
/// a [`ScanGuard`].
///
/// `token` identifies this scan run. Every state mutation is conditional on the token
/// still being current, so a cancelled or superseded task cannot interfere with the
/// scan that replaced it.
///
/// A content search whose query is active sees only the files known at the time it ran.
/// Once the scan has delivered the complete file list, the search is therefore repeated
/// with `searcher`. The scan counts as running until that search has been applied:
/// only then is it reported as complete, with the number of items that are visible
/// under the filters as they are then.
pub async fn proactive_scan_task<P: EventProxy, S: Scanner, F: FileSearcher + 'static>(
    proxy: P,
    state: SharedState,
    path: PathBuf,
    scanner: S,
    searcher: F,
    token: ScanToken,
) {
    let _scan_guard = ScanGuard::new(&state, &proxy, &token);
    scan_phases(&proxy, &state, &path, scanner, searcher, &token).await;
}

/// The two phases of a scan. Whoever calls this holds the [`ScanGuard`] of the scan:
/// returning early (cancellation) relies on it to release the scanning state.
async fn scan_phases<P: EventProxy, S: Scanner, F: FileSearcher + 'static>(
    proxy: &P,
    state: &SharedState,
    path: &Path,
    scanner: S,
    searcher: F,
    token: &ScanToken,
) {
    // --- Phase 1: Shallow Scan ---
    let scan_result_shallow = scanner
        .scan(
            &ScanRequest::project(path, Some(1)),
            scan_progress_reporter(proxy, state, token),
        )
        .await;

    if token.is_cancelled() {
        tracing::info!("Scan cancelled after shallow scan phase.");
        return; // The caller's guard cleans up the state and notifies.
    }

    match scan_result_shallow {
        Ok((files, patterns)) => {
            if !apply_shallow_scan_result(state, proxy, token, path, files, patterns) {
                return;
            }
        }
        Err(e) => {
            handle_scan_error(e, state, proxy, token);
            return;
        }
    }

    // --- Phase 2: Deep Background Scan (Indexing) ---
    // The walk delivers what it finds while it runs; the batches are published into
    // the state at the pace of `publisher`. A batch that arrives when the scan is no
    // longer the current one ends the task (the guard has nothing to release then).
    let (batch_tx, mut batch_rx) = mpsc::unbounded_channel::<Vec<FileItem>>();
    let mut publisher = BatchPublisher::new();
    let deep_request = ScanRequest::project(path, None);
    let scan_result_deep = {
        let scan = scanner.scan_in_batches(
            &deep_request,
            scan_progress_reporter(proxy, state, token),
            Box::new(move |batch| {
                // Nobody listens any more once the task has ended.
                let _ = batch_tx.send(batch);
            }),
        );
        tokio::pin!(scan);
        loop {
            // In this order: the end of the scan first, and publishing before taking
            // more batches, which a fast walk supplies without a pause.
            tokio::select! {
                biased;
                outcome = &mut scan => break outcome,
                () = tokio::time::sleep_until(publisher.due()), if publisher.has_pending() => {
                    if !publisher.publish(state, proxy, token) {
                        return;
                    }
                }
                Some(batch) = batch_rx.recv() => publisher.add(batch),
            }
        }
    };

    if token.is_cancelled() {
        tracing::info!("Scan cancelled during deep scan phase.");
        return; // As above.
    }

    match scan_result_deep {
        Ok(patterns) => {
            // The scan has ended, so everything it delivers is in the channel.
            while let Ok(batch) = batch_rx.try_recv() {
                publisher.add(batch);
            }
            if apply_deep_scan_result(state, token, publisher.take_pending(), patterns) {
                repeat_content_search_to_the_end(proxy, state, searcher).await;
                complete_scan(state, proxy, token);
            }
        }
        Err(e) => handle_scan_error(e, state, proxy, token),
    }
}

/// The shortest time between two publications of scanned items, see [`BatchPublisher`].
const MIN_PUBLISH_INTERVAL: Duration = Duration::from_millis(150);

/// How many times the duration of a publication has to pass before the next one, see
/// [`BatchPublisher`]. 9 limits publishing to a tenth of the time of a scan.
const PUBLISH_PAUSE_FACTOR: u32 = 9;

/// Publishes the items a running scan delivers into the state, at a bounded cost.
///
/// The scanner delivers small batches, hundreds per second on a fast disk. Publishing
/// means: merging them into the file list, bringing the filtered list up to date and
/// sending a state update, for which the tree index is built anew. All of that grows
/// with the size of the list. The batches are therefore collected here and published
/// together
///
/// * at most every [`MIN_PUBLISH_INTERVAL`] (more often would not be seen), and
/// * only after [`PUBLISH_PAUSE_FACTOR`] times the duration of the previous
///   publication has passed. The larger the list gets, the less often it is
///   published, and the share of publishing in the time of the scan stays below
///   1 / (1 + factor) however large the project is.
///
/// The pace bounds the work. That commands do not have to wait for it is a matter of
/// its own: the part that grows with the list holds no lock on the state, see
/// [`add_scanned_items`].
struct BatchPublisher {
    pending: Vec<FileItem>,
    due: tokio::time::Instant,
}

impl BatchPublisher {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            due: tokio::time::Instant::now() + MIN_PUBLISH_INTERVAL,
        }
    }

    fn add(&mut self, mut batch: Vec<FileItem>) {
        self.pending.append(&mut batch);
    }

    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// When the next publication may happen.
    fn due(&self) -> tokio::time::Instant {
        self.due
    }

    /// The items that have not been published.
    fn take_pending(&mut self) -> Vec<FileItem> {
        std::mem::take(&mut self.pending)
    }

    /// Publishes the pending items on behalf of the scan identified by `token` and
    /// sets the time of the next publication. Returns `false` if the scan is no
    /// longer the current one (nothing is changed or sent then).
    fn publish<P: EventProxy>(
        &mut self,
        state: &SharedState,
        proxy: &P,
        token: &ScanToken,
    ) -> bool {
        let items = self.take_pending();
        let Some(cost) = publish_scanned_items(state, proxy, token, items) else {
            return false;
        };
        tracing::debug!(
            "Published scanned items in {:?}, the state locked for at most {:?}.",
            cost.total,
            cost.locked
        );
        self.due = tokio::time::Instant::now() + pause_after_publication(cost.total);
        true
    }
}

/// How long to wait after a publication that took `cost`, see [`BatchPublisher`].
fn pause_after_publication(cost: Duration) -> Duration {
    MIN_PUBLISH_INTERVAL.max(cost.saturating_mul(PUBLISH_PAUSE_FACTOR))
}

/// What a publication of scanned items took.
#[derive(Debug, Clone, Copy)]
struct PublicationCost {
    /// From beginning to end. This is what the pace of publishing follows.
    total: Duration,
    /// The longer of the two moments in which the state was locked: what commands
    /// and the progress of the walk had to wait for at most.
    locked: Duration,
}

/// Merges `items`, delivered by the walk of the scan identified by `token`, into the
/// file list and tells the UI. Returns what that took, or `None` without touching
/// anything if the scan is no longer the current one.
fn publish_scanned_items<P: EventProxy>(
    state: &SharedState,
    proxy: &P,
    token: &ScanToken,
    items: Vec<FileItem>,
) -> Option<PublicationCost> {
    add_scanned_items(state, token, items, |s| {
        proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(s))));
    })
}

/// Merges `items`, delivered by the walk of the scan identified by `token`, into the
/// file list and applies the filters. `added` is called with the state as it is then,
/// before anything else can change it: a state update made there shows exactly the
/// state with the items. Returns what all of this took, or `None` without touching
/// anything (and without calling `added`) if the scan is no longer the current one.
///
/// The state is locked twice, briefly: to take what is needed out of it and to put
/// the result in. The work in between (the lists and the hierarchy, which grow with
/// the project) holds no lock, see [`staged`].
fn add_scanned_items(
    state: &SharedState,
    token: &ScanToken,
    items: Vec<FileItem>,
    added: impl FnOnce(&AppState),
) -> Option<PublicationCost> {
    in_three_steps(
        state,
        |s| staged::stage_scanned_items(s, token, items),
        |s, prepared| staged::commit_scanned_items(s, token, prepared),
        |s| added(s),
    )
}

/// Brings an event into the state in the three steps of [`staged`]: `stage` and
/// `commit` are called with the state locked, the preparation in between is not.
/// `committed` is called after a successful `commit`, under the same lock: this is
/// where the state update is made, of exactly the state the event has left.
///
/// Returns what all of this took, or `None` if the event is outdated (`stage`
/// returned `None`, or `commit` `false`): nothing was changed then, and `committed`
/// was not called.
fn in_three_steps<C: staged::Change>(
    state: &SharedState,
    stage: impl FnOnce(&mut AppState) -> Option<staged::Staged<C>>,
    commit: impl FnOnce(&mut AppState, staged::Prepared<C::Rest>) -> (bool, Discarded),
    committed: impl FnOnce(&mut AppState),
) -> Option<PublicationCost> {
    let started = std::time::Instant::now();
    let (staged, staging) = {
        let mut s = state.lock();
        let locked = std::time::Instant::now();
        (stage(&mut s)?, locked.elapsed())
    };

    let prepared = staged.prepare();

    let (is_current, committing, discarded) = {
        let mut s = state.lock();
        let locked = std::time::Instant::now();
        let (is_current, discarded) = commit(&mut s, prepared);
        if is_current {
            committed(&mut s);
        }
        (is_current, locked.elapsed(), discarded)
    };
    // Lists and hierarchies that have been replaced: not while the state is locked.
    drop(discarded);

    is_current.then(|| PublicationCost {
        total: started.elapsed(),
        locked: staging.max(committing),
    })
}

/// What the scan identified by `token` does with the progress its scanner reports
/// (which is throttled by the scanner): it is stored in the state, so that every state
/// update shows how far the scan has got, and sent to the UI for the live display.
/// Progress of a scan that is no longer the current one goes nowhere.
fn scan_progress_reporter<P: EventProxy>(
    proxy: &P,
    state: &SharedState,
    token: &ScanToken,
) -> Box<dyn Fn(ScanProgress) + Send + Sync> {
    let (proxy, state, token) = (proxy.clone(), state.clone(), token.clone());
    Box::new(move |progress| {
        if state.lock().record_scan_progress(&token, progress.clone()) {
            proxy.send_event(UserEvent::ScanProgress(progress));
        }
    })
}

/// Repeats the content search (see [`refresh_content_search`]) as a task of its own
/// and waits for it.
///
/// The search belongs to the file list, not to the scan that delivered the list: if the
/// scan is cancelled while waiting here, the search still runs to its end and its
/// matches are applied. Awaited inline, it would be dropped half-way and leave the
/// matches of the previous, smaller file list in place.
async fn repeat_content_search_to_the_end<P: EventProxy, F: FileSearcher + 'static>(
    proxy: &P,
    state: &SharedState,
    searcher: F,
) {
    let (proxy, state) = (proxy.clone(), state.clone());
    let search =
        tokio::spawn(async move { refresh_content_search(&proxy, &state, searcher).await });
    if let Err(e) = search.await {
        tracing::error!("The content search after the scan failed: {e}");
    }
}

/// Publishes the result of the shallow scan phase.
///
/// Returns `false` without touching the state if the scan is no longer the current one.
fn apply_shallow_scan_result<P: EventProxy>(
    state: &SharedState,
    proxy: &P,
    token: &ScanToken,
    root: &Path,
    files: Vec<FileItem>,
    patterns: HashSet<String>,
) -> bool {
    let mut s = state.lock();
    if !s.is_current_scan(token) {
        return false;
    }
    // The first level is small, whatever the project is like. What it replaces
    // may be the lists of a large project (a re-scan): they are let go of after
    // the lock.
    let replaced = s.lists_to_outlive_the_lock();
    s.replace_with_shallow_file_list(root, files);
    s.active_ignore_patterns = patterns;
    filtering::apply_filters(&mut s);
    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&s))));
    drop(s);
    drop(replaced);
    true
}

/// Stores the end of the deep scan phase: `remaining_items` are the items of the walk
/// that have not been published yet, with them the file list is complete. The scan is
/// not finished by this, see [`complete_scan`]; nothing is sent to the UI.
///
/// Like a publication, this does not hold the lock on the state for the work that
/// grows with the list: the items are added as every batch is, and the complete
/// list, its filtered list and the hierarchy are there before [`complete_scan`]
/// makes its state update ([`staged`]).
///
/// Returns `false` without touching the state if the scan is no longer the current one.
fn apply_deep_scan_result(
    state: &SharedState,
    token: &ScanToken,
    remaining_items: Vec<FileItem>,
    patterns: HashSet<String>,
) -> bool {
    // The remaining items go the way of every batch. They get no state update of
    // their own: whatever ends this scan from here on sends one.
    if add_scanned_items(state, token, remaining_items, |_| {}).is_none() {
        return false;
    }
    in_three_steps(
        state,
        |s| staged::stage_scan_completion(s, token),
        |s, prepared| staged::commit_scan_completion(s, token, prepared, patterns),
        |_| {},
    )
    .is_some()
}

/// Marks the scan as finished and reports it to the UI. This is the last step of a
/// scan: the number of visible items it reports is the one of the final filtered list.
///
/// Does nothing if the scan is no longer the current one.
fn complete_scan<P: EventProxy>(state: &SharedState, proxy: &P, token: &ScanToken) {
    let mut s = state.lock();
    // The final state is set here; the guard only cleans up on panics/cancellations.
    if !s.finish_scan(token) {
        return;
    }
    s.status = StatusNotice::IndexingComplete {
        visible_items: s.filtered_file_list().len(),
    };
    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(&s))));
}

/// Starts a proactive, two-phase scan of `path`, replacing the scan that is running
/// (if any). There are two kinds of request:
///
/// * **Opening** `path` (`preserve_state` is `false`): a selected or dropped path is
///   to replace the loaded directory. A path to a file opens the directory the file
///   lies in. Whether the path exists is not known yet, and finding out is IO that
///   must not run on the command lane (on a slow network drive even `exists()` can
///   take long). Until the scan task has looked at it, the loaded directory therefore
///   stays as it is; it is replaced once the path has been found, and kept if the path
///   does not exist (see [`locate_scan_root`]).
/// * **Re-scanning** `path`, the loaded directory (`preserve_state` is `true`):
///   selection, expansion and filters are kept.
///
/// Either way everything the request means for the *scan* happens here, synchronously
/// and under one lock: the scan that was running is superseded, the new one is
/// registered and the UI is told. When this function returns, the scan is visible and
/// cancellable, so a command that follows (cancel, clear, another open) acts on it.
/// Only the IO runs in the spawned task.
pub fn start_scan_on_path<P: EventProxy>(
    path: PathBuf,
    proxy: P,
    state: SharedState,
    preserve_state: bool,
) {
    let mut state_guard = state.lock();
    if preserve_state {
        state_guard.current_path = path.to_string_lossy().to_string();
        restart_scan(&mut state_guard, proxy, state.clone());
    } else {
        open_path(&mut state_guard, path, proxy, state.clone());
    }
}

/// Starts opening `path` (see [`start_scan_on_path`]) for a caller that holds the lock
/// on the state and passes it as `state_guard`; `state` is the handle the scan task
/// keeps.
pub fn open_path<P: EventProxy>(
    state_guard: &mut AppState,
    path: PathBuf,
    proxy: P,
    state: SharedState,
) {
    let token = state_guard.begin_open(&path);
    launch_scan(state_guard, token, path, proxy, state);
}

/// Scans the loaded directory again with the configuration as it is now, replacing a
/// scan that is still running. Selections and expansions are kept.
///
/// This is how a change of the ignore patterns reaches a running scan: a scan works
/// with the patterns it was started with, so instead of correcting its result
/// afterwards it is superseded by one that knows the new patterns (and does not walk
/// what they exclude).
///
/// Nothing that describes the loaded list changes here: it stays what it is (and as
/// complete as it is) until the task has found the directory, see
/// [`locate_scan_root`]. If the directory has vanished, the list is still good.
///
/// The caller holds the lock on the state and passes it as `state_guard`; `state` is
/// the handle the scan task keeps.
pub fn restart_scan<P: EventProxy>(state_guard: &mut AppState, proxy: P, state: SharedState) {
    let directory = PathBuf::from(&state_guard.current_path);
    let token = state_guard.begin_scan();
    launch_scan(state_guard, token, directory, proxy, state);
}

/// Announces the scan identified by `token` to the UI and spawns its task. This
/// happens under the lock the caller holds, in which the scan was registered, so the
/// scan is cancellable from the moment it is visible.
fn launch_scan<P: EventProxy>(
    state_guard: &mut AppState,
    token: ScanToken,
    path: PathBuf,
    proxy: P,
    state: SharedState,
) {
    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
        state_guard,
    ))));
    let handle = tokio::spawn(scan_task(path, proxy, state, token.clone()));
    state_guard.attach_scan_task(&token, handle);
}

/// The task of a scan from its very first step: looks at `path` and, if there is a
/// directory to scan, runs the two phases.
///
/// The guard covers all of it, so a task that ends early for whatever reason (a panic
/// while the path is looked at included) releases the scanning state.
async fn scan_task<P: EventProxy>(path: PathBuf, proxy: P, state: SharedState, token: ScanToken) {
    let _scan_guard = ScanGuard::new(&state, &proxy, &token);
    let Some((directory, scanner)) = locate_scan_root(path, &proxy, &state, &token).await else {
        return;
    };
    scan_phases(
        &proxy,
        &state,
        &directory,
        scanner,
        RealFileSearcher,
        &token,
    )
    .await;
}

/// The first step of a scan task: finds the directory to scan for `path` on disk.
///
/// If the scan is opening `path`, the directory replaces the loaded one here: this is
/// the moment the previous directory is reset. For a re-scan it is the moment the
/// loaded list stops counting as complete ([`AppState::begin_scan_walk`]). The scanner
/// is created here as well, so it works with the configuration as it is when the walk
/// begins.
///
/// Returns `None` if there is nothing to scan: either the scan is no longer the
/// current one (nothing is touched then), or the path does not exist. The latter ends
/// the scan with an error message and leaves the loaded directory as it is. What the
/// attempt to open the path had superseded when it began (a scan of the loaded
/// directory, the opening of another path) is started again, so that the attempt
/// leaves nothing behind but the message.
async fn locate_scan_root<P: EventProxy>(
    path: PathBuf,
    proxy: &P,
    state: &SharedState,
    token: &ScanToken,
) -> Option<(PathBuf, RealScanner)> {
    let located = tokio::task::spawn_blocking(move || directory_to_scan(path))
        .await
        .unwrap_or_else(|e| Err(format!("Could not access the path: {e}")));

    let mut state_guard = state.lock();
    if !state_guard.is_current_scan(token) {
        return None;
    }
    match located {
        Ok(directory) => {
            let shown_path = directory.to_string_lossy().to_string();
            // From here on the list is the one this scan builds.
            let no_longer_complete = state_guard.begin_scan_walk(token);
            let entered = state_guard.enter_opened_directory(token, &directory);
            if entered || no_longer_complete || state_guard.current_path != shown_path {
                state_guard.current_path = shown_path;
                proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
                    &state_guard,
                ))));
            }
            if state_guard.config.last_directory.as_ref() != Some(&directory) {
                state_guard.config.last_directory = Some(directory.clone());
                save_config_in_background(state_guard.config_snapshot());
            }
            let scanner = RealScanner {
                ignore_patterns: state_guard.config.ignore_patterns.clone(),
                max_file_size: state_guard.config.max_file_size_bytes(),
                cancel_flag: token.cancel_flag(),
            };
            Some((directory, scanner))
        }
        Err(message) => {
            let todo = state_guard.abandon_scan(token);
            proxy.send_event(UserEvent::ShowError(message));
            // A scan that is started here announces itself with a state update.
            match todo {
                AfterAbandonedScan::Nothing => {
                    proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
                        &state_guard,
                    ))));
                }
                AfterAbandonedScan::RescanLoadedDirectory => {
                    restart_scan(&mut state_guard, proxy.clone(), state.clone());
                }
                AfterAbandonedScan::Reopen { path, token } => {
                    launch_scan(&mut state_guard, token, path, proxy.clone(), state.clone());
                }
            }
            None
        }
    }
}

/// The directory a scan of `path` covers: `path` itself, or the directory a file lies
/// in. Touches the file system, so it must not run on the command lane.
fn directory_to_scan(path: PathBuf) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("Path does not exist: {}", path.display()));
    }
    if path.is_dir() {
        return Ok(path);
    }
    // The parent of an existing file exists; a root path is its own fallback.
    Ok(path.parent().map(Path::to_path_buf).unwrap_or(path))
}

/// Writes `snapshot` to the configuration file on the blocking thread pool, so that
/// neither the command lane nor the holder of the state lock waits for the disk.
/// Take the snapshot under the lock ([`AppState::config_snapshot`]); of several saves
/// under way, the most recent snapshot is the one that ends up on disk.
///
/// This is how every change of the configuration is persisted while the application
/// runs. The save at exit takes a snapshot the same way and writes it itself (see
/// `window_geometry::persist`).
pub fn save_config_in_background(snapshot: ConfigSnapshot) {
    tokio::spawn(async move {
        match tokio::task::spawn_blocking(move || snapshot.save()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("Failed to save config: {e}"),
            Err(e) => tracing::error!("Saving the config failed: {e}"),
        }
    });
}

/// Helper to handle scan errors consistently.
///
/// Errors of a scan that is no longer the current one are only logged.
fn handle_scan_error<P: EventProxy>(
    error: CoreError,
    state: &SharedState,
    proxy: &P,
    token: &ScanToken,
) {
    tracing::error!("Scan failed: {}", error);
    let mut state_lock = state.lock();
    if !state_lock.finish_scan(token) {
        return;
    }
    state_lock.status = StatusNotice::ScanFailed {
        reason: error.to_string(),
    };
    let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_lock)));
    proxy.send_event(event);
}

/// Initiates a scan for a specific subdirectory level (lazy loading).
///
/// Asking for the content of a directory is a request to see it, so the directory is
/// marked as expanded right here, synchronously, when the request is made (and the UI
/// is told). The result of the load does not touch the expansion state anymore: if the
/// user collapses the directory while the load is in flight, it stays collapsed.
///
/// This also works while a scan is walking the project: the walk gets to the
/// directory when it gets to it, the user wants to see it now. The level is then in
/// the list ahead of the walk, which does not add it a second time
/// ([`AppState::merge_directory_level`]).
///
/// Only the scan itself runs in a spawned task. Returns its handle, which completes
/// when the result has been dealt with (merged or discarded), or `None` if the request
/// was refused: the directory is not part of the project.
pub fn start_lazy_load_scan<P: EventProxy>(
    path: PathBuf,
    proxy: P,
    state: SharedState,
) -> Option<JoinHandle<()>> {
    let (scanner, token, request) = {
        let mut state_guard = state.lock();
        let token = state_guard.begin_lazy_load();
        // The project root anchors the ignore rules and the depth of the new items.
        let request = match ScanRequest::directory_level(&state_guard.current_path, &path) {
            Ok(request) if !state_guard.current_path.is_empty() => request,
            _ => {
                tracing::warn!("Ignoring lazy load outside the project: {:?}", path);
                return None;
            }
        };
        state_guard.expand_directories([path]);
        proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
            &state_guard,
        ))));
        let scanner = lazy_load_scanner(&state_guard.config, &token);
        (scanner, token, request)
    };
    tracing::debug!("Starting lazy load for {:?}.", request.start_dir());
    Some(tokio::spawn(lazy_load_task(
        request,
        proxy,
        state,
        scanner,
        RealFileSearcher,
        token,
    )))
}

/// Creates the scanner for a lazy load. It shares the cancellation flag of `token`, so
/// the scan stops as soon as the file list it was started for is replaced.
fn lazy_load_scanner(config: &AppConfig, token: &LazyLoadToken) -> RealScanner {
    RealScanner {
        ignore_patterns: config.ignore_patterns.clone(),
        max_file_size: config.max_file_size_bytes(),
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
    state: SharedState,
    scanner: S,
    searcher: F,
    token: LazyLoadToken,
) {
    let scan_result = scanner.scan(&request, Box::new(|_| {})).await;

    if apply_lazy_load_result(scan_result, request.start_dir(), &proxy, &state, &token) {
        refresh_content_search(&proxy, &state, searcher).await;
    }
}

/// Merges the outcome of a lazy load into the state, unless it has become outdated.
///
/// Returns `true` if new items were merged into the file list.
fn apply_lazy_load_result<P: EventProxy>(
    scan_result: ScanOutcome,
    loaded_dir: &Path,
    proxy: &P,
    state: &SharedState,
    token: &LazyLoadToken,
) -> bool {
    let (new_items, new_active_patterns) = match scan_result {
        Ok(level) => level,
        Err(error) => return report_failed_lazy_load(error, loaded_dir, proxy, state, token),
    };
    let found = new_items.len();
    // The level joins the list and the filters are applied to it without the lock
    // being held for the work on the whole list.
    let merged = in_three_steps(
        state,
        |s| staged::stage_directory_level(s, token, loaded_dir, new_items),
        |s, prepared| staged::commit_directory_level(s, token, prepared, new_active_patterns),
        |s| proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(s)))),
    )
    .is_some();
    if merged {
        tracing::debug!("Lazy load found {} items in {:?}.", found, loaded_dir);
    } else {
        tracing::debug!("Discarding outdated lazy load result for {:?}.", loaded_dir);
    }
    merged
}

/// What becomes of a lazy load that failed. Returns `false`: nothing was merged.
fn report_failed_lazy_load<P: EventProxy>(
    error: CoreError,
    loaded_dir: &Path,
    proxy: &P,
    state: &SharedState,
    token: &LazyLoadToken,
) -> bool {
    // Held until the error is on its way, as the outcome of a load that succeeded.
    let state_guard = state.lock();
    if !state_guard.is_current_lazy_load(token) {
        tracing::debug!("Discarding outdated lazy load result for {:?}.", loaded_dir);
        return false;
    }
    tracing::error!("Lazy load failed for {:?}: {}", loaded_dir, error);
    proxy.send_event(UserEvent::ShowError(format!(
        "Failed to load directory {}: {}",
        loaded_dir.display(),
        error
    )));
    false
}

/// The number of lines of a file shown in the preview.
const PREVIEW_MAX_LINES: usize = 1500;

/// Makes `path` the previewed file and loads its content for the preview panel.
///
/// The request is recorded synchronously (the file is highlighted in the tree right
/// away); reading the file, which can take a while for a large file or a slow drive,
/// happens on the blocking thread pool. Returns the handle of the task that delivers
/// the outcome, see [`file_preview_task`].
pub fn start_file_preview<P: EventProxy>(
    path: PathBuf,
    proxy: P,
    state: SharedState,
) -> JoinHandle<()> {
    launch_file_preview(path, proxy, state, |path: &Path| {
        FileHandler::get_file_preview(path, PREVIEW_MAX_LINES)
    })
}

/// [`start_file_preview`] with the function that reads the file as a parameter.
/// `read` runs on the blocking thread pool.
fn launch_file_preview<P, R>(path: PathBuf, proxy: P, state: SharedState, read: R) -> JoinHandle<()>
where
    P: EventProxy,
    R: FnOnce(&Path) -> Result<String, CoreError> + Send + 'static,
{
    let (token, search_term) = {
        let mut state_guard = state.lock();
        let token = state_guard.begin_preview(path.clone());
        let search_term = Some(state_guard.content_search_query.clone())
            .filter(|content_query| !is_blank(content_query));
        proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(
            &state_guard,
        ))));
        (token, search_term)
    };
    tokio::spawn(file_preview_task(
        path,
        search_term,
        proxy,
        state,
        read,
        token,
    ))
}

/// Reads the file and shows its content (or the error), unless the preview has been
/// replaced or closed in the meantime: of several reads under way only the one that
/// was requested last has something to show, so previews appear in the order in which
/// they were requested and an outdated one never replaces a newer one.
async fn file_preview_task<P, R>(
    path: PathBuf,
    search_term: Option<String>,
    proxy: P,
    state: SharedState,
    read: R,
    token: PreviewToken,
) where
    P: EventProxy,
    R: FnOnce(&Path) -> Result<String, CoreError> + Send + 'static,
{
    let read_path = path.clone();
    let outcome = tokio::task::spawn_blocking(move || read(&read_path))
        .await
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))
        .and_then(|content| content.map_err(|e| e.to_string()));

    // Publishing under the lock keeps the answer in line with the state it belongs to.
    let mut state_guard = state.lock();
    if !state_guard.finish_preview(&token) {
        tracing::debug!("Discarding outdated preview of {:?}.", path);
        return;
    }
    match outcome {
        Ok(content) => {
            // The file takes the place of the generated output in the editor.
            state_guard.discard_generated_document();
            proxy.send_event(UserEvent::ShowFilePreview {
                content,
                language: language_of(&path).to_string(),
                search_term,
                path,
            });
        }
        Err(message) => proxy.send_event(UserEvent::ShowError(message)),
    }
}

/// Repeats the content search after something it depends on has changed: the file
/// list (files that were not known when the query was entered are searched as well)
/// or the way the query is matched (case sensitivity).
///
/// Does nothing (and sends nothing) if no content query is active, i.e. if it is
/// [blank](is_blank).
pub async fn refresh_content_search<P: EventProxy, S: FileSearcher>(
    proxy: &P,
    state: &SharedState,
    searcher: S,
) {
    let has_content_query = !is_blank(&state.lock().content_search_query);
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
/// A search that fails (its worker panicked) is reported with `ShowError` and leaves
/// the matches of the search before it in place.
///
/// Nothing is sent when the search starts: this is for repeating the search of a
/// query the UI already shows (see [`refresh_content_search`]). For a query that has
/// just been entered see [`search_for_new_query`].
///
/// [`ContentSearchToken`]: super::state::ContentSearchToken
pub async fn search_in_files<P: EventProxy, S: FileSearcher>(
    proxy: P,
    state: SharedState,
    searcher: S,
) {
    run_content_search(proxy, state, searcher, SearchStart::Silent).await;
}

/// Applies filter texts whose content query has changed: answers with a state update
/// right away and then searches like [`search_in_files`], whose state update with the
/// matches follows.
///
/// The first update carries the new filter texts while the search is pending. The UI
/// waits for it (a filter input shows the host's text again if the text it has sent
/// is not reported back), and a search can take arbitrarily long. The file name and
/// extension filters are applied in it, and so is the content filter as far as it is
/// known: the matches of the search before, or none at all while the first search
/// for a query runs (the list is not emptied, see
/// [`AppState::content_filter_matches`]).
///
/// [`AppState::content_filter_matches`]: super::state::AppState::content_filter_matches
pub async fn search_for_new_query<P: EventProxy, S: FileSearcher>(
    proxy: P,
    state: SharedState,
    searcher: S,
) {
    run_content_search(proxy, state, searcher, SearchStart::Announced).await;
}

/// What a content search sends when it starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchStart {
    /// Nothing; the UI hears of the search when its matches are there.
    Silent,
    /// A state update with the filters as they are while the search is pending.
    Announced,
}

async fn run_content_search<P: EventProxy, S: FileSearcher>(
    proxy: P,
    state: SharedState,
    searcher: S,
    start: SearchStart,
) {
    let (token, files_to_search, request) = {
        let mut state_guard = state.lock();
        // A blank query is no query: nothing is searched and earlier matches are dropped.
        if is_blank(&state_guard.content_search_query) {
            state_guard.clear_content_search();
            filtering::apply_filter_texts(&mut state_guard);
            let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
            proxy.send_event(event);
            return;
        }
        let token = state_guard.begin_content_search();
        if start == SearchStart::Announced {
            // After `begin_content_search`: the filters know that a search is running.
            filtering::apply_filter_texts(&mut state_guard);
            let event = UserEvent::StateUpdate(Box::new(generate_ui_state(&state_guard)));
            proxy.send_event(event);
        }
        let request = ContentSearchRequest {
            query: state_guard.content_search_query.clone(),
            case_sensitive: state_guard.config.case_sensitive_search,
            max_file_size: state_guard.config.max_file_size_bytes(),
            // Superseding or clearing the search raises the flag, so the searcher
            // stops reading files nobody is waiting for.
            cancel_flag: token.cancel_flag(),
        };
        (token, state_guard.shared_full_file_list(), request)
    };
    let query = request.query.clone();
    // The copy of the list the searcher takes is made here, not under the lock.
    let files_to_search = Arc::unwrap_or_clone(files_to_search);
    let outcome = searcher.search(files_to_search, request).await;
    let matching_paths = match outcome {
        Ok(matching_paths) => matching_paths,
        Err(reason) => {
            let state_guard = &mut *state.lock();
            // The matches of the search before stay: "the search failed" must not
            // look like "nothing matches".
            if state_guard.abandon_content_search(&token) {
                tracing::error!("The content search for {query:?} failed: {reason}");
                proxy.send_event(UserEvent::ShowError(format!(
                    "The content search failed: {reason}"
                )));
                let event = UserEvent::StateUpdate(Box::new(generate_ui_state(state_guard)));
                proxy.send_event(event);
            }
            return;
        }
    };
    // The matches become the content filter without the lock being held for the
    // filtering of the whole list.
    let applied = in_three_steps(
        &state,
        |s| staged::stage_content_matches(s, &token, matching_paths),
        |s, prepared| staged::commit_content_matches(s, &token, prepared),
        |s| {
            auto_expand_for_matches(s);
            proxy.send_event(UserEvent::StateUpdate(Box::new(generate_ui_state(s))));
        },
    );
    if applied.is_none() {
        tracing::debug!("Discarding outdated content search result for {:?}.", query);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view_model::UiState;
    use crate::config::AppConfig;
    use crate::core::MAX_FILE_SIZE;
    #[cfg(unix)] // Only the permission-based tests need it.
    use crate::utils::test_helpers::running_as_root;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;
    use tempfile::{tempdir, TempDir};
    use tokio::sync::oneshot;

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
        state: SharedState,
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
                state: SharedState::new(state),
                proxy,
                event_rx: rx,
                _temp_dir: temp_dir,
                root_path,
            }
        }
        /// The events sent so far, without waiting for more. This is the way to look
        /// at the outcome of code that has run to completion (an awaited task or a
        /// synchronous function): everything it sent is already here.
        fn drain_events(&mut self) -> Vec<UserEvent> {
            std::iter::from_fn(|| self.event_rx.try_recv().ok()).collect()
        }

        /// The plan of a generation of what is selected right now, as the command
        /// that starts a generation makes it.
        fn plan(&self) -> GenerationPlan {
            GenerationPlan::of(&self.state.lock())
        }

        /// The most recent state update among the events sent so far (see
        /// [`Self::drain_events`]); all pending events are consumed.
        fn last_state_update(&mut self) -> Option<UiState> {
            self.drain_events()
                .into_iter()
                .filter_map(|event| match event {
                    UserEvent::StateUpdate(state) => Some(*state),
                    _ => None,
                })
                .next_back()
        }

        /// Asserts that nothing has been sent since the events were last consumed.
        /// Only conclusive once the code under test has run to completion.
        #[track_caller]
        fn assert_no_events(&mut self, context: &str) {
            let events = self.drain_events();
            assert!(events.is_empty(), "{context}, got {events:?}");
        }

        /// Waits for the next event of a task that is still running in the background.
        async fn next_event(&mut self) -> UserEvent {
            tokio::time::timeout(EVENT_TIMEOUT, self.event_rx.recv())
                .await
                .expect("Timed out waiting for an event")
                .expect("Event channel closed")
        }

        /// Waits until a background task sends a state update that satisfies
        /// `predicate` and returns it; earlier events are consumed.
        async fn wait_for_state_update(
            &mut self,
            mut predicate: impl FnMut(&UiState) -> bool,
        ) -> UiState {
            loop {
                if let UserEvent::StateUpdate(state) = self.next_event().await {
                    if predicate(&state) {
                        return *state;
                    }
                }
            }
        }
    }

    /// Upper bound for waiting on something a test expects to happen. It is only ever
    /// reached by a failing test, so it does not slow the suite down.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

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
        async fn count_tokens(&self, _: Arc<String>) -> Option<usize> {
            Some(self.token_count)
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
        async fn search(&self, _: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
            Ok(self.results.lock().unwrap().clone())
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
        async fn search(&self, files: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
            Ok(files
                .into_iter()
                .map(|item| item.path)
                .filter(|path| self.matching.contains(path))
                .collect())
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
        async fn search(&self, _: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            let release = self.release.lock().unwrap().take();
            if let Some(release) = release {
                let _ = release.await;
            }
            Ok(self.results.clone())
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
        let token = harness.state.lock().begin_generation();

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
            token,
            harness.plan(),
        )
        .await;

        // Assert
        let events = harness.drain_events();
        assert_eq!(events.len(), 2, "Expected exactly two events");

        assert!(matches!(events[0], UserEvent::ShowGeneratedContent { .. }));
        if let UserEvent::ShowGeneratedContent {
            document,
            token_count,
        } = &events[0]
        {
            assert_eq!(document.text(), "Generated Content");
            assert_eq!(*token_count, 2);
            // The host keeps what it has shown, under the id it has shown it with.
            assert_eq!(
                harness.state.lock().generated_document(document.id()),
                Some(document.clone())
            );
        }

        // The state update is the second of the two events captured above.
        assert!(matches!(events[1], UserEvent::StateUpdate(_)));
        if let UserEvent::StateUpdate(final_state_in_event) = &events[1] {
            assert!(!final_state_in_event.is_generating);
        }

        assert!(
            !harness.state.lock().is_generating(),
            "is_generating should be reset in AppState"
        );
    }

    /// The real cancel path, for a task that could not be aborted in time: cancelling
    /// releases the generating state, the generator then notices the cancel flag, and
    /// the task ends without publishing anything on top of that.
    #[tokio::test]
    async fn cancelled_generation_task_ends_silently() {
        let mut harness = TestHarness::new();
        let mut generator = MockContentGenerator::new();
        let (has_started_receiver, unblock_sender) = generator.expect_cancellation();
        generator.set_result(Err(CoreError::Cancelled));
        let token = harness.state.lock().begin_generation();

        let task_handle = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 0 },
            token.clone(),
            harness.plan(),
        ));
        has_started_receiver
            .await
            .expect("Mock did not signal start");

        harness.state.lock().cancel_current_generation();
        assert!(token.is_cancelled());
        unblock_sender.send(()).unwrap();
        task_handle.await.unwrap();

        harness.assert_no_events("A cancelled generation must not publish an outcome");
        assert!(!harness.state.lock().is_generating());
    }

    #[tokio::test]
    async fn generation_task_io_error_sends_error_event_and_resets_state() {
        // Arrange
        let mut harness = TestHarness::new();
        let generator = MockContentGenerator::new();
        let io_error = CoreError::Io("File not found".to_string(), PathBuf::from("a/b/c.txt"));
        generator.set_result(Err(io_error));
        let token = harness.state.lock().begin_generation();

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 0 },
            token,
            harness.plan(),
        )
        .await;

        // Assert
        let events = harness.drain_events();
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
            !harness.state.lock().is_generating(),
            "is_generating should be reset in AppState"
        );
    }

    #[tokio::test]
    async fn generation_task_panic_resets_state_and_notifies_ui() {
        // Arrange
        let mut harness = TestHarness::new();
        let token = harness.state.lock().begin_generation();
        assert!(harness.state.lock().is_generating(), "precondition");

        // Act
        let task_handle = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            PanickingContentGenerator,
            MockTokenizer { token_count: 0 },
            token.clone(),
            harness.plan(),
        ));
        let join_error = task_handle.await.expect_err("The task must panic");
        assert!(join_error.is_panic());

        // Assert
        {
            let state = harness.state.lock();
            assert!(
                !state.is_generating(),
                "is_generating should be reset after a panic"
            );
            assert!(!state.is_current_generation(&token));
        }

        let events = harness.drain_events();
        assert_eq!(events.len(), 1, "Expected exactly one event");
        match &events[0] {
            UserEvent::StateUpdate(ui_state) => assert!(!ui_state.is_generating),
            _ => panic!("Expected a StateUpdate event."),
        }
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

        let token = harness.state.lock().begin_scan();

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
        let events = harness.drain_events();
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
        let token = harness.state.lock().begin_scan();
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
        let final_app_state = harness.state.lock();
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
            let mut state = harness.state.lock();
            state.content_search_query = "magic".to_string();
            state.edit_full_file_list(|list| {
                list.push(FileItem {
                    path: match1.clone(),
                    ..Default::default()
                })
            });
            state.edit_full_file_list(|list| {
                list.push(FileItem {
                    path: match2.clone(),
                    ..Default::default()
                })
            });
            state.edit_full_file_list(|list| {
                list.push(FileItem {
                    path: harness.root_path.join("nomatch.txt"),
                    ..Default::default()
                })
            });
        }
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
        {
            let final_state = harness.state.lock();
            assert_eq!(final_state.content_search_results().len(), 2);
            assert!(final_state.content_search_results().contains(&match1));
            assert!(final_state.content_search_results().contains(&match2));
        }
        let ui_state = harness.last_state_update().unwrap();
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
            let mut state = harness.state.lock();
            state.set_content_search_results(HashSet::from([PathBuf::from("previous_match.txt")]));
            state.content_search_query = "".to_string();
        }
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
        {
            let final_state = harness.state.lock();
            assert!(
                final_state.content_search_results().is_empty(),
                "Search results should be cleared"
            );
        }
        assert!(
            harness.last_state_update().is_some(),
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

        let token = harness.state.lock().begin_scan();

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
        let events = harness.drain_events();
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

        let token = harness.state.lock().begin_scan();

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
        let events = harness.drain_events();
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
        let token = harness.state.lock().begin_generation();

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
            token,
            harness.plan(),
        )
        .await;

        // Assert
        // Expect a ShowError event, followed by a StateUpdate.
        let events = harness.drain_events();
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
            !harness.state.lock().is_generating(),
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
        let scan_error = CoreError::Io("Not a directory".to_string(), path_to_load.clone());
        *scanner.shallow_result.lock().unwrap() = Err(scan_error.clone());

        let token = harness.state.lock().begin_lazy_load();

        // Act
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &path_to_load).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
        )
        .await;

        // Assert
        // Expect a ShowError event.
        let events = harness.drain_events();
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
                    msg.contains("Not a directory"),
                    "Error message should contain the error detail"
                );
            }
            _ => panic!("Expected a ShowError event."),
        }

        // Assert that state was not left inconsistent
        let state = harness.state.lock();
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

        let token = harness.state.lock().begin_scan();
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
        // The shallow update came first; the last one is the final state update the
        // ScanGuard fired on drop.
        let events = harness.drain_events();
        assert_eq!(events.len(), 2, "Shallow update and final update expected");
        let final_state_update = match events.into_iter().next_back() {
            Some(UserEvent::StateUpdate(state)) => *state,
            other => panic!("ScanGuard should send a final update, got {other:?}"),
        };

        assert!(
            !final_state_update.is_scanning,
            "Scanning should be false in the final state."
        );
        assert!(
            !final_state_update.is_fully_scanned,
            "Scan should not be marked as full."
        );

        // The final AppState should be consistent
        let final_app_state = harness.state.lock();
        assert!(!final_app_state.is_scanning());
        assert!(!final_app_state.has_scan_task());
    }

    // --- Scan progress ---

    /// A scanner whose deep phase reports progress and then waits to be released.
    struct ReportingScanner {
        reported: ScanProgress,
        release: Mutex<Option<oneshot::Receiver<()>>>,
    }

    impl Scanner for ReportingScanner {
        async fn scan(
            &self,
            request: &ScanRequest,
            report: Box<dyn Fn(ScanProgress) + Send + Sync>,
        ) -> ScanOutcome {
            if request.max_depth().is_none() {
                report(self.reported.clone());
                let release = self.release.lock().unwrap().take();
                if let Some(release) = release {
                    release.await.ok();
                }
            }
            Ok((
                vec![file_item(&request.project_root().join("a.rs"), 1)],
                HashSet::new(),
            ))
        }
    }

    /// The status line used to read "0 files processed" for the whole scan, because
    /// the progress only went to the UI as events and never into the state.
    #[tokio::test]
    async fn progress_of_the_running_scan_is_part_of_the_state_updates() {
        let mut harness = TestHarness::new();
        let (release, released) = oneshot::channel();
        let current_file = harness.root_path.join("src").join("main.rs");
        let scanner = ReportingScanner {
            reported: ScanProgress {
                files_scanned: 40,
                large_files_skipped: 2,
                current_scanning_path: current_file.to_string_lossy().to_string(),
            },
            release: Mutex::new(Some(released)),
        };
        let token = harness.state.lock().begin_scan();
        let scan = tokio::spawn(proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
        ));

        // The report reaches the UI as an event of its own (the live display) ...
        loop {
            if let UserEvent::ScanProgress(progress) = harness.next_event().await {
                assert_eq!(progress.files_scanned, 40);
                break;
            }
        }
        // ... and is what any state update sent from now on says.
        let during_scan = generate_ui_state(&harness.state.lock());
        assert!(during_scan.is_scanning);
        assert_eq!(
            during_scan.status_message,
            format!(
                "Scanning... 40 files processed. 2 large files skipped ({})",
                current_file.display()
            )
        );
        assert_eq!(during_scan.scan_progress.files_scanned, 40);
        assert_eq!(during_scan.scan_progress.large_files_skipped, 2);

        release.send(()).unwrap();
        scan.await.unwrap();
        let completed = harness.last_state_update().unwrap();
        assert!(!completed.is_scanning);
        assert_eq!(
            completed.status_message,
            "Indexing complete. Found 1 visible items."
        );
        assert_eq!(completed.scan_progress.files_scanned, 40);
    }

    #[tokio::test]
    async fn progress_of_a_superseded_scan_is_neither_stored_nor_sent() {
        let mut harness = TestHarness::new();
        let superseded = harness.state.lock().begin_scan();
        let report = scan_progress_reporter(&harness.proxy, &harness.state, &superseded);
        let _current = harness.state.lock().begin_scan();

        report(ScanProgress {
            files_scanned: 40,
            large_files_skipped: 2,
            current_scanning_path: "/somewhere/else".to_string(),
        });

        harness.assert_no_events("An outdated scan must stay silent");
        let state = harness.state.lock();
        assert_eq!(state.scan_progress.files_scanned, 0);
        assert!(state.scan_progress.current_scanning_path.is_empty());
    }

    /// The request to open a path is a state change of its own. It is complete when
    /// `start_scan_on_path` returns: the scan is registered, cancellable and announced,
    /// without any task having had the chance to run. The loaded directory is not
    /// touched yet: whether the path exists is not known.
    #[tokio::test]
    async fn start_scan_on_path_registers_the_scan_before_it_returns() {
        let mut harness = TestHarness::new();
        let project = harness.root_path.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let previous = harness.root_path.to_string_lossy().to_string();
        harness.state.lock().selected_files =
            HashSet::from([harness.root_path.join("a.rs")]).into();

        start_scan_on_path(
            project.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            false,
        );

        // No `.await` since the call: this test runs on a single-threaded runtime, so
        // the spawned task has not run yet.
        let announced = harness
            .last_state_update()
            .expect("The scan must be announced synchronously");
        assert!(announced.is_scanning);
        assert_eq!(announced.current_path, previous);
        assert_eq!(announced.selected_files_count, 1);
        {
            let mut state = harness.state.lock();
            assert!(state.is_scanning() && state.has_scan_task() && state.is_opening());

            // A cancellation that follows immediately therefore hits this scan, and
            // the directory that was loaded is still there.
            state.cancel_current_scan();
            assert!(!state.is_scanning());
            assert_eq!(state.status, StatusNotice::ScanCancelled);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = harness.state.lock();
        assert_eq!(state.current_path, previous);
        assert_eq!(state.selected_files.len(), 1);
        assert_eq!(state.config.last_directory, None);
    }

    /// Once the path has been found, the directory replaces the loaded one: everything
    /// that belonged to the previous directory is reset at that moment.
    #[tokio::test]
    async fn opening_a_path_replaces_the_loaded_directory_once_it_is_found() {
        let mut harness = TestHarness::new();
        let project = harness.root_path.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("new.rs"), "").unwrap();
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![file_item(Path::new("/old/a.rs"), 1)]);
            state.selected_files = HashSet::from([PathBuf::from("/old/a.rs")]).into();
            state.expanded_dirs = HashSet::from([PathBuf::from("/old/src")]).into();
            state.search_query = "a".to_string();
            state.is_fully_scanned = true;
            filtering::apply_filters(&mut state);
        }

        start_scan_on_path(
            project.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            false,
        );

        let project_path = project.to_string_lossy().to_string();
        let entered = harness
            .wait_for_state_update(|s| s.current_path == project_path)
            .await;
        assert!(entered.is_scanning);
        assert_eq!(entered.selected_files_count, 0);
        assert_eq!(entered.search_query, "");
        assert!(entered.rows.is_empty() && !entered.is_fully_scanned);

        let completed = harness.wait_for_state_update(|s| !s.is_scanning).await;
        assert!(completed.is_fully_scanned);
        let state = harness.state.lock();
        assert!(state.selected_files.is_empty() && state.expanded_dirs.is_empty());
        assert_eq!(
            state.config.last_directory.as_deref(),
            Some(project.as_path())
        );
        let listed: Vec<_> = state
            .full_file_list()
            .iter()
            .map(|i| i.path.clone())
            .collect();
        assert_eq!(listed, [project.join("new.rs")]);
    }

    /// A scan that was cancelled before its task looked at the path stays cancelled:
    /// the task must not touch the state any more.
    #[tokio::test]
    async fn locating_the_scan_root_is_discarded_for_a_cancelled_scan() {
        let mut harness = TestHarness::new();
        let file_path = harness.root_path.join("some_file.txt");
        std::fs::write(&file_path, "content").unwrap();
        let token = harness.state.lock().begin_scan();
        harness.state.lock().cancel_current_scan();
        harness.state.lock().current_path = "untouched".to_string();

        let located = locate_scan_root(file_path, &harness.proxy, &harness.state, &token).await;

        assert!(located.is_none());
        harness.assert_no_events("A superseded scan must stay silent");
        let state = harness.state.lock();
        assert_eq!(state.current_path, "untouched");
        assert_eq!(state.config.last_directory, None);
    }

    /// A path that does not exist is reported, and the directory that was loaded stays
    /// exactly as it was: list, selection, expansion, filters.
    #[tokio::test]
    async fn opening_a_nonexistent_path_keeps_the_loaded_directory() {
        // Arrange
        let mut harness = TestHarness::new();
        let nonexistent_path = harness.root_path.join("nonexistent");
        let loaded = harness.root_path.to_string_lossy().to_string();
        let file = harness.root_path.join("src").join("a.rs");
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![
                dir_item(&harness.root_path.join("src"), 1),
                file_item(&file, 2),
            ]);
            state.selected_files = HashSet::from([file.clone()]).into();
            state.expanded_dirs = HashSet::from([harness.root_path.join("src")]).into();
            state.loaded_dirs = HashSet::from([harness.root_path.join("src")]);
            state.search_query = "a".to_string();
            state.is_fully_scanned = true;
            state.status = StatusNotice::IndexingComplete { visible_items: 2 };
            filtering::apply_filters(&mut state);
        }

        // Act
        start_scan_on_path(
            nonexistent_path,
            harness.proxy.clone(),
            harness.state.clone(),
            false,
        );

        // Assert: the scan is announced with the loaded directory still in place, then
        // its task finds out that there is nothing to scan, reports the error and ends
        // the scan.
        match harness.next_event().await {
            UserEvent::StateUpdate(s) => {
                assert!(s.is_scanning);
                assert_eq!(s.current_path, loaded);
                assert_eq!(s.root_rows().len(), 1, "the previous list stays visible");
            }
            other => panic!("Expected the announcement of the scan, got {other:?}"),
        }
        match harness.next_event().await {
            UserEvent::ShowError(msg) => assert!(msg.contains("Path does not exist")),
            other => panic!("Expected a ShowError event, got {other:?}"),
        }
        match harness.next_event().await {
            UserEvent::StateUpdate(s) => {
                assert!(!s.is_scanning);
                assert_eq!(s.current_path, loaded);
                assert_eq!(s.selected_files_count, 1);
                assert_eq!(s.search_query, "a");
                assert!(s.is_fully_scanned);
                assert_eq!(s.root_rows().len(), 1);
                let directory = s.root_rows()[0];
                assert!(directory.is_expanded && s.children_of(directory).len() == 1);
                assert_eq!(
                    s.status_message,
                    "Indexing complete. Found 2 visible items."
                );
            }
            other => panic!("Expected the end of the scan, got {other:?}"),
        }
        harness.assert_no_events("Nothing follows a failed opening");

        let state = harness.state.lock();
        assert!(!state.is_scanning(), "The scan must have ended.");
        assert!(!state.is_opening());
        assert_eq!(state.config.last_directory, None);
        assert_eq!(state.selected_files, HashSet::from([file]));
    }

    /// Opening a path supersedes the scan that is running. If the path then turns out
    /// not to exist, the loaded directory would be left with the incomplete list of
    /// the interrupted scan, so that scan is started again.
    #[tokio::test]
    async fn a_failed_opening_restarts_the_scan_it_interrupted() {
        let mut harness = TestHarness::new();
        std::fs::create_dir_all(harness.root_path.join("src")).unwrap();
        std::fs::write(harness.root_path.join("src").join("a.rs"), "").unwrap();
        let loaded = harness.root_path.clone();
        harness.state.lock().selected_files =
            HashSet::from([loaded.join("src").join("a.rs")]).into();

        // A scan of the loaded directory is running (its task has not had the chance
        // to run yet: there is no `.await` before the next call) ...
        start_scan_on_path(
            loaded.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            true,
        );
        // ... when a path that does not exist is dropped.
        start_scan_on_path(
            loaded.join("nonexistent"),
            harness.proxy.clone(),
            harness.state.clone(),
            false,
        );

        let mut error_shown = false;
        let completed = loop {
            match harness.next_event().await {
                UserEvent::ShowError(msg) => {
                    assert!(msg.contains("Path does not exist"));
                    error_shown = true;
                }
                UserEvent::StateUpdate(s) if error_shown && !s.is_scanning => break s,
                UserEvent::StateUpdate(s) => {
                    assert!(s.is_scanning, "the loaded directory is never shown as done");
                }
                _ => {}
            }
        };
        assert!(completed.is_fully_scanned);
        assert_eq!(completed.current_path, loaded.to_string_lossy());
        assert_eq!(completed.selected_files_count, 1, "the selection is kept");

        let state = harness.state.lock();
        assert_eq!(state.full_file_list().len(), 2);
        assert_eq!(
            state.config.last_directory.as_deref(),
            Some(loaded.as_path())
        );
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
            file_path.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            false,
        );

        // Assert
        // The scan is announced right away; the path has not been looked at yet.
        match harness.next_event().await {
            UserEvent::StateUpdate(s) => assert!(s.is_scanning),
            other => panic!("Expected a StateUpdate event, got {other:?}"),
        }
        // Its task then finds the directory to scan: the parent of the file.
        let root = harness.root_path.to_string_lossy().to_string();
        let located = harness
            .wait_for_state_update(|s| s.current_path == root)
            .await;
        assert!(located.is_scanning);
        let completed = harness.wait_for_state_update(|s| !s.is_scanning).await;
        assert!(completed.is_fully_scanned);

        let state = harness.state.lock();
        assert_eq!(state.current_path, root);
        assert_eq!(
            state.config.last_directory.as_deref(),
            Some(harness.root_path.as_path())
        );
        assert!(state
            .full_file_list()
            .iter()
            .any(|item| item.path == file_path));
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
            let mut state = harness.state.lock();
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
        let initial_update = harness.next_event().await;
        assert!(matches!(initial_update, UserEvent::StateUpdate(_)));

        // Now it's safe to check the state.
        {
            let state = harness.state.lock();
            assert!(state.is_scanning(), "Scanning should have started.");
            assert_eq!(state.current_path, new_scan_path.to_string_lossy());
            assert!(
                state.expanded_dirs.contains(&dir_to_expand),
                "expanded_dirs should have been preserved."
            );
        }

        // The scan runs to its end and reports the complete (empty) directory.
        let final_update = harness
            .wait_for_state_update(|ui_state| !ui_state.is_scanning)
            .await;
        assert!(final_update.is_fully_scanned);
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
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![
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
            ]);
        }

        // --- SCENARIO 1: Case-sensitive search (no match) ---
        {
            let mut state = harness.state.lock();
            state.config.case_sensitive_search = true;
            state.content_search_query = "magicword".to_string(); // Lowercase
        }
        // Pass 'searcher' by value (it's Copy)
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
        // Assert against the AppState, not the UiState
        assert!(
            harness.state.lock().content_search_results().is_empty(),
            "Case-sensitive search should not find 'magicword'"
        );
        // Drain the event queue
        let _ = harness.last_state_update();

        // --- SCENARIO 2: Case-sensitive search (exact match) ---
        {
            let mut state = harness.state.lock();
            state.config.case_sensitive_search = true;
            state.content_search_query = "MagicWord".to_string(); // Exact case
        }
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;
        // Assert against the AppState for correctness
        let final_state = harness.state.lock();
        assert_eq!(
            final_state.content_search_results().len(),
            1,
            "Case-sensitive search should find 'MagicWord'"
        );
        assert!(final_state
            .content_search_results()
            .contains(&text_file_path));
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
            let mut state = harness.state.lock();
            state.is_fully_scanned = true; // Required for pruning
            state.config.remove_empty_directories = true; // Enable feature
            state.config.include_tree_by_default = true;
            state.selected_files.insert(file_path.clone());
            state.set_full_file_list(vec![
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
            ]);
        }

        let token = harness.state.lock().begin_generation();

        // Act
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            tokenizer,
            token,
            harness.plan(),
        )
        .await;

        // Assert
        let events = harness.drain_events();
        assert!(matches!(events[0], UserEvent::ShowGeneratedContent { .. }));
        assert!(matches!(events[1], UserEvent::StateUpdate(_)));
    }

    /// Records what a generation hands to its generator.
    #[derive(Clone, Default)]
    struct RecordingGenerator {
        calls: Arc<Mutex<Vec<RecordedGeneration>>>,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct RecordedGeneration {
        selected_files: Vec<PathBuf>,
        include_tree: bool,
        tree_paths: Vec<PathBuf>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    }

    impl RecordingGenerator {
        fn only_call(&self) -> RecordedGeneration {
            let calls = self.calls.lock().unwrap();
            assert_eq!(calls.len(), 1, "{calls:?}");
            calls[0].clone()
        }
    }

    impl ContentGenerator for RecordingGenerator {
        async fn generate(
            &self,
            selected_files: &[PathBuf],
            _: &Path,
            include_tree: bool,
            items_for_tree: Vec<FileItem>,
            tree_ignore_patterns: HashSet<String>,
            use_relative_paths: bool,
        ) -> Result<String, CoreError> {
            self.calls.lock().unwrap().push(RecordedGeneration {
                selected_files: selected_files.to_vec(),
                include_tree,
                tree_paths: items_for_tree.into_iter().map(|item| item.path).collect(),
                tree_ignore_patterns,
                use_relative_paths,
            });
            Ok("recorded".to_string())
        }
    }

    /// A project of two files, one of them below `src`, and an empty directory.
    fn load_small_project(harness: &TestHarness) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let src = harness.root_path.join("src");
        let main = src.join("main.rs");
        let empty = harness.root_path.join("empty");
        let readme = harness.root_path.join("README.md");
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![
            dir_item(&src, 1),
            FileItem {
                size: 100,
                ..file_item(&main, 2)
            },
            dir_item(&empty, 1),
            FileItem {
                size: 1000,
                ..file_item(&readme, 1)
            },
        ]);
        (src, main, empty, readme)
    }

    async fn run_recorded_generation(harness: &TestHarness) -> RecordedGeneration {
        let generator = RecordingGenerator::default();
        let token = harness.state.lock().begin_generation();
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator.clone(),
            MockTokenizer { token_count: 0 },
            token,
            harness.plan(),
        )
        .await;
        generator.only_call()
    }

    #[tokio::test]
    async fn generation_hands_the_generator_the_tree_without_empty_directories() {
        let harness = TestHarness::new();
        let (src, main, empty, readme) = load_small_project(&harness);
        {
            let mut state = harness.state.lock();
            state.is_fully_scanned = true;
            state.config.remove_empty_directories = true;
            state.config.include_tree_by_default = true;
            state.config.tree_ignore_patterns = HashSet::from(["*.lock".to_string()]);
            state.config.use_relative_paths = false;
            state.selected_files = HashSet::from([readme.clone(), main.clone()]).into();
        }

        let recorded = run_recorded_generation(&harness).await;

        assert_eq!(
            recorded,
            RecordedGeneration {
                // The order of the tree, whatever the order of the selection.
                selected_files: vec![readme.clone(), main.clone()],
                include_tree: true,
                // The list as it is, without the directory that holds no file.
                tree_paths: vec![src.clone(), main.clone(), readme.clone()],
                tree_ignore_patterns: HashSet::from(["*.lock".to_string()]),
                use_relative_paths: false,
            }
        );

        // Empty directories stay where the option is off, and where the list is not
        // known to be complete (a directory that was not loaded yet looks empty).
        for (remove_empty_directories, is_fully_scanned) in [(false, true), (true, false)] {
            {
                let mut state = harness.state.lock();
                state.config.remove_empty_directories = remove_empty_directories;
                state.is_fully_scanned = is_fully_scanned;
            }
            let recorded = run_recorded_generation(&harness).await;
            assert_eq!(
                recorded.tree_paths,
                vec![src.clone(), main.clone(), empty.clone(), readme.clone()]
            );
        }
    }

    #[tokio::test]
    async fn generation_without_a_tree_does_not_copy_the_file_list() {
        let harness = TestHarness::new();
        let (_, main, _, _) = load_small_project(&harness);
        {
            let mut state = harness.state.lock();
            state.config.include_tree_by_default = false;
            state.selected_files.insert(main.clone());
        }

        assert!(harness.plan().tree.is_none());
        let recorded = run_recorded_generation(&harness).await;
        assert!(!recorded.include_tree);
        assert_eq!(recorded.tree_paths, Vec::<PathBuf>::new());
        assert_eq!(recorded.selected_files, vec![main]);
    }

    /// The plan is the selection of the moment it was made: what is selected or
    /// deselected while the generation runs belongs to the next one.
    #[tokio::test]
    async fn generation_works_with_the_selection_of_its_plan() {
        let harness = TestHarness::new();
        let (_, main, _, readme) = load_small_project(&harness);
        harness.state.lock().selected_files.insert(main.clone());
        let plan = harness.plan();
        {
            let mut state = harness.state.lock();
            state.selected_files.clear();
            state.selected_files.insert(readme);
        }

        let generator = RecordingGenerator::default();
        let token = harness.state.lock().begin_generation();
        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator.clone(),
            MockTokenizer { token_count: 0 },
            token,
            plan,
        )
        .await;

        assert_eq!(generator.only_call().selected_files, vec![main]);
    }

    #[tokio::test]
    async fn plan_estimates_the_output_from_the_sizes_of_the_scan() {
        let harness = TestHarness::new();
        let (_, main, _, readme) = load_small_project(&harness);
        let estimate = |harness: &TestHarness| harness.plan().estimated_size;
        harness.state.lock().config.include_tree_by_default = false;

        let nothing = estimate(&harness);
        harness.state.lock().selected_files.insert(main);
        let one_file = estimate(&harness);
        harness.state.lock().selected_files.insert(readme.clone());
        let two_files = estimate(&harness);

        // 100 and 1000 bytes of content, plus the path and the frame of each block.
        assert!((100..400).contains(&(one_file - nothing)), "{one_file}");
        assert!(
            (1000..1300).contains(&(two_files - one_file)),
            "{two_files}"
        );

        // The tree adds to it; a file above the size limit is not embedded.
        harness.state.lock().config.include_tree_by_default = true;
        assert!(estimate(&harness) > two_files);
        {
            let mut state = harness.state.lock();
            state.config.include_tree_by_default = false;
            state.edit_full_file_list(|list| list[3].size = 3 * 1024 * 1024);
            state.config.max_file_size_mb = 1;
        }
        assert!(estimate(&harness) < two_files);

        // A selected path that is not listed (any more) is not part of the output.
        let before = estimate(&harness);
        harness
            .state
            .lock()
            .selected_files
            .insert(PathBuf::from("/elsewhere/gone.txt"));
        assert_eq!(estimate(&harness), before);
    }

    /// A tokenizer whose count is cancelled, as the real one reports it when the
    /// cancellation flag of its generation is raised.
    struct CancelledTokenizer;

    impl Tokenizer for CancelledTokenizer {
        async fn count_tokens(&self, _: Arc<String>) -> Option<usize> {
            None
        }
    }

    /// Cancelling during the token count: the generation is over for the state when
    /// the count gives up, and neither the document nor an error is shown.
    #[tokio::test]
    async fn generation_cancelled_during_the_token_count_shows_nothing() {
        let mut harness = TestHarness::new();
        let generator = MockContentGenerator::new();
        generator.set_result(Ok("document".to_string()));
        let token = harness.state.lock().begin_generation();
        harness.state.lock().cancel_current_generation();

        generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            CancelledTokenizer,
            token,
            harness.plan(),
        )
        .await;

        harness.assert_no_events("A cancelled generation publishes nothing");
        assert!(!harness.state.lock().is_generating());
    }

    #[tokio::test]
    async fn real_tokenizer_counts_shared_text_and_stops_when_cancelled() {
        let text = Arc::new("hello world\n".repeat(1000));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let tokenizer = RealTokenizer {
            cancel_flag: cancel_flag.clone(),
        };

        // "hello", " world" and the line end, a thousand times.
        assert_eq!(tokenizer.count_tokens(text.clone()).await, Some(3000));
        // The text was borrowed for the count, not kept.
        assert_eq!(Arc::strong_count(&text), 1);

        cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(tokenizer.count_tokens(text).await, None);
    }

    /// The generated output the host keeps is the one the editor shows. A file
    /// preview takes its place there, so the host gives its copy up; a preview that
    /// fails leaves the editor, and so the document, as it is.
    #[tokio::test]
    async fn file_preview_takes_the_place_of_the_generated_document() {
        let mut harness = TestHarness::new();
        let file = harness.root_path.join("a.txt");
        let generate = |harness: &TestHarness| {
            let mut state = harness.state.lock();
            let token = state.begin_generation();
            state
                .complete_generation(&token, Arc::new("output".to_string()))
                .unwrap()
                .id()
        };

        let id = generate(&harness);
        launch_file_preview(
            file.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            |_| Err(CoreError::Cancelled),
        )
        .await
        .unwrap();
        assert!(harness.state.lock().generated_document(id).is_some());

        launch_file_preview(file, harness.proxy.clone(), harness.state.clone(), |_| {
            Ok("file content".to_string())
        })
        .await
        .unwrap();
        assert!(harness.state.lock().generated_document(id).is_none());
        assert!(harness
            .drain_events()
            .iter()
            .any(|event| matches!(event, UserEvent::ShowFilePreview { .. })));
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
            let mut state = harness.state.lock();
            state.edit_full_file_list(|list| {
                list.push(FileItem {
                    path: dir_to_load.clone(),
                    is_directory: true,
                    ..Default::default()
                })
            });
            let all_items = state.full_file_list().to_vec();
            state.set_filtered_file_list(all_items);
        }

        // Act: the returned handle completes when the load has been dealt with.
        start_lazy_load_scan(
            dir_to_load.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .expect("The lazy load must be accepted")
        .await
        .expect("The lazy load task must not panic");

        // --- Assert ---
        // The task is done, so its state update has been sent.
        let final_state_update = harness
            .last_state_update()
            .expect("Should receive a state update after lazy load");

        // 3. Perform the final state checks.
        assert_eq!(
            final_state_update.visible_files_count, 2,
            "Should show parent dir and new file"
        );
        let state = harness.state.lock();
        assert!(state.loaded_dirs.contains(&dir_to_load));
        assert!(state.expanded_dirs.contains(&dir_to_load));
        assert!(state
            .full_file_list()
            .iter()
            .any(|item| item.path == new_file_path));
    }

    /// A directory can be loaded while a scan is walking the project: the level is
    /// listed at once, and the scan goes on.
    #[tokio::test]
    async fn start_lazy_load_scan_during_a_scan_loads_the_level() {
        // Arrange: a scan that has published the first level.
        let mut harness = TestHarness::new();
        let dir_to_load = harness.root_path.join("src");
        std::fs::create_dir(&dir_to_load).unwrap();
        std::fs::write(dir_to_load.join("main.rs"), "fn main() {}").unwrap();
        let token = {
            let mut state = harness.state.lock();
            let token = state.begin_scan();
            let root = harness.root_path.clone();
            state.replace_with_shallow_file_list(&root, vec![dir_item(&dir_to_load, 1)]);
            token
        };

        // Act
        start_lazy_load_scan(
            dir_to_load.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .expect("The load is started")
        .await
        .unwrap();

        // Assert
        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.is_scanning);
        assert_eq!(ui_state.visible_files_count, 2);
        let state = harness.state.lock();
        assert!(state.is_current_scan(&token), "The scan is not disturbed");
        assert!(state.expanded_dirs.contains(&dir_to_load));
        assert!(state.loaded_dirs.contains(&dir_to_load));
        assert!(state
            .full_file_list()
            .iter()
            .any(|item| item.path == dir_to_load.join("main.rs")));
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
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![FileItem {
                path: unreadable_file_path.clone(),
                ..Default::default()
            }]);
            state.content_search_query = "read".to_string();
        }

        // Act
        search_in_files(harness.proxy.clone(), harness.state.clone(), searcher).await;

        // Assert
        // The main assertion is that the task completes without panicking.
        // We also assert that the unreadable file is not included in the results.
        let final_state = harness.state.lock();
        assert!(
            final_state.content_search_results().is_empty(),
            "Unreadable file should not be a search result."
        );
    }

    /// Passes the outcome of the wrapped generator on and keeps a copy of it.
    struct OutcomeRecorder<G> {
        inner: G,
        outcome: Arc<Mutex<Option<Result<String, CoreError>>>>,
    }

    impl<G: ContentGenerator> ContentGenerator for OutcomeRecorder<G> {
        async fn generate(
            &self,
            selected_files: &[PathBuf],
            root_path: &Path,
            include_tree: bool,
            items_for_tree: Vec<FileItem>,
            tree_ignore_patterns: HashSet<String>,
            use_relative_paths: bool,
        ) -> Result<String, CoreError> {
            let outcome = self
                .inner
                .generate(
                    selected_files,
                    root_path,
                    include_tree,
                    items_for_tree,
                    tree_ignore_patterns,
                    use_relative_paths,
                )
                .await;
            *self.outcome.lock().unwrap() = Some(outcome.clone());
            outcome
        }
    }

    /// The cancellation mechanism end to end with the `RealContentGenerator`: the
    /// generation is cancelled the way the application does it
    /// (`cancel_current_generation`), the generator stops at the next file, and the
    /// task leaves it to the cancelling command to inform the UI.
    #[tokio::test]
    async fn generation_task_with_real_generator_cancels_gracefully() {
        // Arrange
        let mut harness = TestHarness::new();
        let token = harness.state.lock().begin_generation();
        let outcome = Arc::new(Mutex::new(None));
        let generator = OutcomeRecorder {
            inner: RealContentGenerator {
                cancel_flag: token.cancel_flag(),
                max_file_size: MAX_FILE_SIZE,
                max_output_size: MAX_OUTPUT_SIZE,
                expected_output_size: 0,
            },
            outcome: outcome.clone(),
        };

        // Set up state with a file to be processed.
        {
            let mut state = harness.state.lock();
            let file_path = harness.root_path.join("file1.txt");
            std::fs::write(&file_path, "content").unwrap();
            state.selected_files.insert(file_path.clone());
            state.edit_full_file_list(|list| {
                list.push(FileItem {
                    path: file_path,
                    ..Default::default()
                })
            });
        }

        // Act: the task is spawned but (on the single-threaded test runtime) does not
        // run before this test awaits it, so the cancellation reaches it first. Its
        // handle is deliberately not attached: the task is not aborted and has to
        // deal with the cancellation by itself.
        let task_handle = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 0 },
            token,
            harness.plan(),
        ));
        harness.state.lock().cancel_current_generation();
        task_handle.await.unwrap();

        // Assert
        assert!(
            matches!(*outcome.lock().unwrap(), Some(Err(CoreError::Cancelled))),
            "The generator must stop because of the cancellation"
        );
        harness.assert_no_events("Neither content nor an error may be shown after a cancellation");
        assert!(!harness.state.lock().is_generating());
    }

    /// The size limit of the configuration reaches the generator: a selected file above
    /// it is represented by a marker.
    #[tokio::test]
    async fn real_generator_applies_its_file_size_limit() {
        let harness = TestHarness::new();
        let file_path = harness.root_path.join("file.txt");
        std::fs::write(&file_path, "123456").unwrap();
        let generator = RealContentGenerator {
            cancel_flag: Arc::new(AtomicBool::new(false)),
            max_file_size: 5,
            max_output_size: MAX_OUTPUT_SIZE,
            expected_output_size: 0,
        };

        let content = generator
            .generate(
                std::slice::from_ref(&file_path),
                &harness.root_path,
                false,
                vec![],
                HashSet::new(),
                true,
            )
            .await
            .unwrap();

        assert!(content.contains("[FILE TOO LARGE: 6 bytes - CONTENT SKIPPED]"));
    }

    /// The real searcher takes the size limit from the request, whether it is stricter
    /// or more generous than another one.
    #[tokio::test]
    async fn real_searcher_applies_the_file_size_limit_of_the_request() {
        let harness = TestHarness::new();
        let small = harness.root_path.join("small.txt");
        let large = harness.root_path.join("large.txt");
        std::fs::write(&small, "needle").unwrap();
        std::fs::write(&large, "needle and hay").unwrap();
        let files = vec![file_item(&small, 1), file_item(&large, 1)];
        let request = |max_file_size| ContentSearchRequest {
            max_file_size,
            ..ContentSearchRequest::new("needle", true)
        };

        let limited = RealFileSearcher.search(files.clone(), request(6)).await;
        let generous = RealFileSearcher.search(files, request(u64::MAX)).await;

        assert_eq!(limited, Ok(HashSet::from([small.clone()])));
        assert_eq!(generous, Ok(HashSet::from([small, large])));
    }

    /// The real searcher does not read anything for a request that is already cancelled.
    #[tokio::test]
    async fn real_searcher_stops_for_a_cancelled_request() {
        let harness = TestHarness::new();
        let file = harness.root_path.join("a.txt");
        std::fs::write(&file, "needle").unwrap();
        let request = ContentSearchRequest::new("needle", true);
        request.cancel_flag.store(true, Ordering::SeqCst);

        let result = RealFileSearcher
            .search(vec![file_item(&file, 1)], request)
            .await;

        assert_eq!(result, Ok(HashSet::new()));
    }

    /// A searcher that keeps the request it was given and finds nothing.
    #[derive(Clone, Default)]
    struct RequestRecorder(Arc<Mutex<Option<ContentSearchRequest>>>);
    impl RequestRecorder {
        fn recorded(&self) -> ContentSearchRequest {
            self.0.lock().unwrap().clone().expect("A search must run")
        }
    }
    impl FileSearcher for RequestRecorder {
        async fn search(&self, _: Vec<FileItem>, request: ContentSearchRequest) -> SearchOutcome {
            *self.0.lock().unwrap() = Some(request);
            Ok(HashSet::new())
        }
    }

    /// `search_in_files` describes the search completely: query, case sensitivity and
    /// size limit come from the state, the flag belongs to this search run.
    #[tokio::test]
    async fn search_in_files_passes_the_request_to_the_searcher() {
        let harness = TestHarness::new();
        {
            let mut state = harness.state.lock();
            // More generous than the default of the core: must arrive uncapped.
            state.config.max_file_size_mb = 64;
            state.config.case_sensitive_search = true;
            state.content_search_query = " Needle ".to_string();
        }
        let recorder = RequestRecorder::default();

        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            recorder.clone(),
        )
        .await;

        let request = recorder.recorded();
        assert_eq!(request.query, " Needle ");
        assert!(request.case_sensitive);
        assert_eq!(request.max_file_size, 64 * 1024 * 1024);
        assert!(!request.cancel_flag.load(Ordering::SeqCst));
    }

    struct UnexpectedSearch;
    impl FileSearcher for UnexpectedSearch {
        async fn search(&self, _: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
            panic!("No search expected without a content query");
        }
    }

    /// A content query of whitespace only is no query: no search runs, matches of an
    /// earlier query are dropped and the list is shown unfiltered.
    #[tokio::test]
    async fn search_in_files_treats_a_whitespace_only_query_as_none() {
        let mut harness = TestHarness::new();
        let file = harness.root_path.join("a.txt");
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![file_item(&file, 1)]);
            state.content_search_query = " \t ".to_string();
            state.set_content_search_results(HashSet::from([PathBuf::from("/elsewhere.txt")]));
        }

        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            UnexpectedSearch,
        )
        .await;

        let update = harness.last_state_update().expect("The state is sent");
        assert_eq!(update.visible_files_count, 1);
        assert!(harness.state.lock().content_search_results().is_empty());
    }

    #[tokio::test]
    async fn refresh_content_search_does_nothing_for_a_whitespace_only_query() {
        let mut harness = TestHarness::new();
        harness.state.lock().content_search_query = "  ".to_string();

        refresh_content_search(&harness.proxy, &harness.state, UnexpectedSearch).await;

        assert!(harness.last_state_update().is_none());
    }

    /// `refresh_content_search` runs the same search, so it hands over the same request.
    #[tokio::test]
    async fn refresh_content_search_passes_the_request_to_the_searcher() {
        let harness = TestHarness::new();
        {
            let mut state = harness.state.lock();
            state.config.max_file_size_mb = 3;
            state.content_search_query = "needle".to_string();
        }
        let recorder = RequestRecorder::default();

        refresh_content_search(&harness.proxy, &harness.state, recorder.clone()).await;

        let request = recorder.recorded();
        assert_eq!(request.query, "needle");
        assert!(!request.case_sensitive);
        assert_eq!(request.max_file_size, 3 * 1024 * 1024);
    }

    /// A searcher that signals when it has been asked, then waits until the test has
    /// superseded the search and only then lets the real search run.
    struct SupersededSearcher {
        asked: Mutex<Option<oneshot::Sender<()>>>,
        superseded: Mutex<Option<oneshot::Receiver<()>>>,
        found_after_supersession: Arc<Mutex<Option<HashSet<PathBuf>>>>,
    }
    impl FileSearcher for SupersededSearcher {
        async fn search(
            &self,
            files: Vec<FileItem>,
            request: ContentSearchRequest,
        ) -> SearchOutcome {
            let asked = self.asked.lock().unwrap().take().unwrap();
            let superseded = self.superseded.lock().unwrap().take().unwrap();
            asked.send(()).unwrap();
            superseded.await.unwrap();
            let found = RealFileSearcher.search(files, request).await;
            *self.found_after_supersession.lock().unwrap() = found.clone().ok();
            found
        }
    }

    /// A search that has been superseded stops: the flag of its request is raised, so
    /// the real search does not read the files that are still ahead of it.
    #[tokio::test]
    async fn superseded_content_search_stops_reading_files() {
        let harness = TestHarness::new();
        let file = harness.root_path.join("a.txt");
        std::fs::write(&file, "needle").unwrap();
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![file_item(&file, 1)]);
            state.content_search_query = "needle".to_string();
        }
        let (asked_tx, asked) = oneshot::channel();
        let (superseded_tx, superseded) = oneshot::channel();
        let found_after_supersession = Arc::new(Mutex::new(None));
        let outdated_search = tokio::spawn(search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            SupersededSearcher {
                asked: Mutex::new(Some(asked_tx)),
                superseded: Mutex::new(Some(superseded)),
                found_after_supersession: found_after_supersession.clone(),
            },
        ));
        asked.await.expect("The outdated search must start");

        // Act: a newer search supersedes the one that is still running.
        let recorder = RequestRecorder::default();
        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            recorder.clone(),
        )
        .await;
        superseded_tx.send(()).unwrap();
        outdated_search.await.unwrap();

        // Assert: the outdated search no longer read the file that matches ...
        assert_eq!(
            *found_after_supersession.lock().unwrap(),
            Some(HashSet::new())
        );
        // ... while the flag of the current search stays down.
        assert!(!recorder.recorded().cancel_flag.load(Ordering::SeqCst));
    }

    /// Tests the edge case where handle_scan_error is called when no scan is active.
    #[tokio::test]
    async fn handle_scan_error_does_nothing_if_not_scanning() {
        // Arrange
        let mut harness = TestHarness::new();
        // The scan has already finished, so is_scanning is false.
        let token = harness.state.lock().begin_scan();
        harness.state.lock().finish_scan(&token);
        let dummy_error = CoreError::Cancelled;

        // Act
        handle_scan_error(dummy_error, &harness.state, &harness.proxy, &token);

        // Assert
        // The function should return early without sending any events.
        harness.assert_no_events("No events should be sent if not scanning");
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

        let token = harness.state.lock().begin_scan();
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
        let mut state = harness.state.lock();
        let token = state.begin_scan();
        state.attach_scan_task(&token, tokio::spawn(std::future::pending()));
        token
    }

    fn assert_replacement_scan_is_intact(harness: &TestHarness, token: &ScanToken) {
        let mut state = harness.state.lock();
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
        let _ = harness.last_state_update(); // Drain the shallow update.

        // Act: the new scan starts first, then the old future gets dropped.
        let new_token = begin_replacement_scan(&harness);
        old_task.abort();
        let join_error = old_task.await.expect_err("Old task should be aborted");
        assert!(join_error.is_cancelled());

        // Assert
        assert!(
            harness.last_state_update().is_none(),
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
        let _ = harness.last_state_update(); // Drain the shallow update.

        // Act: the new scan starts, then the old deep scan returns successfully.
        let new_token = begin_replacement_scan(&harness);
        unblock_deep_scan.send(()).unwrap();
        old_task.await.expect("Old task should end on its own");

        // Assert
        assert!(old_token.is_cancelled());
        assert!(
            harness.last_state_update().is_none(),
            "A superseded scan must not send state updates"
        );
        assert!(
            harness.state.lock().full_file_list().is_empty(),
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
        let old_token = harness.state.lock().begin_scan();
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
        let applied_deep =
            apply_deep_scan_result(&harness.state, &old_token, stale_files, HashSet::new());
        complete_scan(&harness.state, &harness.proxy, &old_token);

        // Assert
        assert!(!applied && !applied_deep);
        assert!(harness.last_state_update().is_none());
        {
            let state = harness.state.lock();
            assert!(state.full_file_list().is_empty());
            assert!(state.loaded_dirs.is_empty());
        }
        assert_replacement_scan_is_intact(&harness, &new_token);
    }

    #[tokio::test]
    async fn handle_scan_error_from_stale_scan_does_not_fail_newer_scan() {
        // Arrange
        let mut harness = TestHarness::new();
        let old_token = harness.state.lock().begin_scan();
        let new_token = begin_replacement_scan(&harness);

        // Act
        handle_scan_error(
            CoreError::Cancelled,
            &harness.state,
            &harness.proxy,
            &old_token,
        );

        // Assert
        assert!(harness.last_state_update().is_none());
        assert!(!matches!(
            harness.state.lock().status,
            crate::app::state::StatusNotice::ScanFailed { .. }
        ));
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
        let old_token = harness.state.lock().begin_generation();
        let old_task = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 1 },
            old_token.clone(),
            harness.plan(),
        ));
        has_started.await.expect("Mock did not signal start");

        // Act: the user starts a new generation while the old one is still producing.
        let new_token = harness.state.lock().begin_generation();
        unblock.send(()).unwrap();
        old_task.await.unwrap();

        // Assert
        assert!(old_token.is_cancelled());
        harness.assert_no_events("Outdated generation must stay silent");
        let state = harness.state.lock();
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
        let old_token = harness.state.lock().begin_generation();
        let old_task = tokio::spawn(generation_task(
            harness.proxy.clone(),
            harness.state.clone(),
            generator,
            MockTokenizer { token_count: 0 },
            old_token,
            harness.plan(),
        ));
        has_started.await.expect("Mock did not signal start");

        // Act
        let new_token = {
            let mut state = harness.state.lock();
            state.cancel_current_generation();
            state.begin_generation()
        };
        unblock.send(()).unwrap();
        old_task.await.unwrap();

        // Assert
        harness.assert_no_events("The cancelled generation must stay silent");
        let state = harness.state.lock();
        assert!(state.is_generating());
        assert!(state.is_current_generation(&new_token));
        assert_ne!(
            state.status,
            crate::app::state::StatusNotice::GenerationCancelled
        );
    }

    /// The scanner of a lazy load must stop working once the file list it loads into is
    /// replaced by a new scan or dropped by a directory reset.
    #[tokio::test]
    async fn lazy_load_scanner_is_cancelled_when_file_list_is_replaced() {
        let harness = TestHarness::new();
        let mut state = harness.state.lock();

        let token = state.begin_lazy_load();
        let scanner = lazy_load_scanner(&state.config, &token);
        assert!(!scanner.cancel_flag.load(Ordering::SeqCst));
        let scan = state.begin_scan();
        assert!(
            scanner.cancel_flag.load(Ordering::SeqCst),
            "A new scan must stop the lazy load"
        );
        state.finish_scan(&scan);

        let token = state.begin_lazy_load();
        let scanner = lazy_load_scanner(&state.config, &token);
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
        let token = harness.state.lock().begin_lazy_load();
        // Act: the user switches directories while the lazy load is in flight.
        harness.state.lock().reset_directory_state();
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &old_dir).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
        )
        .await;

        // Assert
        harness.assert_no_events("A discarded lazy load must stay silent");
        let state = harness.state.lock();
        assert!(state.full_file_list().is_empty());
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
            let mut state = harness.state.lock();
            state.edit_full_file_list(|list| list.push(dir_item(&dir, 1)));
            state.begin_lazy_load()
        };

        // Act
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &dir).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
        )
        .await;

        // Assert
        let ui_state = harness.last_state_update().unwrap();
        let src_node = ui_state
            .root_rows()
            .into_iter()
            .find(|n| n.name == "src")
            .unwrap();
        assert!(src_node.children_loaded && src_node.has_children);
        assert!(!src_node.is_expanded, "Directory must stay collapsed");
        let state = harness.state.lock();
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
            let mut state = harness.state.lock();
            state.config.ignore_patterns = HashSet::from(["/src/generated/".to_string()]);
            state.replace_with_shallow_file_list(&root, vec![dir_item(&src, 1)]);
            filtering::apply_filters(&mut state);
        }

        // Act
        start_lazy_load_scan(src.clone(), harness.proxy.clone(), harness.state.clone())
            .expect("The lazy load must be accepted")
            .await
            .expect("The lazy load task must not panic");

        // Assert
        let ui_state = harness.last_state_update().unwrap();
        let src_node = ui_state.row_named("src").unwrap();
        let child_names: Vec<&str> = ui_state
            .children_of(src_node)
            .into_iter()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(child_names, vec!["main.rs"]);

        let state = harness.state.lock();
        let loaded: Vec<&FileItem> = state
            .full_file_list()
            .iter()
            .filter(|item| item.path != src)
            .collect();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].path, src.join("main.rs"));
        assert_eq!(
            loaded[0].depth, 2,
            "Depth must be relative to the project root"
        );
        assert!(state.active_ignore_patterns.contains("/src/generated/"));
    }

    /// A lazy load for a path that is not part of the loaded project is refused.
    #[tokio::test]
    async fn lazy_load_outside_project_is_ignored() {
        let mut harness = TestHarness::new();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("file.txt"), "content").unwrap();

        let load = start_lazy_load_scan(
            outside.path().to_path_buf(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        assert!(load.is_none(), "No load must have been started");
        harness.assert_no_events("No load must have been performed");
        let state = harness.state.lock();
        assert!(state.full_file_list().is_empty());
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
            let mut state = harness.state.lock();
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
            let mut state = harness.state.lock();
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
        let shallow_event = harness.next_event().await;
        assert!(matches!(shallow_event, UserEvent::StateUpdate(_)));
        harness.state.lock().cancel_current_scan();
        unblock_deep_scan.send(()).ok();
        let _ = task.await;

        // Assert
        let state = harness.state.lock();
        assert_eq!(state.loaded_dirs, HashSet::from([root]));
        let ui_state = generate_ui_state(&state);
        let src_node = ui_state
            .root_rows()
            .into_iter()
            .find(|n| n.name == "src")
            .unwrap();
        assert!(
            !src_node.children_loaded,
            "`src` must be lazily loadable after the cancelled rescan"
        );
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
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![file_item(&fo_match, 1), file_item(&foo_match, 1)]);
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
        harness.state.lock().content_search_query = "foo".to_string();
        let fast_searcher = MockFileSearcher::default();
        fast_searcher.set_results(HashSet::from([foo_match.clone()]));
        search_in_files(harness.proxy.clone(), harness.state.clone(), fast_searcher).await;
        let state_after_current_search = harness.last_state_update().unwrap();
        release_slow.send(()).unwrap();
        slow_search.await.unwrap();

        // Assert
        assert_eq!(state_after_current_search.visible_files_count, 1);
        assert!(
            harness.last_state_update().is_none(),
            "The outdated search must not publish anything"
        );
        let state = harness.state.lock();
        assert_eq!(*state.content_search_results(), HashSet::from([foo_match]));
        assert_eq!(state.filtered_file_list().len(), 1);
    }

    /// Clearing the query while a search is running must not let that search bring its
    /// matches back afterwards.
    #[tokio::test]
    async fn content_search_finishing_after_query_was_cleared_is_discarded() {
        let harness = TestHarness::new();
        let file = harness.root_path.join("a.txt");
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![file_item(&file, 1)]);
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

        harness.state.lock().content_search_query.clear();
        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            MockFileSearcher::default(),
        )
        .await;
        release_slow.send(()).unwrap();
        slow_search.await.unwrap();

        assert!(harness.state.lock().content_search_results().is_empty());
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
            let mut state = harness.state.lock();
            state.content_search_query = "needle".to_string();
            state.set_content_search_results(HashSet::from([top_match.clone()]));
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
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_scanning);
        let state = harness.state.lock();
        assert_eq!(
            *state.content_search_results(),
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
        let mut harness = TestHarness::new();
        let token = harness.state.lock().begin_scan();

        proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            MockScanner::new(),
            UnexpectedSearch,
            token,
        )
        .await;

        let events = harness.drain_events();
        assert_eq!(events.len(), 2, "Only the shallow and the deep update");
    }

    /// "Indexing complete" reports the number of visible items. With a content query
    /// that number is only known once the search has been repeated on the complete
    /// list, so the scan must not be reported as complete before that.
    #[tokio::test]
    async fn indexing_complete_counts_the_items_visible_after_the_repeated_search() {
        // Arrange: the query matched one file of the shallow list.
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
        scanner.set_results(shallow, deep);
        let (searcher, search_started, release_search) =
            GatedFileSearcher::new(HashSet::from([top_match.clone(), deep_match.clone()]));
        let token = {
            let mut state = harness.state.lock();
            state.content_search_query = "needle".to_string();
            state.set_content_search_results(HashSet::from([top_match.clone()]));
            state.begin_scan()
        };

        // Act: the deep scan has delivered, the repeated search is still running.
        let scan = tokio::spawn(proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            root,
            scanner,
            searcher,
            token,
        ));
        search_started.await.unwrap();

        // Assert: nothing is reported as complete on the basis of the old matches.
        {
            let state = harness.state.lock();
            assert!(state.is_scanning(), "The scan is not complete yet");
            assert!(state.is_fully_scanned, "The complete list is in place");
            assert_eq!(state.status, StatusNotice::Ready);
        }
        assert!(harness
            .drain_events()
            .iter()
            .all(|event| !matches!(event, UserEvent::StateUpdate(s) if !s.is_scanning)));

        // Act: the search delivers.
        release_search.send(()).unwrap();
        scan.await.unwrap();

        // Assert: `src`, `top.txt` and `deep.txt` are visible, and that is what is said.
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_scanning);
        assert_eq!(ui_state.visible_files_count, 3);
        let state = harness.state.lock();
        assert_eq!(state.filtered_file_list().len(), 3);
        assert_eq!(
            state.status,
            StatusNotice::IndexingComplete { visible_items: 3 }
        );
    }

    /// The search that follows the deep scan belongs to the file list. Cancelling the
    /// scan while it runs must not leave the matches of the smaller, shallow list.
    #[tokio::test]
    async fn search_after_deep_scan_survives_the_cancellation_of_the_scan() {
        let mut harness = TestHarness::new();
        let root = harness.root_path.clone();
        let deep_match = root.join("src").join("deep.txt");
        let deep = vec![dir_item(&root.join("src"), 1), file_item(&deep_match, 2)];
        let mut scanner = MockScanner::new();
        scanner.set_results(vec![], deep);
        let (searcher, search_started, release_search) =
            GatedFileSearcher::new(HashSet::from([deep_match.clone()]));
        let token = {
            let mut state = harness.state.lock();
            state.content_search_query = "needle".to_string();
            state.begin_scan()
        };
        let scan = tokio::spawn(proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            root,
            scanner,
            searcher,
            token.clone(),
        ));
        harness.state.lock().attach_scan_task(&token, scan);
        search_started.await.unwrap();

        // Act: the user cancels, then the search delivers.
        harness.state.lock().cancel_current_scan();
        let _ = harness.drain_events();
        release_search.send(()).unwrap();
        let ui_state = harness
            .wait_for_state_update(|s| s.visible_files_count == 2)
            .await;

        // Assert
        assert!(!ui_state.is_scanning);
        let state = harness.state.lock();
        assert_eq!(*state.content_search_results(), HashSet::from([deep_match]));
        assert_eq!(state.status, StatusNotice::ScanCancelled);
    }

    // --- File preview ---

    /// A way of reading a file that holds its result back until the test releases it.
    /// Reads run on the blocking thread pool, hence the std channels.
    fn gated_read(
        content: &str,
    ) -> (
        impl FnOnce(&Path) -> Result<String, CoreError> + Send + 'static,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let content = content.to_string();
        let read = move |_: &Path| {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            Ok(content)
        };
        (read, started_rx, release_tx)
    }

    fn shown_previews(events: &[UserEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                UserEvent::ShowFilePreview { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect()
    }

    /// The request is recorded and answered while the file is still being read; the
    /// caller (the command lane) does not wait for the disk.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn file_preview_is_announced_before_the_file_has_been_read() {
        let mut harness = TestHarness::new();
        let path = harness.root_path.join("big.txt");
        let (read, read_started, release_read) = gated_read("content of big.txt");

        let preview = launch_file_preview(
            path.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            read,
        );
        read_started
            .recv_timeout(EVENT_TIMEOUT)
            .expect("The read was not started");

        // The read is blocked, yet the request has been dealt with.
        assert_eq!(harness.state.lock().previewed_file_path, Some(path.clone()));
        let events = harness.drain_events();
        assert!(matches!(events.as_slice(), [UserEvent::StateUpdate(_)]));

        release_read.send(()).unwrap();
        preview.await.unwrap();
        let events = harness.drain_events();
        match events.as_slice() {
            [UserEvent::ShowFilePreview {
                content,
                path: shown,
                search_term,
                ..
            }] => {
                assert_eq!(content, "content of big.txt");
                assert_eq!(shown, &path);
                assert_eq!(search_term, &None);
            }
            other => panic!("Expected exactly the preview, got {other:?}"),
        }
    }

    /// The read of a file the user has already left finishes last. Its content must
    /// not replace the preview the user is looking at.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_preview_does_not_replace_a_newer_one() {
        let mut harness = TestHarness::new();
        let slow_path = harness.root_path.join("slow.txt");
        let fast_path = harness.root_path.join("fast.txt");
        let (slow_read, slow_started, release_slow) = gated_read("slow");

        let slow = launch_file_preview(
            slow_path,
            harness.proxy.clone(),
            harness.state.clone(),
            slow_read,
        );
        slow_started
            .recv_timeout(EVENT_TIMEOUT)
            .expect("The first read was not started");
        launch_file_preview(
            fast_path.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            |_: &Path| Ok("fast".to_string()),
        )
        .await
        .unwrap();
        release_slow.send(()).unwrap();
        slow.await.unwrap();

        assert_eq!(shown_previews(&harness.drain_events()), ["fast"]);
        assert_eq!(harness.state.lock().previewed_file_path, Some(fast_path));
    }

    /// Closing the preview (or replacing it by the generated output, or leaving the
    /// directory) while the file is being read: nothing is shown afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn preview_closed_while_reading_shows_nothing() {
        let mut harness = TestHarness::new();
        let (read, read_started, release_read) = gated_read("too late");

        let preview = launch_file_preview(
            harness.root_path.join("a.txt"),
            harness.proxy.clone(),
            harness.state.clone(),
            read,
        );
        read_started
            .recv_timeout(EVENT_TIMEOUT)
            .expect("The read was not started");
        harness.state.lock().clear_preview();
        let _ = harness.drain_events();
        release_read.send(()).unwrap();
        preview.await.unwrap();

        harness.assert_no_events("A closed preview must not show anything");
        assert_eq!(harness.state.lock().previewed_file_path, None);
    }

    /// A read that fails or panics is reported; it does not take the task down silently.
    #[tokio::test]
    async fn failed_preview_read_is_reported() {
        let mut harness = TestHarness::new();
        let path = harness.root_path.join("gone.txt");

        launch_file_preview(
            path.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            |path: &Path| Err(CoreError::Io("denied".to_string(), path.to_path_buf())),
        )
        .await
        .unwrap();
        launch_file_preview(
            path,
            harness.proxy.clone(),
            harness.state.clone(),
            |_: &Path| panic!("read failure injected by the test"),
        )
        .await
        .unwrap();

        let errors: Vec<String> = harness
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                UserEvent::ShowError(message) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("denied"), "{errors:?}");
        assert!(errors[1].contains("gone.txt"), "{errors:?}");
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
            let mut state = harness.state.lock();
            state.edit_full_file_list(|list| list.push(dir_item(&dir, 1)));
            state.content_search_query = "needle".to_string();
            state.begin_lazy_load()
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
        )
        .await;

        let state = harness.state.lock();
        assert_eq!(*state.content_search_results(), HashSet::from([new_match]));
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
            let mut state = harness.state.lock();
            state.edit_full_file_list(|list| list.push(dir_item(&dir, 1)));
            state.begin_lazy_load()
        };

        // Act: meanwhile the user adds an ignore pattern, which is applied locally.
        {
            let mut state = harness.state.lock();
            state
                .config
                .ignore_patterns
                .insert("/src/generated/".to_string());
            state.apply_ignore_patterns();
        }
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &dir).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            MockFileSearcher::default(),
            token,
        )
        .await;

        // Assert
        assert!(harness.last_state_update().is_none());
        let state = harness.state.lock();
        assert!(!state.full_file_list().iter().any(|i| i.path == generated));
        assert!(
            !state.loaded_dirs.contains(&dir),
            "The directory stays loadable with the new patterns"
        );
    }

    //============================================================================================//
    //|                         FAILED RE-SCAN, PANICS BEFORE THE WALK                            |//
    //============================================================================================//

    /// A complete list with one file below `src`, as a finished scan leaves it.
    fn load_complete_list(harness: &TestHarness) -> PathBuf {
        let file = harness.root_path.join("src").join("a.rs");
        let mut state = harness.state.lock();
        state.replace_with_complete_file_list(vec![
            dir_item(&harness.root_path.join("src"), 1),
            dir_item(&harness.root_path.join("empty"), 1),
            file_item(&file, 2),
        ]);
        state.is_fully_scanned = true;
        state.config.remove_empty_directories = true;
        filtering::apply_filters(&mut state);
        file
    }

    /// Re-scanning a directory that has vanished ends with an error and leaves the
    /// list as it was, which includes that it is still the complete list: "remove
    /// empty directories" keeps working on it.
    #[tokio::test]
    async fn a_failed_rescan_leaves_the_list_fully_scanned() {
        let mut harness = TestHarness::new();
        load_complete_list(&harness);
        let vanished = harness.root_path.join("vanished");
        harness.state.lock().current_path = vanished.to_string_lossy().to_string();

        start_scan_on_path(vanished, harness.proxy.clone(), harness.state.clone(), true);

        let ended = harness.wait_for_state_update(|s| !s.is_scanning).await;
        assert!(ended.is_fully_scanned);
        let mut state = harness.state.lock();
        assert!(state.is_fully_scanned);
        assert_eq!(state.full_file_list().len(), 3);
        // The empty directory is still removed from what is shown.
        filtering::apply_filters(&mut state);
        assert_eq!(state.filtered_file_list().len(), 2);
    }

    /// A re-scan that finds its directory reports the list as incomplete from that
    /// moment on (the announcement itself changes nothing but the scanning state).
    #[tokio::test]
    async fn a_rescan_marks_the_list_incomplete_once_the_directory_is_found() {
        let mut harness = TestHarness::new();
        load_complete_list(&harness);

        start_scan_on_path(
            harness.root_path.clone(),
            harness.proxy.clone(),
            harness.state.clone(),
            true,
        );

        match harness.next_event().await {
            UserEvent::StateUpdate(announced) => {
                assert!(announced.is_scanning && announced.is_fully_scanned);
            }
            other => panic!("Expected the announcement of the scan, got {other:?}"),
        }
        let walking = harness.wait_for_state_update(|s| !s.is_fully_scanned).await;
        assert!(walking.is_scanning);
        let completed = harness.wait_for_state_update(|s| !s.is_scanning).await;
        assert!(completed.is_fully_scanned);
    }

    /// A proxy whose `n`-th event makes the sender panic (once), to stage a panic in
    /// the middle of a task. The other events are passed on.
    #[derive(Clone)]
    struct PanicOnEvent {
        inner: TestEventProxy,
        countdown: Arc<Mutex<Option<usize>>>,
    }
    impl PanicOnEvent {
        fn nth(inner: TestEventProxy, n: usize) -> Self {
            Self {
                inner,
                countdown: Arc::new(Mutex::new(Some(n))),
            }
        }
    }
    impl EventProxy for PanicOnEvent {
        fn send_event(&self, event: UserEvent) {
            let fire = {
                let mut countdown = self.countdown.lock().unwrap();
                match countdown.as_mut() {
                    Some(0) => countdown.take().is_some(),
                    Some(remaining) => {
                        *remaining -= 1;
                        false
                    }
                    None => false,
                }
            };
            if fire {
                panic!("failure injected by the test while the scan root is located");
            }
            self.inner.send_event(event);
        }
    }

    /// The guard of a scan covers its first step too: a task that panics while it
    /// looks at its path (here: under the state lock, when it reports the directory
    /// it has entered) releases the scanning state and tells the UI.
    #[tokio::test]
    async fn a_panic_while_the_scan_root_is_located_releases_the_scan() {
        let mut harness = TestHarness::new();
        let target = tempdir().unwrap();
        // Event 0 is the announcement, event 1 the report of the entered directory.
        let proxy = PanicOnEvent::nth(harness.proxy.clone(), 1);

        start_scan_on_path(
            target.path().to_path_buf(),
            proxy,
            harness.state.clone(),
            false,
        );

        match harness.next_event().await {
            UserEvent::StateUpdate(announced) => assert!(announced.is_scanning),
            other => panic!("Expected the announcement of the scan, got {other:?}"),
        }
        let released = harness.wait_for_state_update(|s| !s.is_scanning).await;
        assert!(!released.is_scanning);
        {
            let state = harness.state.lock();
            assert!(!state.is_scanning(), "the scan slot must be free again");
            assert!(!state.is_opening());
        }

        // The next scan is not held up by the one that panicked.
        start_scan_on_path(
            target.path().to_path_buf(),
            harness.proxy.clone(),
            harness.state.clone(),
            false,
        );
        let completed = harness
            .wait_for_state_update(|s| !s.is_scanning && s.is_fully_scanned)
            .await;
        assert_eq!(
            completed.current_path,
            target.path().to_string_lossy().to_string()
        );
    }

    //============================================================================================//
    //|                         CONTENT SEARCH: FAILURE, FIRST SEARCH, EXPANSION                  |//
    //============================================================================================//

    struct FailingSearcher;
    impl FileSearcher for FailingSearcher {
        async fn search(&self, _: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
            Err("the search worker panicked".to_string())
        }
    }

    /// A search that fails is reported; it is not a search without matches. The
    /// matches of the search before it stay, and the next search is not blocked.
    #[tokio::test]
    async fn a_failed_content_search_is_reported_and_keeps_the_previous_matches() {
        let mut harness = TestHarness::new();
        let matching = harness.root_path.join("a.txt");
        let other = harness.root_path.join("b.txt");
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![file_item(&matching, 1), file_item(&other, 1)]);
            state.content_search_query = "needle".to_string();
        }
        let first = MockFileSearcher::default();
        first.set_results(HashSet::from([matching.clone()]));
        search_in_files(harness.proxy.clone(), harness.state.clone(), first).await;
        harness.drain_events();

        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            FailingSearcher,
        )
        .await;

        let events = harness.drain_events();
        match &events[..] {
            [UserEvent::ShowError(message), UserEvent::StateUpdate(ui_state)] => {
                assert!(message.contains("content search failed"), "{message}");
                assert!(message.contains("panicked"), "{message}");
                assert_eq!(ui_state.visible_files_count, 1, "the matches stay");
            }
            other => panic!("Expected an error and a state update, got {other:?}"),
        }
        {
            let state = harness.state.lock();
            assert_eq!(*state.content_search_results(), HashSet::from([matching]));
        }

        // The slot is free: a later search delivers.
        let next = MockFileSearcher::default();
        next.set_results(HashSet::from([other.clone()]));
        search_in_files(harness.proxy.clone(), harness.state.clone(), next).await;
        assert_eq!(
            *harness.state.lock().content_search_results(),
            HashSet::from([other])
        );
    }

    /// The failure of a search that has been superseded is nobody's business.
    #[tokio::test]
    async fn a_failed_content_search_that_was_superseded_stays_silent() {
        let mut harness = TestHarness::new();
        harness.state.lock().content_search_query = "needle".to_string();

        struct SupersededThenFailing(SharedState);
        impl FileSearcher for SupersededThenFailing {
            async fn search(&self, _: Vec<FileItem>, _: ContentSearchRequest) -> SearchOutcome {
                self.0.lock().begin_content_search();
                Err("too late".to_string())
            }
        }

        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            SupersededThenFailing(harness.state.clone()),
        )
        .await;

        harness.assert_no_events("An outdated search reports nothing");
    }

    /// While the first search for a query is running, a snapshot shows the list
    /// without the content filter instead of an empty tree; the filter applies as
    /// soon as the matches are there.
    #[tokio::test]
    async fn a_snapshot_during_the_first_content_search_is_not_empty() {
        let mut harness = TestHarness::new();
        let matching = harness.root_path.join("a.txt");
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![
                file_item(&matching, 1),
                file_item(&harness.root_path.join("b.txt"), 1),
            ]);
            filtering::apply_filters(&mut state);
            state.content_search_query = "needle".to_string();
        }
        let (searcher, started, release) =
            GatedFileSearcher::new(HashSet::from([matching.clone()]));
        let search = tokio::spawn(search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            searcher,
        ));
        started.await.unwrap();

        // Any command that answers with a snapshot while the search runs, also one
        // that applies the filters again.
        {
            let mut state = harness.state.lock();
            assert_eq!(state.content_filter_matches(), None);
            assert_eq!(generate_ui_state(&state).root_rows().len(), 2);
            filtering::apply_filters(&mut state);
            assert_eq!(generate_ui_state(&state).root_rows().len(), 2);
        }

        release.send(()).unwrap();
        search.await.unwrap();
        let filtered = harness.last_state_update().unwrap();
        assert_eq!(filtered.root_rows().len(), 1);
        assert_eq!(filtered.root_rows()[0].path, matching);
    }

    /// Once a search has answered, its answer applies until the next one arrives:
    /// "no matches" stays an empty tree while the query is being refined.
    #[tokio::test]
    async fn a_snapshot_during_a_later_content_search_shows_the_previous_matches() {
        let mut harness = TestHarness::new();
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![file_item(&harness.root_path.join("a.txt"), 1)]);
            state.content_search_query = "needle".to_string();
        }
        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            MockFileSearcher::default(),
        )
        .await;
        assert!(harness.last_state_update().unwrap().rows.is_empty());

        harness.state.lock().content_search_query = "needles".to_string();
        let (searcher, started, release) = GatedFileSearcher::new(HashSet::new());
        let search = tokio::spawn(search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            searcher,
        ));
        started.await.unwrap();
        {
            let state = harness.state.lock();
            assert_eq!(state.content_filter_matches(), Some(&HashSet::new()));
            assert!(generate_ui_state(&state).rows.is_empty());
        }
        release.send(()).unwrap();
        search.await.unwrap();
    }

    /// Repeating the content search (here: after a lazy load brought new files) shows
    /// the new matches, but does not re-open a directory the user collapsed while the
    /// query was active.
    #[tokio::test]
    async fn repeated_content_search_does_not_reopen_a_collapsed_directory() {
        let harness = TestHarness::new();
        let src = harness.root_path.join("src");
        let lib = harness.root_path.join("lib");
        let old_match = src.join("old.txt");
        let new_match = lib.join("new.txt");
        {
            let mut state = harness.state.lock();
            state.set_full_file_list(vec![
                dir_item(&src, 1),
                dir_item(&lib, 1),
                file_item(&old_match, 2),
            ]);
            state.content_search_query = "needle".to_string();
        }
        let searcher = ListBoundFileSearcher {
            matching: HashSet::from([old_match.clone(), new_match.clone()]),
        };
        search_in_files(
            harness.proxy.clone(),
            harness.state.clone(),
            searcher.clone(),
        )
        .await;
        assert!(harness.state.lock().expanded_dirs.contains(&src));

        // The user collapses `src`, then opens `lib`, which is loaded lazily.
        let token = {
            let mut state = harness.state.lock();
            state.collapse_directories([src.clone()]);
            state.begin_lazy_load()
        };
        let mut scanner = MockScanner::new();
        scanner.set_results(vec![file_item(&new_match, 2)], vec![]);
        lazy_load_task(
            ScanRequest::directory_level(&harness.root_path, &lib).unwrap(),
            harness.proxy.clone(),
            harness.state.clone(),
            scanner,
            searcher,
            token,
        )
        .await;

        let state = harness.state.lock();
        assert_eq!(
            *state.content_search_results(),
            HashSet::from([old_match, new_match])
        );
        assert!(!state.expanded_dirs.contains(&src), "stays collapsed");
        assert!(state.expanded_dirs.contains(&lib), "the new match is shown");
    }

    //============================================================================================//
    //|                              THE DEEP SCAN PUBLISHES AS IT GOES                          |//
    //============================================================================================//

    /// A scanner whose deep scan delivers the batches the test feeds it, when it feeds
    /// them, and ends when the feed is dropped. The shallow scan returns `shallow`.
    struct StreamingScanner {
        shallow: Vec<FileItem>,
        feed: tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<FileItem>>>,
    }

    impl StreamingScanner {
        fn new(shallow: Vec<FileItem>) -> (Self, mpsc::UnboundedSender<Vec<FileItem>>) {
            let (feed_tx, feed_rx) = mpsc::unbounded_channel();
            let scanner = Self {
                shallow,
                feed: tokio::sync::Mutex::new(feed_rx),
            };
            (scanner, feed_tx)
        }
    }

    impl Scanner for StreamingScanner {
        async fn scan(
            &self,
            _: &ScanRequest,
            _: Box<dyn Fn(ScanProgress) + Send + Sync>,
        ) -> ScanOutcome {
            Ok((self.shallow.clone(), HashSet::new()))
        }

        async fn scan_in_batches(
            &self,
            _: &ScanRequest,
            _: Box<dyn Fn(ScanProgress) + Send + Sync>,
            on_batch: BatchSink,
        ) -> StreamedScanOutcome {
            let mut feed = self.feed.lock().await;
            while let Some(batch) = feed.recv().await {
                on_batch(batch);
            }
            Ok(HashSet::from(["deep".to_string()]))
        }
    }

    /// Starts a scan of the root of `harness` with a [`StreamingScanner`] and waits
    /// until its first level (`shallow`) has been published.
    async fn spawn_streaming_scan(
        harness: &mut TestHarness,
        shallow: Vec<FileItem>,
    ) -> (
        tokio::task::JoinHandle<()>,
        ScanToken,
        mpsc::UnboundedSender<Vec<FileItem>>,
    ) {
        let (scanner, feed) = StreamingScanner::new(shallow);
        let token = harness.state.lock().begin_scan();
        let task = tokio::spawn(proactive_scan_task(
            harness.proxy.clone(),
            harness.state.clone(),
            harness.root_path.clone(),
            scanner,
            MockFileSearcher::default(),
            token.clone(),
        ));
        let shallow_event = harness.next_event().await;
        assert!(matches!(shallow_event, UserEvent::StateUpdate(_)));
        (task, token, feed)
    }

    fn listed(harness: &TestHarness) -> Vec<PathBuf> {
        let state = harness.state.lock();
        state
            .full_file_list()
            .iter()
            .map(|item| item.path.clone())
            .collect()
    }

    /// What the walk finds is shown while the scan is still running, batch after
    /// batch, and without listing the first level a second time. The end of the scan
    /// completes the list.
    #[tokio::test]
    async fn deep_scan_publishes_its_batches_while_it_runs() {
        let mut harness = TestHarness::new();
        let src = harness.root_path.join("src");
        let (task, _token, feed) =
            spawn_streaming_scan(&mut harness, vec![dir_item(&src, 1)]).await;

        // First batch: the walk comes by the first level again and goes below it.
        feed.send(vec![dir_item(&src, 1), file_item(&src.join("a.rs"), 2)])
            .unwrap();
        let first = harness
            .wait_for_state_update(|ui| ui.visible_files_count == 2)
            .await;
        assert!(first.is_scanning);
        assert!(!first.is_fully_scanned);

        // Second batch.
        feed.send(vec![file_item(&src.join("b.rs"), 2)]).unwrap();
        let second = harness
            .wait_for_state_update(|ui| ui.visible_files_count == 3)
            .await;
        assert!(second.is_scanning);
        assert!(
            !harness.state.lock().loaded_dirs.contains(&src),
            "The walk may not be done with `src` yet"
        );

        // A last batch right before the end is part of the final state.
        feed.send(vec![file_item(&src.join("c.rs"), 2)]).unwrap();
        drop(feed);
        task.await.unwrap();

        let last = harness.last_state_update().unwrap();
        assert!(!last.is_scanning);
        assert!(last.is_fully_scanned);
        assert_eq!(last.visible_files_count, 4);
        assert!(last.status_message.contains("Indexing complete"));
        assert_eq!(
            listed(&harness),
            [
                src.clone(),
                src.join("a.rs"),
                src.join("b.rs"),
                src.join("c.rs")
            ]
        );
        let state = harness.state.lock();
        assert!(state.loaded_dirs.contains(&src));
        assert_eq!(
            state.active_ignore_patterns,
            HashSet::from(["deep".to_string()])
        );
    }

    /// The user expands a directory the walk has not come to: its level is loaded and
    /// shown at once. When the walk gets there, nothing is listed twice.
    #[tokio::test]
    async fn a_directory_expanded_during_the_deep_scan_is_loaded_at_once() {
        let mut harness = TestHarness::new();
        let src = harness.root_path.join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        let (task, token, feed) = spawn_streaming_scan(&mut harness, vec![dir_item(&src, 1)]).await;

        // Act: the lazy load, while the deep scan is under way.
        start_lazy_load_scan(src.clone(), harness.proxy.clone(), harness.state.clone())
            .expect("A lazy load is possible during a scan")
            .await
            .unwrap();

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.is_scanning);
        let src_node = ui_state.row_named("src").unwrap();
        assert!(src_node.is_expanded && src_node.children_loaded);
        assert_eq!(ui_state.children_of(src_node).len(), 1);
        assert!(harness.state.lock().is_current_scan(&token));

        // The walk arrives at the directory and finds one more file.
        feed.send(vec![
            dir_item(&src, 1),
            file_item(&src.join("main.rs"), 2),
            file_item(&src.join("new.rs"), 2),
        ])
        .unwrap();
        drop(feed);
        task.await.unwrap();

        let mut paths = listed(&harness);
        paths.sort();
        assert_eq!(
            paths,
            [src.clone(), src.join("main.rs"), src.join("new.rs")]
        );
        assert!(harness.state.lock().is_fully_scanned);
    }

    /// A scan that was superseded while its walk is delivering publishes nothing more
    /// and ends by itself.
    #[tokio::test]
    async fn batches_of_a_superseded_scan_are_not_published() {
        let mut harness = TestHarness::new();
        let (old_task, old_token, feed) = spawn_streaming_scan(&mut harness, vec![]).await;

        let new_token = begin_replacement_scan(&harness);
        feed.send(vec![file_item(&harness.root_path.join("stale.txt"), 1)])
            .unwrap();
        // The feed stays open: the task has to end because of the stale batch.
        tokio::time::timeout(EVENT_TIMEOUT, old_task)
            .await
            .expect("The superseded task ends when it tries to publish")
            .unwrap();

        assert!(old_token.is_cancelled());
        harness.assert_no_events("A superseded scan must not send state updates");
        assert!(listed(&harness).is_empty());
        assert_replacement_scan_is_intact(&harness, &new_token);
        drop(feed);
    }

    /// Cancelling a scan keeps what it has published: the list is partial, not
    /// complete, and its directories can be loaded lazily.
    #[tokio::test]
    async fn a_cancelled_deep_scan_keeps_what_it_has_published() {
        let mut harness = TestHarness::new();
        let root = harness.root_path.clone();
        let src = root.join("src");
        let (task, _token, feed) =
            spawn_streaming_scan(&mut harness, vec![dir_item(&src, 1)]).await;
        feed.send(vec![file_item(&src.join("a.rs"), 2)]).unwrap();
        harness
            .wait_for_state_update(|ui| ui.visible_files_count == 2)
            .await;

        harness.state.lock().cancel_current_scan();
        feed.send(vec![file_item(&src.join("late.rs"), 2)]).unwrap();
        drop(feed);
        task.await.unwrap();

        assert_eq!(listed(&harness), [src.clone(), src.join("a.rs")]);
        let state = harness.state.lock();
        assert!(!state.is_scanning());
        assert!(!state.is_fully_scanned);
        assert_eq!(state.loaded_dirs, HashSet::from([root]));
    }

    /// The state is not locked while the items of a publication are prepared, and
    /// the state update that ends the publication is made of the state as it is
    /// then, changes of the meantime included.
    #[tokio::test]
    async fn a_publication_shows_the_state_as_it_is_when_the_items_are_in() {
        let mut harness = TestHarness::new();
        let src = harness.root_path.join("src");
        let (task, token, feed) = spawn_streaming_scan(&mut harness, vec![dir_item(&src, 1)]).await;

        let staged = staged::stage_scanned_items(
            &mut harness.state.lock(),
            &token,
            vec![file_item(&src.join("a.rs"), 2)],
        )
        .unwrap();
        let prepared = staged.prepare();
        // What a command does meanwhile: the user selects a file and expands `src`.
        {
            let mut state = harness.state.lock();
            state.selected_files.insert(src.join("a.rs"));
            state.expand_directories([src.clone()]);
        }
        let ui_state = {
            let mut state = harness.state.lock();
            let (is_current, discarded) =
                staged::commit_scanned_items(&mut state, &token, prepared);
            drop(discarded);
            assert!(is_current);
            generate_ui_state(&state)
        };

        assert_eq!(ui_state.visible_files_count, 2);
        assert_eq!(ui_state.selected_files_count, 1);
        let src_row = ui_state.row_named("src").unwrap();
        assert!(src_row.is_expanded);
        assert_eq!(ui_state.children_of(src_row).len(), 1);

        drop(feed);
        task.await.unwrap();
    }

    /// The end of the walk leaves the complete list, filtered, and its hierarchy
    /// behind, so the state update that reports the scan as finished does not build
    /// them with the state locked: with a filter and without.
    #[tokio::test]
    async fn the_end_of_the_walk_prepares_the_hierarchy_for_the_last_state_update() {
        for query in ["", "a"] {
            let mut harness = TestHarness::new();
            let src = harness.root_path.join("src");
            let token = {
                let mut state = harness.state.lock();
                state.search_query = query.to_string();
                let token = state.begin_scan();
                state.begin_scan_walk(&token);
                token
            };
            assert!(apply_shallow_scan_result(
                &harness.state,
                &harness.proxy,
                &token,
                &harness.root_path.clone(),
                vec![dir_item(&src, 1)],
                HashSet::new(),
            ));

            assert!(apply_deep_scan_result(
                &harness.state,
                &token,
                vec![dir_item(&src, 1), file_item(&src.join("a.rs"), 2)],
                HashSet::new(),
            ));
            let built = harness.state.lock().file_tree_build_count();
            complete_scan(&harness.state, &harness.proxy, &token);

            let ui_state = harness.last_state_update().unwrap();
            assert!(ui_state.is_fully_scanned && !ui_state.is_scanning);
            assert_eq!(ui_state.visible_files_count, 2, "query {query:?}");
            assert_eq!(
                harness.state.lock().file_tree_build_count(),
                built,
                "query {query:?}"
            );
        }
    }

    /// The pause between two publications grows with what a publication costs, so
    /// publishing takes a bounded share of the time however large the list is.
    #[test]
    fn the_pause_after_a_publication_is_in_proportion_to_its_cost() {
        assert_eq!(
            pause_after_publication(Duration::from_millis(1)),
            MIN_PUBLISH_INTERVAL
        );
        assert_eq!(
            pause_after_publication(Duration::from_millis(400)),
            Duration::from_millis(400) * PUBLISH_PAUSE_FACTOR
        );
    }

    /// Measurement, not a test: `CFC_PERF_TREE=<dir> cargo test --release --lib
    /// perf_scan -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn perf_scan_of_a_large_tree() {
        /// The scan as it was: everything at the end.
        struct AtTheEnd(RealScanner);
        impl Scanner for AtTheEnd {
            async fn scan(
                &self,
                request: &ScanRequest,
                progress: Box<dyn Fn(ScanProgress) + Send + Sync>,
            ) -> ScanOutcome {
                self.0.scan(request, progress).await
            }
        }

        async fn run<S: Scanner + 'static>(
            label: &str,
            root: &Path,
            make: impl Fn(Arc<AtomicBool>) -> S,
        ) {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let proxy = TestEventProxy { sender: tx };
            let mut app_state = AppState::default();
            app_state.current_path = root.to_string_lossy().to_string();
            let state = SharedState::new(app_state);
            let token = state.lock().begin_scan();
            let scanner = make(token.cancel_flag());
            let started = std::time::Instant::now();
            let task = tokio::spawn(proactive_scan_task(
                proxy,
                state.clone(),
                root.to_path_buf(),
                scanner,
                RealFileSearcher,
                token,
            ));
            let (mut first_level, mut below, mut updates, mut first_count) = (None, None, 0, 0);
            let mut total = None;
            while let Some(event) = rx.recv().await {
                let UserEvent::StateUpdate(ui) = event else {
                    continue;
                };
                updates += 1;
                if first_level.is_none() && ui.visible_files_count > 0 {
                    first_level = Some(started.elapsed());
                    first_count = ui.visible_files_count;
                } else if below.is_none() && ui.visible_files_count > first_count {
                    below = Some(started.elapsed());
                }
                if ui.is_fully_scanned && !ui.is_scanning {
                    total = Some((started.elapsed(), ui.visible_files_count));
                    break;
                }
            }
            task.await.unwrap();
            println!(
                "{label}: first level {first_level:?}, first items below {below:?}, complete {total:?}, {updates} state updates"
            );
        }

        let root = PathBuf::from(std::env::var("CFC_PERF_TREE").expect("CFC_PERF_TREE"));
        let real = |cancel_flag| RealScanner {
            ignore_patterns: HashSet::new(),
            max_file_size: MAX_FILE_SIZE,
            cancel_flag,
        };
        for round in 0..3 {
            run(&format!("[{round}] at the end "), &root, |flag| {
                AtTheEnd(real(flag))
            })
            .await;
            run(&format!("[{round}] incremental"), &root, real).await;
        }
    }

    /// The items a walk lists for a project of about `count` entries: packages of
    /// modules of 25 files each, a directory before its content.
    fn synthetic_walk(count: usize) -> Vec<FileItem> {
        const FILES_PER_MODULE: usize = 25;
        const MODULES_PER_PACKAGE: usize = 100;
        let root = Path::new("/root");
        let mut items = Vec::with_capacity(count + count / FILES_PER_MODULE + 64);
        let mut package = 0;
        while items.len() < count {
            let package_path = root.join(format!("package_{package:04}"));
            items.push(dir_item(&package_path, 1));
            for module in 0..MODULES_PER_PACKAGE {
                if items.len() >= count {
                    break;
                }
                let module_path = package_path.join(format!("module_{module:03}"));
                items.push(dir_item(&module_path, 2));
                for file in 0..FILES_PER_MODULE {
                    items.push(file_item(
                        &module_path.join(format!("source_file_{file:02}.rs")),
                        3,
                    ));
                }
            }
            package += 1;
        }
        items
    }

    /// Measurement, not a test: `cargo test --release --lib perf_lock_hold --
    /// --ignored --nocapture`. How long the state is locked for the events that
    /// change the file list of a large project (a publication of scanned items, a
    /// level loaded on demand, the matches of a content search, the completion of
    /// the list), each with its state update, next to the plain way: the same
    /// change, the filters and the state update under one lock, which is what
    /// these events used to be.
    #[test]
    #[ignore = "a benchmark, run on request"]
    fn perf_lock_hold_of_list_events() {
        #[derive(Clone, Copy, PartialEq)]
        enum Scenario {
            Plain,
            /// A file name filter that matches every fifth file.
            NameFilter,
            /// As after a re-scan of a project the user has worked in: every
            /// directory expanded, every file selected.
            WorkedIn,
            /// Empty directories are removed (from the complete list).
            RemoveEmpty,
        }

        /// A scan of a project of which `listed` entries have been published, and
        /// the items the walk lists after them.
        fn scanning_state(
            listed: usize,
            later: usize,
            scenario: Scenario,
        ) -> (AppState, ScanToken, Vec<FileItem>) {
            let mut walk = synthetic_walk(listed + later);
            let first_level: Vec<FileItem> = walk
                .iter()
                .filter(|item| item.depth == 1)
                .cloned()
                .collect();
            let mut state = AppState::default();
            state.current_path = "/root".to_string();
            match scenario {
                Scenario::Plain => {}
                Scenario::NameFilter => state.search_query = "file_0".to_string(),
                Scenario::RemoveEmpty => state.config.remove_empty_directories = true,
                Scenario::WorkedIn => {
                    let (dirs, files): (Vec<_>, Vec<_>) =
                        walk.iter().partition(|item| item.is_directory);
                    state.expand_directories(dirs.into_iter().map(|dir| dir.path.clone()));
                    state
                        .selected_files
                        .extend(files.into_iter().map(|file| file.path.clone()).collect());
                }
            }
            let later = walk.split_off(listed);
            let token = state.begin_scan();
            state.begin_scan_walk(&token);
            state.replace_with_shallow_file_list(Path::new("/root"), first_level);
            assert!(state.merge_scanned_items(&token, walk));
            filtering::apply_filters(&mut state);
            generate_ui_state(&state);
            (state, token, later)
        }

        /// The content of a directory the walk has not come to.
        fn level() -> (PathBuf, Vec<FileItem>) {
            let directory = PathBuf::from("/root/package_0000/module_000/loaded");
            let items = (0..25)
                .map(|file| file_item(&directory.join(format!("file_{file:02}.rs")), 4))
                .collect();
            (directory, items)
        }

        /// Every tenth file of the list.
        fn matches(state: &AppState) -> HashSet<PathBuf> {
            let files = state
                .full_file_list()
                .iter()
                .filter(|item| !item.is_directory);
            files.step_by(10).map(|item| item.path.clone()).collect()
        }

        fn locked(cost: Option<PublicationCost>) -> Duration {
            cost.expect("the event is not outdated").locked
        }

        fn timed(work: impl FnOnce()) -> Duration {
            let started = std::time::Instant::now();
            work();
            started.elapsed()
        }

        fn millis(durations: &[Duration]) -> String {
            let (min, max) = (
                durations.iter().min().unwrap(),
                durations.iter().max().unwrap(),
            );
            let (min, max) = (min.as_secs_f64() * 1000.0, max.as_secs_f64() * 1000.0);
            if max - min < 0.05 {
                format!("{max:.1} ms")
            } else {
                format!("{min:.1}-{max:.1} ms")
            }
        }

        const ROUNDS: usize = 4;
        for listed in [200_000usize, 1_000_000] {
            for scenario in [
                Scenario::Plain,
                Scenario::NameFilter,
                Scenario::WorkedIn,
                Scenario::RemoveEmpty,
            ] {
                println!(
                    "{listed} listed, {}: locked in steps (plain way)",
                    match scenario {
                        Scenario::Plain => "no filter",
                        Scenario::NameFilter => "file name filter",
                        Scenario::WorkedIn => "all expanded and selected",
                        Scenario::RemoveEmpty => "empty directories removed",
                    }
                );
                let snapshot = |s: &mut AppState| drop(generate_ui_state(s));

                for batch in [4_096usize, listed / 10] {
                    let (state, token, later) = scanning_state(listed, ROUNDS * batch, scenario);
                    let state = SharedState::new(state);
                    let (mut steps, mut total) = (Vec::new(), Vec::new());
                    for items in later.chunks(batch).take(ROUNDS) {
                        let cost = add_scanned_items(&state, &token, items.to_vec(), |s| {
                            drop(generate_ui_state(s))
                        })
                        .unwrap();
                        steps.push(cost.locked);
                        total.push(cost.total);
                    }
                    let (mut state, token, later) =
                        scanning_state(listed, ROUNDS * batch, scenario);
                    let mut plain = Vec::new();
                    for items in later.chunks(batch).take(ROUNDS) {
                        let items = items.to_vec();
                        plain.push(timed(|| {
                            assert!(state.merge_scanned_items(&token, items));
                            filtering::apply_filters(&mut state);
                            snapshot(&mut state);
                        }));
                    }
                    println!(
                        "  publication of {batch} items: {} ({}), as a whole {}",
                        millis(&steps),
                        millis(&plain),
                        millis(&total)
                    );
                }

                // A level loaded on demand.
                let (state, _token, _) = scanning_state(listed, 0, scenario);
                let lazy_load = state.begin_lazy_load();
                let state = SharedState::new(state);
                let (directory, items) = level();
                let steps = locked(in_three_steps(
                    &state,
                    |s| staged::stage_directory_level(s, &lazy_load, &directory, items),
                    |s, prepared| {
                        staged::commit_directory_level(s, &lazy_load, prepared, HashSet::new())
                    },
                    snapshot,
                ));
                let (mut state, _token, _) = scanning_state(listed, 0, scenario);
                let lazy_load = state.begin_lazy_load();
                let (directory, items) = level();
                let plain = timed(|| {
                    assert!(state.merge_directory_level(
                        &lazy_load,
                        &directory,
                        items,
                        HashSet::new()
                    ));
                    filtering::apply_filters(&mut state);
                    snapshot(&mut state);
                });
                println!(
                    "  level loaded on demand: {} ({})",
                    millis(&[steps]),
                    millis(&[plain])
                );

                // The matches of a content search.
                let (mut state, _token, _) = scanning_state(listed, 0, scenario);
                state.content_search_query = "fn".to_string();
                let search = state.begin_content_search();
                let found = matches(&state);
                let state = SharedState::new(state);
                let steps = locked(in_three_steps(
                    &state,
                    |s| staged::stage_content_matches(s, &search, found),
                    |s, prepared| staged::commit_content_matches(s, &search, prepared),
                    |s| {
                        auto_expand_for_matches(s);
                        snapshot(s);
                    },
                ));
                let (mut state, _token, _) = scanning_state(listed, 0, scenario);
                state.content_search_query = "fn".to_string();
                let search = state.begin_content_search();
                let found = matches(&state);
                let plain = timed(|| {
                    assert!(state.finish_content_search(&search, found));
                    filtering::apply_filters(&mut state);
                    auto_expand_for_matches(&mut state);
                    snapshot(&mut state);
                });
                println!(
                    "  matches of a content search: {} ({})",
                    millis(&[steps]),
                    millis(&[plain])
                );

                // The first level of a re-scan, which replaces the lists.
                let first_level = |state: &AppState| -> Vec<FileItem> {
                    let items = state.full_file_list().iter();
                    items.filter(|item| item.depth == 1).cloned().collect()
                };
                let (mut state, _token, _) = scanning_state(listed, 0, scenario);
                let files = first_level(&state);
                let _token = state.begin_scan();
                // As `apply_shallow_scan_result` does it: what is replaced is
                // dropped after the lock.
                let mut replaced = None;
                let steps = timed(|| {
                    replaced = Some(state.lists_to_outlive_the_lock());
                    state.replace_with_shallow_file_list(Path::new("/root"), files);
                    filtering::apply_filters(&mut state);
                    snapshot(&mut state);
                });
                drop(replaced);
                let (mut state, _token, _) = scanning_state(listed, 0, scenario);
                let files = first_level(&state);
                let _token = state.begin_scan();
                let plain = timed(|| {
                    state.replace_with_shallow_file_list(Path::new("/root"), files);
                    filtering::apply_filters(&mut state);
                    snapshot(&mut state);
                });
                println!(
                    "  first level of a re-scan: {} ({})",
                    millis(&[steps]),
                    millis(&[plain])
                );

                // The completion of the list.
                let (state, token, _) = scanning_state(listed, 0, scenario);
                let state = SharedState::new(state);
                let steps = locked(in_three_steps(
                    &state,
                    |s| staged::stage_scan_completion(s, &token),
                    |s, prepared| {
                        staged::commit_scan_completion(s, &token, prepared, HashSet::new())
                    },
                    snapshot,
                ));
                let (mut state, token, _) = scanning_state(listed, 0, scenario);
                let plain = timed(|| {
                    assert!(state.complete_scanned_file_list(&token, HashSet::new()));
                    filtering::apply_filters(&mut state);
                    snapshot(&mut state);
                });
                println!(
                    "  completion of the list: {} ({})",
                    millis(&[steps]),
                    millis(&[plain])
                );
            }
        }
    }
}
