//! Window geometry: tracking, and persistence in a file of its own.
//!
//! The window belongs to the composition root (`main.rs`), and so does its geometry:
//! the event loop owns a [`WindowGeometry`], reports every resize/move to it and hands
//! it to a [`GeometryStore`]: a moment after the window has come to rest
//! ([`SaveDebounce`]; written in the background, so that a crash costs at most the
//! last move), and when the application quits. Neither the application state
//! nor the configuration knows about it, so following the window never waits for the
//! state lock, and saving the geometry never competes with a save of the configuration.
//!
//! All values are *logical* units: the window is restored with logical size/position,
//! and mixing them with the physical pixels the windowing system reports would make the
//! window grow on every restart on HiDPI displays.
//!
//! The geometry is kept in `window.json` beside the configuration file. Versions up to
//! 0.3.1 kept it in the configuration file itself (and stored physical pixels, without
//! saying so); a later version kept it there in logical units, marked as such. Where
//! there is no `window.json` yet, the geometry of such a file is read once, converted
//! if necessary, and written to `window.json` ([`GeometryStore::open`]). The
//! configuration file loses it with its next save, as the configuration has no such
//! fields any more.

use crate::config::settings;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The name of the file the geometry is kept in, in the directory of the configuration.
pub const WINDOW_FILE: &str = "window.json";

/// The geometry of a window nothing is known about.
const DEFAULT_GEOMETRY: WindowGeometry = WindowGeometry {
    size: (1200.0, 800.0),
    position: (100.0, 100.0),
};

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

/// Size and position of the main window in logical units. Serialized, this is the
/// content of `window.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WindowGeometry {
    pub size: (f64, f64),
    pub position: (f64, f64),
}

impl WindowGeometry {
    /// A geometry of `size` and `position` in logical units where they are usable, of
    /// the defaults where they are not (or not there).
    fn from_stored(size: Option<(f64, f64)>, position: Option<(f64, f64)>) -> Self {
        Self {
            size: size
                .filter(|size| is_persistable_size(*size))
                .unwrap_or(DEFAULT_GEOMETRY.size),
            position: position
                .filter(|position| position.0.is_finite() && position.1.is_finite())
                .unwrap_or(DEFAULT_GEOMETRY.position),
        }
    }

    /// Records a new inner size reported in physical pixels; degenerate sizes are
    /// ignored. Returns whether the geometry has changed.
    pub fn record_size(&mut self, physical: (f64, f64), scale: f64) -> bool {
        let before = *self;
        if let Some(size) = to_logical(physical, scale).filter(|size| is_persistable_size(*size)) {
            self.size = size;
        }
        *self != before
    }

    /// Records a new outer position reported in physical pixels; the position of a
    /// minimized window is ignored. Returns whether the geometry has changed.
    pub fn record_position(&mut self, physical: (f64, f64), scale: f64) -> bool {
        if !is_persistable_position(physical) {
            return false;
        }
        let before = *self;
        if let Some(position) = to_logical(physical, scale) {
            self.position = position;
        }
        *self != before
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
}

impl Default for WindowGeometry {
    fn default() -> Self {
        DEFAULT_GEOMETRY
    }
}

/// What `window.json` holds, read leniently: a value that is missing or `null` is
/// replaced by its default without costing the other one.
#[derive(Deserialize, Default)]
#[serde(default)]
struct StoredGeometry {
    size: Option<(f64, f64)>,
    position: Option<(f64, f64)>,
}

/// The geometry as a configuration file of an older version holds it.
#[derive(Deserialize, Default)]
#[serde(default)]
struct LegacyGeometry {
    window_size: Option<(f64, f64)>,
    window_position: Option<(f64, f64)>,
    geometry_units: Option<LegacyUnits>,
}

/// The unit of a geometry in a configuration file.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum LegacyUnits {
    /// Physical pixels. This is what versions up to 0.3.1 stored, without saying so:
    /// a geometry that does not name its unit is physical.
    Physical,
    /// Logical units, independent of the scale factor of the display.
    Logical,
}

/// Reads the geometry out of the content of a configuration file of an older version,
/// in logical units. `None` if the file holds no geometry (or cannot be read).
///
/// A geometry stored in physical pixels is converted with `primary_scale`, the scale
/// factor of the primary monitor: that is where such a window is most likely to have
/// been, and the stored values do not say. Without the conversion the first start
/// after an upgrade would open the window at twice its size on a 2x display. Values
/// that are unusable (also after the conversion) give way to the defaults.
fn legacy_geometry(config_content: &str, primary_scale: f64) -> Option<WindowGeometry> {
    let legacy: LegacyGeometry = serde_json::from_str(config_content).ok()?;
    if legacy.window_size.is_none() && legacy.window_position.is_none() {
        return None;
    }
    let in_logical_units = |stored: (f64, f64)| match legacy.geometry_units {
        Some(LegacyUnits::Logical) => Some(stored),
        Some(LegacyUnits::Physical) | None => to_logical(stored, primary_scale),
    };
    Some(WindowGeometry::from_stored(
        legacy.window_size.and_then(in_logical_units),
        legacy.window_position.and_then(in_logical_units),
    ))
}

/// How long the window has to rest after a resize/move before its geometry is saved.
pub const GEOMETRY_SAVE_DELAY: Duration = Duration::from_millis(750);

