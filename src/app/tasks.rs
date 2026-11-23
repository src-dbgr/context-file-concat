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
use super::snapshot::send_snapshot;
use super::staged;
use super::state::{
    AfterAbandonedScan, AppState, GenerationToken, LazyLoadToken, PreviewToken, ScanToken,
    StatusNotice,
};
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
        let config = &state.config();
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
    send_snapshot(proxy, &state_guard);
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
            send_snapshot(self.proxy, &state);
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
            send_snapshot(&self.proxy, &state);
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
        send_snapshot(proxy, s);
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
/// If the state changed while the event was prepared, `commit` hands the event
/// back, staged for the state as it is, and it is prepared and committed a second
/// time; `commit` does not ask for a third.
///
/// Returns what all of this took, or `None` if the event is outdated (`stage`
/// returned `None`, or `commit` says so): nothing was changed then, and `committed`
/// was not called.
fn in_three_steps<C: staged::Change>(
    state: &SharedState,
    stage: impl FnOnce(&mut AppState) -> Option<staged::Staged<C>>,
    mut commit: impl FnMut(&mut AppState, staged::Prepared<C::Rest>) -> staged::Committed<C>,
    committed: impl FnOnce(&mut AppState),
) -> Option<PublicationCost> {
    let started = std::time::Instant::now();
    let (mut staged, mut longest_lock) = {
        let mut s = state.lock();
        let locked = std::time::Instant::now();
        (stage(&mut s)?, locked.elapsed())
    };

    let mut committed = Some(committed);
    let is_current = loop {
        let prepared = staged.prepare();

        let mut s = state.lock();
        let locked = std::time::Instant::now();
        let outcome = commit(&mut s, prepared);
        if matches!(outcome, staged::Committed::Done(true, _)) {
            if let Some(committed) = committed.take() {
                committed(&mut s);
            }
        }
        longest_lock = longest_lock.max(locked.elapsed());
        drop(s);
        // Lists and hierarchies that have been replaced: not while the state is
        // locked.
        match outcome {
            staged::Committed::Again(again, discarded) => {
                drop(discarded);
                staged = again;
            }
            staged::Committed::Done(is_current, discarded) => {
                drop(discarded);
                break is_current;
            }
        }
    };

    is_current.then(|| PublicationCost {
        total: started.elapsed(),
        locked: longest_lock,
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
    send_snapshot(proxy, &s);
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
        |s| staged::stage_scan_completion(s, token, patterns),
        |s, prepared| staged::commit_scan_completion(s, token, prepared),
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
    send_snapshot(proxy, &s);
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
    send_snapshot(&proxy, state_guard);
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
                send_snapshot(proxy, &state_guard);
            }
            if let Some(snapshot) =
                state_guard.edit_config(|config| config.last_directory = Some(directory.clone()))
            {
                save_config_in_background(snapshot);
            }
            let scanner = RealScanner {
                ignore_patterns: state_guard.config().ignore_patterns.clone(),
                max_file_size: state_guard.config().max_file_size_bytes(),
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
                    send_snapshot(proxy, &state_guard);
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
    send_snapshot(proxy, &state_lock);
}

/// The directory of the loaded list that `path` names, as the list writes it; `None`
/// if there is none (or no directory is loaded).
///
/// The list is the reference, not the disk: it is what the user was shown, and
/// comparing against it needs no file system access. Paths are compared name by
/// name, so a path that only resolves to a listed directory (`a/../b`) is not one.
fn listed_directory(state: &AppState, path: &Path) -> Option<PathBuf> {
    if state.current_path.is_empty() {
        return None;
    }
    state
        .full_file_list()
        .iter()
        .find(|item| item.is_directory && item.path == path)
        .map(|item| item.path.clone())
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
/// Only a directory of the loaded list can be loaded: the path has to be one the scan
/// has listed, as a directory (compared, not resolved; the file system is not asked
/// on the command lane). That is what the UI can ask for, since it only knows the
/// rows of the tree, and it leaves nothing a `..` in the path could get around; the
/// request for the scan checks the path a second time ([`ScanRequest::directory_level`]).
/// Any other path is refused: nothing is expanded or scanned, and the request is
/// answered with the state as it is.
///
/// Only the scan itself runs in a spawned task. Returns its handle, which completes
/// when the result has been dealt with (merged or discarded), or `None` if the request
/// was refused.
pub fn start_lazy_load_scan<P: EventProxy>(
    path: PathBuf,
    proxy: P,
    state: SharedState,
) -> Option<JoinHandle<()>> {
    let (scanner, token, request) = {
        let mut state_guard = state.lock();
        // The directory as the list names it, and the request for its level. The
        // project root anchors the ignore rules and the depth of the new items.
        let listed = listed_directory(&state_guard, &path);
        let request = listed.and_then(|directory| {
            ScanRequest::directory_level(&state_guard.current_path, &directory)
                .ok()
                .map(|request| (directory, request))
        });
        let Some((path, request)) = request else {
            // Refused, and answered like every request: with the state as it is.
            tracing::warn!("Refusing to load {path:?}: not a directory of the loaded list.");
            send_snapshot(&proxy, &state_guard);
            return None;
        };
        let token = state_guard.begin_lazy_load();
        state_guard.expand_directories([path]);
        send_snapshot(&proxy, &state_guard);
        let scanner = lazy_load_scanner(state_guard.config(), &token);
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
        |s| staged::stage_directory_level(s, token, loaded_dir, new_items, new_active_patterns),
        |s, prepared| staged::commit_directory_level(s, token, prepared),
        |s| send_snapshot(proxy, s),
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
        send_snapshot(&proxy, &state_guard);
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
            send_snapshot(&proxy, &state_guard);
            return;
        }
        let token = state_guard.begin_content_search();
        if start == SearchStart::Announced {
            // After `begin_content_search`: the filters know that a search is running.
            filtering::apply_filter_texts(&mut state_guard);
            send_snapshot(&proxy, &state_guard);
        }
        let request = ContentSearchRequest {
            query: state_guard.content_search_query.clone(),
            case_sensitive: state_guard.config().case_sensitive_search,
            max_file_size: state_guard.config().max_file_size_bytes(),
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
                send_snapshot(&proxy, state_guard);
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
            send_snapshot(&proxy, s);
        },
    );
    if applied.is_none() {
        tracing::debug!("Discarding outdated content search result for {:?}.", query);
    }
}

#[cfg(test)]
mod tests;
