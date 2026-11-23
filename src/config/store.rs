//! Ordered persistence of the configuration: the one way the configuration file is
//! written while the application runs.
//!
//! The configuration is edited in memory (under the lock of the application state) and
//! written to disk outside of that lock, mostly in the background. Without further
//! measures two writes could overtake each other and leave the older content on disk.
//! A [`ConfigStore`] rules that out:
//!
//! * Whoever wants the configuration saved asks the store for a [`ConfigSnapshot`]
//!   *while holding the state lock*. Snapshots are numbered in the order they are
//!   taken, which is therefore the order of the edits.
//! * Writing a snapshot ([`ConfigSnapshot::save`]) may happen any time later, on any
//!   thread. Writes are serialized, and a snapshot is skipped once a newer one has been
//!   written.
//! * The last save of a session ([`ConfigStore::close`]) writes the most recent
//!   snapshot if it is not on disk yet and closes the store. A save that is still
//!   queued in the background can neither overtake it nor run after it.

use super::{settings, AppConfig};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

/// The revision that marks a closed store: no snapshot is newer than it.
const CLOSED: u64 = u64::MAX;

/// Hands out numbered snapshots of the configuration and writes them in order.
///
/// Cloning is cheap; all clones are the same store.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    /// The file the configuration is written to; `None` for a detached store.
    path: Option<PathBuf>,
    /// Locked only for a moment, so taking a snapshot is cheap for the holder of the
    /// state lock. Never held during file IO.
    issued: Mutex<Issued>,
    /// The revision of the newest snapshot written so far. Held while the file is
    /// written, which serializes the writes. Where both locks are needed, this one is
    /// taken first.
    written: Mutex<u64>,
}

#[derive(Debug)]
struct Issued {
    /// The number of snapshots taken so far.
    revision: u64,
    /// The configuration of the most recent snapshot (initially: as it was loaded).
    latest: Arc<AppConfig>,
}

impl ConfigStore {
    /// Opens the configuration file at `path`: reads it (see
    /// [`settings::load_config`] for how a missing or damaged file is dealt with) and
    /// returns the store together with the configuration it holds.
    pub fn open(path: PathBuf) -> (Self, AppConfig) {
        let config = settings::load_config(&path).unwrap_or_else(|e| {
            tracing::warn!("Failed to load the config from {path:?}: {e}. Using defaults.");
            AppConfig::default()
        });
        (Self::new(Some(path), config.clone()), config)
    }

    /// A store that is not bound to a file: snapshots are numbered and remembered, but
    /// nothing is written. Used when no configuration directory can be determined, and
    /// by a default [`AppState`](crate::app::state::AppState).
    pub fn detached() -> Self {
        Self::new(None, AppConfig::default())
    }

    fn new(path: Option<PathBuf>, config: AppConfig) -> Self {
        Self {
            shared: Arc::new(Shared {
                path,
                issued: Mutex::new(Issued {
                    revision: 0,
                    latest: Arc::new(config),
                }),
                written: Mutex::new(0),
            }),
        }
    }

    /// The file this store writes to, if any.
    #[cfg(test)]
    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        self.shared.path.as_deref()
    }

    /// Captures `config` for saving. To be called while holding the lock under which
    /// the configuration is edited, right after the edit: the order of the snapshots
    /// is what decides which one ends up on disk.
    pub fn snapshot(&self, config: &AppConfig) -> ConfigSnapshot {
        let config = Arc::new(config.clone());
        let mut issued = lock(&self.shared.issued);
        issued.revision += 1;
        issued.latest = config.clone();
        ConfigSnapshot {
            config,
            revision: issued.revision,
            store: self.clone(),
        }
    }

    /// The configuration of the most recent snapshot, or the configuration as it was
    /// loaded if none has been taken yet.
    #[cfg(test)]
    pub(crate) fn latest(&self) -> AppConfig {
        AppConfig::clone(&lock(&self.shared.issued).latest)
    }

    /// The last save of a session: writes the most recent snapshot unless it is on
    /// disk already, and closes the store.
    ///
    /// "Most recent" is the newest snapshot taken, whether its own save has had its
    /// turn or not. Every change of the configuration takes one (under the state
    /// lock), so this is the configuration as the application holds it, and nothing
    /// has to be asked of the application state. A write that is under way is waited
    /// for. Nothing is written to the file after this call, by this store or its
    /// clones: a save that was queued before is part of what is written here, and a
    /// snapshot that is taken afterwards is skipped.
    ///
    /// If the write fails, the store stays open and unchanged.
    pub fn close(&self) -> Result<()> {
        let shared = &self.shared;
        let mut written = lock(&shared.written);
        if *written == CLOSED {
            tracing::debug!("The config store is closed already.");
            return Ok(());
        }
        let (revision, latest) = {
            let issued = lock(&shared.issued);
            (issued.revision, issued.latest.clone())
        };
        if revision > *written {
            if let Some(path) = &shared.path {
                settings::save_config(&latest, path)?;
            }
        }
        *written = CLOSED;
        Ok(())
    }
}

