//! Defines the central, mutable state of the application.

use crate::app::tree_index::{TreeIndex, TreeIndexCache};
use crate::config::{AppConfig, ConfigSnapshot, ConfigStore};
use crate::core::{FileItem, FileNameMatcher, ScanProgress};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Proof of identity of one background run, plus the means to tell it to stop.
#[derive(Debug, Clone)]
struct RunTicket {
    id: u64,
    cancel_flag: Arc<AtomicBool>,
}

impl RunTicket {
    fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

/// The run that currently occupies a [`RunSlot`].
struct ActiveRun {
    ticket: RunTicket,
    /// The task executing the run. `None` until it has been attached.
    task: Option<JoinHandle<()>>,
}

/// Exclusive ownership of one kind of background work (scanning, generating, searching).
///
/// At most one run is active per slot. Beginning a new run supersedes the previous one,
/// and only the ticket of the active run can attach a task to the slot or release it.
/// This is what keeps a task that is still winding down from touching the state of its
/// successor.
#[derive(Default)]
struct RunSlot {
    last_id: u64,
    active: Option<ActiveRun>,
}

impl RunSlot {
    /// Starts a new run, cancelling the one that is still active (if any).
    fn begin(&mut self) -> RunTicket {
        self.cancel();
        self.last_id += 1;
        let ticket = RunTicket {
            id: self.last_id,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        };
        self.active = Some(ActiveRun {
            ticket: ticket.clone(),
            task: None,
        });
        ticket
    }

    fn is_active(&self) -> bool {
        self.active.is_some()
    }

    fn is_current(&self, ticket: &RunTicket) -> bool {
        self.active
            .as_ref()
            .is_some_and(|run| run.ticket.id == ticket.id)
    }

    /// Registers the task executing the run identified by `ticket`. The task of a run
    /// that is no longer current is aborted instead.
    fn attach_task(&mut self, ticket: &RunTicket, handle: JoinHandle<()>) {
        match self.active.as_mut() {
            Some(run) if run.ticket.id == ticket.id => run.task = Some(handle),
            _ => handle.abort(),
        }
    }

    /// Releases the slot on behalf of the run identified by `ticket`, which has come to
    /// its end by itself. Returns `false` (and does nothing) for a run that is not current.
    fn finish(&mut self, ticket: &RunTicket) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.active = None;
        true
    }

    /// Stops the active run: raises its cancellation flag and aborts its task.
    /// Returns `false` if there was no active run.
    fn cancel(&mut self) -> bool {
        let Some(run) = self.active.take() else {
            return false;
        };
        run.ticket.cancel_flag.store(true, Ordering::SeqCst);
        if let Some(task) = run.task {
            task.abort();
        }
        true
    }

    #[cfg(test)]
    fn has_task(&self) -> bool {
        self.active.as_ref().is_some_and(|run| run.task.is_some())
    }
}

/// Identifies one run of a full directory scan.
///
/// A token is handed out by [`AppState::begin_scan`] and must accompany every state
/// mutation performed on behalf of that scan. Once the scan has finished, was cancelled
/// or was superseded by a newer scan, the token is stale and `AppState` ignores it, so a
/// task that is still winding down can never corrupt the state of its successor.
#[derive(Debug, Clone)]
pub struct ScanToken(RunTicket);

impl ScanToken {
    /// The cancellation flag owned by this scan, to be handed to the scanner.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.0.cancel_flag()
    }

    /// `true` once this scan has been cancelled or superseded by a newer scan.
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// Identifies one run of the content generation.
///
/// The counterpart of [`ScanToken`], handed out by [`AppState::begin_generation`]. A
/// generation that was cancelled or superseded holds a stale token and can neither
/// publish its result nor reset the generating state of its successor.
#[derive(Debug, Clone)]
pub struct GenerationToken(RunTicket);

impl GenerationToken {
    /// The cancellation flag owned by this generation, to be handed to the generator.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.0.cancel_flag()
    }

    /// `true` once this generation has been cancelled or superseded by a newer one.
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// Identifies one run of the content search.
///
/// Handed out by [`AppState::begin_content_search`]. Beginning a search supersedes the
/// one before it, so of several searches running concurrently only the most recently
/// started one may publish its matches, no matter in which order they finish.
#[derive(Debug, Clone)]
pub struct ContentSearchToken(RunTicket);

impl ContentSearchToken {
    /// The cancellation flag owned by this search, to be handed to the searcher so that
    /// it stops reading files nobody is waiting for any more.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.0.cancel_flag()
    }

    /// `true` once this search has been superseded by a newer one or cleared.
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// Identifies one request to preview a file.
///
/// Handed out by [`AppState::begin_preview`]. Reading the file takes place outside the
/// state; only the read that belongs to the most recent request may show its content,
/// so a slow read of a file the user has already left never replaces what the user is
/// looking at now.
#[derive(Debug, Clone)]
pub struct PreviewToken(RunTicket);

/// The most recent noteworthy outcome, shown to the user while nothing is in progress.
///
/// This is *what happened*, as data. How it is worded is decided by the view model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum StatusNotice {
    /// Nothing has happened yet (or the directory was reset).
    #[default]
    Ready,
    /// The scan was cancelled before it completed.
    ScanCancelled,
    /// A full scan completed.
    IndexingComplete {
        /// The number of items of the filtered list at that moment.
        visible_items: usize,
    },
    /// The scan ended with an error.
    ScanFailed {
        /// The error, in the words of the scanner.
        reason: String,
    },
    /// The generation of the output was cancelled.
    GenerationCancelled,
}

/// The progress of a scan that has not reported anything (yet).
fn no_scan_progress() -> ScanProgress {
    ScanProgress {
        files_scanned: 0,
        large_files_skipped: 0,
        current_scanning_path: String::new(),
    }
}

/// Ties a lazy (single directory level) load to the file list it was started for.
///
/// Handed out by [`AppState::begin_lazy_load`]. The token goes stale as soon as the file
/// list is replaced wholesale (new scan, directory reset) or the rules deciding what
/// belongs into it change (ignore patterns applied). From then on the lazy load is told
/// to stop via its cancellation flag, and whatever it still produces must be discarded
/// instead of being merged into a list it was not computed for.
#[derive(Debug, Clone)]
pub struct LazyLoadToken {
    file_list_generation: u64,
    cancel_flag: Arc<AtomicBool>,
}

impl LazyLoadToken {
    /// The cancellation flag of this lazy load, to be handed to the scanner.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }

