//! Window geometry tracking and persistence.
//!
//! The UI thread reports every resize/move here. The tracker is a small value owned by
//! the event loop, so following the window never waits for the (potentially long-held)
//! [`AppState`] mutex. The geometry only meets the configuration when it is persisted.
//!
//! All values are *logical* units: the window is restored with logical size/position,
//! and mixing them with the physical pixels the windowing system reports would make the
//! window grow on every restart on HiDPI displays.
//!
//! Versions up to 0.3.1 did mix them: they stored physical pixels. The configuration
//! therefore names the unit of its geometry ([`GeometryUnits`]); a geometry without
//! that mark is physical and is converted once, when it is read
//! ([`WindowGeometry::from_config`]). Everything written from here is logical and
//! marked as such.

use crate::app::shared_state::SharedState;
use crate::app::state::AppState;
use crate::config::{AppConfig, ConfigStore, GeometryUnits};
use anyhow::Result;
use std::sync::MutexGuard;
use std::time::{Duration, Instant};

/// Smallest inner size the window may have, in logical units.
pub const MIN_WINDOW_SIZE: (f64, f64) = (900.0, 600.0);

/// Windows parks minimized windows at (-32000, -32000); nothing real lives out there.
const PARKED_COORDINATE: f64 = -32000.0;

/// Tolerance for sizes that land just below the minimum after physical -> logical
/// rounding at fractional scale factors.
const SIZE_EPSILON: f64 = 1.0;

/// A restored window must show at least this much of itself on some monitor, so the
/// user can still grab it.
const MIN_VISIBLE: (f64, f64) = (120.0, 60.0);

/// Offset from a monitor's top-left corner used when the stored position is unusable.
const FALLBACK_OFFSET: (f64, f64) = (100.0, 100.0);

/// How long persisting waits for the application state before bypassing it.
const STATE_LOCK_BUDGET: Duration = Duration::from_millis(250);
const STATE_LOCK_POLL: Duration = Duration::from_millis(5);

/// How often the last save of a session is repeated because a background save got in
/// its way, see [`persist`]. Each repetition needs another configuration change to
/// be saved in the split second before; one is the realistic maximum.
const FINAL_SAVE_ATTEMPTS: usize = 8;

/// An axis-aligned rectangle in logical units (a monitor's area).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// Converts a rectangle given in physical pixels using its own scale factor.
    ///
    /// Returns `None` for an unusable scale factor or an empty rectangle.
    pub fn from_physical(x: f64, y: f64, width: f64, height: f64, scale: f64) -> Option<Self> {
        let (x, y) = to_logical((x, y), scale)?;
        let (width, height) = to_logical((width, height), scale)?;
        (width > 0.0 && height > 0.0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// Size of the overlap between `self` and a window at `position` with `size`.
    fn overlap(&self, position: (f64, f64), size: (f64, f64)) -> (f64, f64) {
        let width = (position.0 + size.0).min(self.x + self.width) - position.0.max(self.x);
        let height = (position.1 + size.1).min(self.y + self.height) - position.1.max(self.y);
        (width.max(0.0), height.max(0.0))
    }
}

/// Converts a pair of physical pixels to logical units.
///
/// Returns `None` if the scale factor or the result is not a usable number.
pub fn to_logical(physical: (f64, f64), scale: f64) -> Option<(f64, f64)> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let logical = (physical.0 / scale, physical.1 / scale);
    (logical.0.is_finite() && logical.1.is_finite()).then_some(logical)
}

/// Whether a logical inner size is worth remembering.
///
/// Rejects the `0x0` a minimized window reports on Windows and anything below the
/// minimum size the window can legitimately have.
pub fn is_persistable_size(size: (f64, f64)) -> bool {
    size.0.is_finite()
        && size.1.is_finite()
        && size.0 >= MIN_WINDOW_SIZE.0 - SIZE_EPSILON
        && size.1 >= MIN_WINDOW_SIZE.1 - SIZE_EPSILON
}

