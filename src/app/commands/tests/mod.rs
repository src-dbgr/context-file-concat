//! Tests of the command handlers. The harness is here; the tests are grouped by topic
//! in the files beside it.

use super::*;
use crate::app::file_dialog::DialogService;
use crate::app::state::AppState;
use crate::app::view_model::UiState;
use crate::config::ConfigStore;
use crate::core::FileItem;
use crate::utils::test_helpers::setup_test_logging;
use serde_json::json;
use std::fs as std_fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::{tempdir, TempDir};
use tokio::sync::mpsc;

mod configuration;
mod directory;
mod filters;
mod generation;
mod ignore_path;
mod preview;
mod tree;

/// The payload of a command, read the way a message of the UI is: as the type
/// the command carries. A test of a handler starts where the handler does, with
/// a payload that fits; what does not fit is refused before (see `app::ipc`).
fn typed<T: serde::de::DeserializeOwned>(payload: serde_json::Value) -> T {
    serde_json::from_value(payload).expect("the payload fits its command")
}

// A mock EventProxy for capturing events sent to the UI.
#[derive(Clone)]
struct TestEventProxy {
    sender: mpsc::UnboundedSender<UserEvent>,
}

impl EventProxy for TestEventProxy {
    fn send_event(&self, event: UserEvent) {
        self.sender.send(event).expect("Test receiver dropped");
    }
}

// A mock DialogService to simulate user interaction with file dialogs.
#[derive(Default)]
struct MockDialogService {
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
    fn set_pick_folder(&self, path: Option<PathBuf>) {
        *self.picked_folder.lock().unwrap() = path;
    }

    fn set_pick_file(&self, path: Option<PathBuf>) {
        *self.picked_file.lock().unwrap() = path;
    }

