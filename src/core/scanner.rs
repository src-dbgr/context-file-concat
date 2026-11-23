//! Provides the functionality for recursively scanning directories.

use super::{CoreError, FileItem, IgnoreRules, MAX_FILE_SIZE};
use crate::utils::file_detection::is_text_file;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
    ///
    /// "Inside" is decided on the path as it is written, without asking the file
    /// system: `directory` has to be the project root followed by names, and
    /// nothing else. A path that starts with the project root and then leaves it
    /// (`/project/../etc`) is refused, however it would resolve; so is one that
    /// merely starts with the same characters (`/project-two`). The directory that
    /// is scanned is the path made of exactly those names, so `.` and doubled
    /// separators, which name no other directory, do not survive either.
    pub fn directory_level(
        project_root: impl Into<PathBuf>,
        directory: impl Into<PathBuf>,
    ) -> Result<Self, CoreError> {
        let project_root = project_root.into();
        let directory = directory.into();
        let mut start_dir = project_root.clone();
        let mut start_depth = 0;
        for component in directory.strip_prefix(&project_root)?.components() {
            match component {
                std::path::Component::Normal(name) => {
                    start_dir.push(name);
                    start_depth += 1;
                }
                // `..`, and whatever else is no name.
                _ => return Err(CoreError::OutsideProject(directory)),
            }
        }
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
    /// Files larger than this many bytes are not listed.
    max_file_size: u64,
    /// The minimum time between two progress reports.
    progress_throttle: Duration,
    /// How the result is cut into batches.
    batch_limits: BatchLimits,
}

/// What has to be looked at to tell whether a listed entry is binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentKind {
    /// A directory, or something that has no content to read (a socket, a device).
    None,
    /// A regular file: its own content.
    File,
    /// A symbolic link (the walk does not follow links): the content of the file it
    /// points to, which is what the preview, the search and the generation read.
    Link,
}

impl ContentKind {
    /// The kind of an entry with this metadata (of the entry itself, not of the
    /// target of a link).
    fn of(metadata: &std::fs::Metadata) -> Self {
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            Self::Link
        } else if file_type.is_file() {
            Self::File
        } else {
            Self::None
        }
    }
}

/// An entry the walk has listed, together with what has to be looked at to tell
/// whether it is binary (`item.is_binary` is `false` until that has happened).
#[derive(Debug)]
struct ListedEntry {
    item: FileItem,
    kind: ContentKind,
}

/// When the walk closes the batch it is filling and hands it over.
///
/// A batch is the unit in which a scan delivers its result
/// ([`DirectoryScanner::scan_in_batches`]) and at the same time the unit of work of
/// the threads that look at the contents. The count keeps a batch small enough for
/// those threads to start while the walk is still in its first directories and makes
/// handing over cheap; the age makes sure that a slow walk (a network drive) delivers
/// what it has found without waiting for a batch to fill up. The age is watched by a
/// thread of its own ([`hand_over_aged_batches`]), so a batch goes out in time even
/// while the walk is stuck in a single slow directory listing.
#[derive(Debug, Clone, Copy)]
struct BatchLimits {
    max_items: usize,
    max_age: Duration,
}

/// The limits of a scan in production. A walk of a local disk lists some 100,000
/// entries per second, so the count decides there (a few hundred small batches per
/// second, which is what the measurements of [`MAX_CONTENT_THREADS`] were made with).
const BATCH_LIMITS: BatchLimits = BatchLimits {
    max_items: 256,
    max_age: Duration::from_millis(100),
};

/// Tells whether the entry at `path` is binary by the application-wide rule
/// (`utils::file_detection`), which `is_text` applies to a path ([`is_text_file`] in
/// production).
///
/// A file that cannot be read is taken as binary, to be on the safe side. A link is
/// judged by the file it points to; a link that leads to no file (broken, or to a
/// directory) has no content and is not binary, like the preview and the generation,
/// which report such an entry as unreadable rather than as binary.
fn is_binary_content(
    path: &Path,
    kind: ContentKind,
    is_text: &impl Fn(&Path) -> anyhow::Result<bool>,
) -> bool {
    let has_content = match kind {
        ContentKind::None => false,
        ContentKind::File => true,
        // `metadata` follows the link.
        ContentKind::Link => std::fs::metadata(path).is_ok_and(|target| target.is_file()),
    };
    has_content && !is_text(path).unwrap_or(false)
}

/// Looks at the content of every entry of `batch` and returns the finished items, in
/// the order of the batch. `None` if the scan was cancelled before the end of the
/// batch: no further file is opened then, and a batch that is not complete is not
/// delivered.
fn classify_batch(
    batch: Vec<ListedEntry>,
    cancel_flag: &AtomicBool,
    is_text: &impl Fn(&Path) -> anyhow::Result<bool>,
) -> Option<Vec<FileItem>> {
    let mut items = Vec::with_capacity(batch.len());
    for ListedEntry { mut item, kind } in batch {
        if cancel_flag.load(Ordering::Relaxed) {
            return None;
        }
        item.is_binary = is_binary_content(&item.path, kind, is_text);
        items.push(item);
    }
    Some(items)
}

/// Delivers batches that are finished in any order in the order they were handed
/// over: a batch waits here until all the batches before it have been delivered.
///
/// `deliver` is called under a lock, one batch at a time, from whichever thread
/// completes the next batch in line. It should therefore be quick (in production it
/// passes the batch on through a channel).
struct InOrder<'a> {
    deliver: &'a (dyn Fn(Vec<FileItem>) + Sync),
    line: Mutex<Line>,
}

/// The state of [`InOrder`]: the number of the batch that is delivered next, and the
/// finished batches that come after it.
#[derive(Default)]
struct Line {
    next: usize,
    waiting: BTreeMap<usize, Vec<FileItem>>,
}

impl<'a> InOrder<'a> {
    fn new(deliver: &'a (dyn Fn(Vec<FileItem>) + Sync)) -> Self {
        Self {
            deliver,
            line: Mutex::new(Line::default()),
        }
    }

    /// Takes the finished batch with the number `sequence` and delivers every batch
    /// whose turn it is now.
    fn submit(&self, sequence: usize, batch: Vec<FileItem>) {
        let mut line = self
            .line
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        line.waiting.insert(sequence, batch);
        loop {
            let next = line.next;
            let Some(batch) = line.waiting.remove(&next) else {
                break;
            };
            (self.deliver)(batch);
            line.next += 1;
        }
    }
}