    /// `true` once the file list this lazy load was started for has been replaced.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

/// A path that is being opened and has not been looked at yet, see
/// [`AppState::begin_open`].
#[derive(Debug, Clone)]
struct PendingOpen {
    path: PathBuf,
    /// What the opening superseded when it began.
    superseded: Superseded,
}

/// What an opening superseded when it began, i.e. what has to be taken up again if
/// the path turns out not to exist.
#[derive(Debug, Clone)]
enum Superseded {
    Nothing,
    /// A scan of the loaded directory.
    ScanOfLoadedDirectory,
    /// Another opening that had not found its path yet.
    Opening(Box<PendingOpen>),
}

/// What is to be done after a scan has been abandoned ([`AppState::abandon_scan`]), so
/// that an attempt to open a path that does not exist leaves nothing behind but its
/// error message.
#[derive(Debug)]
pub enum AfterAbandonedScan {
    /// Nothing: the scan had not superseded anything (or was not the current one).
    Nothing,
    /// The scan of the loaded directory that the abandoned scan had superseded has to
    /// be started again: the file list it was building is incomplete.
    RescanLoadedDirectory,
    /// The opening of `path` that the abandoned scan had superseded has been
    /// registered again, as the scan identified by `token`; its task has to be
    /// started.
    Reopen { path: PathBuf, token: ScanToken },
}

/// Holds the complete, mutable state of the application.
///
/// It is shared between threads (the event loop, the command lanes and background
/// tasks) as a [`SharedState`](super::shared_state::SharedState), the one place that
/// locks it.
pub struct AppState {
    /// The application's configuration settings.
    pub config: AppConfig,
    /// The absolute path to the currently loaded directory.
    pub current_path: String,
    /// The complete, unfiltered list of all files and directories found in the scan.
    pub full_file_list: Vec<FileItem>,
    /// The list of files and directories visible in the UI after applying filters.
    /// Private so that every change goes through [`AppState::set_filtered_file_list`].
    filtered_file_list: Vec<FileItem>,
    /// The set of absolute paths to files that are currently selected by the user.
    pub selected_files: HashSet<PathBuf>,
    /// The set of absolute paths to directories that are expanded in the UI tree.
    pub expanded_dirs: HashSet<PathBuf>,
    /// The set of absolute paths to directories whose children have been loaded.
    pub loaded_dirs: HashSet<PathBuf>,
    /// The current search query for filenames.
    pub search_query: String,
    /// The current filter for file extensions.
    pub extension_filter: String,
    /// The current search query for file content.
    pub content_search_query: String,
    /// The set of paths that match the current content search query.
    pub content_search_results: HashSet<PathBuf>,
    /// The filename of the currently loaded configuration file, if any.
    pub current_config_filename: Option<String>,
    /// The progress of the directory scan as it was last reported by the scan that is
    /// running (see [`AppState::record_scan_progress`]). `current_scanning_path` is the
    /// path the scan is working on and empty while there is none; the counters of a
    /// scan that has ended stay until the next one begins. Outcomes are kept in `status`.
    pub scan_progress: ScanProgress,
    /// The most recent outcome worth telling the user about.
    pub status: StatusNotice,
    /// The path of the file currently being previewed in the editor.
    pub previewed_file_path: Option<PathBuf>,
    /// The set of ignore patterns that were actually matched during the last scan.
    pub active_ignore_patterns: HashSet<String>,
    /// `true` if a full, non-lazy scan has been completed successfully.
    pub is_fully_scanned: bool,
    /// Indicates whether patterns were removed and a re-scan is recommended.
    /// This flag is set when ignore patterns are removed, as the current file list
    /// might be missing files that were previously filtered out.
    pub patterns_need_rescan: bool,
    /// Incremented whenever the file list is replaced wholesale (a scan begins or the
    /// directory is reset) or pruned by new ignore patterns. Lets background tasks detect
    /// that their results are outdated.
    file_list_generation: u64,
    /// Shared by all lazy loads started for the current file list; raised when that list
    /// is replaced, so that those loads stop instead of scanning for nothing.
    lazy_load_cancel_flag: Arc<AtomicBool>,
    /// The directory scan that is currently in progress, if any.
    scan: RunSlot,
    /// Set while the current scan has not yet found the path it is to open.
    opening: Option<PendingOpen>,
    /// The content generation that is currently in progress, if any.
    generation: RunSlot,
    /// The content search that is currently in progress, if any.
    content_search: RunSlot,
    /// The file preview that is currently being read, if any.
    preview: RunSlot,
    /// Persists the configuration, see [`AppState::config_snapshot`].
    config_store: ConfigStore,
    /// Incremented whenever `filtered_file_list` changes. Derived data (the tree index)
    /// is keyed by this revision.
    filtered_revision: u64,
    /// The hierarchy derived from `filtered_file_list`, see [`AppState::file_tree`].
    tree_index_cache: TreeIndexCache,
}

impl Default for AppState {
    /// An empty state with the default configuration that is not bound to a
    /// configuration file: nothing is read, and saving the configuration writes
    /// nothing ([`ConfigStore::detached`]).
    fn default() -> Self {
        Self::new(AppConfig::default(), ConfigStore::detached())
    }
}

impl AppState {
    /// An empty state (no directory loaded) with the given configuration, which is
    /// persisted through `config_store`.
    pub fn new(config: AppConfig, config_store: ConfigStore) -> Self {
        Self {
            config,
            current_path: String::new(),
            full_file_list: Vec::new(),
            filtered_file_list: Vec::new(),
            selected_files: HashSet::new(),
            expanded_dirs: HashSet::new(),
            loaded_dirs: HashSet::new(),
            search_query: String::new(),
            extension_filter: String::new(),
            content_search_query: String::new(),
            content_search_results: HashSet::new(),
            current_config_filename: None,
            scan_progress: no_scan_progress(),
            status: StatusNotice::Ready,
            previewed_file_path: None,
            active_ignore_patterns: HashSet::new(),
            is_fully_scanned: false,
            patterns_need_rescan: false,
            file_list_generation: 0,
            lazy_load_cancel_flag: Arc::new(AtomicBool::new(false)),
            scan: RunSlot::default(),
            opening: None,
            generation: RunSlot::default(),
            content_search: RunSlot::default(),
            preview: RunSlot::default(),
            config_store,
            filtered_revision: 0,
            tree_index_cache: TreeIndexCache::default(),
        }
    }

    /// `true` if a directory scan is currently in progress.
    pub fn is_scanning(&self) -> bool {
        self.scan.is_active()
    }

    /// Starts a new scan run and returns the token identifying it.
    ///
    /// Any scan that is still active is superseded: its task is aborted, its cancellation
    /// flag is raised and its token becomes stale. The returned token is the only key
    /// that allows a scan task to modify the scanning state from now on. Because the scan
    /// is going to replace the file list, pending lazy loads are cancelled as well.
    pub fn begin_scan(&mut self) -> ScanToken {
        self.invalidate_file_list();
        self.opening = None;
        self.scan_progress = no_scan_progress();
        ScanToken(self.scan.begin())
    }

    /// Starts the scan that is going to open another path, and returns its token.
    ///
    /// Like [`AppState::begin_scan`] this supersedes the scan that is running, and the
    /// new scan is visible and cancellable at once. Unlike it, nothing else changes
    /// yet: whether the path exists is only known once the scan task has looked at it,
    /// and until then the loaded directory stays as it is, lazy loads included. The
    /// task then either replaces it ([`AppState::enter_opened_directory`]) or gives up
    /// and leaves it alone ([`AppState::abandon_scan`]). `path` is what is being
    /// opened, as it was given.
    pub fn begin_open(&mut self, path: &Path) -> ScanToken {
        let superseded = match self.opening.take() {
            Some(pending) => Superseded::Opening(Box::new(pending)),
            None if self.scan.is_active() => Superseded::ScanOfLoadedDirectory,
            None => Superseded::Nothing,
        };
        self.register_open(PendingOpen {
            path: path.to_path_buf(),
            superseded,
        })
    }

    fn register_open(&mut self, pending: PendingOpen) -> ScanToken {
        self.opening = Some(pending);
        self.scan_progress = no_scan_progress();
        ScanToken(self.scan.begin())
    }

    /// `true` while the current scan has not yet found the path it is to open, i.e.
    /// while the directory in the state is still the one that was loaded before.
    pub fn is_opening(&self) -> bool {
        self.opening.is_some()
    }