/// The configuration as it was at one moment, to be written to disk outside the state
/// lock. See the [module documentation](self).
#[derive(Debug)]
pub struct ConfigSnapshot {
    config: Arc<AppConfig>,
    revision: u64,
    store: ConfigStore,
}

impl ConfigSnapshot {
    /// The captured configuration.
    #[cfg(test)]
    pub(crate) fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Writes the snapshot to the configuration file, unless a newer snapshot has been
    /// written already or the store has been closed. Blocks on file IO: not to be
    /// called on the ordered command lane or while holding the state lock.
    pub fn save(self) -> Result<()> {
        let shared = &self.store.shared;
        let mut written = lock(&shared.written);
        if self.revision <= *written {
            tracing::debug!("Skipping the save of a superseded config snapshot.");
            return Ok(());
        }
        if let Some(path) = &shared.path {
            settings::save_config(&self.config, path)?;
        }
        // A failed write records nothing, so the next snapshot is written regardless.
        *written = self.revision;
        Ok(())
    }
}

/// Locks one of the store's own mutexes. They guard plain values that are valid at any
/// time, so a lock poisoned by a panicking writer is simply taken over.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn named(output_filename: &str) -> AppConfig {
        AppConfig {
            output_filename: output_filename.to_string(),
            ..AppConfig::default()
        }
    }

    /// A store writing to a file in a fresh temporary directory.
    fn store_on_disk() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = ConfigStore::open(dir.path().join("config.json"));
        (dir, store)
    }

    fn name_on_disk(store: &ConfigStore) -> String {
        settings::load_config(store.path().unwrap())
            .unwrap()
            .output_filename
    }

    #[test]
    fn open_reads_the_file_and_creates_it_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");

        let (store, config) = ConfigStore::open(path.clone());
        assert_eq!(config, AppConfig::default());
        assert!(path.is_file(), "a missing config file is created");
        assert_eq!(store.latest(), config);

        settings::save_config(&named("stored.txt"), &path).unwrap();
        let (store, config) = ConfigStore::open(path);
        assert_eq!(config.output_filename, "stored.txt");
        assert_eq!(store.latest(), config);
    }

    #[test]
    fn snapshots_are_written_in_the_order_they_were_taken() {
        let (_dir, store) = store_on_disk();
        let older = store.snapshot(&named("older.txt"));
        let newer = store.snapshot(&named("newer.txt"));

        older.save().unwrap();
        assert_eq!(name_on_disk(&store), "older.txt");
        newer.save().unwrap();
        assert_eq!(name_on_disk(&store), "newer.txt");
    }

    /// The race this type exists for: the save of the older snapshot gets to run after
    /// the save of the newer one.
    #[test]
    fn an_overtaken_snapshot_is_not_written() {
        let (_dir, store) = store_on_disk();
        let older = store.snapshot(&named("older.txt"));
        let newer = store.snapshot(&named("newer.txt"));

        newer.save().unwrap();
        older.save().unwrap();

        assert_eq!(name_on_disk(&store), "newer.txt");
    }

    #[test]
    fn a_failed_write_does_not_block_the_next_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = ConfigStore::open(dir.path().join("config.json"));
        let path = store.path().unwrap().to_path_buf();

        // A directory in place of the file makes the write fail.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(store.snapshot(&named("failing.txt")).save().is_err());

        fs::remove_dir(&path).unwrap();
        store.snapshot(&named("next.txt")).save().unwrap();
        assert_eq!(name_on_disk(&store), "next.txt");
    }

    fn on_disk(store: &ConfigStore) -> AppConfig {
        settings::load_config(store.path().unwrap()).unwrap()
    }

    #[test]
    fn close_writes_the_newest_snapshot_that_is_not_on_disk_yet() {
        let (_dir, store) = store_on_disk();
        store.snapshot(&named("written.txt")).save().unwrap();
        let _unsaved = store.snapshot(&named("newest.txt"));

        store.close().unwrap();

        let saved = on_disk(&store);
        assert_eq!(saved.output_filename, "newest.txt");
        assert_eq!(store.latest(), saved, "the store knows what it wrote");
    }

    /// Nothing to save: the file is left exactly as it is, also one that holds what
    /// this version does not write (here: fields of an older version).
    #[test]
    fn close_without_an_unsaved_snapshot_leaves_the_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let stored = r#"{ "output_filename": "stored.txt", "window_size": [800.0, 600.0] }"#;
        fs::write(&path, stored).unwrap();
        let (store, _) = ConfigStore::open(path.clone());

        store.close().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), stored);

        // The same once every snapshot has been written.
        let (store, _) = ConfigStore::open(path.clone());
        store.snapshot(&named("saved.txt")).save().unwrap();
        let saved = fs::read(&path).unwrap();
        fs::write(&path, stored).unwrap();
        store.close().unwrap();
        assert_ne!(saved, stored.as_bytes());
        assert_eq!(fs::read_to_string(&path).unwrap(), stored);
    }

    #[test]
    fn nothing_is_written_after_close() {
        let (_dir, store) = store_on_disk();
        let queued = store.snapshot(&named("queued.txt"));
        let last = store.snapshot(&named("last.txt"));

        store.close().unwrap();
        let closed = fs::read(store.path().unwrap()).unwrap();

        // Saves that were queued before, in either order, one that is requested
        // afterwards (also through a clone), and a second close.
        last.save().unwrap();
        queued.save().unwrap();
        store.snapshot(&named("too-late.txt")).save().unwrap();
        store.clone().snapshot(&named("clone.txt")).save().unwrap();
        store.close().unwrap();

        assert_eq!(fs::read(store.path().unwrap()).unwrap(), closed);
        assert_eq!(on_disk(&store).output_filename, "last.txt");
    }

    #[test]
    fn close_waits_for_a_write_that_is_under_way() {
        let (_dir, store) = store_on_disk();
        let _last = store.snapshot(&named("last.txt"));

        // A writer holds the store exactly as `save` does while it writes the file.
        let writing = lock(&store.shared.written);
        let closing = {
            let store = store.clone();
            std::thread::spawn(move || store.close().unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !closing.is_finished(),
            "close must not write while another write is under way"
        );
        drop(writing);

        closing.join().unwrap();
        assert_eq!(name_on_disk(&store), "last.txt");
    }

    /// Taking a snapshot never waits for file IO, not even for the last write of a
    /// session: whoever takes one holds the state lock.
    #[test]
    fn a_snapshot_is_taken_without_waiting_for_a_write() {
        let (_dir, store) = store_on_disk();

        // A writer (a save or the close) holds the store while it writes the file.
        let writing = lock(&store.shared.written);
        let snapshot = store.snapshot(&named("meanwhile.txt"));
        drop(writing);

        assert_eq!(store.latest().output_filename, "meanwhile.txt");
        snapshot.save().unwrap();
        assert_eq!(name_on_disk(&store), "meanwhile.txt");
    }

    #[test]
    fn a_failed_close_leaves_the_store_open() {
        let (_dir, store) = store_on_disk();
        let path = store.path().unwrap().to_path_buf();
        let _unsaved = store.snapshot(&named("unsaved.txt"));

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(store.close().is_err());

        fs::remove_dir(&path).unwrap();
        store.snapshot(&named("next.txt")).save().unwrap();
        assert_eq!(name_on_disk(&store), "next.txt");
    }

    #[test]
    fn a_detached_store_can_be_closed() {
        let store = ConfigStore::detached();
        let _unsaved = store.snapshot(&named("final.txt"));
        store.close().unwrap();
        assert_eq!(store.latest().output_filename, "final.txt");
    }

    #[test]
    fn latest_is_the_most_recent_snapshot_even_if_it_has_not_been_written() {
        let (_dir, store) = store_on_disk();
        assert_eq!(store.latest(), AppConfig::default());

        let _unsaved = store.snapshot(&named("unsaved.txt"));

        assert_eq!(store.latest().output_filename, "unsaved.txt");
        assert_eq!(name_on_disk(&store), AppConfig::default().output_filename);
    }

    #[test]
    fn a_detached_store_numbers_snapshots_but_writes_nothing() {
        let store = ConfigStore::detached();
        assert_eq!(store.path(), None);

        let snapshot = store.snapshot(&named("kept-in-memory.txt"));
        assert_eq!(snapshot.config().output_filename, "kept-in-memory.txt");
        snapshot.save().unwrap();

        assert_eq!(store.latest().output_filename, "kept-in-memory.txt");
    }
}