/// Whether an outer position given in physical pixels is worth remembering.
///
/// Rejects the parking spot of minimized windows on Windows. Negative coordinates as
/// such are fine: monitors left of / above the primary one have them.
pub fn is_persistable_position(physical: (f64, f64)) -> bool {
    physical.0.is_finite()
        && physical.1.is_finite()
        && physical.0 > PARKED_COORDINATE
        && physical.1 > PARKED_COORDINATE
}

/// Size and position of the main window in logical units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowGeometry {
    pub size: (f64, f64),
    pub position: (f64, f64),
}

impl WindowGeometry {
    /// Reads the stored geometry in logical units, replacing unusable values with the
    /// defaults.
    ///
    /// A geometry stored in physical pixels (a configuration written by a version that
    /// did not name the unit) is converted with `primary_scale`, the scale factor of
    /// the primary monitor: that is where such a window is most likely to have been,
    /// and the stored values do not say. Without the conversion the first start after
    /// an upgrade would open the window at twice its size on a 2x display.
    pub fn from_config(config: &AppConfig, primary_scale: f64) -> Self {
        let (size, position) = match config.geometry_units {
            GeometryUnits::Logical => (Some(config.window_size), Some(config.window_position)),
            GeometryUnits::Physical => (
                to_logical(config.window_size, primary_scale),
                to_logical(config.window_position, primary_scale),
            ),
        };
        let defaults = AppConfig::default();
        Self {
            size: size
                .filter(|size| is_persistable_size(*size))
                .unwrap_or(defaults.window_size),
            position: position
                .filter(|position| position.0.is_finite() && position.1.is_finite())
                .unwrap_or(defaults.window_position),
        }
    }

    /// Writes the geometry into a configuration, in logical units and marked as such.
    pub fn apply_to(&self, config: &mut AppConfig) {
        config.window_size = self.size;
        config.window_position = self.position;
        config.geometry_units = GeometryUnits::Logical;
    }

    /// Records a new inner size reported in physical pixels; degenerate sizes are ignored.
    pub fn record_size(&mut self, physical: (f64, f64), scale: f64) {
        if let Some(size) = to_logical(physical, scale).filter(|size| is_persistable_size(*size)) {
            self.size = size;
        }
    }

    /// Records a new outer position reported in physical pixels; the position of a
    /// minimized window is ignored.
    pub fn record_position(&mut self, physical: (f64, f64), scale: f64) {
        if !is_persistable_position(physical) {
            return;
        }
        if let Some(position) = to_logical(physical, scale) {
            self.position = position;
        }
    }

    /// Moves the window back onto a monitor if (almost) none of it would be visible,
    /// e.g. because the monitor it was last on is gone.
    ///
    /// `monitors` are the available monitors in logical units, the preferred fallback
    /// (primary) first. Without monitor information the geometry is returned unchanged.
    pub fn clamped_to_visible(self, monitors: &[Rect]) -> Self {
        let Some(fallback) = monitors.first() else {
            return self;
        };
        let reachable = monitors.iter().any(|monitor| {
            let (width, height) = monitor.overlap(self.position, self.size);
            // Small monitors cannot be asked for more than they have.
            width >= MIN_VISIBLE.0.min(monitor.width) && height >= MIN_VISIBLE.1.min(monitor.height)
        });
        if reachable {
            self
        } else {
            Self {
                position: (
                    fallback.x + FALLBACK_OFFSET.0,
                    fallback.y + FALLBACK_OFFSET.1,
                ),
                ..self
            }
        }
    }

    /// Mirrors the geometry into the in-memory configuration if the state is free right
    /// now. Never blocks: a busy worker must not make resizing stutter, and the final
    /// geometry is written by [`persist`] regardless.
    pub fn sync_if_uncontended(&self, state: &SharedState) {
        if let Some(mut guard) = state.try_lock() {
            self.apply_to(&mut guard.config);
        }
    }
}