    /// Makes `directory` the loaded one on behalf of the scan identified by `token`,
    /// which has found the path it was to open: everything that belonged to the
    /// previous directory is reset, the scan keeps running.
    ///
    /// Does nothing and returns `false` for a stale token and for a scan that is not
    /// opening a path (a re-scan keeps the directory it has).
    pub fn enter_opened_directory(&mut self, token: &ScanToken, directory: &Path) -> bool {
        if !self.is_current_scan(token) || self.opening.take().is_none() {
            return false;
        }
        self.clear_directory_data();
        self.current_path = directory.to_string_lossy().to_string();
        true
    }

    /// Ends the scan identified by `token` because there is nothing it could scan (its
    /// path does not exist). The loaded directory stays as it is.
    ///
    /// A scan that was to open a path superseded whatever was running when it began: a
    /// scan of the loaded directory, or another opening. That work would be lost for
    /// an attempt that came to nothing, so it is taken up again; the return value says
    /// what the caller has to do for it. Does nothing for a stale token.
    pub fn abandon_scan(&mut self, token: &ScanToken) -> AfterAbandonedScan {
        let pending = self.opening.take();
        if !self.finish_scan(token) {
            self.opening = pending;
            return AfterAbandonedScan::Nothing;
        }
        match pending.map(|pending| pending.superseded) {
            Some(Superseded::Opening(previous)) => AfterAbandonedScan::Reopen {
                path: previous.path.clone(),
                token: self.register_open(*previous),
            },
            Some(Superseded::ScanOfLoadedDirectory) if !self.current_path.is_empty() => {
                AfterAbandonedScan::RescanLoadedDirectory
            }
            _ => AfterAbandonedScan::Nothing,
        }
    }

    /// Stores the progress reported by the scan identified by `token`, so that the
    /// next state update shows how far the scan has got. Progress of a scan that is
    /// no longer the current one is ignored.
    ///
    /// Returns `true` if the progress was stored.
    pub fn record_scan_progress(&mut self, token: &ScanToken, progress: ScanProgress) -> bool {
        let is_current = self.is_current_scan(token);
        if is_current {
            self.scan_progress = progress;
        }
        is_current
    }

    /// `true` if `token` belongs to the scan that currently owns the scanning state.
    pub fn is_current_scan(&self, token: &ScanToken) -> bool {
        self.scan.is_current(&token.0)
    }

    /// Stores the handle of the task that runs the scan identified by `token`.
    ///
    /// If the scan is no longer current (it already finished, was cancelled or was
    /// superseded before the handle could be registered), the task is aborted instead,
    /// so a stale handle never replaces the one of a newer scan.
    pub fn attach_scan_task(&mut self, token: &ScanToken, handle: JoinHandle<()>) {
        self.scan.attach_task(&token.0, handle);
    }

    /// Marks the scan identified by `token` as finished and releases the scanning state.
    ///
    /// Does nothing for a stale token. Returns `true` if this call ended the running
    /// scan, i.e. if the UI needs to be told about the change.
    pub fn finish_scan(&mut self, token: &ScanToken) -> bool {
        if !self.scan.finish(&token.0) {
            return false;
        }
        self.opening = None;
        // The counters stay as the result of the scan; it works on no path any more.
        self.scan_progress.current_scanning_path.clear();
        true
    }

    /// Cancels the current scan, if any, and resets the scanning state.
    ///
    /// A scan counts as active from `begin_scan` on, even before its task handle has
    /// been attached, so a cancellation issued in that window is not lost.
    pub fn cancel_current_scan(&mut self) {
        if !self.scan.cancel() {
            tracing::debug!("No active scan to cancel.");
            return;
        }

        self.opening = None;
        self.scan_progress = no_scan_progress();
        self.status = StatusNotice::ScanCancelled;
        tracing::info!("Scan cancelled.");
    }

    /// Returns a token for a lazy load of a single directory level, or `None` if a full
    /// scan is in progress (which will load that level anyway).
    pub fn begin_lazy_load(&self) -> Option<LazyLoadToken> {
        if self.is_scanning() {
            return None;
        }
        Some(LazyLoadToken {
            file_list_generation: self.file_list_generation,
            cancel_flag: self.lazy_load_cancel_flag.clone(),
        })
    }

    /// `true` if the file list the lazy load was started for is still the current one.
    pub fn is_current_lazy_load(&self, token: &LazyLoadToken) -> bool {
        self.file_list_generation == token.file_list_generation
    }

    /// Declares the current file list obsolete because it is about to be replaced
    /// wholesale, or because the rules that decide what belongs into it have changed:
    /// lazy loads started for it go stale and are told to stop.
    fn invalidate_file_list(&mut self) {
        self.file_list_generation += 1;
        let obsolete_flag = std::mem::replace(
            &mut self.lazy_load_cancel_flag,
            Arc::new(AtomicBool::new(false)),
        );
        obsolete_flag.store(true, Ordering::SeqCst);
    }

    /// `true` if the concatenated output is currently being generated.
    pub fn is_generating(&self) -> bool {
        self.generation.is_active()
    }

    /// Starts a new generation run and returns the token identifying it.
    ///
    /// A generation that is still active is superseded: its task is aborted, its
    /// cancellation flag is raised and its token becomes stale.
    pub fn begin_generation(&mut self) -> GenerationToken {
        GenerationToken(self.generation.begin())
    }

    /// `true` if `token` belongs to the generation that currently owns the generating state.
    pub fn is_current_generation(&self, token: &GenerationToken) -> bool {
        self.generation.is_current(&token.0)
    }

    /// Stores the handle of the task that runs the generation identified by `token`.
    /// The task of a generation that is no longer current is aborted instead.
    pub fn attach_generation_task(&mut self, token: &GenerationToken, handle: JoinHandle<()>) {
        self.generation.attach_task(&token.0, handle);
    }

    /// Marks the generation identified by `token` as finished and releases the generating
    /// state.
    ///
    /// Does nothing for a stale token. Returns `true` if this call ended the running
    /// generation, i.e. if its outcome may be published to the UI.
    pub fn finish_generation(&mut self, token: &GenerationToken) -> bool {
        self.generation.finish(&token.0)
    }

    /// Cancels the current generation, if any, and resets the generating state.
    pub fn cancel_current_generation(&mut self) {
        self.generation.cancel();
    }

    /// Starts a new content search run and returns the token identifying it.
    ///
    /// A search that is still running is superseded: its token becomes stale, so its
    /// matches (for an older query or an older file list) are never published.
    pub fn begin_content_search(&mut self) -> ContentSearchToken {
        ContentSearchToken(self.content_search.begin())
    }

    /// Stores the matches of the search identified by `token` and releases the slot.
    ///
    /// Does nothing for a stale token. Returns `true` if the matches were accepted, i.e.
    /// if the filters need to be re-applied.
    pub fn finish_content_search(
        &mut self,
        token: &ContentSearchToken,
        matches: HashSet<PathBuf>,
    ) -> bool {
        if !self.content_search.finish(&token.0) {
            return false;
        }
        self.content_search_results = matches;
        true
    }

    /// Drops the matches of the content search and supersedes a search that is still
    /// running, so it cannot bring matches back. To be used when the query is cleared.
    pub fn clear_content_search(&mut self) {
        self.content_search.cancel();
        self.content_search_results.clear();
    }

