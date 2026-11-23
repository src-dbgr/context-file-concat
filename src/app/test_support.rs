//! What the tests of the application layer are built from.
//!
//! Small parts that each do one thing, and a harness that puts them together:
//!
//! * [`event_channel`]: a mock [`EventProxy`] and the [`EventStream`] of what it was
//!   sent, with the ways to look at it (what has been sent, what arrives next, that
//!   nothing arrives);
//! * [`TempProject`]: a directory on disk to scan, read and write in;
//! * [`MockDialogService`]: dialogs that answer what a test tells them to;
//! * [`state_in`]: an application state with a directory loaded;
//! * [`TestHarness`]: all of the above around one shared state, for the tests of the
//!   tasks and of the command handlers.
//!
//! A test that needs something else (the dispatcher, a dialog that blocks, a proxy
//! that records) takes the parts it can use and adds its own, see `app/tests/mod.rs`.
//!
//! The integration tests (`tests/integration_tests.rs`) are another crate and see the
//! public API only; they keep a small harness of their own.

use crate::app::events::{SaveResult, UserEvent};
use crate::app::file_dialog::DialogService;
use crate::app::proxy::EventProxy;
use crate::app::shared_state::SharedState;
use crate::app::state::AppState;
use crate::app::tasks::GenerationPlan;
use crate::app::view_model::UiState;
use crate::config::{settings, AppConfig, ConfigStore};
use crate::core::FileItem;
use std::fs;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::{tempdir, TempDir};
use tokio::sync::mpsc;

/// Upper bound for waiting on something a test expects to happen. It is only ever
/// reached by a failing test, so it does not slow the suite down.
pub(crate) const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a test listens for events that must not be sent.
pub(crate) const QUIET_PERIOD: Duration = Duration::from_millis(50);

// --- Events ---------------------------------------------------------------------------

/// A mock [`EventProxy`] for capturing the events sent to the UI.
#[derive(Clone)]
pub(crate) struct TestEventProxy {
    sender: mpsc::UnboundedSender<UserEvent>,
    /// Whether an event that nobody receives any more fails the sender.
    strict: bool,
}

impl TestEventProxy {
    /// A proxy that sends to `sender` and panics if the receiver is gone.
    pub(crate) fn new(sender: mpsc::UnboundedSender<UserEvent>) -> Self {
        Self {
            sender,
            strict: true,
        }
    }

    /// A proxy that sends to `sender` as long as somebody listens: for tests whose
    /// background work may still send when the test has finished.
    pub(crate) fn lenient(sender: mpsc::UnboundedSender<UserEvent>) -> Self {
        Self {
            sender,
            strict: false,
        }
    }
}

impl EventProxy for TestEventProxy {
    fn send_event(&self, event: UserEvent) {
        let sent = self.sender.send(event);
        if self.strict {
            sent.expect("Test receiver dropped");
        }
    }
}

/// A proxy and the stream of the events it is sent.
pub(crate) fn event_channel() -> (TestEventProxy, EventStream) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (TestEventProxy::new(sender), EventStream(receiver))
}

/// [`event_channel`] with a proxy that does not mind a receiver that is gone (see
/// [`TestEventProxy::lenient`]).
pub(crate) fn lenient_event_channel() -> (TestEventProxy, EventStream) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (TestEventProxy::lenient(sender), EventStream(receiver))
}

/// The events a [`TestEventProxy`] was sent. It is the receiver of the channel (and
/// can be used as such), with the questions tests ask about it.
pub(crate) struct EventStream(mpsc::UnboundedReceiver<UserEvent>);