/// Runs `produce`, looks at the content of the entries of every batch it hands over
/// while it runs, and passes the finished batches to `on_batch`, in the order they
/// were handed over. Returns the result of `produce` once all of that is done.
///
/// This is the part of a scan that opens files, one per listed file whose extension
/// does not settle the question. Opening a file is slow where a directory listing is
/// fast, in particular the first time, so it is neither done in the walk nor after
/// it: `produce` (the walk) runs on the calling thread, and the threads of
/// [`content_pool`] work off its batches meanwhile, one batch per task.
///
/// Once `cancel_flag` is raised no further file is opened. The batch a thread was
/// working on is then incomplete and is dropped, and with it every batch after it
/// (nothing is delivered out of order). What a cancelled scan has delivered up to
/// then is a correct, if partial, listing.
fn classify_alongside<R>(
    cancel_flag: &AtomicBool,
    is_text: &(impl Fn(&Path) -> anyhow::Result<bool> + Sync),
    on_batch: &(dyn Fn(Vec<FileItem>) + Sync),
    produce: impl FnOnce(&mut (dyn FnMut(Vec<ListedEntry>) + Send)) -> R,
) -> R {
    let in_order = InOrder::new(on_batch);
    // `in_place_scope`: `produce` stays on this thread, only the batches go to the pool.
    match content_pool() {
        Some(pool) => pool.in_place_scope(|scope| {
            hand_over_batches(scope, cancel_flag, is_text, &in_order, produce)
        }),
        None => rayon::in_place_scope(|scope| {
            hand_over_batches(scope, cancel_flag, is_text, &in_order, produce)
        }),
    }
}

/// The body of [`classify_alongside`] inside the scope of the thread pool: runs
/// `produce` and spawns a task for every batch it hands over.
fn hand_over_batches<'scope, R>(
    scope: &rayon::Scope<'scope>,
    cancel_flag: &'scope AtomicBool,
    is_text: &'scope (impl Fn(&Path) -> anyhow::Result<bool> + Sync),
    in_order: &'scope InOrder<'scope>,
    produce: impl FnOnce(&mut (dyn FnMut(Vec<ListedEntry>) + Send)) -> R,
) -> R {
    let mut handed_over = 0;
    produce(&mut |batch| {
        let sequence = handed_over;
        handed_over += 1;
        scope.spawn(move |_| {
            if let Some(items) = classify_batch(batch, cancel_flag, is_text) {
                in_order.submit(sequence, items);
            }
        });
    })
}

/// The most threads that open files at the same time during a scan.
///
/// Opening files scales with the number of threads only up to a point (the threads
/// get in each other's way inside the file system). Measured on macOS with 16 cores,
/// scanning 26,000 files, of which the walk alone takes 0.14 s:
///
/// | Threads | Content cached | Files opened for the first time |
/// | ------- | -------------- | ------------------------------- |
/// | 1       | 0.50 s         | 1.4 s                           |
/// | 4       | 0.26 s         | 0.70 s                          |
/// | 8       | 0.32 s         | 0.44 s                          |
/// | 16      | 0.48 s         | (not measured)                  |
///
/// 8 is close to the best of both columns. (The figures are from looking at the
/// contents after the walk; doing it alongside the walk takes the walk's share off
/// all of them.)
const MAX_CONTENT_THREADS: usize = 8;

/// The number of threads of [`content_pool`].
fn content_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(MAX_CONTENT_THREADS)
}

/// The thread pool the contents are looked at on: [`content_threads`] threads, built
/// when the first scan needs it. `None` if it cannot be built, in which case the
/// work runs on rayon's global pool.
fn content_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(content_threads())
            .thread_name(|index| format!("cfc-scan-content-{index}"))
            .build()
            .map_err(|e| tracing::warn!("No thread pool for the scan: {e}. Using the default."))
            .ok()
    })
    .as_ref()
}

/// The number of threads the contents are actually looked at on: those of
/// [`content_pool`], or of rayon's global pool where that stands in.
#[cfg(test)]
fn content_pool_threads() -> usize {
    content_pool().map_or_else(rayon::current_num_threads, |pool| {
        pool.current_num_threads()
    })
}

/// What a scan loop needs besides the entries of its walk.
struct WalkSettings<'a> {
    /// Entries up to and including this depth (the start directory of the scan and
    /// the directories leading to it) are walked but neither counted nor reported.
    start_depth: usize,
    /// Files larger than this many bytes are counted as skipped and not reported.
    max_file_size: u64,
    cancel_flag: &'a AtomicBool,
    /// The minimum time between two progress reports.
    progress_throttle: Duration,
    batch_limits: BatchLimits,
}

/// A scan: turns the entries a walk yields into `FileItem`s and passes them to
/// `on_batch`, batch by batch, in the order of the walk.
///
/// It takes the entries as an iterator rather than walking itself, so the loop
/// (throttling, cancellation, what happens when an entry changes or disappears between
/// being listed and being looked at) can be driven deterministically.
///
/// The walk itself ([`walk_entries`]) is sequential: it defines the order of the
/// result, the progress and where a cancellation stops it, and it does not open a
/// file. Whether the files are binary is found out on other threads while the walk
/// goes on, see [`classify_alongside`]; a batch is delivered when that is known for
/// all of its entries, so every item that leaves the scan is final.
fn stream_walker_results<F>(
    walker: impl Iterator<Item = Result<ignore::DirEntry, ignore::Error>>,
    settings: &WalkSettings,
    progress_callback: F,
    on_batch: &(dyn Fn(Vec<FileItem>) + Sync),
) where
    F: Fn(ScanProgress),
{
    let max_age = settings.batch_limits.max_age;
    classify_alongside(settings.cancel_flag, &is_text_file, on_batch, |hand_over| {
        let open_batch = Mutex::new(OpenBatch::new(settings.batch_limits, hand_over));
        let walk_ended = WalkEnded::default();
        std::thread::scope(|threads| {
            // Signalled when the walk returns, also when it does so by panicking:
            // the scope waits for the thread.
            let _signal = walk_ended.signal_on_drop();
            threads.spawn(|| hand_over_aged_batches(&open_batch, max_age, &walk_ended));
            walk_entries(walker, settings, progress_callback, &open_batch);
        });
    });
}

/// The batch the walk is filling, and where it goes when it is closed. Shared by the
/// walk, which adds the entries, and the thread that watches the age of the batch.
struct OpenBatch<'a> {
    entries: Vec<ListedEntry>,
    /// When the first entry of the batch was added.
    opened: Instant,
    limits: BatchLimits,
    hand_over: &'a mut (dyn FnMut(Vec<ListedEntry>) + Send),
}

impl<'a> OpenBatch<'a> {
    fn new(limits: BatchLimits, hand_over: &'a mut (dyn FnMut(Vec<ListedEntry>) + Send)) -> Self {
        Self {
            entries: Vec::with_capacity(limits.max_items),
            opened: Instant::now(),
            limits,
            hand_over,
        }
    }

