//! Defines the central, mutable state of the application.

use crate::app::filtering::FilterInputs;
use crate::app::selection::{DirectorySelection, Rejected, SelectedFiles, SelectionCountsCache};
use crate::app::tracked::Tracked;
use crate::app::tree_index::{IndexedList, TreeIndex, TreeIndexCache};
use crate::app::view_model::TreeViewState;
use crate::config::{AppConfig, ConfigSnapshot, ConfigStore};
use crate::core::{is_blank, FileItem, FileNameMatcher, ScanProgress};
use std::collections::{HashMap, HashSet};
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
    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// The output document of a finished generation, as the host keeps it.
///
/// The UI is sent the text for display, together with the `id`. A save names that id
/// and the host writes its own copy, so the text does not travel back (see
/// `commands::save_file`). The id is the one of the generation run that produced the
/// document: no two documents of a session share it.
///
/// Cloning is cheap: the text is shared, not copied.
#[derive(Clone, PartialEq, Eq)]
pub struct GeneratedDocument {
    id: u64,
    text: Arc<String>,
}

impl GeneratedDocument {
    /// What the UI names when it refers to this document.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The text of the document.
    pub fn text(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub(crate) fn for_test(id: u64, text: &str) -> Self {
        Self {
            id,
            text: Arc::new(text.to_string()),
        }
    }
}

/// Shows the size instead of the text: a document can weigh many megabytes, and a
/// log line or a failed assertion that prints an event must not print all of it.
impl std::fmt::Debug for GeneratedDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedDocument")
            .field("id", &self.id)
            .field("bytes", &self.text.len())
            .finish()
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
    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
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
    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

/// The items of the file list that did not come from the walk of the scan that is
/// building the list: the first level (published ahead of the walk) and the levels
/// the user asked for while the walk was under way (lazy loads).
///
/// The walk lists these items as well. Keeping track of them is what lets its batches
/// be merged without duplicates and without comparing every batch against the whole
/// list, and what makes the finished list exactly what the walk saw: an item the
/// walk has not confirmed by the end (deleted in the meantime) is removed again.
#[derive(Debug, Default)]
struct AheadOfWalk {
    /// By path: `true` once the walk has listed the item too.
    confirmed: HashMap<PathBuf, bool>,
}

impl AheadOfWalk {
    fn add(&mut self, path: &Path) {
        self.confirmed.entry(path.to_path_buf()).or_insert(false);
    }

    /// `true` if `path` is already in the list; the walk has confirmed it by this.
    fn confirm(&mut self, path: &Path) -> bool {
        match self.confirmed.get_mut(path) {
            Some(confirmed) => {
                *confirmed = true;
                true
            }
            None => false,
        }
    }

