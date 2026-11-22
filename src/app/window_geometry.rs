//! Window geometry tracking and persistence.
//!
//! The UI thread reports every resize/move here. The tracker is a small value owned by
//! the event loop, so following the window never waits for the (potentially long-held)
//! [`AppState`] mutex. The geometry only meets the configuration when it is persisted.
//!
//! All values are *logical* units: the window is restored with logical size/position,
//! and mixing them with the physical pixels the windowing system reports would make the
//! window grow on every restart on HiDPI displays.

use crate::app::state::AppState;
use crate::config::{settings, AppConfig};
use anyhow::Result;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, TryLockError};
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
    /// Reads the stored geometry, replacing unusable values with the defaults.
    pub fn from_config(config: &AppConfig) -> Self {
        let defaults = AppConfig::default();
        let size = if is_persistable_size(config.window_size) {
            config.window_size
        } else {
            defaults.window_size
        };
        let position = config.window_position;
        let position = if position.0.is_finite() && position.1.is_finite() {
            position
        } else {
            defaults.window_position
        };
        Self { size, position }
    }

    /// Writes the geometry into a configuration.
    pub fn apply_to(&self, config: &mut AppConfig) {
        config.window_size = self.size;
        config.window_position = self.position;
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
    pub fn sync_if_uncontended(&self, state: &Mutex<AppState>) {
        if let Some(mut guard) = lock_within(state, Duration::ZERO) {
            self.apply_to(&mut guard.config);
        }
    }
}

/// Persists the geometry together with the current configuration.
///
/// The in-memory configuration is the source of truth, so the state lock is preferred,
/// but only for [`STATE_LOCK_BUDGET`]: quitting must not hang behind a worker that holds
/// the state during a large scan/filter. If the budget runs out, the configuration is
/// taken from disk instead. That is equivalent in practice because every command saves
/// the configuration when it changes it; only a change made in that very moment could
/// be missed. Disk I/O always happens without holding the state lock.
pub fn persist(
    geometry: WindowGeometry,
    state: &Mutex<AppState>,
    path_override: Option<&Path>,
) -> Result<()> {
    let config = match lock_within(state, STATE_LOCK_BUDGET) {
        Some(mut guard) => {
            geometry.apply_to(&mut guard.config);
            guard.config.clone()
        }
        None => {
            tracing::warn!("Application state is busy; saving window geometry from disk config.");
            let mut config = settings::load_config(path_override)?;
            geometry.apply_to(&mut config);
            config
        }
    };
    settings::save_config(&config, path_override)
}

/// Tries to lock `mutex` for at most `budget`. A poisoned lock is recovered: the
/// configuration inside is still the best information available.
fn lock_within<T>(mutex: &Mutex<T>, budget: Duration) -> Option<MutexGuard<'_, T>> {
    let deadline = Instant::now() + budget;
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(STATE_LOCK_POLL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
        let g = WindowGeometry::from_config(&config);
        assert_eq!(g.size, defaults.window_size);
        assert_eq!(g.position, defaults.window_position);

        let config = AppConfig {
            window_size: (1000.0, 700.0),
            window_position: (-50.0, 20.0),
            ..AppConfig::default()
        };
        let g = WindowGeometry::from_config(&config);
        assert_eq!((g.size, g.position), ((1000.0, 700.0), (-50.0, 20.0)));
    }

    #[test]
    fn apply_to_writes_both_fields() {
        let mut config = AppConfig::default();
        let g = WindowGeometry {
            size: (1000.0, 700.0),
            position: (7.0, 8.0),
        };
        g.apply_to(&mut config);
        assert_eq!(config.window_size, (1000.0, 700.0));
        assert_eq!(config.window_position, (7.0, 8.0));
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
    fn lock_within_gives_up_on_a_held_lock_and_recovers_a_poisoned_one() {
        let mutex = Arc::new(Mutex::new(1));
        {
            let _held = mutex.lock().unwrap();
            assert!(lock_within(&mutex, Duration::ZERO).is_none());
            assert!(lock_within(&mutex, Duration::from_millis(20)).is_none());
        }
        assert_eq!(*lock_within(&mutex, Duration::ZERO).unwrap(), 1);

        let poisoner = mutex.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(mutex.is_poisoned());
        assert_eq!(*lock_within(&mutex, Duration::ZERO).unwrap(), 1);
    }

    #[test]
    fn sync_if_uncontended_updates_free_state_and_skips_busy_state() {
        let state = Mutex::new(AppState::default());
        let g = WindowGeometry {
            size: (1000.0, 700.0),
            position: (7.0, 8.0),
        };
        g.sync_if_uncontended(&state);
        assert_eq!(state.lock().unwrap().config.window_size, (1000.0, 700.0));

        let other = geometry();
        {
            let _held = state.lock().unwrap();
            other.sync_if_uncontended(&state); // must return immediately
        }
        assert_eq!(state.lock().unwrap().config.window_size, (1000.0, 700.0));
    }

    #[test]
    fn persist_saves_the_in_memory_config_with_the_geometry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut app_state = AppState::default();
        app_state.config = AppConfig {
            output_filename: "in-memory.txt".into(),
            ..AppConfig::default()
        };
        let state = Mutex::new(app_state);
        let g = WindowGeometry {
            size: (1000.0, 700.0),
            position: (7.0, 8.0),
        };

        persist(g, &state, Some(&path)).unwrap();

        let saved = settings::load_config(Some(&path)).unwrap();
        assert_eq!(saved.output_filename, "in-memory.txt");
        assert_eq!(saved.window_size, (1000.0, 700.0));
        assert_eq!(saved.window_position, (7.0, 8.0));
        assert_eq!(state.lock().unwrap().config, saved);
    }

    #[test]
    fn persist_falls_back_to_the_disk_config_when_the_state_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let on_disk = AppConfig {
            output_filename: "on-disk.txt".into(),
            ..AppConfig::default()
        };
        settings::save_config(&on_disk, Some(&path)).unwrap();

        let state = Mutex::new(AppState::default());
        let g = WindowGeometry {
            size: (1000.0, 700.0),
            position: (7.0, 8.0),
        };
        {
            let _held = state.lock().unwrap();
            let started = Instant::now();
            persist(g, &state, Some(&path)).unwrap();
            assert!(started.elapsed() < Duration::from_secs(5));
        }

        let saved = settings::load_config(Some(&path)).unwrap();
        assert_eq!(saved.output_filename, "on-disk.txt");
        assert_eq!(saved.window_size, (1000.0, 700.0));
        assert_eq!(saved.window_position, (7.0, 8.0));
    }
}