    /// Adds an entry; a batch that is full by this is handed over.
    fn push(&mut self, entry: ListedEntry) {
        if self.entries.is_empty() {
            self.opened = Instant::now();
        }
        self.entries.push(entry);
        if self.entries.len() >= self.limits.max_items {
            self.close();
        }
    }

    /// Hands the batch over if it has been open for long enough.
    fn close_if_aged(&mut self) {
        if self.opened.elapsed() >= self.limits.max_age {
            self.close();
        }
    }

    /// Hands over what the batch holds (if anything) and starts the next one.
    fn close(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let batch = std::mem::replace(&mut self.entries, Vec::with_capacity(self.limits.max_items));
        (self.hand_over)(batch);
    }
}

/// Tells the thread that watches the age of the open batch that the walk has ended.
#[derive(Default)]
struct WalkEnded {
    ended: Mutex<bool>,
    changed: std::sync::Condvar,
}

impl WalkEnded {
    /// A guard that raises the signal when it is dropped.
    fn signal_on_drop(&self) -> impl Drop + '_ {
        struct Signal<'a>(&'a WalkEnded);
        impl Drop for Signal<'_> {
            fn drop(&mut self) {
                *lock(&self.0.ended) = true;
                self.0.changed.notify_all();
            }
        }
        Signal(self)
    }
}

/// Hands over the open batch whenever it has reached `max_age`, until the walk has
/// ended. Runs on a thread of its own beside the walk: the walk can only look at the
/// age of its batch between two entries, and on a slow file system it may wait for a
/// single directory listing for a long time with entries in hand.
///
/// It looks twice per `max_age`, so a batch leaves at most half of that late.
fn hand_over_aged_batches(open_batch: &Mutex<OpenBatch>, max_age: Duration, walk: &WalkEnded) {
    let mut ended = lock(&walk.ended);
    while !*ended {
        ended = walk
            .changed
            .wait_timeout(ended, max_age / 2)
            .map_or_else(|poisoned| poisoned.into_inner().0, |(guard, _)| guard);
        if !*ended {
            lock(open_batch).close_if_aged();
        }
    }
}

