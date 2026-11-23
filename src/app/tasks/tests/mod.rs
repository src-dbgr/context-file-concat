//! Tests of the background tasks. The harness and the test doubles that several
//! topics share are here; the tests are grouped by topic in the files beside it.

use super::*;
use crate::app::view_model::{generate_ui_state, UiState};
use crate::config::AppConfig;
use crate::core::MAX_FILE_SIZE;
#[cfg(unix)] // Only the permission-based tests need it.
use crate::utils::test_helpers::running_as_root;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tempfile::{tempdir, TempDir};
use tokio::sync::oneshot;

mod content_search;
mod deep_scan;
mod generation;
mod lazy_load;
mod preview;
mod scan;

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
                UserEvent::StateUpdate(state) => Some(*state.into_ui_state()),
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
                    return *state.into_ui_state();
                }
            }
        }
    }
}

/// Upper bound for waiting on something a test expects to happen. It is only ever
/// reached by a failing test, so it does not slow the suite down.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

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