    /// The paths the walk has not listed.
    fn unconfirmed(&self) -> HashSet<PathBuf> {
        self.confirmed
            .iter()
            .filter(|(_, confirmed)| !**confirmed)
            .map(|(path, _)| path.clone())
            .collect()
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

/// What the state has no use for any more and what takes time to drop: a file list or
/// the hierarchy of one (an allocation or two per item, tens of milliseconds for a
/// large project). Whoever is handed one holds the lock on the state; it is to be
/// dropped after the lock has been released.
#[must_use = "drop it after the lock on the state has been released"]
#[derive(Default)]
pub(crate) struct Discarded(Vec<Box<dyn Send>>);

impl Discarded {
    /// Adds `value` to what is dropped with this.
    pub(crate) fn add(&mut self, value: impl Send + 'static) {
        self.0.push(Box::new(value));
    }
}

/// Lists that were prepared without holding the lock on the state, to be installed
/// in one step ([`AppState::install_file_lists`]).
pub(crate) struct FileLists {
    /// The new file list; `None` if the file list stays as it is.
    pub full: Option<Arc<Vec<FileItem>>>,
    /// The new filtered list with its hierarchy; `None` if the filtered list stays
    /// as it is.
    pub filtered: Option<IndexedList>,
    /// `Some(n)` if the new filtered list is the one before it with items appended
    /// from position `n` on: what was counted for the list before still holds then.
    pub appended_from: Option<usize>,
    /// The filters the new filtered list was made with, out of the new file list.
    pub made_with: Arc<FilterInputs>,
}

/// The items of `items` whose path is not in `list`, in their order.
pub(crate) fn unlisted_items(list: &[FileItem], items: Vec<FileItem>) -> Vec<FileItem> {
    let new_paths: HashSet<&Path> = items.iter().map(|item| item.path.as_path()).collect();
    let already_listed: HashSet<PathBuf> = list
        .iter()
        .filter(|item| new_paths.contains(item.path.as_path()))
        .map(|item| item.path.clone())
        .collect();
    drop(new_paths);
    let mut items = items;
    items.retain(|item| !already_listed.contains(&item.path));
    items
}

/// The paths of the directories in `list`.
pub(crate) fn directories_of(list: &[FileItem]) -> HashSet<PathBuf> {
    list.iter()
        .filter(|item| item.is_directory)
        .map(|item| item.path.clone())
        .collect()
}

/// Holds the complete, mutable state of the application.
///
/// It is shared between threads (the event loop, the command lanes and background
/// tasks) as a [`SharedState`](super::shared_state::SharedState), the one place that
/// locks it.
pub struct AppState {
    /// The application's configuration settings.
    ///
    /// Private: they are read through [`Self::config`] and changed through
    /// [`Self::edit_config`] only, which hands out the snapshot to save. So every
    /// change takes a snapshot, which is what the last save of a session relies on:
    /// it writes the most recent snapshot and does not look at this field.
    config: AppConfig,
    /// The absolute path to the currently loaded directory.
    pub current_path: String,
    /// The complete, unfiltered list of all files and directories found in the scan.
    /// Private: it is read through [`AppState::full_file_list`] and changed through
    /// methods that give it a new revision (`file_list_revision`), which is how
    /// work that was prepared for one list can tell that the list is another one
    /// now. Shared, so that such work can read it without holding the lock on the
    /// state; whoever holds a second handle keeps an unchanged list.
    full_file_list: Arc<Vec<FileItem>>,
    /// Identifies the current content of `full_file_list`.
    file_list_revision: u64,
    /// The list of files and directories visible in the UI after applying filters.
    /// Private so that every change goes through [`AppState::set_filtered_file_list`]
    /// or its relatives. Shared like the file list, and never changed in place.
    /// Where the filters let everything through it is the file list itself, the
    /// same allocation ([`AppState::filtered_list_is_file_list`]).
    filtered_file_list: Arc<Vec<FileItem>>,
    /// The revision of the file list the filtered list was made from.
    filtered_from: u64,
    /// The filters the filtered list was made with, if whoever made it said so
    /// ([`AppState::filters_of_filtered_list`]).
    filtered_with: Option<Arc<FilterInputs>>,
    /// The set of absolute paths to files that are currently selected by the user.
    pub selected_files: SelectedFiles,
    /// The set of absolute paths to directories that are expanded in the UI tree.
    /// Tracked, because the rows of the tree are derived from it.
    pub expanded_dirs: Tracked<HashSet<PathBuf>>,
    /// The directories the user collapsed while a filter was active. The host does not
    /// expand them again on its own for as long as that filter session lasts, see
    /// [`AppState::reveal_directory`].
    collapsed_under_filter: HashSet<PathBuf>,
    /// The set of absolute paths to directories whose children have been loaded.
    pub loaded_dirs: HashSet<PathBuf>,
    /// The current search query for filenames.
    pub search_query: String,
    /// The current filter for file extensions.
    pub extension_filter: String,
    /// The current search query for file content.
    pub content_search_query: String,
    /// The set of paths that match the current content search query. Private and
    /// shared for the same reason as the file list: whoever holds a second handle
    /// can tell by it whether the matches are still the same.
    content_search_results: Arc<HashSet<PathBuf>>,
    /// `true` once a search for the content query has delivered its matches. Until
    /// then there is nothing the content filter could work with, see
    /// [`AppState::content_filter_matches`].
    content_search_answered: bool,
    /// `true` if the search that ended last has failed. Matters only as long as no
    /// search has delivered: there are no matches then, and none to fall back on.
    content_search_failed: bool,
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
    /// Set while the walk of the current scan fills the file list, see
    /// [`AppState::merge_scanned_items`].
    ahead_of_walk: Option<AheadOfWalk>,
    /// The levels that were loaded lazily between the beginning of a re-scan and the
    /// publication of its first level, which replaces the list they were merged
    /// into. They belong to the list of the new scan and are entered into it again
    /// ([`AppState::replace_with_shallow_file_list`]). `None` when no scan is in
    /// that phase.
    levels_before_walk: Option<Vec<(PathBuf, Vec<FileItem>)>>,
    /// Set while the current scan has not yet found the path it is to open.
    opening: Option<PendingOpen>,
    /// The content generation that is currently in progress, if any.
    generation: RunSlot,
    /// The document the UI shows as generated output, see
    /// [`AppState::complete_generation`].
    generated_document: Option<GeneratedDocument>,
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
    /// The selection counts of the directories, derived from the hierarchy and
    /// `selected_files`, see [`AppState::directory_selection`].
    selection_counts_cache: SelectionCountsCache,
    /// What the UI looks at of the tree (its viewport) and the row list derived from
    /// the hierarchy and `expanded_dirs`. Owned by the view model.
    pub(crate) tree_view: TreeViewState,
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
            full_file_list: Arc::default(),
            file_list_revision: 0,
            filtered_file_list: Arc::default(),
            filtered_from: 0,
            filtered_with: None,
            selected_files: SelectedFiles::default(),
            expanded_dirs: Tracked::default(),
            collapsed_under_filter: HashSet::new(),
            loaded_dirs: HashSet::new(),
            search_query: String::new(),
            extension_filter: String::new(),
            content_search_query: String::new(),
            content_search_results: Arc::default(),
            content_search_answered: false,
            content_search_failed: false,
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
            ahead_of_walk: None,
            levels_before_walk: None,
            opening: None,
            generation: RunSlot::default(),
            generated_document: None,
            content_search: RunSlot::default(),
            preview: RunSlot::default(),
            config_store,
            filtered_revision: 0,
            tree_index_cache: TreeIndexCache::default(),
            selection_counts_cache: SelectionCountsCache::default(),
            tree_view: TreeViewState::default(),
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
        self.ahead_of_walk = None;
        self.levels_before_walk = Some(Vec::new());
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
        self.ahead_of_walk = None;
        self.levels_before_walk = None;
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

    /// Records that the scan identified by `token` has found its directory and is about
    /// to walk it: from here on the file list is the one this scan is building, so it
    /// no longer counts as complete. Until this moment a scan changes nothing but the
    /// scanning state itself, which is why a scan that never finds its directory
    /// ([`AppState::abandon_scan`]) leaves the loaded list as complete as it was.
    ///
    /// Returns `true` if this changed what the UI shows. Does nothing and returns
    /// `false` for a stale token.
    pub fn begin_scan_walk(&mut self, token: &ScanToken) -> bool {
        if !self.is_current_scan(token) {
            return false;
        }
        std::mem::replace(&mut self.is_fully_scanned, false)
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
        self.ahead_of_walk = None;
        self.levels_before_walk = None;
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
        self.ahead_of_walk = None;
        self.levels_before_walk = None;
        self.scan_progress = no_scan_progress();
        self.status = StatusNotice::ScanCancelled;
        tracing::info!("Scan cancelled.");
    }

    /// Returns a token for a lazy load of a single directory level of the current
    /// file list.
    ///
    /// A lazy load is possible at any time, also while a scan is walking the project:
    /// the walk gets to a directory when it gets to it, the user who expands it wants
    /// to see it now. The level is then in the list ahead of the walk, see
    /// [`AppState::merge_directory_level`].
    pub fn begin_lazy_load(&self) -> LazyLoadToken {
        LazyLoadToken {
            file_list_generation: self.file_list_generation,
            cancel_flag: self.lazy_load_cancel_flag.clone(),
        }
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

    /// Ends the generation identified by `token` with the document it has produced,
    /// like [`Self::finish_generation`], and keeps the document for saving.
    ///
    /// Returns the document as it is kept, to be shown by the UI. For a stale token
    /// nothing changes and `None` is returned: the document of a generation that was
    /// cancelled or superseded is dropped, and the one that is kept stays.
    ///
    /// The document replaces the one of the generation before it. Until then the
    /// earlier one stays available, also while a new generation is running: it is what
    /// the UI shows during that time, and what a save refers to.
    pub fn complete_generation(
        &mut self,
        token: &GenerationToken,
        text: Arc<String>,
    ) -> Option<GeneratedDocument> {
        if !self.finish_generation(token) {
            return None;
        }
        let document = GeneratedDocument {
            id: token.0.id,
            text,
        };
        self.generated_document = Some(document.clone());
        Some(document)
    }

    /// The generated document with this id, if it is the one that is kept.
    pub fn generated_document(&self, id: u64) -> Option<GeneratedDocument> {
        self.generated_document
            .as_ref()
            .filter(|document| document.id == id)
            .cloned()
    }

    /// Gives up the generated document, so that its memory is released. To be called
    /// exactly where the UI stops showing it: a file preview takes its place
    /// (`showPreviewContent`) or the preview is cleared (`clearPreviewState`).
    /// Anything else would leave the UI with an output it can no longer name in a
    /// save. A save that names the document in the moment between this call and the
    /// arrival of the event at the UI is answered with
    /// [`SaveResult::DocumentUnavailable`](crate::app::events::SaveResult), from which
    /// the UI recovers.
    pub fn discard_generated_document(&mut self) {
        self.generated_document = None;
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
        self.finish_content_search_shared(token, Arc::new(matches))
    }

    /// [`AppState::finish_content_search`] with matches that the caller holds as
    /// well: they become the matches of the state as that very set.
    pub(crate) fn finish_content_search_shared(
        &mut self,
        token: &ContentSearchToken,
        matches: Arc<HashSet<PathBuf>>,
    ) -> bool {
        if !self.content_search.finish(&token.0) {
            return false;
        }
        self.content_search_results = matches;
        self.content_search_answered = true;
        self.content_search_failed = false;
        true
    }

    /// `true` if `token` belongs to the content search whose matches are awaited.
    pub(crate) fn is_current_content_search(&self, token: &ContentSearchToken) -> bool {
        self.content_search.is_current(&token.0)
    }

    /// The paths that match the content query, as the last search delivered them.
    pub fn content_search_results(&self) -> &HashSet<PathBuf> {
        &self.content_search_results
    }

    /// Replaces the matches of the content query. They are a set of their own from
    /// here on, also if they are equal to the previous ones.
    pub fn set_content_search_results(&mut self, matches: HashSet<PathBuf>) {
        self.content_search_results = Arc::new(matches);
    }

    /// Ends the search identified by `token` without a result (the search failed). The
    /// matches of the search before it stay in place; if there was none before it,
    /// there is no content filter until a search delivers
    /// ([`AppState::content_filter_matches`]).
    ///
    /// Does nothing for a stale token. Returns `true` if this call ended the running
    /// search, i.e. if the failure is the caller's to report.
    pub fn abandon_content_search(&mut self, token: &ContentSearchToken) -> bool {
        let ended = self.content_search.finish(&token.0);
        if ended {
            self.content_search_failed = true;
        }
        ended
    }

    /// Drops the matches of the content search and supersedes a search that is still
    /// running, so it cannot bring matches back. To be used when the query is cleared.
    pub fn clear_content_search(&mut self) {
        self.content_search.cancel();
        self.set_content_search_results(HashSet::new());
        self.content_search_answered = false;
        self.content_search_failed = false;
    }

    /// The matches the content filter works with, or `None` if there is no content
    /// filter to apply: the query is [blank](is_blank), or the first search for it is
    /// still running, or it has failed. In the latter cases the list stays as it is
    /// without the content filter, instead of being empty: neither "still searching"
    /// nor "the search failed" may look like "nothing matches". Once a search has
    /// delivered, its matches apply until the next one delivers (also while that next
    /// one is running, and if it fails).
    pub fn content_filter_matches(&self) -> Option<&HashSet<PathBuf>> {
        if is_blank(&self.content_search_query) {
            return None;
        }
        let no_matches_yet = (self.content_search.is_active() || self.content_search_failed)
            && !self.content_search_answered;
        (!no_matches_yet).then_some(&*self.content_search_results)
    }

    /// [`AppState::content_filter_matches`] as a handle that can be kept after the
    /// lock on the state has been released, and compared with a later one: the
    /// same allocation is the same set of matches.
    pub(crate) fn shared_content_filter_matches(&self) -> Option<Arc<HashSet<PathBuf>>> {
        self.content_filter_matches()
            .is_some()
            .then(|| Arc::clone(&self.content_search_results))
    }

    /// `true` if at least one of the three filters is set (not [blank](is_blank)).
    pub fn has_active_filter(&self) -> bool {
        !is_blank(&self.search_query)
            || !is_blank(&self.extension_filter)
            || !is_blank(&self.content_search_query)
    }

    /// Stores the filter texts as the user entered them. Returns `true` if the content
    /// query has changed, i.e. if the content has to be searched again.
    ///
    /// With the last filter text gone the filter session is over: what the user
    /// collapsed during it no longer holds back [`AppState::reveal_directory`].
    ///
    /// Other filters show another list: the UI shows it from the top (it scrolls
    /// there when the filter texts of a snapshot change), so the window of rows
    /// starts at the top as well and the answer already holds the rows it needs.
    pub fn set_filter_texts(
        &mut self,
        search_query: String,
        extension_filter: String,
        content_search_query: String,
    ) -> bool {
        let content_query_changed = self.content_search_query != content_search_query;
        if content_query_changed
            || self.search_query != search_query
            || self.extension_filter != extension_filter
        {
            self.tree_view.reset();
        }
        self.search_query = search_query;
        self.extension_filter = extension_filter;
        self.content_search_query = content_search_query;
        if !self.has_active_filter() {
            self.collapsed_under_filter.clear();
        }
        content_query_changed
    }

    /// Expands `directories` because the user asked for it (a click, "expand all", a
    /// lazy load).
    pub fn expand_directories(&mut self, directories: impl IntoIterator<Item = PathBuf>) {
        for directory in directories {
            self.collapsed_under_filter.remove(&directory);
            // Only a real change touches the tracked set: the rows of the tree are
            // rebuilt whenever it is borrowed mutably.
            if !self.expanded_dirs.contains(&directory) {
                self.expanded_dirs.insert(directory);
            }
        }
    }

    /// Collapses `directories` because the user asked for it (a click, "collapse
    /// all"). While a filter is active the directories are remembered, so that the
    /// filter does not open them again, see [`AppState::reveal_directory`].
    pub fn collapse_directories(&mut self, directories: impl IntoIterator<Item = PathBuf>) {
        let remember = self.has_active_filter();
        for directory in directories {
            if self.expanded_dirs.contains(&directory) {
                self.expanded_dirs.remove(&directory);
            }
            if remember {
                self.collapsed_under_filter.insert(directory);
            }
        }
    }

    /// Expands `directory` on behalf of a filter, to show a match below it.
    ///
    /// The one rule of the automatic expansion: **a directory the user collapsed while
    /// a filter was active is not opened again by the host until the filters have been
    /// cleared** (all three texts blank) or the user expands it. Everything else is
    /// expanded, so a match in a directory the user has not touched still shows up.
    /// Re-applying the same filter (after a lazy load, a change of the case
    /// sensitivity, the end of a scan) therefore never undoes a collapse.
    pub fn reveal_directory(&mut self, directory: &Path) {
        if !self.collapsed_under_filter.contains(directory)
            && !self.expanded_dirs.contains(directory)
        {
            self.expanded_dirs.insert(directory.to_path_buf());
        }
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

    /// The application's configuration settings.
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Changes the configuration. This is the only way to do so.
    ///
    /// Returns the snapshot of the changed configuration, which the caller has saved
    /// (normally with `tasks::save_config_in_background`, after or while it still
    /// holds the state lock); `None` if `edit` left the configuration as it was, in
    /// which case there is nothing to save.
    #[must_use = "a changed configuration has to be saved: pass the snapshot on"]
    pub fn edit_config(&mut self, edit: impl FnOnce(&mut AppConfig)) -> Option<ConfigSnapshot> {
        let previous = self.config.clone();
        edit(&mut self.config);
        (self.config != previous).then(|| self.config_snapshot())
    }

    /// The configuration, to set up the state of a test. Nothing is saved.
    #[cfg(test)]
    pub(crate) fn config_mut(&mut self) -> &mut AppConfig {
        &mut self.config
    }

    /// Captures the configuration for saving it outside the state lock. A change of
    /// the configuration gets its snapshot from [`Self::edit_config`]; this is for a
    /// save that follows no change.
    ///
    /// Take the snapshot while holding the lock and save it afterwards: the snapshots
    /// are numbered in the order they are taken, which is what keeps a save that is
    /// overtaken by a later one from overwriting it (see [`ConfigStore`]). This is the
    /// only way the application saves its configuration.
    pub fn config_snapshot(&self) -> ConfigSnapshot {
        self.config_store.snapshot(&self.config)
    }

    /// The store the configuration is persisted through.
    #[cfg(test)]
    pub(crate) fn config_store(&self) -> ConfigStore {
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

    /// The complete, unfiltered list of all files and directories found in the scan.
    pub fn full_file_list(&self) -> &[FileItem] {
        &self.full_file_list
    }

    /// The file list in a form that can be read after the lock on the state has
    /// been released. It is the list of [`AppState::file_list_revision`] as it is
    /// now; the state gets another one when the list changes.
    pub(crate) fn shared_full_file_list(&self) -> Arc<Vec<FileItem>> {
        Arc::clone(&self.full_file_list)
    }

    /// Identifies the current content of the file list: it changes with every
    /// change of the list, whatever the change is.
    pub(crate) fn file_list_revision(&self) -> u64 {
        self.file_list_revision
    }

    /// Replaces the file list. The filtered list is not touched; the caller applies
    /// the filters.
    pub fn set_full_file_list(&mut self, files: Vec<FileItem>) {
        self.full_file_list = Arc::new(files);
        self.file_list_revision += 1;
    }

    /// Changes the file list in place (as far as nobody else holds it: otherwise the
    /// change is made to a copy, which becomes the list). The filtered list is not
    /// touched; the caller applies the filters.
    pub fn edit_full_file_list(&mut self, edit: impl FnOnce(&mut Vec<FileItem>)) {
        edit(Arc::make_mut(&mut self.full_file_list));
        self.file_list_revision += 1;
    }

    /// Makes `files` the result of the first, shallow phase of a scan of `root`.
    ///
    /// The list then holds a single level, so `root` is the only directory whose
    /// children are loaded; whatever was loaded in the list before is forgotten.
    ///
    /// The one exception are the levels the user asked for since this scan began
    /// (a directory expanded right after a re-scan was started): they were listed
    /// for the list of this scan and stay loaded.
    ///
    /// The walk of the whole project that follows lists all of this again; the items
    /// are remembered as being ahead of it, see [`AppState::merge_scanned_items`].
    pub fn replace_with_shallow_file_list(&mut self, root: &std::path::Path, files: Vec<FileItem>) {
        let mut ahead_of_walk = AheadOfWalk::default();
        for item in &files {
            ahead_of_walk.add(&item.path);
        }
        self.ahead_of_walk = Some(ahead_of_walk);
        self.set_full_file_list(files);
        self.loaded_dirs = HashSet::from([root.to_path_buf()]);
        for (directory, items) in self.levels_before_walk.take().unwrap_or_default() {
            let added = unlisted_items(&self.full_file_list, items);
            self.note_directory_level(&directory, &added);
            self.edit_full_file_list(|list| list.extend(added));
        }
    }

    /// Adds `items`, which the walk of the scan identified by `token` has listed, to
    /// the file list. An item that is already there (because its level was published
    /// ahead of the walk) is not added a second time.
    ///
    /// The directories of the items do not count as loaded by this: the walk may not
    /// have listed all of their children yet. That is settled for all directories at
    /// once by [`AppState::complete_scanned_file_list`].
    ///
    /// Does nothing and returns `false` for a stale token. The filtered list is not
    /// touched; the caller applies the filters when it publishes the change.
    pub fn merge_scanned_items(&mut self, token: &ScanToken, items: Vec<FileItem>) -> bool {
        let Some(mut new_items) = self.confirm_scanned_items(token, items) else {
            return false;
        };
        if !new_items.is_empty() {
            self.edit_full_file_list(|list| list.append(&mut new_items));
        }
        true
    }

    /// Tells which of `items`, listed by the walk of the scan identified by `token`,
    /// are not in the file list yet, and returns those; the others (their level was
    /// published ahead of the walk) are confirmed as seen by the walk. The file list
    /// itself is not changed: this is the first half of
    /// [`AppState::merge_scanned_items`], for a caller that adds the items later
    /// ([`AppState::install_file_lists`]).
    ///
    /// Returns `None` for a stale token.
    pub(crate) fn confirm_scanned_items(
        &mut self,
        token: &ScanToken,
        mut items: Vec<FileItem>,
    ) -> Option<Vec<FileItem>> {
        if !self.is_current_scan(token) {
            return None;
        }
        let ahead_of_walk = self.ahead_of_walk.get_or_insert_with(AheadOfWalk::default);
        items.retain(|item| !ahead_of_walk.confirm(&item.path));
        Some(items)
    }

    /// Declares the file list complete on behalf of the scan identified by `token`,
    /// whose walk has come to its end and has delivered all its items
    /// ([`AppState::merge_scanned_items`]): every directory in the list is loaded,
    /// `active_patterns` are the ignore patterns that were active, and the list is
    /// exactly what the walk saw. Items that were in the list ahead of the walk and
    /// that the walk did not find are removed, and so are selected paths that are not
    /// in the list (any more).
    ///
    /// Does nothing and returns `false` for a stale token. The filtered list is not
    /// touched; the caller applies the filters.
    pub fn complete_scanned_file_list(
        &mut self,
        token: &ScanToken,
        active_patterns: HashSet<String>,
    ) -> bool {
        let Some(not_found) = self.items_the_walk_did_not_find(token) else {
            return false;
        };
        if !not_found.is_empty() {
            self.edit_full_file_list(|list| list.retain(|item| !not_found.contains(&item.path)));
        }
        let loaded_dirs = directories_of(&self.full_file_list);
        let listed: HashSet<&Path> = if self.selected_files.is_empty() {
            HashSet::new()
        } else {
            let items = self.full_file_list.iter();
            items.map(|item| item.path.as_path()).collect()
        };
        self.selected_files
            .retain(|path| listed.contains(path.as_path()));
        self.end_walk(active_patterns, loaded_dirs);
        true
    }

    /// The paths that are in the file list ahead of the walk of the scan identified
    /// by `token` and that the walk has not listed: when the walk has ended, they
    /// are what has to leave the list. `None` for a stale token.
    pub(crate) fn items_the_walk_did_not_find(
        &self,
        token: &ScanToken,
    ) -> Option<HashSet<PathBuf>> {
        self.is_current_scan(token).then(|| {
            self.ahead_of_walk
                .as_ref()
                .map(AheadOfWalk::unconfirmed)
                .unwrap_or_default()
        })
    }

    /// [`AppState::complete_scanned_file_list`] without its work on the file list,
    /// for a caller that has done that work without holding the lock and installs
    /// its result in the same breath ([`AppState::install_file_lists`]):
    /// `loaded_dirs` are the directories of the complete list. The caller has also
    /// asked about a snapshot of the selection which selected paths are not in the
    /// list (`not_listed`); `is_listed` answers that for what was selected since.
    ///
    /// Does nothing and returns `false` for a stale token.
    pub(crate) fn complete_walk(
        &mut self,
        token: &ScanToken,
        active_patterns: HashSet<String>,
        loaded_dirs: HashSet<PathBuf>,
        not_listed: Rejected,
        is_listed: impl Fn(&Path) -> bool,
    ) -> bool {
        if !self.is_current_scan(token) {
            return false;
        }
        self.selected_files.retain_since(not_listed, is_listed);
        self.end_walk(active_patterns, loaded_dirs);
        true
    }

    /// What the end of a walk means for everything but the lists.
    fn end_walk(&mut self, active_patterns: HashSet<String>, loaded_dirs: HashSet<PathBuf>) {
        self.ahead_of_walk = None;
        self.loaded_dirs = loaded_dirs;
        self.active_ignore_patterns = active_patterns;
        self.is_fully_scanned = true;
    }

    /// Adds `items`, the content of `directory` as a lazy load has listed it, to the
    /// file list and marks the directory as loaded; `active_patterns` are the ignore
    /// patterns that were active in that level. Items that are already in the list
    /// (the walk of a running scan has been there) are not added a second time.
    ///
    /// Does nothing and returns `false` if the file list is no longer the one the
    /// lazy load was started for. The filtered list is not touched; the caller
    /// applies the filters.
    pub fn merge_directory_level(
        &mut self,
        token: &LazyLoadToken,
        directory: &Path,
        items: Vec<FileItem>,
        active_patterns: HashSet<String>,
    ) -> bool {
        if !self.is_current_lazy_load(token) {
            return false;
        }
        let added = unlisted_items(&self.full_file_list, items.clone());
        self.record_directory_level(token, directory, items, &added, active_patterns);
        self.edit_full_file_list(|list| list.extend(added));
        true
    }

    /// [`AppState::merge_directory_level`] without its work on the file list, for a
    /// caller that has done that work without holding the lock and installs its
    /// result in the same breath ([`AppState::install_file_lists`]): `items` is the
    /// level as it was listed, `added` are the items of it that were not in the
    /// list.
    ///
    /// Does nothing and returns `false` for a stale token.
    pub(crate) fn record_directory_level(
        &mut self,
        token: &LazyLoadToken,
        directory: &Path,
        items: Vec<FileItem>,
        added: &[FileItem],
        active_patterns: HashSet<String>,
    ) -> bool {
        if !self.is_current_lazy_load(token) {
            return false;
        }
        self.active_ignore_patterns.extend(active_patterns);
        // The list this goes into is about to be replaced by the first level of the
        // scan that has begun; the level is entered into that list again.
        if let Some(levels) = self.levels_before_walk.as_mut() {
            levels.push((directory.to_path_buf(), items));
        }
        self.note_directory_level(directory, added);
        true
    }

    /// `directory` is loaded, and `added`, the items of its level that were not in
    /// the list, are ahead of the walk while a scan walks the project.
    fn note_directory_level(&mut self, directory: &Path, added: &[FileItem]) {
        self.loaded_dirs.insert(directory.to_path_buf());
        if let Some(ahead_of_walk) = self.ahead_of_walk.as_mut() {
            for item in added {
                ahead_of_walk.add(&item.path);
            }
        }
    }

    /// Makes `files` the complete content of the project: every directory in it is
    /// loaded.
    #[cfg(test)]
    pub(crate) fn replace_with_complete_file_list(&mut self, files: Vec<FileItem>) {
        self.loaded_dirs = directories_of(&files);
        self.set_full_file_list(files);
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

    /// Replaces the filtered file list by what the filters have made of the file
    /// list as it is now.
    ///
    /// This and [`AppState::install_file_lists`] are the only ways to change the
    /// list, which guarantees that data derived from it is never used with a
    /// different list than it was built from.
    pub fn set_filtered_file_list(&mut self, items: Vec<FileItem>) {
        self.set_shared_filtered_file_list(Arc::new(items));
    }

    /// [`AppState::set_filtered_file_list`] for a list that is shared, which is how
    /// the file list itself becomes the filtered list where the filters let
    /// everything through. A list that already is the filtered list stays: what is
    /// derived from it is kept.
    pub(crate) fn set_shared_filtered_file_list(&mut self, items: Arc<Vec<FileItem>>) {
        self.filtered_from = self.file_list_revision;
        self.filtered_with = None;
        if !Arc::ptr_eq(&self.filtered_file_list, &items) {
            self.filtered_file_list = items;
            self.filtered_revision += 1;
        }
    }

    /// Records that the filtered list, as it has just been set, is what `inputs`
    /// make of the file list as it is now.
    pub(crate) fn note_filters_of_filtered_list(&mut self, inputs: Arc<FilterInputs>) {
        self.filtered_with = Some(inputs);
    }

    /// The filters that made the filtered list out of the file list as it is now.
    /// `None` if that is not known: the file list has changed since, or the
    /// filtered list was set without saying what it was made with (which
    /// [`AppState::note_filters_of_filtered_list`] and [`FileLists::made_with`]
    /// do). Nothing is assumed about the filters of the state as they are now; the
    /// caller compares ([`FilterInputs::filters_like`]).
    pub(crate) fn filters_of_filtered_list(&self) -> Option<Arc<FilterInputs>> {
        self.filtered_with
            .as_ref()
            .filter(|_| self.filtered_from == self.file_list_revision)
            .map(Arc::clone)
    }

    /// Handles of the lists and of the hierarchy as they are now, for a caller that
    /// is about to replace them while it holds the lock on the state: with these in
    /// its hands, what it replaces is not dropped there and then (which takes time
    /// for a large project) but when the caller drops them, after the lock.
    pub(crate) fn lists_to_outlive_the_lock(&self) -> Discarded {
        let mut kept = Discarded::default();
        kept.add((
            Arc::clone(&self.full_file_list),
            Arc::clone(&self.filtered_file_list),
            self.tree_index_cache.cached(),
        ));
        kept
    }

    /// `true` if the filtered list is the file list: not an equal list, but the
    /// same one, made from the file list as it is now. Nothing has to be compared
    /// or assumed for this; a file list that is changed stops being the same.
    pub(crate) fn filtered_list_is_file_list(&self) -> bool {
        self.filtered_from == self.file_list_revision
            && Arc::ptr_eq(&self.filtered_file_list, &self.full_file_list)
    }

    /// Installs lists that were prepared without holding the lock on the state, see
    /// [`FileLists`]. The filtered list comes with its hierarchy, so the next state
    /// update does not have to build it.
    ///
    /// This is for a caller that has made sure, now that it holds the lock, that
    /// the lists are what its change and the filters make of the state as it is:
    /// that the file list they were made from still has the same revision, and that
    /// the filters are the same.
    pub(crate) fn install_file_lists(&mut self, lists: FileLists) -> Discarded {
        let mut discarded = Discarded::default();
        if let Some(full) = lists.full {
            discarded.add(std::mem::replace(&mut self.full_file_list, full));
            self.file_list_revision += 1;
        }
        self.filtered_from = self.file_list_revision;
        let Some(filtered) = lists.filtered else {
            discarded.add(lists.made_with);
            return discarded;
        };

        discarded.add(self.filtered_with.replace(lists.made_with));
        let (items, root_path, index) = filtered.into_parts();
        // The selection counts of a list that only grew follow from the ones the
        // state has; everything else is counted when it is asked for.
        let counts = lists
            .appended_from
            .filter(|_| root_path == Path::new(&self.current_path))
            .and_then(|base_len| self.extended_selection_counts(&items, &index, base_len));
        discarded.add(std::mem::replace(&mut self.filtered_file_list, items));
        self.filtered_revision += 1;
        if let Some(counts) = counts {
            self.selection_counts_cache.install(
                self.filtered_revision,
                &root_path,
                self.selected_files.revision(),
                counts,
            );
        }
        discarded.add(
            self.tree_index_cache
                .install(self.filtered_revision, root_path, index),
        );
        discarded
    }

    /// The selection counts of `items`, which is the filtered list with items
    /// appended from `base_len` on, and whose hierarchy is `index`: made from the
    /// counts of the filtered list, if the state has them at hand.
    fn extended_selection_counts(
        &self,
        items: &[FileItem],
        index: &TreeIndex,
        base_len: usize,
    ) -> Option<DirectorySelection> {
        let root_path = Path::new(&self.current_path);
        if base_len != self.filtered_file_list.len()
            || !self
                .tree_index_cache
                .holds(self.filtered_revision, root_path)
        {
            return None;
        }
        self.selection_counts_cache
            .take_current(
                self.filtered_revision,
                root_path,
                &self.filtered_file_list,
                &self.file_tree(),
                &self.selected_files,
            )?
            .extended(items, index, base_len, &self.selected_files)
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
    pub(crate) fn filtered_revision(&self) -> u64 {
        self.filtered_revision
    }

    /// The number of selected files below every directory of the filtered file list.
    ///
    /// Derived data like [`AppState::file_tree`]: counted on first use, and kept up
    /// to date from the journal of `selected_files` for as long as the list stays
    /// the same, so a click on one checkbox does not count the whole tree again.
    pub(crate) fn directory_selection(&self) -> Arc<DirectorySelection> {
        self.selection_counts_cache.get_or_update(
            self.filtered_revision,
            Path::new(&self.current_path),
            &self.filtered_file_list,
            &self.file_tree(),
            &self.selected_files,
        )
    }

    /// How often the selection of the whole tree had to be counted.
    #[cfg(test)]
    pub(crate) fn selection_count_count(&self) -> usize {
        self.selection_counts_cache.full_count_count()
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
    ///
    /// The generated document is not part of that: the UI goes on showing an output
    /// when another directory is opened, and gives it up itself where it stops
    /// (`clearPreviewState`, which it also sends when the directory is closed).
    fn clear_directory_data(&mut self) {
        self.cancel_current_generation();
        self.invalidate_file_list();
        self.ahead_of_walk = None;
        self.levels_before_walk = None;

        self.current_path = String::new();
        self.set_full_file_list(Vec::new());
        self.set_filtered_file_list(Vec::new());
        self.selected_files.clear();
        self.expanded_dirs.clear();
        self.collapsed_under_filter.clear();
        self.tree_view.reset();
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
            .full_file_list()
            .iter()
            .map(|item| (item.path.clone(), item.is_directory))
            .collect();

        self.edit_full_file_list(|list| {
            list.retain(|item| !rules.is_ignored(&item.path, item.is_directory));
        });

        self.selected_files.retain(|path| {
            let is_dir = path_info.get(path).copied().unwrap_or(false);
            !rules.is_ignored(path, is_dir)
        });

        // Note: active_ignore_patterns are typically recalculated during a full scan
        // or could be updated here if needed, but for local filtering this is sufficient.
    }
}

#[cfg(test)]
mod tests;