/// The loop of a scan: lists the entries of `walker` in its order, reports the
/// progress and stops when the scan is cancelled. The entries are added to
/// `open_batch`, which hands them over in batches (see [`BatchLimits`]);
/// `FileItem::is_binary` is left `false` here.
fn walk_entries<F>(
    walker: impl Iterator<Item = Result<ignore::DirEntry, ignore::Error>>,
    settings: &WalkSettings,
    progress_callback: F,
    open_batch: &Mutex<OpenBatch>,
) where
    F: Fn(ScanProgress),
{
    let mut large_files_skipped = 0;
    let mut files_scanned = 0;
    let mut last_update = Instant::now();

    for result in walker {
        if settings.cancel_flag.load(Ordering::SeqCst) {
            // What has not been handed over yet is not going to be looked at either.
            return;
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

        // Check for the start directory (and its ancestors) and skip it *before*
        // incrementing the counter. This prevents inflating the scanned count with
        // directories that are not part of the result.
        if entry.depth() <= settings.start_depth {
            continue;
        }

        // All logic related to counting and progress now happens only for valid entries.
        files_scanned += 1;
        if Instant::now().duration_since(last_update) > settings.progress_throttle {
            tracing::debug!(
                "[SCANNER] Throttling condition met. Invoking progress callback for {} files.",
                files_scanned
            );
            let path_str = entry.path().to_string_lossy().into_owned();
            progress_callback(ScanProgress {
                files_scanned,
                large_files_skipped,
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

        if !metadata.is_dir() && metadata.len() > settings.max_file_size {
            large_files_skipped += 1;
            continue;
        }

        lock(open_batch).push(ListedEntry {
            item: FileItem {
                path: entry.path().to_path_buf(),
                is_directory: metadata.is_dir(),
                // Entered once the content has been looked at.
                is_binary: false,
                size: metadata.len(),
                depth: entry.depth(),
                parent: entry.path().parent().map(PathBuf::from),
            },
            kind: ContentKind::of(&metadata),
        });
    }
    lock(open_batch).close();
}

impl DirectoryScanner {
    /// Creates a scanner that skips files above the default size limit
    /// ([`MAX_FILE_SIZE`]).
    pub fn new(ignore_patterns: HashSet<String>) -> Self {
        Self {
            ignore_patterns,
            max_file_size: MAX_FILE_SIZE,
            progress_throttle: PROGRESS_UPDATE_THROTTLE,
            batch_limits: BATCH_LIMITS,
        }
    }

    /// Replaces the size limit: files larger than `max_file_size` bytes are not listed.
    pub fn with_max_file_size(mut self, max_file_size: u64) -> Self {
        self.max_file_size = max_file_size;
        self
    }

    /// A scanner that reports its progress at most every `throttle`.
    #[cfg(test)]
    fn new_with_throttle(ignore_patterns: HashSet<String>, throttle: Duration) -> Self {
        Self {
            progress_throttle: throttle,
            ..Self::new(ignore_patterns)
        }
    }

    /// Performs the scan described by `request` and returns everything it lists at
    /// once: the items in the order of the walk, and the custom ignore patterns that
    /// were active. This is [`DirectoryScanner::scan_in_batches`] with the batches
    /// collected; see there for how the scan works.
    ///
    /// A scan that is cancelled returns what it had delivered up to then.
    pub async fn scan<F>(
        &self,
        request: &ScanRequest,
        cancel_flag: Arc<AtomicBool>,
        progress_callback: F,
    ) -> Result<(Vec<FileItem>, HashSet<String>), CoreError>
    where
        F: Fn(ScanProgress) + Send + Sync + 'static,
    {
        let collected = Arc::new(Mutex::new(Vec::new()));
        let collector = Arc::clone(&collected);
        let matched_patterns = self
            .scan_in_batches(request, cancel_flag, progress_callback, move |batch| {
                lock(&collector).extend(batch);
            })
            .await?;
        let items = std::mem::take(&mut *lock(&collected));
        Ok((items, matched_patterns))
    }

    /// Performs the scan described by `request` asynchronously and delivers what it
    /// lists while it runs: `on_batch` is called with one batch of finished items
    /// after the other, in the order of the walk, every item exactly once. Progress
    /// is reported via `progress_callback`. The result is the set of custom ignore
    /// patterns that were active.
    ///
    /// `on_batch` is called from the threads of the scan and holds up the delivery of
    /// the following batches while it runs, so it should only pass the batch on. How
    /// the walk is cut into batches is described at [`BatchLimits`]. When the scan is
    /// cancelled, the batches delivered so far are a correct but partial listing.
    ///
    /// This function performs the scan in a blocking thread to avoid blocking the async runtime,
    /// while allowing for cancellation and progress reporting. It uses the `ignore` crate
    /// for high-performance, gitignore-aware directory traversal. The custom ignore
    /// patterns are applied through [`IgnoreRules`], which also reports which of them
    /// were active.
    ///
    /// The walk always begins at the project root, even when only a subdirectory is
    /// requested: on its way down it visits nothing but the directories leading to the
    /// start directory. That way the `.gitignore` files of all ancestors inside the
    /// project, the anchoring of the custom patterns and the depth of the items are the
    /// same as in a full scan by construction. A start directory that is itself ignored
    /// consequently yields nothing.
    pub async fn scan_in_batches<F, B>(
        &self,
        request: &ScanRequest,
        cancel_flag: Arc<AtomicBool>,
        progress_callback: F,
        on_batch: B,
    ) -> Result<HashSet<String>, CoreError>
    where
        F: Fn(ScanProgress) + Send + Sync + 'static,
        B: Fn(Vec<FileItem>) + Send + Sync + 'static,
    {
        let request = request.clone();
        let ignore_patterns_clone = self.ignore_patterns.clone();
        let max_file_size = self.max_file_size;
        let progress_throttle = self.progress_throttle;
        let batch_limits = self.batch_limits;

        let blocking_task_handle = tokio::task::spawn_blocking(move || {
            let mut walker_builder = ignore::WalkBuilder::new(request.project_root());

            let rules = Arc::new(IgnoreRules::new(
                request.project_root(),
                &ignore_patterns_clone,
            ));

            walker_builder
                .max_depth(request.max_depth_from_project_root())
                .hidden(false)
                .parents(false)
                .git_global(true)
                .git_ignore(true)
                .git_exclude(true)
                .require_git(false) // CRITICAL: Don't require a .git repo to exist.
                .follow_links(false);

            // The walk is top-down and does not descend into excluded directories, so
            // asking the rules about each entry on its own is the complete answer. The
            // rules remember which patterns decided, which is the report of active ones.
            let walk_rules = Arc::clone(&rules);
            let settings = WalkSettings {
                start_depth: request.start_depth,
                max_file_size,
                cancel_flag: &cancel_flag,
                progress_throttle,
                batch_limits,
            };
            walker_builder.filter_entry(move |entry| {
                // Stay on the route to the start directory. Entries beside it are not
                // part of this scan, so they must not activate a pattern either.
                if !request.is_on_route(entry.path()) {
                    return false;
                }
                let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
                !walk_rules.ignores_entry(entry.path(), is_dir)
            });

            let walker = walker_builder.build();

            stream_walker_results(walker, &settings, progress_callback, &on_batch);

            rules.matched_patterns()
        });

        // Await the result. If the task panicked, spawn_blocking returns a JoinError,
        // which we convert into our custom CoreError::Join.
        blocking_task_handle.await.map_err(CoreError::from)
    }
}

/// Locks `mutex`, whether or not a thread panicked while it held the lock.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The entries of `walker`, with `observe` called on each one right before the scan
    /// loop gets to see it. This is how a test makes something happen at an exact
    /// point of a scan: delete the file that comes up next, raise the cancellation
    /// flag after the fifth entry, let time pass.
    fn observing_each_entry(
        walker: ignore::Walk,
        mut observe: impl FnMut(&ignore::DirEntry),
    ) -> impl Iterator<Item = Result<ignore::DirEntry, ignore::Error>> {
        walker.inspect(move |entry| {
            if let Ok(entry) = entry {
                observe(entry);
            }
        })
    }
    /// The whole result of the scan loop over `walker`, in the order of the walk,
    /// cut into batches by the limits of production.
    fn process_walker_results<F>(
        walker: impl Iterator<Item = Result<ignore::DirEntry, ignore::Error>>,
        start_depth: usize,
        max_file_size: u64,
        cancel_flag: Arc<AtomicBool>,
        progress_callback: F,
        progress_throttle: Duration,
    ) -> Vec<FileItem>
    where
        F: Fn(ScanProgress),
    {
        let settings = WalkSettings {
            start_depth,
            max_file_size,
            cancel_flag: &cancel_flag,
            progress_throttle,
            batch_limits: BATCH_LIMITS,
        };
        batches_of_walk(walker, &settings, progress_callback).concat()
    }

    /// The batches the scan loop delivers for `walker`, in the order of delivery.
    fn batches_of_walk<F>(
        walker: impl Iterator<Item = Result<ignore::DirEntry, ignore::Error>>,
        settings: &WalkSettings,
        progress_callback: F,
    ) -> Vec<Vec<FileItem>>
    where
        F: Fn(ScanProgress),
    {
        let batches = Mutex::new(Vec::new());
        stream_walker_results(walker, settings, progress_callback, &|batch| {
            batches.lock().unwrap().push(batch);
        });
        batches.into_inner().unwrap()
    }

    use crate::utils::test_helpers::setup_test_logging;
    use std::fs;
    use std::panic;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
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

        // A small delay per entry makes sure the cancellation happens mid-scan.
        let walker = observing_each_entry(walker, |_| {
            std::thread::sleep(std::time::Duration::from_millis(1));
        });

        let handle = tokio::task::spawn_blocking(move || {
            process_walker_results(
                walker,
                0,
                MAX_FILE_SIZE,
                task_cancel_flag,
                move |progress| {
                    if progress.files_scanned > 20 {
                        if let Some(s) = task_sender.lock().unwrap().take() {
                            let _ = s.send(());
                        }
                    }
                },
                PROGRESS_UPDATE_THROTTLE, // Use standard throttle for this test
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

        // What was delivered are the batches that were complete by then (possibly
        // none), not the whole walk.
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

        let mut entries_seen = 0;
        // The walk yields the root dir first (depth 0), then file1.txt. Sleeping when
        // the *first file* comes up lets the throttle interval pass exactly once.
        let walker = observing_each_entry(walker, move |entry| {
            entries_seen += 1;
            if entry.depth() > 0 && entries_seen == 2 {
                std::thread::sleep(PROGRESS_UPDATE_THROTTLE + Duration::from_millis(10));
            }
        });

        process_walker_results(
            walker,
            0,
            MAX_FILE_SIZE,
            Arc::new(AtomicBool::new(false)),
            move |progress| {
                updates_clone.lock().unwrap().push(progress);
            },
            PROGRESS_UPDATE_THROTTLE, // Use standard throttle
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

    /// A negated pattern re-includes what another custom pattern excludes, and both
    /// are reported as active. The scan agrees with [`IgnoreRules::is_ignored`] about
    /// every path, which is what keeps the scan and the local filtering in step.
    #[tokio::test]
    async fn test_negated_custom_pattern_reincludes_entries() {
        setup_test_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        fs::create_dir_all(root.join("docs/drafts")).unwrap();
        fs::create_dir_all(root.join("out")).unwrap();
        let all = [
            "docs/a.md",
            "docs/keep.md",
            "docs/drafts/keep.md",
            "notes.md",
            "main.rs",
            "out/keep.md",
        ];
        for path in all {
            fs::write(root.join(path), b"text").unwrap();
        }
        let patterns: HashSet<String> = ["*.md", "!keep.md", "out", "*.unused"]
            .into_iter()
            .map(String::from)
            .collect();

        let (files, active_patterns) = DirectoryScanner::new(patterns.clone())
            .scan(
                &ScanRequest::project(&root, None),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await
            .expect("Scan should succeed");

        let listed: HashSet<PathBuf> = files
            .iter()
            .filter(|item| !item.is_directory)
            .map(|item| item.path.clone())
            .collect();
        let expected: HashSet<PathBuf> = ["docs/keep.md", "docs/drafts/keep.md", "main.rs"]
            .into_iter()
            .map(|path| root.join(path))
            .collect();
        assert_eq!(listed, expected);
        assert_eq!(
            active_patterns,
            ["*.md", "!keep.md", "out"]
                .into_iter()
                .map(String::from)
                .collect::<HashSet<_>>()
        );

        let rules = IgnoreRules::new(&root, &patterns);
        for path in all {
            let path = root.join(path);
            assert_eq!(
                rules.is_ignored(&path, false),
                !listed.contains(&path),
                "{path:?}"
            );
        }
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
        use crate::utils::test_helpers::running_as_root;
        use std::os::unix::fs::PermissionsExt;

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
    ///
    /// Not on Windows: there the walker hands out the metadata it read together with
    /// the directory listing, so what happens to the file afterwards is not seen.
    #[cfg(not(windows))]
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
        let walker = observing_each_entry(walker, move |entry: &ignore::DirEntry| {
            if entry.path() == hook_file_to_delete {
                // Deleting the file here causes the subsequent `entry.metadata()` call to fail.
                fs::remove_file(&hook_file_to_delete).unwrap();
            }
        });

        // The loop is driven directly, with entries that change under its feet.
        let files = process_walker_results(
            walker,
            0,
            MAX_FILE_SIZE,
            Arc::new(AtomicBool::new(false)),
            |_| {},
            PROGRESS_UPDATE_THROTTLE,
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

        let walker = observing_each_entry(walker, move |entry: &ignore::DirEntry| {
            if entry.depth() > 0 {
                // Skip counting the root directory itself.
                let count = hook_files_processed.fetch_add(1, Ordering::SeqCst);
                if count == 5 {
                    hook_cancel_flag.store(true, Ordering::SeqCst);
                }
            }
        });

        let files = process_walker_results(
            walker,
            0,
            MAX_FILE_SIZE,
            cancel_flag,
            |_| {},
            PROGRESS_UPDATE_THROTTLE,
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
    ///
    /// Not on Windows: there the walker hands out the metadata it read together with
    /// the directory listing, so what happens to the file afterwards is not seen.
    #[cfg(not(windows))]
    #[tokio::test]
    async fn test_scanner_handles_file_being_replaced_by_directory() {
        setup_test_logging();
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let path_to_replace = root.join("race_condition.data");
        fs::write(&path_to_replace, "i will be replaced").unwrap();

        let walker = ignore::WalkBuilder::new(root).build();

        let hook_path_to_replace = path_to_replace.clone();
        let walker = observing_each_entry(walker, move |entry: &ignore::DirEntry| {
            if entry.path() == hook_path_to_replace {
                // This simulates a race condition where the file is replaced by a directory
                // after being discovered by the directory walker.
                fs::remove_file(&hook_path_to_replace).unwrap();
                fs::create_dir(&hook_path_to_replace).unwrap();
            }
        });

        let files = process_walker_results(
            walker,
            0,
            MAX_FILE_SIZE,
            Arc::new(AtomicBool::new(false)),
            |_| {},
            PROGRESS_UPDATE_THROTTLE,
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
        let scanner = DirectoryScanner::new(HashSet::from(["/src/generated/".to_string()]));
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
        let pattern = "/src/generated/".to_string();
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

    /// The request is the boundary: nothing gets past it that is not the project
    /// root followed by names, whatever the path would resolve to on disk.
    #[test]
    fn directory_level_request_rejects_a_path_that_leaves_the_project() {
        for outside in [
            "/project/..",
            "/project/../etc",
            "/project/../project-two/src",
            "/project/src/../../etc",
            "/project/src/..",
            "/project/./../etc",
            "/project//../etc",
            // Back inside in the end, but not a path made of names.
            "/project/src/../src",
            "/project/../project/src",
        ] {
            let result = ScanRequest::directory_level("/project", outside);
            assert!(
                matches!(result, Err(CoreError::OutsideProject(_))),
                "{outside}: {result:?}"
            );
        }
        // A sibling that starts with the same characters is not inside.
        for sibling in ["/project-two", "/projects/src", "/projec", "/"] {
            let result = ScanRequest::directory_level("/project", sibling);
            assert!(
                matches!(result, Err(CoreError::PathStrip(_))),
                "{sibling}: {result:?}"
            );
        }
        assert!(ScanRequest::directory_level("/project", "relative/src").is_err());
    }

    /// `.` and doubled or trailing separators name the same directory; what is
    /// scanned is the directory written with its names only.
    #[test]
    fn directory_level_request_is_made_of_the_names_below_the_root() {
        let plain = ScanRequest::directory_level("/project", "/project/src/core").unwrap();
        assert_eq!(plain.start_dir(), Path::new("/project/src/core"));
        for spelling in [
            "/project/./src/core",
            "/project//src///core",
            "/project/src/core/",
            "/project/src/./core/.",
        ] {
            let request = ScanRequest::directory_level("/project", spelling).unwrap();
            assert_eq!(request, plain, "{spelling}");
            // As text, not as a path (which would look past `.` and separators):
            // the root as given, and the names joined the way the platform does.
            assert_eq!(
                request.start_dir().as_os_str(),
                Path::new("/project").join("src").join("core").as_os_str(),
                "{spelling}"
            );
        }
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

    /// The size limit is a property of the scanner: the default is the limit of the
    /// core, a custom one decides which files are listed and which count as skipped.
    #[tokio::test]
    async fn scan_applies_the_configured_file_size_limit() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().to_path_buf();
        fs::write(root.join("fits.txt"), b"12345").unwrap();
        fs::write(root.join("too_big.txt"), b"123456").unwrap();
        fs::create_dir(root.join("dir")).unwrap();
        let skipped = Arc::new(AtomicUsize::new(0));
        let skipped_by_scan = skipped.clone();

        let (files, _) = DirectoryScanner::new_with_throttle(HashSet::new(), Duration::ZERO)
            .with_max_file_size(5)
            .scan(
                &ScanRequest::project(&root, None),
                Arc::new(AtomicBool::new(false)),
                move |progress| {
                    skipped_by_scan.fetch_max(progress.large_files_skipped, Ordering::SeqCst);
                },
            )
            .await
            .unwrap();

        let mut names: Vec<String> = files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["dir", "fits.txt"], "Directories have no size");

        let (default_files, _) = DirectoryScanner::new(HashSet::new())
            .scan(
                &ScanRequest::project(&root, None),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(default_files.len(), 3);
    }

    // =========================================================================
    // SECTION: Telling binary files from text (after the walk, in parallel)
    // =========================================================================

    /// Scans `root` with the loop of the scanner, in the (sorted) order of the walk.
    fn scan_sorted(root: &Path) -> Vec<FileItem> {
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .follow_links(false)
            .sort_by_file_path(|a, b| a.cmp(b))
            .build();
        process_walker_results(
            walker,
            0,
            MAX_FILE_SIZE,
            Arc::new(AtomicBool::new(false)),
            |_| {},
            PROGRESS_UPDATE_THROTTLE,
        )
    }

    /// Looks at the content of `items` (of the kinds named index by index) the way a
    /// scan does: handed over in batches of the size of production. `items` becomes
    /// what was delivered, which is the whole list unless the scan was cancelled.
    fn classify_content(
        items: &mut Vec<FileItem>,
        kinds: &[ContentKind],
        cancel_flag: &AtomicBool,
        is_text: &(impl Fn(&Path) -> anyhow::Result<bool> + Sync),
    ) {
        let mut entries: Vec<ListedEntry> = std::mem::take(items)
            .into_iter()
            .zip(kinds)
            .map(|(item, kind)| ListedEntry { item, kind: *kind })
            .collect();
        let delivered = Mutex::new(Vec::new());
        classify_alongside(
            cancel_flag,
            is_text,
            &|batch| delivered.lock().unwrap().extend(batch),
            |hand_over| {
                while !entries.is_empty() {
                    let rest = entries.split_off(entries.len().min(BATCH_LIMITS.max_items));
                    hand_over(std::mem::replace(&mut entries, rest));
                }
            },
        );
        *items = delivered.into_inner().unwrap();
    }

    fn file_items(count: usize) -> Vec<FileItem> {
        (0..count)
            .map(|i| FileItem {
                path: PathBuf::from(format!("/project/file_{i}.txt")),
                ..Default::default()
            })
            .collect()
    }

    /// The list comes out in the order of the walk, each file with its own verdict,
    /// however the parallel look at the contents is scheduled.
    #[test]
    fn test_classifying_in_parallel_keeps_the_order_of_the_walk() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let mut expected = Vec::new();
        for dir in 0..8 {
            let dir_name = format!("dir_{dir}");
            fs::create_dir(root.join(&dir_name)).unwrap();
            expected.push((root.join(&dir_name), false));
            for file in 0..40 {
                // Sorted by name; every third file is binary by content.
                let path = root.join(&dir_name).join(format!("file_{file:02}.dat"));
                let is_binary = file % 3 == 0;
                let content: &[u8] = if is_binary { b"\x00\x01" } else { b"text" };
                fs::write(&path, content).unwrap();
                expected.push((path, is_binary));
            }
        }

        for _ in 0..3 {
            let listed: Vec<(PathBuf, bool)> = scan_sorted(root)
                .into_iter()
                .map(|item| (item.path, item.is_binary))
                .collect();
            assert_eq!(listed, expected);
        }
    }

    /// Only entries with content are looked at, each one exactly once, and what the
    /// rule says is what the item gets. A file that cannot be read counts as binary.
    #[test]
    fn test_classify_content_asks_the_rule_once_per_file() {
        let mut items = file_items(6);
        items[1].is_directory = true;
        let kinds = [
            ContentKind::File,
            ContentKind::None,
            ContentKind::File,
            ContentKind::File,
            ContentKind::None,
            ContentKind::File,
        ];
        let asked = Mutex::new(Vec::new());
        let is_text = |path: &Path| {
            asked.lock().unwrap().push(path.to_path_buf());
            match path.file_name().unwrap().to_str().unwrap() {
                "file_0.txt" => Ok(true),
                "file_2.txt" => Ok(false),
                "file_3.txt" => Err(anyhow::anyhow!("permission denied")),
                "file_5.txt" => Ok(true),
                other => panic!("{other} has no content to look at"),
            }
        };

        classify_content(&mut items, &kinds, &AtomicBool::new(false), &is_text);

        let verdicts: Vec<bool> = items.iter().map(|item| item.is_binary).collect();
        assert_eq!(verdicts, [false, false, true, true, false, false]);
        let mut asked = asked.into_inner().unwrap();
        asked.sort();
        assert_eq!(
            asked,
            [0, 2, 3, 5].map(|i| PathBuf::from(format!("/project/file_{i}.txt")))
        );
    }

    /// A cancellation stops the look at the contents: files that have not been
    /// started are not opened any more.
    #[test]
    fn test_classify_content_stops_when_the_scan_is_cancelled() {
        const FILES: usize = 20_000;
        let mut items = file_items(FILES);
        let kinds = vec![ContentKind::File; FILES];
        let cancel_flag = AtomicBool::new(false);
        let asked = AtomicUsize::new(0);
        // The first file that is looked at cancels the scan.
        let is_text = |_: &Path| {
            asked.fetch_add(1, Ordering::SeqCst);
            cancel_flag.store(true, Ordering::SeqCst);
            Ok(false)
        };

        classify_content(&mut items, &kinds, &cancel_flag, &is_text);

        // Only the files the worker threads had begun with at that moment got through.
        let asked = asked.load(Ordering::SeqCst);
        assert!(
            (1..=content_pool_threads()).contains(&asked),
            "{asked} of {FILES} files were looked at"
        );
        // No batch was complete at that moment, so none was delivered.
        assert!(items.is_empty(), "{} items were delivered", items.len());
    }

    /// The contents are looked at, and the finished batch is delivered, while the walk
    /// is still under way, not after it: the walk below does not end before the first
    /// batch it handed over has been delivered. (If the batch were only worked off
    /// after the walk, the walk would wait in vain and the test would fail.)
    #[test]
    fn test_batches_are_delivered_while_the_walk_goes_on() {
        let (delivered_tx, delivered_rx) = std::sync::mpsc::channel();
        let delivered_tx = Mutex::new(delivered_tx);
        let is_text = |_: &Path| Ok(true);
        let entries: Vec<ListedEntry> = file_items(BATCH_LIMITS.max_items)
            .into_iter()
            .map(|item| ListedEntry {
                item,
                kind: ContentKind::File,
            })
            .collect();

        let delivered_during_the_walk = classify_alongside(
            &AtomicBool::new(false),
            &is_text,
            &|batch| {
                let _ = delivered_tx.lock().unwrap().send(batch.len());
            },
            |hand_over| {
                hand_over(entries);
                // The walk is not done yet: this is still inside it.
                delivered_rx.recv_timeout(Duration::from_secs(30))
            },
        );

        assert_eq!(delivered_during_the_walk, Ok(BATCH_LIMITS.max_items));
    }

    /// Batches that are finished out of order are delivered in the order they were
    /// handed over: here the first batch is the slowest by far.
    #[test]
    fn test_batches_are_delivered_in_the_order_of_the_walk() {
        const BATCHES: usize = 12;
        let is_text = |path: &Path| {
            if path.starts_with("/batch_0") {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(true)
        };
        let delivered = Mutex::new(Vec::new());

        classify_alongside(
            &AtomicBool::new(false),
            &is_text,
            &|batch| delivered.lock().unwrap().push(batch[0].path.clone()),
            |hand_over| {
                for batch in 0..BATCHES {
                    hand_over(vec![ListedEntry {
                        item: FileItem {
                            path: PathBuf::from(format!("/batch_{batch}/file.txt")),
                            ..Default::default()
                        },
                        kind: ContentKind::File,
                    }]);
                }
            },
        );

        let expected: Vec<PathBuf> = (0..BATCHES)
            .map(|batch| PathBuf::from(format!("/batch_{batch}/file.txt")))
            .collect();
        assert_eq!(delivered.into_inner().unwrap(), expected);
    }

    fn walk_settings(cancel_flag: &AtomicBool, batch_limits: BatchLimits) -> WalkSettings<'_> {
        WalkSettings {
            start_depth: 0,
            max_file_size: MAX_FILE_SIZE,
            cancel_flag,
            progress_throttle: PROGRESS_UPDATE_THROTTLE,
            batch_limits,
        }
    }

    fn sorted_walk(root: &Path) -> ignore::Walk {
        ignore::WalkBuilder::new(root)
            .hidden(false)
            .follow_links(false)
            .sort_by_file_path(|a, b| a.cmp(b))
            .build()
    }

    /// A project of `count` files `file_000.txt`, ... in one directory.
    fn flat_project(count: usize) -> TempDir {
        let temp_dir = tempfile::tempdir().unwrap();
        for file in 0..count {
            fs::write(temp_dir.path().join(format!("file_{file:03}.txt")), "text").unwrap();
        }
        temp_dir
    }

    /// The walk closes a batch when it holds the configured number of entries; the
    /// last, incomplete one is not lost. Together the batches are the list, in order.
    #[test]
    fn test_a_walk_is_cut_into_batches_by_count() {
        let project = flat_project(25);
        let cancel_flag = AtomicBool::new(false);
        let limits = BatchLimits {
            max_items: 10,
            max_age: Duration::from_secs(3600),
        };

        let batches = batches_of_walk(
            sorted_walk(project.path()),
            &walk_settings(&cancel_flag, limits),
            |_| {},
        );

        let sizes: Vec<usize> = batches.iter().map(Vec::len).collect();
        assert_eq!(sizes, [10, 10, 5]);
        let names: Vec<String> = batches
            .concat()
            .iter()
            .map(|item| {
                item.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let expected: Vec<String> = (0..25).map(|file| format!("file_{file:03}.txt")).collect();
        assert_eq!(names, expected);
    }

    /// A batch that has reached the configured age is delivered while the walk is
    /// still waiting for its next entry (a slow directory listing): the walk below
    /// only goes on once the entries it has listed so far have arrived.
    #[test]
    fn test_an_aged_batch_is_delivered_while_the_walk_is_stuck() {
        let project = flat_project(6);
        let cancel_flag = AtomicBool::new(false);
        let limits = BatchLimits {
            max_items: 1000,
            max_age: Duration::from_millis(20),
        };
        let (delivered_tx, delivered_rx) = std::sync::mpsc::channel();
        let batches = Mutex::new(Vec::new());
        // The walk hangs before its fourth file (the root comes first).
        let mut seen = 0;
        let mut delivered_while_stuck = None;
        let walker = observing_each_entry(sorted_walk(project.path()), |_| {
            seen += 1;
            if seen == 5 {
                delivered_while_stuck = delivered_rx.recv_timeout(Duration::from_secs(30)).ok();
            }
        });

        stream_walker_results(
            walker,
            &walk_settings(&cancel_flag, limits),
            |_| {},
            &|batch: Vec<FileItem>| {
                let _ = delivered_tx.send(batch.len());
                batches.lock().unwrap().push(batch);
            },
        );

        assert!(
            delivered_while_stuck.is_some_and(|size| (1..=3).contains(&size)),
            "{delivered_while_stuck:?}"
        );
        assert_eq!(batches.into_inner().unwrap().concat().len(), 6);
    }

    /// A batch that has been open for longer than the configured age is handed over
    /// with what it holds: a slow walk delivers what it has found so far.
    #[test]
    fn test_a_slow_walk_delivers_batches_by_age() {
        let project = flat_project(6);
        let cancel_flag = AtomicBool::new(false);
        let limits = BatchLimits {
            max_items: 1000,
            max_age: Duration::from_millis(20),
        };
        // The walk takes its time before the fourth entry (the root comes first).
        let mut seen = 0;
        let walker = observing_each_entry(sorted_walk(project.path()), |_| {
            seen += 1;
            if seen == 5 {
                std::thread::sleep(Duration::from_millis(60));
            }
        });

        let batches = batches_of_walk(walker, &walk_settings(&cancel_flag, limits), |_| {});

        // The pause splits the walk (usually into 3 + 3; a machine that stalls
        // elsewhere too may split it further). Nothing is lost or reordered.
        assert!(batches.len() >= 2, "{} batch(es)", batches.len());
        assert!(batches.iter().all(|batch| !batch.is_empty()));
        let names: Vec<String> = batches
            .concat()
            .iter()
            .map(|item| {
                item.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let expected: Vec<String> = (0..6).map(|file| format!("file_{file:03}.txt")).collect();
        assert_eq!(names, expected);
    }

    /// What a cancelled scan has delivered is a prefix of the walk, in whole batches.
    #[test]
    fn test_a_cancelled_walk_has_delivered_a_prefix_in_whole_batches() {
        let project = flat_project(25);
        let cancel_flag = AtomicBool::new(false);
        let limits = BatchLimits {
            max_items: 10,
            max_age: Duration::from_secs(3600),
        };
        let (delivered_tx, delivered_rx) = std::sync::mpsc::channel();
        let batches = Mutex::new(Vec::new());
        // The scan is cancelled when the walk comes to the 16th file, after the first
        // batch has been delivered.
        let mut seen = 0;
        let walker = observing_each_entry(sorted_walk(project.path()), |_| {
            seen += 1;
            if seen == 17 {
                delivered_rx
                    .recv_timeout(Duration::from_secs(30))
                    .expect("the first batch is delivered during the walk");
                cancel_flag.store(true, Ordering::SeqCst);
            }
        });

        stream_walker_results(
            walker,
            &walk_settings(&cancel_flag, limits),
            |_| {},
            &|batch: Vec<FileItem>| {
                batches.lock().unwrap().push(batch);
                let _ = delivered_tx.send(());
            },
        );

        let batches = batches.into_inner().unwrap();
        assert!(
            batches.len() == 1 && batches[0].len() == 10,
            "one whole batch is delivered, the half-filled one is not: {:?}",
            batches.iter().map(Vec::len).collect::<Vec<_>>()
        );
        assert!(batches[0][9].path.ends_with("file_009.txt"));
    }

    /// The scanner delivers every item exactly once and in the order in which
    /// `scan` returns them, and reports the active patterns at the end.
    #[tokio::test]
    async fn scan_in_batches_delivers_the_result_of_a_scan_piece_by_piece() {
        let (_temp_dir, root) = setup_test_filesystem();
        let patterns = HashSet::from(["*.log".to_string()]);
        let request = ScanRequest::project(&root, None);
        let (expected, expected_patterns) = DirectoryScanner::new(patterns.clone())
            .scan(&request, Arc::new(AtomicBool::new(false)), |_| {})
            .await
            .unwrap();
        let mut scanner = DirectoryScanner::new(patterns);
        scanner.batch_limits.max_items = 2;
        let batches = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&batches);

        let active_patterns = scanner
            .scan_in_batches(
                &request,
                Arc::new(AtomicBool::new(false)),
                |_| {},
                move |batch| sink.lock().unwrap().push(batch),
            )
            .await
            .unwrap();

        let batches = std::mem::take(&mut *batches.lock().unwrap());
        assert!(batches.len() > 1 && batches.iter().all(|batch| batch.len() <= 2));
        // The walk is not sorted, but two walks of an unchanged tree agree.
        assert_eq!(summarize(batches.concat()), summarize(expected));
        assert_eq!(active_patterns, expected_patterns);
    }

    /// However many batches the entries make up, every entry is delivered exactly
    /// once, in its place and with its own verdict.
    #[test]
    fn test_every_submitted_entry_gets_its_verdict() {
        for count in [
            0,
            1,
            BATCH_LIMITS.max_items - 1,
            BATCH_LIMITS.max_items,
            BATCH_LIMITS.max_items + 1,
            3 * BATCH_LIMITS.max_items + 7,
        ] {
            let mut items = file_items(count);
            let kinds = vec![ContentKind::File; count];
            // Binary: every file with an odd number.
            let is_text = |path: &Path| {
                let name = path.file_stem().unwrap().to_str().unwrap();
                let number: usize = name.trim_start_matches("file_").parse().unwrap();
                Ok(number.is_multiple_of(2))
            };

            classify_content(&mut items, &kinds, &AtomicBool::new(false), &is_text);

            let verdicts: Vec<bool> = items.iter().map(|item| item.is_binary).collect();
            let expected: Vec<bool> = (0..count).map(|number| number % 2 == 1).collect();
            assert_eq!(verdicts, expected, "{count} entries");
        }
    }

    #[test]
    fn test_contents_are_looked_at_on_a_bounded_pool() {
        assert!((1..=MAX_CONTENT_THREADS).contains(&content_threads()));
        let pool = content_pool().expect("the pool can be built");
        assert_eq!(pool.current_num_threads(), content_threads());
        // The work does run there.
        let mut items = file_items(1);
        let is_text = |_: &Path| {
            let thread = std::thread::current();
            assert!(thread.name().unwrap().starts_with("cfc-scan-content-"));
            Ok(true)
        };
        classify_content(
            &mut items,
            &[ContentKind::File],
            &AtomicBool::new(false),
            &is_text,
        );
    }

    #[test]
    fn test_classify_content_does_nothing_for_a_scan_cancelled_during_the_walk() {
        let mut items = file_items(100);
        let kinds = vec![ContentKind::File; 100];
        let is_text = |path: &Path| -> anyhow::Result<bool> { panic!("{path:?} was looked at") };

        classify_content(&mut items, &kinds, &AtomicBool::new(true), &is_text);

        assert!(items.iter().all(|item| !item.is_binary));
    }

    /// A symbolic link is judged by the file it points to, which is what the preview,
    /// the search and the generation read. A link without a file behind it has no
    /// content: it is not binary.
    #[cfg(unix)]
    #[test]
    fn test_symbolic_links_are_classified_by_their_target() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("text"), "plain text").unwrap();
        fs::write(outside.path().join("binary"), b"\x00\x01\x02").unwrap();
        fs::create_dir(outside.path().join("directory")).unwrap();

        symlink(outside.path().join("text"), root.join("to_text")).unwrap();
        symlink(outside.path().join("binary"), root.join("to_binary")).unwrap();
        symlink(outside.path().join("text"), root.join("to_text.png")).unwrap();
        symlink(outside.path().join("directory"), root.join("to_directory")).unwrap();
        symlink(outside.path().join("missing"), root.join("broken")).unwrap();
        symlink(root.join("loop"), root.join("loop")).unwrap();

        let verdicts: Vec<(String, bool, bool)> = scan_sorted(root)
            .into_iter()
            .map(|item| {
                let name = item.path.file_name().unwrap().to_str().unwrap().to_string();
                (name, item.is_directory, item.is_binary)
            })
            .collect();

        assert_eq!(
            verdicts,
            [
                ("broken".to_string(), false, false),
                ("loop".to_string(), false, false),
                ("to_binary".to_string(), false, true),
                ("to_directory".to_string(), false, false),
                ("to_text".to_string(), false, false),
                // The extension of the listed name counts, as for a regular file.
                ("to_text.png".to_string(), false, true),
            ]
        );
    }

    /// The scan and the preview agree about a linked file as about any other.
    #[cfg(unix)]
    #[test]
    fn test_scan_and_preview_agree_on_linked_files() {
        use crate::core::FileHandler;
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("text"), "plain text").unwrap();
        fs::write(outside.path().join("binary"), b"\x00\x01\x02").unwrap();
        symlink(outside.path().join("text"), root.join("to_text")).unwrap();
        symlink(outside.path().join("binary"), root.join("to_binary")).unwrap();

        for item in scan_sorted(root) {
            let preview = FileHandler::get_file_preview(&item.path, 10).unwrap();
            assert_eq!(
                preview == "[BINARY FILE]",
                item.is_binary,
                "{:?}",
                item.path
            );
        }
    }
}