    /// Makes `path` the previewed file and returns the token of the read that is going
    /// to deliver its content. A read that is still under way is superseded.
    pub fn begin_preview(&mut self, path: PathBuf) -> PreviewToken {
        self.previewed_file_path = Some(path);
        PreviewToken(self.preview.begin())
    }

    /// Ends the read identified by `token`. Returns `true` if it belongs to the most
    /// recent preview request, i.e. if its content may be shown; `false` (and nothing
    /// happens) if the preview was replaced or closed in the meantime.
    pub fn finish_preview(&mut self, token: &PreviewToken) -> bool {
        self.preview.finish(&token.0)
    }

    /// Closes the preview: no file is previewed, and a read that is still under way
    /// will not show its content any more.
    pub fn clear_preview(&mut self) {
        self.preview.cancel();
        self.previewed_file_path = None;
    }

    /// Captures the configuration for saving it outside the state lock.
    ///
    /// Take the snapshot while holding the lock, right after the change, and save it
    /// afterwards: the snapshots are numbered in the order they are taken, which is
    /// what keeps a save that is overtaken by a later one from overwriting it (see
    /// [`ConfigStore`]). This is the only way the application saves its configuration.
    pub fn config_snapshot(&self) -> ConfigSnapshot {
        self.config_store.snapshot(&self.config)
    }

    /// The store the configuration is persisted through. It can be asked for the most
    /// recent snapshot without locking the state.
    pub fn config_store(&self) -> ConfigStore {
        self.config_store.clone()
    }

    /// The matcher for the filename query and the extension filter as they are now.
    pub fn file_name_matcher(&self) -> FileNameMatcher {
        FileNameMatcher::new(
            &self.search_query,
            &self.extension_filter,
            self.config.case_sensitive_search,
        )
    }

    /// Makes `files` the result of the first, shallow phase of a scan of `root`.
    ///
    /// The list then holds a single level, so `root` is the only directory whose
    /// children are loaded; whatever was loaded in the list before is forgotten.
    pub fn replace_with_shallow_file_list(&mut self, root: &std::path::Path, files: Vec<FileItem>) {
        self.full_file_list = files;
        self.loaded_dirs = HashSet::from([root.to_path_buf()]);
    }

    /// Makes `files` the complete content of the project: every directory in it is loaded.
    pub fn replace_with_complete_file_list(&mut self, files: Vec<FileItem>) {
        self.loaded_dirs = files
            .iter()
            .filter(|item| item.is_directory)
            .map(|item| item.path.clone())
            .collect();
        self.full_file_list = files;
    }

    /// `true` if the task of the current scan has been attached.
    #[cfg(test)]
    pub(crate) fn has_scan_task(&self) -> bool {
        self.scan.has_task()
    }

    /// `true` if the task of the current generation has been attached.
    #[cfg(test)]
    pub(crate) fn has_generation_task(&self) -> bool {
        self.generation.has_task()
    }

    /// The files and directories visible in the UI after applying filters.
    pub fn filtered_file_list(&self) -> &[FileItem] {
        &self.filtered_file_list
    }

    /// Replaces the filtered file list.
    ///
    /// This is the only way to change the list, which guarantees that data derived from
    /// it is never used with a different list than it was built from.
    pub fn set_filtered_file_list(&mut self, items: Vec<FileItem>) {
        self.filtered_file_list = items;
        self.filtered_revision += 1;
    }

    /// The hierarchy of the filtered file list below the current path.
    ///
    /// The index refers to items by their position in [`AppState::filtered_file_list`].
    /// It is derived data: built on first use and reused until the list (tracked by its
    /// revision) or the current path changes, so asking for it repeatedly is cheap.
    pub(crate) fn file_tree(&self) -> Arc<TreeIndex> {
        self.tree_index_cache.get_or_build(
            self.filtered_revision,
            Path::new(&self.current_path),
            &self.filtered_file_list,
        )
    }

    /// Identifies the current content of the filtered file list.
    #[cfg(test)]
    pub(crate) fn filtered_revision(&self) -> u64 {
        self.filtered_revision
    }

    /// How often the hierarchy of the filtered file list had to be built.
    #[cfg(test)]
    pub(crate) fn file_tree_build_count(&self) -> usize {
        self.tree_index_cache.build_count()
    }

    /// Resets all state related to a loaded directory, including the scan that is
    /// running.
    pub fn reset_directory_state(&mut self) {
        self.cancel_current_scan();
        self.clear_directory_data();
    }

    /// Forgets the loaded directory and everything derived from it. The scan slot is
    /// left alone: the caller decides what becomes of the scan that is running.
    fn clear_directory_data(&mut self) {
        self.cancel_current_generation();
        self.invalidate_file_list();

        self.current_path = String::new();
        self.full_file_list.clear();
        self.set_filtered_file_list(Vec::new());
        self.selected_files.clear();
        self.expanded_dirs.clear();
        self.loaded_dirs.clear();
        self.search_query.clear();
        self.extension_filter.clear();
        self.content_search_query.clear();
        self.clear_content_search();
        self.clear_preview();
        self.active_ignore_patterns.clear();
        self.is_fully_scanned = false;
        self.patterns_need_rescan = false;
        self.scan_progress = no_scan_progress();
        self.status = StatusNotice::Ready;
    }