/// Decides when the geometry is saved while the application runs: once the window has
/// rested for a while after the last resize/move, not for every step of a drag.
///
/// A pure value: it is told the time (`now`) by its caller, the event loop, and starts
/// nothing itself. The save at exit does not depend on it.
#[derive(Debug)]
pub struct SaveDebounce {
    delay: Duration,
    due_at: Option<Instant>,
}

impl SaveDebounce {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            due_at: None,
        }
    }

    /// The geometry changed at `now`: a save is due once `delay` has passed without
    /// another change. A save that was due before is put off with it.
    pub fn changed(&mut self, now: Instant) {
        self.due_at = Some(now + self.delay);
    }

    /// When the pending save is due, `None` if there is nothing to save. This is the
    /// time the event loop has to wake up at.
    pub fn due_at(&self) -> Option<Instant> {
        self.due_at
    }

    /// Whether the geometry is to be saved at `now`. Answers `true` once for the
    /// changes made so far; after that there is nothing pending until the next change.
    pub fn take_due(&mut self, now: Instant) -> bool {
        let due = self.due_at.is_some_and(|due_at| now >= due_at);
        if due {
            self.due_at = None;
        }
        due
    }

    /// Forgets a pending save (the caller saves by itself, as at exit).
    pub fn cancel(&mut self) {
        self.due_at = None;
    }
}

/// Keeps the geometry of the window in `window.json`.
///
/// The file is only ever written on behalf of the composition root: when the geometry
/// of an older configuration file is taken over, a moment after the window has been
/// resized or moved (in the background, see [`SaveDebounce`]), and when the
/// application quits.
///
/// Cloning is cheap; all clones are the same store. Saves may run on any thread: they
/// are serialized, and each writes the geometry that was handed to the store last, so
/// a save that gets its turn late cannot put an older geometry over a newer one.
#[derive(Debug, Clone)]
pub struct GeometryStore {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    /// The file the geometry is written to; `None` for a detached store.
    path: Option<PathBuf>,
    /// The geometry handed to [`GeometryStore::save`] last. Locked only for a moment.
    latest: Mutex<Option<WindowGeometry>>,
    /// The geometry this store wrote last. Held while the file is written, which
    /// serializes the writes.
    written: Mutex<Option<WindowGeometry>>,
}

impl GeometryStore {
    fn new(path: Option<PathBuf>) -> Self {
        Self {
            shared: Arc::new(Shared {
                path,
                latest: Mutex::new(None),
                written: Mutex::new(None),
            }),
        }
    }

    /// Opens the geometry that belongs to the configuration file at `config_path`
    /// (`window.json` in the same directory) and returns the store together with the
    /// geometry to start with:
    ///
    /// 1. what `window.json` holds;
    /// 2. without such a file (or with one that cannot be read): the geometry an older
    ///    version left in the configuration file, converted to logical units with
    ///    `primary_scale` if necessary (see [`legacy_geometry`]). It is written to
    ///    `window.json` right away, as the configuration file loses it with its next
    ///    save. (If `window.json` can never be written while the configuration file
    ///    can, e.g. in a read-only directory, that geometry is lost with that save.)
    /// 3. the default geometry otherwise. Nothing is written for it.
    ///
    /// The configuration file is only read here, never created or changed.
    pub fn open(config_path: &Path, primary_scale: f64) -> (Self, WindowGeometry) {
        let store = Self::new(Some(config_path.with_file_name(WINDOW_FILE)));
        if let Some(geometry) = store.read() {
            return (store, geometry);
        }
        let migrated = fs::read_to_string(config_path)
            .ok()
            .and_then(|content| legacy_geometry(&content, primary_scale));
        let Some(geometry) = migrated else {
            return (store, WindowGeometry::default());
        };
        tracing::info!("Taking over the window geometry from {config_path:?}.");
        if let Err(e) = store.save(geometry) {
            tracing::warn!("Failed to save the window geometry that was taken over: {e}");
        }
        (store, geometry)
    }

    /// A store that is not bound to a file: nothing is read, nothing is written. Used
    /// when no configuration directory can be determined.
    pub fn detached() -> Self {
        Self::new(None)
    }

    /// The geometry in the file, `None` if there is no file or no geometry in it.
    fn read(&self) -> Option<WindowGeometry> {
        let content = fs::read_to_string(self.shared.path.as_ref()?).ok()?;
        match serde_json::from_str::<StoredGeometry>(&content) {
            Ok(StoredGeometry {
                size: None,
                position: None,
            }) => None,
            Ok(stored) => Some(WindowGeometry::from_stored(stored.size, stored.position)),
            Err(e) => {
                tracing::warn!("Failed to read the window geometry: {e}");
                None
            }
        }
    }

    /// Saves `geometry`: writes it to the file (atomically, like the configuration),
    /// creating its directory if necessary. Blocks on file IO and on nothing else, in
    /// particular not on the application state.
    ///
    /// What is written is the geometry the store was handed last, by this call or by
    /// one that came after it and is waiting for its turn; a geometry this store has
    /// written already is not written again. If the write fails, the next save tries
    /// again.
    pub fn save(&self, geometry: WindowGeometry) -> Result<()> {
        let shared = &self.shared;
        *lock(&shared.latest) = Some(geometry);

        let mut written = lock(&shared.written);
        let latest = *lock(&shared.latest);
        if latest == *written {
            return Ok(());
        }
        if let (Some(path), Some(latest)) = (&shared.path, latest) {
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            settings::write_json(&latest, path)?;
            tracing::debug!("Saved window geometry to {path:?}");
        }
        *written = latest;
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
mod tests;