/// Persists the geometry together with the current configuration. This is the last
/// save of a session.
///
/// It goes the way every save goes, as a numbered snapshot of the configuration (see
/// [`ConfigStore`]), and it is written here and now instead of in the background: the
/// process ends when the caller returns. Saves that are still queued in the background
/// are older snapshots; one that is being written is waited for, the others are
/// skipped, and the store is closed, so nothing can overwrite the final configuration.
///
/// The in-memory configuration is the source of truth, so the state lock is preferred,
/// but only for [`STATE_LOCK_BUDGET`]: quitting must not hang behind a worker that holds
/// the state during a large scan/filter. If the budget runs out, the most recent
/// snapshot the store knows is used instead. That is the configuration as of the last
/// change that was saved, which every command does when it changes it. Disk I/O always
/// happens without holding the state lock.
///
/// Either way the snapshot is taken at one moment and written at a later one, and a
/// worker can save a newer snapshot in between (the worker that kept the state busy,
/// for one). The store then skips ours as superseded: the geometry would be missing
/// from the file, and the store would stay open. So the save is checked afterwards
/// and repeated with what is then the latest configuration, until no newer snapshot
/// has turned up while it was written.
pub fn persist(geometry: WindowGeometry, state: &SharedState, store: &ConfigStore) -> Result<()> {
    persist_with(geometry, state, store, || {})
}

/// [`persist`] with a hook that runs right before each write, for tests of what
/// happens when another save gets in between.
fn persist_with(
    geometry: WindowGeometry,
    state: &SharedState,
    store: &ConfigStore,
    mut before_write: impl FnMut(),
) -> Result<()> {
    let latest_known_to_the_store = || {
        let mut config = store.latest();
        geometry.apply_to(&mut config);
        (store.snapshot(&config), config)
    };

    let (mut snapshot, mut config) = match lock_within(state, STATE_LOCK_BUDGET) {
        Some(mut guard) => {
            geometry.apply_to(&mut guard.config);
            (guard.config_snapshot(), guard.config.clone())
        }
        None => {
            tracing::warn!(
                "Application state is busy; saving window geometry with the last saved config."
            );
            latest_known_to_the_store()
        }
    };

    for _ in 0..FINAL_SAVE_ATTEMPTS {
        before_write();
        snapshot.save_and_close()?;
        // Nobody has taken a snapshot since ours: it is what was written (or what was
        // on disk already), and the store is closed.
        if store.latest() == config {
            return Ok(());
        }
        // A newer snapshot exists. If it was written before ours, ours was skipped and
        // the store is still open: write again, on top of it. If ours came first, the
        // store is closed and this is a no-op.
        (snapshot, config) = latest_known_to_the_store();
    }
    tracing::warn!("The configuration kept changing while it was saved for the last time.");
    Ok(())
}