impl Deref for EventStream {
    type Target = mpsc::UnboundedReceiver<UserEvent>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for EventStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl EventStream {
    /// The events sent so far, without waiting for more. This is the way to look at
    /// the outcome of code that has run to completion (an awaited task or a
    /// synchronous function): everything it sent is already here.
    pub(crate) fn drain(&mut self) -> Vec<UserEvent> {
        std::iter::from_fn(|| self.0.try_recv().ok()).collect()
    }

    /// Waits for the next event; `None` if none arrives (which fails the test that
    /// expected one, so the waiting time does not matter).
    pub(crate) async fn next(&mut self) -> Option<UserEvent> {
        tokio::time::timeout(EVENT_TIMEOUT, self.0.recv())
            .await
            .ok()
            .flatten()
    }

    /// The most recent state update among the events sent so far, without waiting
    /// for more; all pending events are consumed.
    pub(crate) fn last_state_update(&mut self) -> Option<Box<UiState>> {
        self.drain()
            .into_iter()
            .filter_map(|event| match event {
                UserEvent::StateUpdate(state) => Some(state.into_ui_state()),
                _ => None,
            })
            .next_back()
    }

    /// Waits until a state update that satisfies `predicate` is sent and returns it;
    /// earlier events are consumed.
    pub(crate) async fn wait_for_state(
        &mut self,
        mut predicate: impl FnMut(&UiState) -> bool,
    ) -> Box<UiState> {
        loop {
            match self.next().await {
                Some(UserEvent::StateUpdate(ui_state)) if predicate(&ui_state) => {
                    return ui_state.into_ui_state()
                }
                Some(_) => {}
                None => panic!("Timed out waiting for the expected state update"),
            }
        }
    }

    /// Asserts that no event is sent within `window`. Background tasks get the
    /// chance to run during that time, so this also covers code that might have
    /// (wrongly) started one.
    #[track_caller]
    pub(crate) fn expect_no_event_within<'a>(
        &'a mut self,
        window: Duration,
        context: &'a str,
    ) -> impl Future<Output = ()> + 'a {
        let caller = std::panic::Location::caller();
        async move {
            if let Ok(event) = tokio::time::timeout(window, self.0.recv()).await {
                panic!("{context} (at {caller}), got {event:?}");
            }
        }
    }
}

// --- A project on disk ----------------------------------------------------------------

/// A temporary directory that plays the project; it is removed with this value.
pub(crate) struct TempProject {
    root: PathBuf,
    _dir: TempDir,
}

impl TempProject {
    pub(crate) fn new() -> Self {
        let dir = tempdir().expect("Failed to create temp dir");
        Self {
            root: dir.path().to_path_buf(),
            _dir: dir,
        }
    }

    /// The directory, as the path the temporary directory was created with.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Writes a file below the root, with the directories it lies in.
    pub(crate) fn create_file(&self, relative_path: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }

    /// Creates a directory below the root, with the directories it lies in.
    pub(crate) fn create_dir(&self, relative_path: &str) -> PathBuf {
        let path = self.root.join(relative_path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

// --- State ----------------------------------------------------------------------------

/// A default state (bound to no configuration file) in which `root` is the loaded
/// directory; nothing is listed yet.
pub(crate) fn state_in(root: &Path) -> AppState {
    let mut state = AppState::default();
    *state.config_mut() = AppConfig::default();
    state.current_path = root.to_string_lossy().to_string();
    state
}

/// An entry of a file list as [`TestHarness::set_initial_files`] lists it.
pub(crate) fn file_item(path: PathBuf, is_dir: bool) -> FileItem {
    FileItem {
        path,
        is_directory: is_dir,
        is_binary: false,
        size: if is_dir { 0 } else { 123 },
        depth: 1,
        parent: None,
    }
}

/// Waits until the configuration file of `store` satisfies `predicate` and returns
/// its content. Commands save in the background, so the file follows the state
/// with a delay.
pub(crate) async fn config_on_disk(
    store: &ConfigStore,
    predicate: impl Fn(&AppConfig) -> bool,
) -> AppConfig {
    let path = store.path().expect("the store is bound to a file");
    tokio::time::timeout(EVENT_TIMEOUT, async {
        loop {
            let saved = settings::load_config(path).unwrap();
            if predicate(&saved) {
                return saved;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the expected configuration was not saved")
}

// --- Dialogs --------------------------------------------------------------------------

/// A mock [`DialogService`] to simulate user interaction with file dialogs: every
/// dialog is answered at once, with the path the test has set (none by default, which
/// is the user cancelling).
#[derive(Default)]
pub(crate) struct MockDialogService {
    picked_folder: Mutex<Option<PathBuf>>,
    picked_file: Mutex<Option<PathBuf>>,
    saved_file: Mutex<Option<PathBuf>>,
}

impl Clone for MockDialogService {
    fn clone(&self) -> Self {
        MockDialogService {
            picked_folder: Mutex::new(self.picked_folder.lock().unwrap().clone()),
            picked_file: Mutex::new(self.picked_file.lock().unwrap().clone()),
            saved_file: Mutex::new(self.saved_file.lock().unwrap().clone()),
        }
    }
}

impl MockDialogService {
    pub(crate) fn set_pick_folder(&self, path: Option<PathBuf>) {
        *self.picked_folder.lock().unwrap() = path;
    }

    pub(crate) fn set_pick_file(&self, path: Option<PathBuf>) {
        *self.picked_file.lock().unwrap() = path;
    }

    pub(crate) fn set_save_file(&self, path: Option<PathBuf>) {
        *self.saved_file.lock().unwrap() = path;
    }
}

impl DialogService for MockDialogService {
    fn pick_directory(&self) -> Option<PathBuf> {
        self.picked_folder.lock().unwrap().clone()
    }
    fn pick_config_to_import(&self) -> Option<PathBuf> {
        self.picked_file.lock().unwrap().clone()
    }
    fn export_config_path(&self) -> Option<PathBuf> {
        self.saved_file.lock().unwrap().clone()
    }
    fn save_output_file_path(&self, _config: &AppConfig) -> Option<PathBuf> {
        self.saved_file.lock().unwrap().clone()
    }
}

// --- The harness ----------------------------------------------------------------------

/// A shared state with a temporary project as the loaded directory, a proxy whose
/// events can be looked at, and dialogs that answer what the test tells them to.
pub(crate) struct TestHarness {
    pub(crate) state: SharedState,
    pub(crate) proxy: TestEventProxy,
    pub(crate) event_rx: EventStream,
    pub(crate) dialog: Arc<MockDialogService>,
    /// The root of the project (see [`TempProject::root`]).
    pub(crate) root_path: PathBuf,
    project: TempProject,
    _config_dir: Option<TempDir>,
}

impl TestHarness {
    pub(crate) fn new() -> Self {
        let project = TempProject::new();
        let (proxy, event_rx) = event_channel();
        Self {
            state: SharedState::new(state_in(project.root())),
            proxy,
            event_rx,
            dialog: Arc::new(MockDialogService::default()),
            root_path: project.root().to_path_buf(),
            project,
            _config_dir: None,
        }
    }

    /// A harness whose configuration is kept in a file (in a directory of its own
    /// next to the project), for tests of what ends up on disk.
    pub(crate) fn with_config_file() -> (Self, ConfigStore) {
        let harness = Self::new();
        let config_dir = tempdir().expect("Failed to create the config dir");
        let (store, config) = ConfigStore::open(config_dir.path().join("config.json"));
        {
            let mut state = harness.state.lock();
            let mut with_file = AppState::new(config, store.clone());
            with_file.current_path = state.current_path.clone();
            *state = with_file;
        }
        (
            Self {
                _config_dir: Some(config_dir),
                ..harness
            },
            store,
        )
    }

    // --- The project and what is listed of it ---

    pub(crate) fn create_file(&self, relative_path: &str, content: &str) -> PathBuf {
        self.project.create_file(relative_path, content)
    }

    pub(crate) fn create_dir(&self, relative_path: &str) -> PathBuf {
        self.project.create_dir(relative_path)
    }

    /// Lists `paths` (relative to the root; a directory is one that exists on disk)
    /// as the file list, unfiltered.
    pub(crate) fn set_initial_files(&self, paths: &[&str]) {
        let mut state = self.state.lock();
        let mut items = Vec::new();
        for p_str in paths {
            let path = self.root_path.join(p_str);
            items.push(file_item(path.clone(), path.is_dir()));
        }
        state.set_full_file_list(items.clone());
        state.set_filtered_file_list(items);
    }

    /// The plan of a generation of what is selected right now, as the command
    /// that starts a generation makes it.
    pub(crate) fn plan(&self) -> GenerationPlan {
        GenerationPlan::of(&self.state.lock())
    }

    // --- What has been sent (the code under test has run to completion) ---

    /// See [`EventStream::drain`].
    pub(crate) fn drain_events(&mut self) -> Vec<UserEvent> {
        self.event_rx.drain()
    }

    /// The most recent state update among the events sent so far, without waiting
    /// for more; all pending events are consumed. This is the way to look at the
    /// outcome of code that has run to completion. For work that was handed to a
    /// background task use [`Self::wait_for_state`].
    pub(crate) fn last_state_update(&mut self) -> Option<Box<UiState>> {
        self.event_rx.last_state_update()
    }

    /// The number of state updates among the events sent so far; all pending events
    /// are consumed, and any other event fails the test.
    #[track_caller]
    pub(crate) fn state_updates_sent(&mut self) -> usize {
        self.drain_events()
            .iter()
            .inspect(|event| {
                assert!(
                    matches!(event, UserEvent::StateUpdate(_)),
                    "Only state updates expected, got {event:?}"
                )
            })
            .count()
    }

    /// Asserts that nothing has been sent since the events were last consumed.
    /// Only conclusive once the code under test has run to completion.
    #[track_caller]
    pub(crate) fn assert_no_events(&mut self, context: &str) {
        let events = self.drain_events();
        assert!(events.is_empty(), "{context}, got {events:?}");
    }

    // --- What a background task sends ---

    /// Waits for the next event; `None` if none arrives (see [`EventStream::next`]).
    pub(crate) async fn get_next_event(&mut self) -> Option<UserEvent> {
        self.event_rx.next().await
    }

    /// Waits for the next event of a task that is still running in the background,
    /// which has to arrive.
    pub(crate) async fn next_event(&mut self) -> UserEvent {
        tokio::time::timeout(EVENT_TIMEOUT, self.event_rx.recv())
            .await
            .expect("Timed out waiting for an event")
            .expect("Event channel closed")
    }

    /// The answer to a `saveFile` command, which has to be the next event.
    pub(crate) async fn next_save_result(&mut self) -> SaveResult {
        match self.get_next_event().await {
            Some(UserEvent::SaveResult(result)) => result,
            other => panic!("Expected the result of a save, got {other:?}"),
        }
    }

    /// See [`EventStream::expect_no_event_within`].
    #[track_caller]
    pub(crate) fn expect_no_event_within<'a>(
        &'a mut self,
        window: Duration,
        context: &'a str,
    ) -> impl Future<Output = ()> + 'a {
        self.event_rx.expect_no_event_within(window, context)
    }

    /// Waits until a background task sends a state update that satisfies
    /// `predicate` and returns it; earlier events are consumed.
    pub(crate) async fn wait_for_state(
        &mut self,
        predicate: impl FnMut(&UiState) -> bool,
    ) -> Box<UiState> {
        self.event_rx.wait_for_state(predicate).await
    }

    /// Waits for a scan to run and end: returns the first state update without a
    /// running scan that follows one with a running scan. Updates that were sent
    /// before the scan was announced do not count, so this does not return for a
    /// command that never started a scan (it returns `None` after the timeout).
    pub(crate) async fn wait_for_scan_completion(&mut self) -> Option<Box<UiState>> {
        let mut scan_seen = false;
        loop {
            match self.get_next_event().await? {
                UserEvent::StateUpdate(ui_state) if ui_state.is_scanning => scan_seen = true,
                UserEvent::StateUpdate(ui_state) if scan_seen => {
                    return Some(ui_state.into_ui_state())
                }
                _ => {}
            }
        }
    }
}