    /// Applies the complete set of current ignore patterns to the in-memory file lists.
    /// This function re-builds the matcher from `self.config.ignore_patterns`
    /// and filters `full_file_list` and `selected_files` accordingly.
    ///
    /// Lazy loads that are still in flight were started with the previous patterns and
    /// would bring back entries that are ignored now, so they are invalidated.
    pub fn apply_ignore_patterns(&mut self) {
        if self.config.ignore_patterns.is_empty() {
            return;
        }
        self.invalidate_file_list();
        let rules = crate::core::IgnoreRules::new(&self.current_path, &self.config.ignore_patterns);

        let path_info: std::collections::HashMap<_, _> = self
            .full_file_list
            .iter()
            .map(|item| (item.path.clone(), item.is_directory))
            .collect();

        self.full_file_list
            .retain(|item| !rules.is_ignored(&item.path, item.is_directory));

        self.selected_files.retain(|path| {
            let is_dir = path_info.get(path).copied().unwrap_or(false);
            !rules.is_ignored(path, is_dir)
        });

        // Note: active_ignore_patterns are typically recalculated during a full scan
        // or could be updated here if needed, but for local filtering this is sufficient.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::FileItem;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// Creates a dummy tokio JoinHandle for testing purposes.
    fn create_dummy_task() -> JoinHandle<()> {
        tokio::spawn(async {
            // The task can sleep for a bit to simulate work,
            // though it's not strictly necessary as we abort it immediately.
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
    }

    /// Creates a simple FileItem for testing.
    fn create_test_file_item(path_str: &str, is_dir: bool) -> FileItem {
        FileItem {
            path: PathBuf::from(path_str),
            is_directory: is_dir,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_cancel_scan_with_active_task() {
        // Arrange
        let mut state = AppState::default();
        let token = state.begin_scan();
        state.attach_scan_task(&token, create_dummy_task());
        state.scan_progress.current_scanning_path = "In progress...".to_string();

        // Act
        state.cancel_current_scan();
        // Give tokio a moment to process the abort
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Assert
        assert!(
            !state.has_scan_task(),
            "Scan task should be taken from the state"
        );
        assert!(!state.is_scanning(), "is_scanning should be false");
        assert_eq!(
            state.status,
            StatusNotice::ScanCancelled,
            "The cancellation should be noted"
        );
        assert!(
            state.scan_progress.current_scanning_path.is_empty(),
            "The progress of the cancelled scan should be gone"
        );
        assert!(
            token.is_cancelled(),
            "Cancellation flag should be set to true"
        );
    }

    #[tokio::test]
    async fn test_cancel_scan_with_no_task_does_not_panic() {
        // Arrange
        let mut state = AppState::default();
        assert!(!state.is_scanning(), "precondition");
        assert!(!state.has_scan_task(), "precondition");

        // Act & Assert
        // The test passes if this call does not panic.
        state.cancel_current_scan();
        assert!(!state.is_scanning());
    }

    #[tokio::test]
    async fn test_begin_scan_marks_state_as_scanning_with_fresh_flag() {
        let mut state = AppState::default();

        let token = state.begin_scan();

        assert!(state.is_scanning());
        assert!(state.is_current_scan(&token));
        assert!(!token.is_cancelled());
        state.cancel_current_scan();
        assert!(
            token.is_cancelled(),
            "State must control the cancellation flag of the current scan"
        );
    }

    #[tokio::test]
    async fn test_begin_scan_supersedes_previous_scan() {
        let mut state = AppState::default();
        let old_token = state.begin_scan();
        let old_handle = create_dummy_task();
        let old_abort = old_handle.abort_handle();
        state.attach_scan_task(&old_token, old_handle);

        let new_token = state.begin_scan();
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(old_token.is_cancelled(), "Old scan must be told to stop");
        assert!(old_abort.is_finished(), "Old scan task must be aborted");
        assert!(!state.is_current_scan(&old_token));
        assert!(state.is_current_scan(&new_token));
        assert!(!new_token.is_cancelled());
        assert!(state.is_scanning());
    }

    #[tokio::test]
    async fn test_finish_scan_with_stale_token_leaves_new_scan_untouched() {
        let mut state = AppState::default();
        let old_token = state.begin_scan();
        let new_token = state.begin_scan();
        state.attach_scan_task(&new_token, create_dummy_task());

        let changed = state.finish_scan(&old_token);

        assert!(!changed);
        assert!(state.is_scanning(), "New scan must still be running");
        assert!(state.has_scan_task(), "New scan must stay cancellable");
        assert!(state.is_current_scan(&new_token));
        state.cancel_current_scan();
    }

    #[tokio::test]
    async fn test_finish_scan_with_current_token_releases_scanning_state() {
        let mut state = AppState::default();
        let token = state.begin_scan();
        state.attach_scan_task(&token, create_dummy_task());

        assert!(state.finish_scan(&token));

        assert!(!state.is_scanning());
        assert!(!state.has_scan_task());
        assert!(!state.is_current_scan(&token));
        assert!(!state.finish_scan(&token), "Finishing twice is a no-op");
    }

    #[tokio::test]
    async fn test_attach_scan_task_with_stale_token_aborts_instead_of_storing() {
        let mut state = AppState::default();
        let old_token = state.begin_scan();
        let new_token = state.begin_scan();
        let new_handle = create_dummy_task();
        let new_abort = new_handle.abort_handle();
        state.attach_scan_task(&new_token, new_handle);

        let stale_handle = create_dummy_task();
        let stale_abort = stale_handle.abort_handle();
        state.attach_scan_task(&old_token, stale_handle);
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(stale_abort.is_finished(), "Stale task must be aborted");
        assert!(!new_abort.is_finished(), "Current task must keep running");
        assert!(state.has_scan_task());
        state.cancel_current_scan();
    }

    #[tokio::test]
    async fn test_cancel_scan_before_task_is_attached_is_not_lost() {
        let mut state = AppState::default();
        let token = state.begin_scan();

        state.cancel_current_scan();

        assert!(!state.is_scanning());
        assert!(token.is_cancelled());
        assert!(!state.is_current_scan(&token));
        assert_eq!(state.status, StatusNotice::ScanCancelled);

        // The task handle arriving late must not resurrect the cancelled scan.
        let late_handle = create_dummy_task();
        let late_abort = late_handle.abort_handle();
        state.attach_scan_task(&token, late_handle);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!state.has_scan_task());
        assert!(late_abort.is_finished());
    }

    #[tokio::test]
    async fn test_begin_lazy_load_is_refused_while_scanning() {
        let mut state = AppState::default();
        assert!(state.begin_lazy_load().is_some());

        let _token = state.begin_scan();

        assert!(state.begin_lazy_load().is_none());
    }

    #[tokio::test]
    async fn test_lazy_load_token_goes_stale_when_file_list_is_replaced() {
        let mut state = AppState::default();

        let token = state.begin_lazy_load().expect("not scanning");
        assert!(state.is_current_lazy_load(&token));
        let scan = state.begin_scan();
        assert!(!state.is_current_lazy_load(&token), "stale after new scan");
        state.finish_scan(&scan);

        let token = state.begin_lazy_load().expect("not scanning");
        state.reset_directory_state();
        assert!(!state.is_current_lazy_load(&token), "stale after reset");
    }

    #[tokio::test]
    async fn test_lazy_load_is_cancelled_when_a_new_scan_begins() {
        let mut state = AppState::default();
        let lazy = state.begin_lazy_load().expect("not scanning");
        let scanner_flag = lazy.cancel_flag();
        assert!(!lazy.is_cancelled());

        let scan = state.begin_scan();

        assert!(lazy.is_cancelled(), "Superseded lazy load must stop");
        assert!(scanner_flag.load(Ordering::SeqCst));
        assert!(
            !scan.is_cancelled(),
            "The new scan itself must keep running"
        );
    }

    #[tokio::test]
    async fn test_lazy_load_is_cancelled_when_directory_is_reset() {
        let mut state = AppState::default();
        let lazy = state.begin_lazy_load().expect("not scanning");

        state.reset_directory_state();

        assert!(
            lazy.is_cancelled(),
            "Lazy load of a dropped directory must stop"
        );
    }

    #[tokio::test]
    async fn test_lazy_load_started_for_the_new_file_list_is_not_cancelled() {
        let mut state = AppState::default();
        let outdated = state.begin_lazy_load().expect("not scanning");
        let scan = state.begin_scan();
        state.finish_scan(&scan);

        let current = state.begin_lazy_load().expect("scan has finished");
        let sibling = state.begin_lazy_load().expect("scan has finished");

        assert!(outdated.is_cancelled());
        assert!(!current.is_cancelled());
        assert!(state.is_current_lazy_load(&current));

        // Loads of the same file list are replaced together.
        state.reset_directory_state();
        assert!(current.is_cancelled());
        assert!(sibling.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancelling_a_scan_leaves_the_file_list_and_its_lazy_loads_alone() {
        let mut state = AppState::default();
        let scan = state.begin_scan();
        state.finish_scan(&scan);
        let lazy = state.begin_lazy_load().expect("scan has finished");

        state.cancel_current_scan();
        state.cancel_current_generation();

        assert!(!lazy.is_cancelled());
        assert!(state.is_current_lazy_load(&lazy));
    }

    #[tokio::test]
    async fn test_begin_generation_marks_state_as_generating() {
        let mut state = AppState::default();
        assert!(!state.is_generating());

        let token = state.begin_generation();

        assert!(state.is_generating());
        assert!(state.is_current_generation(&token));
        assert!(!token.is_cancelled());
        assert!(
            !state.is_scanning(),
            "Generating is independent of scanning"
        );
    }

    #[tokio::test]
    async fn test_begin_generation_supersedes_previous_generation() {
        let mut state = AppState::default();
        let old_token = state.begin_generation();
        let old_handle = create_dummy_task();
        let old_abort = old_handle.abort_handle();
        state.attach_generation_task(&old_token, old_handle);

        let new_token = state.begin_generation();
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(
            old_token.is_cancelled(),
            "Old generation must be told to stop"
        );
        assert!(
            old_abort.is_finished(),
            "Old generation task must be aborted"
        );
        assert!(!state.is_current_generation(&old_token));
        assert!(state.is_current_generation(&new_token));
        assert!(!new_token.is_cancelled());
        assert!(state.is_generating());
    }

    #[tokio::test]
    async fn test_finish_generation_with_stale_token_leaves_new_generation_untouched() {
        let mut state = AppState::default();
        let old_token = state.begin_generation();
        let new_token = state.begin_generation();
        state.attach_generation_task(&new_token, create_dummy_task());

        let finished = state.finish_generation(&old_token);

        assert!(!finished);
        assert!(
            state.is_generating(),
            "New generation must still be running"
        );
        assert!(
            state.has_generation_task(),
            "New generation must stay cancellable"
        );
        assert!(state.is_current_generation(&new_token));
        state.cancel_current_generation();
    }

    #[tokio::test]
    async fn test_finish_generation_with_current_token_releases_generating_state() {
        let mut state = AppState::default();
        let token = state.begin_generation();
        state.attach_generation_task(&token, create_dummy_task());

        assert!(state.finish_generation(&token));

        assert!(!state.is_generating());
        assert!(!state.has_generation_task());
        assert!(!token.is_cancelled(), "Finishing is not cancelling");
        assert!(
            !state.finish_generation(&token),
            "Finishing twice is a no-op"
        );
    }

    #[tokio::test]
    async fn test_cancelled_generation_cannot_be_finished_or_resurrected() {
        let mut state = AppState::default();
        let token = state.begin_generation();

        state.cancel_current_generation();

        assert!(!state.is_generating());
        assert!(token.is_cancelled());
        assert!(!state.finish_generation(&token));

        // The task handle arriving late must not resurrect the cancelled generation.
        let late_handle = create_dummy_task();
        let late_abort = late_handle.abort_handle();
        state.attach_generation_task(&token, late_handle);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!state.has_generation_task());
        assert!(late_abort.is_finished());
    }

    #[tokio::test]
    async fn test_reset_directory_state_cancels_scan_and_generation() {
        let mut state = AppState::default();
        let scan = state.begin_scan();
        let generation = state.begin_generation();

        state.reset_directory_state();

        assert!(scan.is_cancelled());
        assert!(generation.is_cancelled());
        assert!(!state.is_scanning());
        assert!(!state.is_generating());
    }

    #[tokio::test]
    async fn test_cancel_generation_with_active_task() {
        // Arrange
        let mut state = AppState::default();
        let token = state.begin_generation();
        state.attach_generation_task(&token, create_dummy_task());

        // Act
        state.cancel_current_generation();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Assert
        assert!(
            !state.has_generation_task(),
            "Generation task should be cleared"
        );
        assert!(!state.is_generating(), "is_generating should be false");
        assert!(
            token.is_cancelled(),
            "Generation cancellation flag should be set"
        );
    }

    #[tokio::test]
    async fn test_cancel_generation_with_no_task_does_not_panic() {
        // Arrange
        let mut state = AppState::default();
        assert!(!state.is_generating(), "precondition");
        assert!(!state.has_generation_task(), "precondition");

        // Act & Assert
        // The test passes if this call does not panic.
        state.cancel_current_generation();
        assert!(!state.is_generating());
    }

    #[tokio::test]
    async fn test_reset_directory_state_clears_all_relevant_fields() {
        // Arrange
        let mut state = AppState {
            current_path: "/test/project".to_string(),
            full_file_list: vec![create_test_file_item("/test/project/file.txt", false)],
            selected_files: HashSet::from([PathBuf::from("/test/project/file.txt")]),
            expanded_dirs: HashSet::from([PathBuf::from("/test/project")]),
            loaded_dirs: HashSet::from([PathBuf::from("/test/project")]),
            search_query: "test".to_string(),
            is_fully_scanned: true,
            ..Default::default()
        };
        state.set_filtered_file_list(state.full_file_list.clone());
        let scan = state.begin_scan();
        state.attach_scan_task(&scan, create_dummy_task());
        let generation = state.begin_generation();
        state.attach_generation_task(&generation, create_dummy_task());
        state.patterns_need_rescan = true;

        // Act
        state.reset_directory_state();

        // Assert
        assert!(state.current_path.is_empty());
        assert!(state.full_file_list.is_empty());
        assert!(state.filtered_file_list().is_empty());
        assert!(state.selected_files.is_empty());
        assert!(state.expanded_dirs.is_empty());
        assert!(state.loaded_dirs.is_empty());
        assert!(state.search_query.is_empty());
        assert!(!state.is_fully_scanned);
        assert!(state.previewed_file_path.is_none());
        assert!(!state.is_generating());
        assert_eq!(state.status, StatusNotice::Ready);

        // Assert that the tasks were cancelled and removed
        assert!(!state.has_scan_task());
        assert!(!state.has_generation_task());
        assert!(!state.patterns_need_rescan);
    }

    #[tokio::test]
    async fn test_apply_ignore_patterns_removes_files_and_selections() {
        // Arrange
        let mut state = AppState::default();
        // NOTE: Ensure a clean state, independent of any config file on disk.
        state.config.ignore_patterns.clear();
        state.current_path = "/project".to_string();

        let file_to_keep = create_test_file_item("/project/src/main.rs", false);
        let dir_to_ignore = create_test_file_item("/project/node_modules", true);
        let file_to_ignore = create_test_file_item("/project/node_modules/dep.js", false);

        state.full_file_list = vec![
            file_to_keep.clone(),
            dir_to_ignore.clone(),
            file_to_ignore.clone(),
        ];
        state.selected_files =
            HashSet::from([file_to_keep.path.clone(), file_to_ignore.path.clone()]);

        // Set the patterns for this specific test case.
        state.config.ignore_patterns = HashSet::from(["node_modules/".to_string()]);

        // Act
        state.apply_ignore_patterns();

        // Assert
        assert_eq!(
            state.full_file_list.len(),
            1,
            "Only one item should remain in the full list"
        );
        assert_eq!(
            state.full_file_list[0].path, file_to_keep.path,
            "The remaining file should be main.rs"
        );
        assert_eq!(
            state.selected_files.len(),
            1,
            "Only one item should remain selected"
        );
        assert!(
            state.selected_files.contains(&file_to_keep.path),
            "main.rs should still be selected"
        );
    }

    #[tokio::test]
    async fn test_apply_ignore_patterns_with_empty_set_does_nothing() {
        // Arrange
        let mut state = AppState::default();
        // NOTE: Ensure a clean state, independent of any config file on disk.
        state.config.ignore_patterns.clear();

        let initial_file_list = vec![create_test_file_item("/project/file.txt", false)];
        state.full_file_list = initial_file_list.clone();

        // Act
        state.apply_ignore_patterns();

        // Assert
        assert_eq!(state.full_file_list.len(), initial_file_list.len());
        assert_eq!(state.full_file_list[0].path, initial_file_list[0].path);
    }

    #[tokio::test]
    async fn test_newer_content_search_supersedes_older_one() {
        let mut state = AppState::default();
        let older = state.begin_content_search();
        let newer = state.begin_content_search();
        let older_matches = HashSet::from([PathBuf::from("/p/older.txt")]);
        let newer_matches = HashSet::from([PathBuf::from("/p/newer.txt")]);

        // The older search finishes last and must be ignored, whatever the order.
        assert!(state.finish_content_search(&newer, newer_matches.clone()));
        assert!(!state.finish_content_search(&older, older_matches));

        assert_eq!(state.content_search_results, newer_matches);
        assert!(
            !state.finish_content_search(&newer, HashSet::new()),
            "A search can only publish once"
        );
        assert_eq!(state.content_search_results, newer_matches);
    }

    #[tokio::test]
    async fn test_clear_content_search_discards_running_search() {
        let mut state = AppState::default();
        state
            .content_search_results
            .insert(PathBuf::from("/p/old.txt"));
        let running = state.begin_content_search();

        state.clear_content_search();

        assert!(state.content_search_results.is_empty());
        assert!(!state.finish_content_search(&running, HashSet::from([PathBuf::from("/p/a")])));
        assert!(state.content_search_results.is_empty());
    }

    #[tokio::test]
    async fn test_reset_directory_state_discards_running_content_search() {
        let mut state = AppState::default();
        let running = state.begin_content_search();

        state.reset_directory_state();

        assert!(!state.finish_content_search(&running, HashSet::from([PathBuf::from("/p/a")])));
        assert!(state.content_search_results.is_empty());
    }

    #[tokio::test]
    async fn test_content_search_is_told_to_stop_when_superseded_or_cleared() {
        let mut state = AppState::default();

        let first = state.begin_content_search();
        let first_flag = first.cancel_flag();
        assert!(!first.is_cancelled());

        let second = state.begin_content_search();
        assert!(first.is_cancelled(), "A newer search stops the older one");
        assert!(first_flag.load(Ordering::SeqCst));
        assert!(!second.is_cancelled());

        state.clear_content_search();
        assert!(second.is_cancelled(), "Clearing the query stops the search");

        let third = state.begin_content_search();
        assert!(state.finish_content_search(&third, HashSet::new()));
        assert!(
            !third.is_cancelled(),
            "A search that came to its end was never cancelled"
        );
    }

    #[tokio::test]
    async fn test_apply_ignore_patterns_invalidates_pending_lazy_loads() {
        let mut state = AppState::default();
        state.config.ignore_patterns.clear();
        state.current_path = "/project".to_string();
        let token = state.begin_lazy_load().expect("No scan is running");

        // Without patterns nothing changes, so the lazy load stays valid.
        state.apply_ignore_patterns();
        assert!(state.is_current_lazy_load(&token));

        state.config.ignore_patterns.insert("target/".to_string());
        state.apply_ignore_patterns();

        assert!(!state.is_current_lazy_load(&token));
        assert!(token.is_cancelled(), "The lazy scan must be told to stop");
        let fresh = state.begin_lazy_load().expect("No scan is running");
        assert!(state.is_current_lazy_load(&fresh) && !fresh.is_cancelled());
    }

    #[tokio::test]
    async fn test_shallow_file_list_forgets_previously_loaded_directories() {
        let mut state = AppState::default();
        let root = PathBuf::from("/project");
        state.replace_with_complete_file_list(vec![
            create_test_file_item("/project/src", true),
            create_test_file_item("/project/src/core", true),
            create_test_file_item("/project/src/core/lib.rs", false),
        ]);
        assert_eq!(
            state.loaded_dirs,
            HashSet::from([
                PathBuf::from("/project/src"),
                PathBuf::from("/project/src/core")
            ])
        );

        state.replace_with_shallow_file_list(
            &root,
            vec![create_test_file_item("/project/src", true)],
        );

        assert_eq!(state.loaded_dirs, HashSet::from([root]));
        assert_eq!(state.full_file_list.len(), 1);
    }

    // --- Preview requests ---

    #[test]
    fn test_only_the_most_recent_preview_request_may_show_its_content() {
        let mut state = AppState::default();
        let first = state.begin_preview(PathBuf::from("/project/a.rs"));
        let second = state.begin_preview(PathBuf::from("/project/b.rs"));

        assert!(!state.finish_preview(&first), "Superseded by the second");
        assert_eq!(
            state.previewed_file_path,
            Some(PathBuf::from("/project/b.rs"))
        );
        assert!(state.finish_preview(&second));
        assert!(!state.finish_preview(&second), "A read ends only once");
    }

    #[test]
    fn test_closing_the_preview_invalidates_the_read_under_way() {
        let mut state = AppState::default();
        let token = state.begin_preview(PathBuf::from("/project/a.rs"));

        state.clear_preview();

        assert_eq!(state.previewed_file_path, None);
        assert!(!state.finish_preview(&token));
    }

    #[test]
    fn test_reset_directory_state_invalidates_the_preview_read_under_way() {
        let mut state = AppState::default();
        let token = state.begin_preview(PathBuf::from("/project/a.rs"));

        state.reset_directory_state();

        assert!(!state.finish_preview(&token));
    }

    // --- Opening a path ---

    fn state_with_loaded_directory() -> AppState {
        let mut state = AppState {
            current_path: "/old".to_string(),
            full_file_list: vec![create_test_file_item("/old/a.rs", false)],
            selected_files: HashSet::from([PathBuf::from("/old/a.rs")]),
            is_fully_scanned: true,
            ..AppState::default()
        };
        state.set_filtered_file_list(state.full_file_list.clone());
        state
    }

    #[test]
    fn test_begin_open_registers_the_scan_and_leaves_the_loaded_directory_alone() {
        let mut state = state_with_loaded_directory();
        let lazy_load = state.begin_lazy_load().unwrap();

        let token = state.begin_open(Path::new("/new"));

        assert!(state.is_scanning() && state.is_opening());
        assert!(state.is_current_scan(&token));
        assert_eq!(state.current_path, "/old");
        assert_eq!(state.full_file_list.len(), 1);
        assert_eq!(state.selected_files.len(), 1);
        assert!(state.is_fully_scanned);
        assert!(
            state.is_current_lazy_load(&lazy_load) && !lazy_load.is_cancelled(),
            "the file list is still the current one"
        );
    }

    #[test]
    fn test_entering_the_opened_directory_replaces_the_loaded_one_and_keeps_the_scan() {
        let mut state = state_with_loaded_directory();
        let lazy_load = state.begin_lazy_load().unwrap();
        let token = state.begin_open(Path::new("/new"));

        assert!(state.enter_opened_directory(&token, Path::new("/new")));

        assert!(state.is_scanning() && !state.is_opening());
        assert!(state.is_current_scan(&token) && !token.is_cancelled());
        assert_eq!(state.current_path, "/new");
        assert!(state.full_file_list.is_empty() && state.filtered_file_list().is_empty());
        assert!(state.selected_files.is_empty());
        assert!(!state.is_fully_scanned);
        assert!(lazy_load.is_cancelled());
        assert!(
            !state.enter_opened_directory(&token, Path::new("/again")),
            "a directory is entered once"
        );
        assert_eq!(state.current_path, "/new");
    }

    #[test]
    fn test_only_the_scan_that_is_opening_enters_its_directory() {
        let mut state = state_with_loaded_directory();
        let superseded = state.begin_open(Path::new("/new"));
        let current = state.begin_open(Path::new("/new"));
        assert!(!state.enter_opened_directory(&superseded, Path::new("/stale")));
        assert_eq!(state.current_path, "/old");
        assert!(state.enter_opened_directory(&current, Path::new("/new")));

        // A re-scan keeps the directory it has.
        let mut state = state_with_loaded_directory();
        let rescan = state.begin_scan();
        assert!(!state.enter_opened_directory(&rescan, Path::new("/new")));
        assert_eq!(state.current_path, "/old");
    }

    #[test]
    fn test_abandoning_an_open_keeps_the_loaded_directory() {
        let mut state = state_with_loaded_directory();
        let token = state.begin_open(Path::new("/nonexistent"));

        let todo = state.abandon_scan(&token);

        assert!(
            matches!(todo, AfterAbandonedScan::Nothing),
            "nothing was running when the opening began, got {todo:?}"
        );
        assert!(!state.is_scanning() && !state.is_opening());
        assert_eq!(state.current_path, "/old");
        assert_eq!(state.selected_files.len(), 1);
        assert!(state.is_fully_scanned);
        assert_eq!(state.status, StatusNotice::Ready);
    }

    #[test]
    fn test_abandoning_an_open_asks_for_the_scan_it_superseded() {
        let mut state = state_with_loaded_directory();
        let _superseded = state.begin_scan();
        let token = state.begin_open(Path::new("/nonexistent"));

        let todo = state.abandon_scan(&token);

        assert!(matches!(todo, AfterAbandonedScan::RescanLoadedDirectory));
        assert!(!state.is_scanning());

        // Without a loaded directory there is nothing to scan again.
        let mut state = AppState::default();
        let _superseded = state.begin_scan();
        let token = state.begin_open(Path::new("/nonexistent"));
        assert!(matches!(
            state.abandon_scan(&token),
            AfterAbandonedScan::Nothing
        ));

        // A re-scan that finds its directory gone superseded nothing worth resuming.
        let mut state = state_with_loaded_directory();
        let rescan = state.begin_scan();
        assert!(matches!(
            state.abandon_scan(&rescan),
            AfterAbandonedScan::Nothing
        ));
        assert!(!state.is_scanning());
    }

    /// Two paths are opened in a row and the second one does not exist: the first one
    /// is opened after all, and it still knows what it had superseded itself.
    #[test]
    fn test_abandoning_an_open_registers_the_opening_it_superseded_again() {
        let mut state = state_with_loaded_directory();
        let _scan = state.begin_scan();
        let first = state.begin_open(Path::new("/first"));
        let second = state.begin_open(Path::new("/nonexistent"));

        assert!(
            matches!(state.abandon_scan(&first), AfterAbandonedScan::Nothing),
            "a stale token changes nothing"
        );
        assert!(state.is_opening() && state.is_current_scan(&second));

        let AfterAbandonedScan::Reopen { path, token } = state.abandon_scan(&second) else {
            panic!("the superseded opening must be taken up again");
        };
        assert_eq!(path, Path::new("/first"));
        assert!(state.is_scanning() && state.is_opening());
        assert!(state.is_current_scan(&token) && !token.is_cancelled());
        assert!(!state.is_current_scan(&first), "it is a run of its own");

        // The first path does not exist either: back to the scan of the loaded directory.
        assert!(matches!(
            state.abandon_scan(&token),
            AfterAbandonedScan::RescanLoadedDirectory
        ));
        assert!(!state.is_scanning() && !state.is_opening());
    }

    #[test]
    fn test_cancelling_or_resetting_ends_the_opening() {
        let mut state = state_with_loaded_directory();
        let token = state.begin_open(Path::new("/new"));
        state.cancel_current_scan();
        assert!(!state.is_opening() && !state.is_scanning());
        assert!(!state.enter_opened_directory(&token, Path::new("/new")));
        assert_eq!(state.current_path, "/old");

        let token = state.begin_open(Path::new("/new"));
        state.reset_directory_state();
        assert!(!state.is_opening() && !state.is_scanning());
        assert!(!state.enter_opened_directory(&token, Path::new("/new")));
        assert_eq!(state.current_path, "");

        let _open = state.begin_open(Path::new("/new"));
        let _rescan = state.begin_scan();
        assert!(!state.is_opening(), "a re-scan is not an opening");
    }

    // --- Scan progress ---

    fn progress(files_scanned: usize, path: &str) -> ScanProgress {
        ScanProgress {
            files_scanned,
            large_files_skipped: 1,
            current_scanning_path: path.to_string(),
        }
    }

    #[test]
    fn test_progress_of_the_current_scan_is_recorded() {
        let mut state = AppState::default();
        let token = state.begin_scan();

        assert!(state.record_scan_progress(&token, progress(40, "/project/src")));

        assert_eq!(state.scan_progress.files_scanned, 40);
        assert_eq!(state.scan_progress.large_files_skipped, 1);
        assert_eq!(state.scan_progress.current_scanning_path, "/project/src");
    }

    #[test]
    fn test_progress_of_an_outdated_scan_is_ignored() {
        let mut state = AppState::default();
        let superseded = state.begin_scan();
        let current = state.begin_scan();
        assert!(!state.record_scan_progress(&superseded, progress(40, "/old")));
        assert_eq!(state.scan_progress.files_scanned, 0);

        state.cancel_current_scan();
        state.record_scan_progress(&current, progress(50, "/cancelled"));
        assert_eq!(state.scan_progress.files_scanned, 0);
        assert!(state.scan_progress.current_scanning_path.is_empty());
    }

    #[test]
    fn test_a_new_scan_starts_counting_from_zero_and_a_finished_one_keeps_its_counters() {
        let mut state = AppState::default();
        let token = state.begin_scan();
        state.record_scan_progress(&token, progress(40, "/project/src"));

        assert!(state.finish_scan(&token));
        assert_eq!(state.scan_progress.files_scanned, 40);
        assert!(
            state.scan_progress.current_scanning_path.is_empty(),
            "a finished scan works on no path"
        );

        let _next = state.begin_scan();
        assert_eq!(state.scan_progress.files_scanned, 0);
        let token = state.begin_open(Path::new("/new"));
        state.record_scan_progress(&token, progress(7, "/other"));
        let _next = state.begin_open(Path::new("/new"));
        assert_eq!(state.scan_progress.files_scanned, 0);
    }

    // --- Configuration ---

    #[test]
    fn test_default_state_has_the_default_config_and_is_not_bound_to_a_file() {
        let state = AppState::default();

        assert_eq!(state.config, AppConfig::default());
        assert_eq!(state.config_store().path(), None);
    }

    #[test]
    fn test_config_snapshots_are_taken_from_the_store_of_the_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let (store, config) = ConfigStore::open(path.clone());
        let mut state = AppState::new(config, store);

        state.config.output_filename = "older.txt".to_string();
        let older = state.config_snapshot();
        state.config.output_filename = "newer.txt".to_string();
        let newer = state.config_snapshot();
        newer.save().unwrap();
        older.save().unwrap();

        let saved = crate::config::settings::load_config(&path).unwrap();
        assert_eq!(saved.output_filename, "newer.txt");
        assert_eq!(state.config_store().latest(), state.config);
    }
}