/// Tries to lock the state for at most `budget`.
fn lock_within(state: &SharedState, budget: Duration) -> Option<MutexGuard<'_, AppState>> {
    let deadline = Instant::now() + budget;
    loop {
        if let Some(guard) = state.try_lock() {
            return Some(guard);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(STATE_LOCK_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings;
    use std::path::Path;

    const MONITOR: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    fn geometry() -> WindowGeometry {
        WindowGeometry {
            size: (1200.0, 800.0),
            position: (100.0, 100.0),
        }
    }

    #[test]
    fn to_logical_divides_by_scale() {
        assert_eq!(to_logical((2400.0, 1600.0), 2.0), Some((1200.0, 800.0)));
        assert_eq!(to_logical((-300.0, 150.0), 1.5), Some((-200.0, 100.0)));
    }

    #[test]
    fn to_logical_rejects_unusable_input() {
        assert_eq!(to_logical((100.0, 100.0), 0.0), None);
        assert_eq!(to_logical((100.0, 100.0), -1.0), None);
        assert_eq!(to_logical((100.0, 100.0), f64::NAN), None);
        assert_eq!(to_logical((f64::INFINITY, 100.0), 1.0), None);
    }

    #[test]
    fn size_filter_rejects_minimized_and_too_small() {
        assert!(!is_persistable_size((0.0, 0.0)));
        assert!(!is_persistable_size((898.0, 600.0)));
        assert!(!is_persistable_size((900.0, 100.0)));
        assert!(!is_persistable_size((f64::NAN, 800.0)));
        assert!(is_persistable_size(MIN_WINDOW_SIZE));
        // Rounding at fractional scale factors may land just below the minimum.
        assert!(is_persistable_size((899.5, 599.5)));
        assert!(is_persistable_size((1200.0, 800.0)));
    }

    #[test]
    fn position_filter_rejects_parked_windows_only() {
        assert!(!is_persistable_position((-32000.0, -32000.0)));
        assert!(!is_persistable_position((-32000.0, 10.0)));
        assert!(!is_persistable_position((f64::NAN, 0.0)));
        assert!(is_persistable_position((-1920.0, -200.0)));
        assert!(is_persistable_position((0.0, 0.0)));
    }

    #[test]
    fn record_size_converts_and_ignores_degenerate_values() {
        let mut g = geometry();
        g.record_size((2000.0, 1400.0), 2.0);
        assert_eq!(g.size, (1000.0, 700.0));

        g.record_size((0.0, 0.0), 2.0);
        g.record_size((1000.0, 700.0), 2.0); // 500x350 logical: below the minimum
        g.record_size((2000.0, 1400.0), 0.0);
        assert_eq!(g.size, (1000.0, 700.0));
    }

    #[test]
    fn record_position_converts_and_ignores_minimized() {
        let mut g = geometry();
        g.record_position((-400.0, 60.0), 2.0);
        assert_eq!(g.position, (-200.0, 30.0));

        g.record_position((-32000.0, -32000.0), 1.0);
        g.record_position((10.0, 10.0), f64::NAN);
        assert_eq!(g.position, (-200.0, 30.0));
    }

    #[test]
    fn from_config_falls_back_to_defaults_for_unusable_values() {
        let defaults = AppConfig::default();
        let config = AppConfig {
            window_size: (0.0, 0.0),
            window_position: (f64::NAN, 5.0),
            ..AppConfig::default()
        };
        let g = WindowGeometry::from_config(&config, 2.0);
        assert_eq!(g.size, defaults.window_size);
        assert_eq!(g.position, defaults.window_position);
    }

    /// A geometry that is marked as logical is taken as it is, whatever the display.
    #[test]
    fn from_config_takes_a_logical_geometry_as_it_is() {
        let config = AppConfig {
            window_size: (1000.0, 700.0),
            window_position: (-50.0, 20.0),
            geometry_units: GeometryUnits::Logical,
            ..AppConfig::default()
        };
        for scale in [1.0, 1.5, 2.0, f64::NAN] {
            let g = WindowGeometry::from_config(&config, scale);
            assert_eq!((g.size, g.position), ((1000.0, 700.0), (-50.0, 20.0)));
        }
    }

    /// The upgrade from a version that stored physical pixels: on a 2x display the
    /// window comes back at the size and place it had, not at twice of both.
    #[test]
    fn from_config_converts_a_physical_geometry_with_the_primary_scale() {
        let stored_by_an_older_version = AppConfig {
            window_size: (2400.0, 1600.0),
            window_position: (200.0, -100.0),
            geometry_units: GeometryUnits::Physical,
            ..AppConfig::default()
        };

        let g = WindowGeometry::from_config(&stored_by_an_older_version, 2.0);
        assert_eq!((g.size, g.position), ((1200.0, 800.0), (100.0, -50.0)));

        // At 1x physical and logical are the same.
        let g = WindowGeometry::from_config(&stored_by_an_older_version, 1.0);
        assert_eq!((g.size, g.position), ((2400.0, 1600.0), (200.0, -100.0)));

        let g = WindowGeometry::from_config(&stored_by_an_older_version, 1.5);
        assert_eq!(g.size, (1600.0, 1600.0 / 1.5));
    }

    #[test]
    fn from_config_falls_back_to_defaults_when_the_conversion_yields_nothing_usable() {
        let defaults = AppConfig::default();
        let physical = |window_size| AppConfig {
            window_size,
            window_position: (200.0, 100.0),
            geometry_units: GeometryUnits::Physical,
            ..AppConfig::default()
        };

        // Smaller than the minimum once it is converted.
        let g = WindowGeometry::from_config(&physical((1200.0, 800.0)), 2.0);
        assert_eq!(g.size, defaults.window_size);
        assert_eq!(g.position, (100.0, 50.0), "the position is still usable");

        // No usable scale factor: nothing can be converted.
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let g = WindowGeometry::from_config(&physical((2400.0, 1600.0)), scale);
            assert_eq!(g.size, defaults.window_size, "scale {scale}");
            assert_eq!(g.position, defaults.window_position, "scale {scale}");
        }
    }

    /// What the upgrade looks like end to end: a file without the unit mark is read,
    /// converted once and written back as logical, so the next start takes it as it is.
    #[test]
    fn a_config_file_of_an_older_version_is_migrated_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut json = serde_json::to_value(AppConfig {
            window_size: (2400.0, 1600.0),
            window_position: (200.0, 100.0),
            ..AppConfig::default()
        })
        .unwrap();
        json.as_object_mut().unwrap().remove("geometry_units");
        std::fs::write(&path, json.to_string()).unwrap();

        // First start after the upgrade, on a 2x display.
        let mut config = settings::load_config(&path).unwrap();
        let geometry = WindowGeometry::from_config(&config, 2.0);
        geometry.apply_to(&mut config);
        settings::save_config(&config, &path).unwrap();
        assert_eq!(geometry.size, (1200.0, 800.0));

        // Second start.
        let config = settings::load_config(&path).unwrap();
        assert_eq!(config.geometry_units, GeometryUnits::Logical);
        assert_eq!(WindowGeometry::from_config(&config, 2.0), geometry);
    }

    #[test]
    fn apply_to_writes_both_fields_and_marks_them_as_logical() {
        let mut config = AppConfig {
            geometry_units: GeometryUnits::Physical,
            ..AppConfig::default()
        };
        let g = WindowGeometry {
            size: (1000.0, 700.0),
            position: (7.0, 8.0),
        };
        g.apply_to(&mut config);
        assert_eq!(config.window_size, (1000.0, 700.0));
        assert_eq!(config.window_position, (7.0, 8.0));
        assert_eq!(config.geometry_units, GeometryUnits::Logical);
    }

    #[test]
    fn rect_from_physical_uses_the_monitor_scale() {
        assert_eq!(
            Rect::from_physical(3840.0, 0.0, 3840.0, 2160.0, 2.0),
            Some(Rect {
                x: 1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0
            })
        );
        assert_eq!(Rect::from_physical(0.0, 0.0, 0.0, 0.0, 1.0), None);
        assert_eq!(Rect::from_physical(0.0, 0.0, 100.0, 100.0, 0.0), None);
    }

    #[test]
    fn clamp_keeps_visible_and_partially_visible_windows() {
        let g = geometry();
        assert_eq!(g.clamped_to_visible(&[MONITOR]), g);

        // Mostly off the left edge, but enough remains to grab.
        let partial = WindowGeometry {
            position: (-1000.0, 50.0),
            ..g
        };
        assert_eq!(partial.clamped_to_visible(&[MONITOR]), partial);

        // On a secondary monitor left of the primary one.
        let secondary = Rect {
            x: -1920.0,
            ..MONITOR
        };
        let on_secondary = WindowGeometry {
            position: (-1800.0, 100.0),
            ..g
        };
        assert_eq!(
            on_secondary.clamped_to_visible(&[MONITOR, secondary]),
            on_secondary
        );
    }

    #[test]
    fn clamp_moves_unreachable_windows_to_the_first_monitor() {
        let g = geometry();
        let primary = Rect {
            x: 50.0,
            y: 25.0,
            ..MONITOR
        };
        for position in [
            (5000.0, 100.0),      // monitor that no longer exists
            (100.0, 1100.0),      // only a sliver at the bottom
            (-1140.0, 100.0),     // only a sliver at the left
            (-32000.0, -32000.0), // parked (minimized) position from an old config
        ] {
            let moved = WindowGeometry { position, ..g }.clamped_to_visible(&[primary]);
            assert_eq!(moved.position, (150.0, 125.0), "from {position:?}");
            assert_eq!(moved.size, g.size);
        }
    }

    #[test]
    fn clamp_without_monitor_info_changes_nothing() {
        let far_away = WindowGeometry {
            position: (9999.0, 9999.0),
            ..geometry()
        };
        assert_eq!(far_away.clamped_to_visible(&[]), far_away);
    }

    #[test]
    fn lock_within_gives_up_on_a_held_lock() {
        let state = SharedState::new(AppState::default());
        {
            let _held = state.lock();
            assert!(lock_within(&state, Duration::ZERO).is_none());
            assert!(lock_within(&state, Duration::from_millis(20)).is_none());
        }
        assert!(lock_within(&state, Duration::ZERO).is_some());
    }

    #[test]
    fn sync_if_uncontended_updates_free_state_and_skips_busy_state() {
        let state = SharedState::new(AppState::default());
        let g = WindowGeometry {
            size: (1000.0, 700.0),
            position: (7.0, 8.0),
        };
        g.sync_if_uncontended(&state);
        assert_eq!(state.lock().config.window_size, (1000.0, 700.0));

        let other = geometry();
        {
            let _held = state.lock();
            other.sync_if_uncontended(&state); // must return immediately
        }
        assert_eq!(state.lock().config.window_size, (1000.0, 700.0));
    }

    /// A state whose configuration is kept in a file of a fresh temporary directory.
    fn state_with_config_file(dir: &Path) -> (SharedState, ConfigStore) {
        let (store, config) = ConfigStore::open(dir.join("config.json"));
        (
            SharedState::new(AppState::new(config, store.clone())),
            store,
        )
    }

    const FINAL_GEOMETRY: WindowGeometry = WindowGeometry {
        size: (1000.0, 700.0),
        position: (7.0, 8.0),
    };

    #[test]
    fn persist_saves_the_in_memory_config_with_the_geometry() {
        let dir = tempfile::tempdir().unwrap();
        let (state, store) = state_with_config_file(dir.path());
        state.lock().config.output_filename = "in-memory.txt".into();

        persist(FINAL_GEOMETRY, &state, &store).unwrap();

        let saved = settings::load_config(store.path().unwrap()).unwrap();
        assert_eq!(saved.output_filename, "in-memory.txt");
        assert_eq!(saved.window_size, (1000.0, 700.0));
        assert_eq!(saved.window_position, (7.0, 8.0));
        assert_eq!(saved.geometry_units, GeometryUnits::Logical);
        assert_eq!(state.lock().config, saved);
    }

    /// The race the ordered path exists for: a command changed the configuration and
    /// its background save has not run yet when the application quits. Whenever that
    /// save gets its turn, it must not put its older content over the final one.
    #[test]
    fn persist_cannot_be_overtaken_by_a_save_that_is_still_queued() {
        let dir = tempfile::tempdir().unwrap();
        let (state, store) = state_with_config_file(dir.path());
        let queued = {
            let mut guard = state.lock();
            guard.config.output_filename = "first-change.txt".into();
            guard.config_snapshot()
        };
        state.lock().config.output_filename = "second-change.txt".into();

        persist(FINAL_GEOMETRY, &state, &store).unwrap();
        queued.save().unwrap();

        let saved = settings::load_config(store.path().unwrap()).unwrap();
        assert_eq!(saved.output_filename, "second-change.txt");
        assert_eq!(saved.window_size, (1000.0, 700.0));
    }

    #[test]
    fn persist_falls_back_to_the_last_saved_config_when_the_state_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let (state, store) = state_with_config_file(dir.path());
        // A change whose save is still queued: it is not on disk yet.
        let queued = {
            let mut guard = state.lock();
            guard.config.output_filename = "changed.txt".into();
            guard.config_snapshot()
        };
        {
            let _held = state.lock();
            let started = Instant::now();
            persist(FINAL_GEOMETRY, &state, &store).unwrap();
            assert!(started.elapsed() < Duration::from_secs(5));
        }
        queued.save().unwrap();

        let saved = settings::load_config(store.path().unwrap()).unwrap();
        assert_eq!(saved.output_filename, "changed.txt");
        assert_eq!(saved.window_size, (1000.0, 700.0));
        assert_eq!(saved.window_position, (7.0, 8.0));
    }

    /// A save that is attempted after the session's last one must not reach the disk.
    fn assert_store_is_closed(store: &ConfigStore) {
        let before = std::fs::read_to_string(store.path().unwrap()).unwrap();
        let late = AppConfig {
            output_filename: "too-late.txt".into(),
            ..store.latest()
        };
        store.snapshot(&late).save().unwrap();
        let after = std::fs::read_to_string(store.path().unwrap()).unwrap();
        assert_eq!(after, before, "the store must be closed");
    }

    /// The worker that keeps the state busy changes the configuration and saves it
    /// while the exit save is under way: after the exit snapshot was taken, before it
    /// is written. The exit snapshot is then the older one and is skipped; the save
    /// has to be repeated, or the geometry is lost and the store stays open.
    #[test]
    fn persist_is_repeated_when_a_worker_saves_a_newer_snapshot_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let (state, store) = state_with_config_file(dir.path());

        let mut guard = state.lock(); // the busy worker
        let mut worker_saves = 0;
        persist_with(FINAL_GEOMETRY, &state, &store, || {
            if worker_saves == 0 {
                guard.config.output_filename = "worker.txt".into();
                guard.config_snapshot().save().unwrap();
            }
            worker_saves += 1;
        })
        .unwrap();
        drop(guard);

        assert_eq!(worker_saves, 2, "one repetition is enough");
        let saved = settings::load_config(store.path().unwrap()).unwrap();
        assert_eq!(saved.output_filename, "worker.txt", "the latest config");
        assert_eq!(saved.window_size, (1000.0, 700.0), "with the geometry");
        assert_eq!(saved.window_position, (7.0, 8.0));
        assert_eq!(saved.geometry_units, GeometryUnits::Logical);
        assert_store_is_closed(&store);
    }

    /// The same race without the fallback: the state lock was free when the snapshot
    /// was taken, and a command saved its change before the snapshot was written.
    #[test]
    fn persist_is_repeated_when_a_command_saves_between_snapshot_and_write() {
        let dir = tempfile::tempdir().unwrap();
        let (state, store) = state_with_config_file(dir.path());

        let mut first = true;
        persist_with(FINAL_GEOMETRY, &state, &store, || {
            if std::mem::take(&mut first) {
                let mut guard = state.lock();
                guard.config.output_filename = "command.txt".into();
                guard.config_snapshot().save().unwrap();
            }
        })
        .unwrap();

        let saved = settings::load_config(store.path().unwrap()).unwrap();
        assert_eq!(saved.output_filename, "command.txt");
        assert_eq!(saved.window_size, (1000.0, 700.0));
        assert_store_is_closed(&store);
    }

    #[test]
    fn persist_closes_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (state, store) = state_with_config_file(dir.path());

        persist(FINAL_GEOMETRY, &state, &store).unwrap();

        assert_store_is_closed(&store);
    }
}