    fn set_save_file(&self, path: Option<PathBuf>) {
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

struct TestHarness {
    state: SharedState,
    proxy: TestEventProxy,
    event_rx: mpsc::UnboundedReceiver<UserEvent>,
    dialog: Arc<MockDialogService>,
    _temp_dir: TempDir,
    _config_dir: Option<TempDir>,
    root_path: PathBuf,
}

impl TestHarness {
    fn new() -> Self {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let root_path = temp_dir.path().to_path_buf();
        let (tx, rx) = mpsc::unbounded_channel();
        let proxy = TestEventProxy { sender: tx };
        let dialog = Arc::new(MockDialogService::default());

        let mut state = AppState::default();
        state.config = AppConfig::default();
        state.current_path = root_path.to_string_lossy().to_string();

        Self {
            state: SharedState::new(state),
            proxy,
            event_rx: rx,
            dialog,
            _temp_dir: temp_dir,
            _config_dir: None,
            root_path,
        }
    }

    /// A harness whose configuration is kept in a file (in a directory of its own
    /// next to the project), for tests of what ends up on disk.
    fn with_config_file() -> (Self, ConfigStore) {
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

    fn create_file(&self, relative_path: &str, content: &str) -> PathBuf {
        let path = self.root_path.join(relative_path);
        if let Some(parent) = path.parent() {
            std_fs::create_dir_all(parent).unwrap();
        }
        std_fs::write(&path, content).unwrap();
        path
    }

    fn create_dir(&self, relative_path: &str) -> PathBuf {
        let path = self.root_path.join(relative_path);
        std_fs::create_dir_all(&path).unwrap();
        path
    }

    fn set_initial_files(&self, paths: &[&str]) {
        let mut state = self.state.lock();
        let mut items = Vec::new();
        for p_str in paths {
            let path = self.root_path.join(p_str);
            items.push(file_item(path.clone(), path.is_dir()));
        }
        state.set_full_file_list(items.clone());
        state.set_filtered_file_list(items);
    }

    /// The number of state updates among the events sent so far; all pending events
    /// are consumed, and any other event fails the test.
    #[track_caller]
    fn state_updates_sent(&mut self) -> usize {
        std::iter::from_fn(|| self.event_rx.try_recv().ok())
            .inspect(|event| {
                assert!(
                    matches!(event, UserEvent::StateUpdate(_)),
                    "Only state updates expected, got {event:?}"
                )
            })
            .count()
    }

    /// The most recent state update among the events sent so far, without waiting
    /// for more; all pending events are consumed. This is the way to look at the
    /// outcome of a command that has run to completion. For work a command has
    /// handed to a background task use [`Self::wait_for_state`].
    fn last_state_update(&mut self) -> Option<Box<UiState>> {
        std::iter::from_fn(|| self.event_rx.try_recv().ok())
            .filter_map(|event| match event {
                UserEvent::StateUpdate(ui_state) => Some(ui_state.into_ui_state()),
                _ => None,
            })
            .last()
    }

    /// The answer to a `saveFile` command, which has to be the next event.
    async fn next_save_result(&mut self) -> SaveResult {
        match self.get_next_event().await {
            Some(UserEvent::SaveResult(result)) => result,
            other => panic!("Expected the result of a save, got {other:?}"),
        }
    }

    /// Waits for the next event; `None` if none arrives (which fails the test that
    /// expected one, so the waiting time does not matter).
    async fn get_next_event(&mut self) -> Option<UserEvent> {
        tokio::time::timeout(EVENT_TIMEOUT, self.event_rx.recv())
            .await
            .ok()
            .flatten()
    }

    /// Asserts that no event is sent within `window`. Background tasks get the
    /// chance to run during that time, so this also covers commands that might
    /// have (wrongly) started one.
    #[track_caller]
    fn expect_no_event_within<'a>(
        &'a mut self,
        window: std::time::Duration,
        context: &'a str,
    ) -> impl std::future::Future<Output = ()> + 'a {
        let caller = std::panic::Location::caller();
        async move {
            if let Ok(event) = tokio::time::timeout(window, self.event_rx.recv()).await {
                panic!("{context} (at {caller}), got {event:?}");
            }
        }
    }

    /// Waits until a background task sends a state update that satisfies
    /// `predicate` and returns it; earlier events are consumed.
    async fn wait_for_state(
        &mut self,
        mut predicate: impl FnMut(&UiState) -> bool,
    ) -> Box<UiState> {
        loop {
            match self.get_next_event().await {
                Some(UserEvent::StateUpdate(ui_state)) if predicate(&ui_state) => {
                    return ui_state.into_ui_state()
                }
                Some(_) => {}
                None => panic!("Timed out waiting for the expected state update"),
            }
        }
    }

    /// Waits for a scan to run and end: returns the first state update without a
    /// running scan that follows one with a running scan. Updates that were sent
    /// before the scan was announced do not count, so this does not return for a
    /// command that never started a scan (it returns `None` after the timeout).
    async fn wait_for_scan_completion(&mut self) -> Option<Box<UiState>> {
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

/// Upper bound for waiting on something a test expects to happen. It is only ever
/// reached by a failing test, so it does not slow the suite down.
const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Waits until the configuration file of `store` satisfies `predicate` and returns
/// its content. Commands save in the background, so the file follows the state
/// with a delay.
async fn config_on_disk(store: &ConfigStore, predicate: impl Fn(&AppConfig) -> bool) -> AppConfig {
    let path = store.path().expect("the store is bound to a file");
    tokio::time::timeout(EVENT_TIMEOUT, async {
        loop {
            let saved = config::settings::load_config(path).unwrap();
            if predicate(&saved) {
                return saved;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the expected configuration was not saved")
}

/// How long a test listens for events that must not be sent.
const QUIET_PERIOD: std::time::Duration = std::time::Duration::from_millis(50);

fn file_item(path: PathBuf, is_dir: bool) -> FileItem {
    FileItem {
        path,
        is_directory: is_dir,
        is_binary: false,
        size: if is_dir { 0 } else { 123 },
        depth: 1,
        parent: None,
    }
}

// --- Config intent helpers ---

async fn add_pattern(harness: &TestHarness, pattern: &str) {
    add_ignore_pattern(
        typed(json!({ "pattern": pattern })),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
}

async fn remove_pattern(harness: &TestHarness, pattern: &str) {
    remove_ignore_pattern(
        typed(json!({ "pattern": pattern })),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
}

async fn set_option(harness: &TestHarness, option: ConfigOption) {
    set_config_option(
        typed(serde_json::to_value(option).unwrap()),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
}
