//! Window geometry: tracking, and persistence in a file of its own.
//!
//! The window belongs to the composition root (`main.rs`), and so does its geometry:
//! the event loop owns a [`WindowGeometry`], reports every resize/move to it and hands
//! it to a [`GeometryStore`] when the application quits. Neither the application state
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

/// Keeps the geometry of the window in `window.json`.
///
/// The file is only ever written by the composition root: once when the geometry of an
/// older configuration file is taken over, and when the application quits.
#[derive(Debug, Clone)]
pub struct GeometryStore {
    /// The file the geometry is written to; `None` for a detached store.
    path: Option<PathBuf>,
}

impl GeometryStore {
    /// Opens the geometry that belongs to the configuration file at `config_path`
    /// (`window.json` in the same directory) and returns the store together with the
    /// geometry to start with:
    ///
    /// 1. what `window.json` holds;
    /// 2. without such a file (or with one that cannot be read): the geometry an older
    ///    version left in the configuration file, converted to logical units with
    ///    `primary_scale` if necessary (see [`legacy_geometry`]). It is written to
    ///    `window.json` right away, as the configuration file loses it with its next
    ///    save;
    /// 3. the default geometry otherwise. Nothing is written for it.
    ///
    /// The configuration file is only read here, never created or changed.
    pub fn open(config_path: &Path, primary_scale: f64) -> (Self, WindowGeometry) {
        let store = Self {
            path: Some(config_path.with_file_name(WINDOW_FILE)),
        };
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
        Self { path: None }
    }

    /// The geometry in the file, `None` if there is no file or no geometry in it.
    fn read(&self) -> Option<WindowGeometry> {
        let content = fs::read_to_string(self.path.as_ref()?).ok()?;
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

    /// Writes the geometry to the file (atomically, like the configuration), creating
    /// its directory if necessary. Blocks on file IO and on nothing else.
    pub fn save(&self, geometry: WindowGeometry) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        settings::write_json(&geometry, path)?;
        tracing::info!("Saved window geometry to {path:?}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::shared_state::SharedState;
    use crate::app::state::AppState;
    use crate::config::{AppConfig, ConfigStore};
    use serde_json::{json, Value};

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

    // --- The geometry in a configuration file of an older version ---

    /// The content of a configuration file of an older version: its settings and the
    /// given geometry fields.
    fn legacy_config(geometry: Value) -> String {
        let mut config = serde_json::to_value(AppConfig {
            output_filename: "kept.txt".into(),
            ..AppConfig::default()
        })
        .unwrap();
        let fields = config.as_object_mut().unwrap();
        fields.extend(geometry.as_object().unwrap().clone());
        config.to_string()
    }

    fn legacy(geometry: Value, primary_scale: f64) -> Option<WindowGeometry> {
        legacy_geometry(&legacy_config(geometry), primary_scale)
    }

    #[test]
    fn legacy_geometry_falls_back_to_defaults_for_unusable_values() {
        let g = legacy(
            json!({ "window_size": [0.0, 0.0], "window_position": null, "geometry_units": "logical" }),
            2.0,
        )
        .unwrap();
        assert_eq!(g.size, DEFAULT_GEOMETRY.size);
        assert_eq!(g.position, DEFAULT_GEOMETRY.position);

        // A value that is no pair of numbers: nothing of the geometry is read.
        assert_eq!(
            legacy(
                json!({ "window_size": [1000.0, 700.0], "window_position": [null, 5.0] }),
                2.0
            ),
            None
        );
    }

    /// A geometry that is marked as logical is taken as it is, whatever the display.
    #[test]
    fn legacy_geometry_takes_a_logical_geometry_as_it_is() {
        let stored = json!({
            "window_size": [1000.0, 700.0],
            "window_position": [-50.0, 20.0],
            "geometry_units": "logical",
        });
        for scale in [1.0, 1.5, 2.0, f64::NAN] {
            let g = legacy(stored.clone(), scale).unwrap();
            assert_eq!((g.size, g.position), ((1000.0, 700.0), (-50.0, 20.0)));
        }
    }

    /// The upgrade from a version that stored physical pixels: on a 2x display the
    /// window comes back at the size and place it had, not at twice of both.
    #[test]
    fn legacy_geometry_converts_a_physical_geometry_with_the_primary_scale() {
        let unmarked =
            json!({ "window_size": [2400.0, 1600.0], "window_position": [200.0, -100.0] });
        let mut marked = unmarked.clone();
        marked["geometry_units"] = json!("physical");
        let mut null_unit = unmarked.clone();
        null_unit["geometry_units"] = Value::Null;

        for stored_by_an_older_version in [unmarked, marked, null_unit] {
            let g = legacy(stored_by_an_older_version.clone(), 2.0).unwrap();
            assert_eq!((g.size, g.position), ((1200.0, 800.0), (100.0, -50.0)));

            // At 1x physical and logical are the same.
            let g = legacy(stored_by_an_older_version.clone(), 1.0).unwrap();
            assert_eq!((g.size, g.position), ((2400.0, 1600.0), (200.0, -100.0)));

            let g = legacy(stored_by_an_older_version, 1.5).unwrap();
            assert_eq!(g.size, (1600.0, 1600.0 / 1.5));
        }
    }

    #[test]
    fn legacy_geometry_falls_back_to_defaults_when_the_conversion_yields_nothing_usable() {
        let physical = |window_size: (f64, f64)| json!({ "window_size": window_size, "window_position": [200.0, 100.0] });

        // Smaller than the minimum once it is converted.
        let g = legacy(physical((1200.0, 800.0)), 2.0).unwrap();
        assert_eq!(g.size, DEFAULT_GEOMETRY.size);
        assert_eq!(g.position, (100.0, 50.0), "the position is still usable");

        // No usable scale factor: nothing can be converted.
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let g = legacy(physical((2400.0, 1600.0)), scale).unwrap();
            assert_eq!(g.size, DEFAULT_GEOMETRY.size, "scale {scale}");
            assert_eq!(g.position, DEFAULT_GEOMETRY.position, "scale {scale}");
        }
    }

    /// A configuration file that holds no geometry has none to take over: one written
    /// by this version, an empty one, one that is no configuration at all.
    #[test]
    fn legacy_geometry_is_none_for_a_config_without_a_geometry() {
        assert_eq!(legacy(json!({}), 2.0), None);
        assert_eq!(legacy(json!({ "geometry_units": "physical" }), 2.0), None);
        assert_eq!(
            legacy(json!({ "window_size": null, "window_position": null }), 2.0),
            None
        );
        for content in ["", "{}", "[1, 2]", "not json"] {
            assert_eq!(legacy_geometry(content, 2.0), None, "{content:?}");
        }
    }

    /// One of the two is enough; the other one has its default.
    #[test]
    fn legacy_geometry_with_a_size_or_a_position_only() {
        let g = legacy(json!({ "window_size": [2400.0, 1600.0] }), 2.0).unwrap();
        assert_eq!(
            (g.size, g.position),
            ((1200.0, 800.0), DEFAULT_GEOMETRY.position)
        );

        let g = legacy(json!({ "window_position": [20.0, 40.0] }), 2.0).unwrap();
        assert_eq!((g.size, g.position), (DEFAULT_GEOMETRY.size, (10.0, 20.0)));
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

    // --- The file ---

    const FINAL_GEOMETRY: WindowGeometry = WindowGeometry {
        size: (1000.0, 700.0),
        position: (-7.0, 8.0),
    };

    /// A fresh configuration directory: the path of its `config.json` (which does not
    /// exist yet) and of the `window.json` beside it.
    fn config_dir() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let window_path = dir.path().join(WINDOW_FILE);
        (dir, config_path, window_path)
    }

    fn file_names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn json_in(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_window_file_is_kept_beside_the_config_file() {
        let (dir, config_path, window_path) = config_dir();

        let (store, _) = GeometryStore::open(&config_path, 1.0);
        store.save(FINAL_GEOMETRY).unwrap();

        assert_eq!(store.path.as_deref(), Some(window_path.as_path()));
        // An isolated configuration directory (`CFC_CONFIG_DIR`) holds everything.
        assert_eq!(file_names_in(dir.path()), [WINDOW_FILE]);
    }

    #[test]
    fn open_without_any_file_yields_the_default_geometry_and_writes_nothing() {
        let (dir, config_path, _) = config_dir();

        let (_store, geometry) = GeometryStore::open(&config_path, 2.0);

        assert_eq!(geometry, WindowGeometry::default());
        assert_eq!(geometry, DEFAULT_GEOMETRY);
        assert!(file_names_in(dir.path()).is_empty());
    }

    #[test]
    fn a_saved_geometry_is_restored_as_it_is_whatever_the_display() {
        let (_dir, config_path, window_path) = config_dir();
        let (store, _) = GeometryStore::open(&config_path, 1.0);

        store.save(FINAL_GEOMETRY).unwrap();

        assert_eq!(
            json_in(&window_path),
            json!({ "size": [1000.0, 700.0], "position": [-7.0, 8.0] })
        );
        for scale in [1.0, 1.5, 2.0, f64::NAN] {
            let (_, restored) = GeometryStore::open(&config_path, scale);
            assert_eq!(restored, FINAL_GEOMETRY, "scale {scale}");
        }
    }

    /// What a session does: the geometry it started with follows the window, and the
    /// last one is what the next session starts with.
    #[test]
    fn the_geometry_tracked_in_a_session_is_what_the_next_one_starts_with() {
        let (_dir, config_path, _) = config_dir();
        let (store, mut geometry) = GeometryStore::open(&config_path, 2.0);

        geometry.record_size((2000.0, 1400.0), 2.0);
        geometry.record_position((-400.0, 60.0), 2.0);
        // Minimized on the way out: neither value is worth remembering.
        geometry.record_size((0.0, 0.0), 2.0);
        geometry.record_position((-32000.0, -32000.0), 2.0);
        store.save(geometry).unwrap();

        let (_, restored) = GeometryStore::open(&config_path, 2.0);
        assert_eq!(
            restored,
            WindowGeometry {
                size: (1000.0, 700.0),
                position: (-200.0, 30.0),
            }
        );
    }

    #[test]
    fn save_creates_a_missing_directory_and_leaves_no_other_file() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("not").join("there").join("yet");
        let (store, _) = GeometryStore::open(&nested.join("config.json"), 1.0);

        store.save(FINAL_GEOMETRY).unwrap();
        store.save(geometry()).unwrap();

        assert_eq!(file_names_in(&nested), [WINDOW_FILE]);
        assert_eq!(
            GeometryStore::open(&nested.join("config.json"), 1.0).1,
            geometry()
        );
    }

    #[test]
    fn a_failed_save_is_reported_and_leaves_no_file_behind() {
        let (dir, config_path, window_path) = config_dir();
        let (store, _) = GeometryStore::open(&config_path, 1.0);

        // A directory in place of the file makes the write fail.
        fs::create_dir(&window_path).unwrap();
        assert!(store.save(FINAL_GEOMETRY).is_err());
        assert_eq!(file_names_in(dir.path()), [WINDOW_FILE]);

        fs::remove_dir(&window_path).unwrap();
        store.save(FINAL_GEOMETRY).unwrap();
        assert_eq!(GeometryStore::open(&config_path, 1.0).1, FINAL_GEOMETRY);
    }

    #[test]
    fn unusable_values_in_the_window_file_give_way_to_the_defaults() {
        let (_dir, config_path, window_path) = config_dir();
        let open_with = |content: &str| {
            fs::write(&window_path, content).unwrap();
            GeometryStore::open(&config_path, 2.0).1
        };

        // Each value on its own.
        assert_eq!(
            open_with(r#"{ "size": [100.0, 50.0], "position": [7.0, 8.0] }"#),
            WindowGeometry {
                size: DEFAULT_GEOMETRY.size,
                position: (7.0, 8.0),
            }
        );
        assert_eq!(
            open_with(r#"{ "size": [1000.0, 700.0], "position": null }"#),
            WindowGeometry {
                size: (1000.0, 700.0),
                position: DEFAULT_GEOMETRY.position,
            }
        );
        assert_eq!(
            open_with(r#"{ "position": [7.0, 8.0], "something": "else" }"#),
            WindowGeometry {
                size: DEFAULT_GEOMETRY.size,
                position: (7.0, 8.0),
            }
        );
        // No geometry at all.
        for content in ["{}", "", "not json", r#"{ "size": "large" }"#] {
            assert_eq!(open_with(content), DEFAULT_GEOMETRY, "{content:?}");
        }
    }

    #[test]
    fn a_detached_store_reads_and_writes_nothing() {
        let store = GeometryStore::detached();
        assert_eq!(store.read(), None);
        store.save(FINAL_GEOMETRY).unwrap();
        assert_eq!(store.read(), None);
    }

    // --- Taking over the geometry of an older configuration file ---

    /// What the upgrade looks like end to end: the geometry of a configuration file
    /// of a version up to 0.3.1 (physical pixels, no unit) is converted once and
    /// written to `window.json`. The configuration file is left alone by that, and
    /// loses the geometry with its next save.
    #[test]
    fn a_config_file_of_an_older_version_is_migrated_once() {
        let (dir, config_path, window_path) = config_dir();
        let old_config = legacy_config(
            json!({ "window_size": [2400.0, 1600.0], "window_position": [200.0, 100.0] }),
        );
        fs::write(&config_path, &old_config).unwrap();

        // First start after the upgrade, on a 2x display.
        let (geometry_store, geometry) = GeometryStore::open(&config_path, 2.0);
        let (config_store, config) = ConfigStore::open(config_path.clone());
        assert_eq!(geometry.size, (1200.0, 800.0));
        assert_eq!(geometry.position, (100.0, 50.0));
        assert_eq!(config.output_filename, "kept.txt", "the settings are kept");
        assert_eq!(
            json_in(&window_path),
            json!({ "size": [1200.0, 800.0], "position": [100.0, 50.0] }),
            "window.json is written right away"
        );
        assert_eq!(
            fs::read_to_string(&config_path).unwrap(),
            old_config,
            "starting does not rewrite the config file"
        );

        // The next save of the configuration writes it without the geometry.
        let changed = AppConfig {
            case_sensitive_search: true,
            ..config
        };
        config_store.snapshot(&changed).save().unwrap();
        let rewritten = fs::read_to_string(&config_path).unwrap();
        for field in ["window_size", "window_position", "geometry_units"] {
            assert!(!rewritten.contains(field), "{field} in {rewritten}");
        }
        assert_eq!(settings::load_config(&config_path).unwrap(), changed);

        // The session ends; the window has been moved meanwhile.
        geometry_store.save(FINAL_GEOMETRY).unwrap();
        config_store.close().unwrap();

        // Second start: the geometry is taken as it is, nothing is converted again.
        let (_, geometry) = GeometryStore::open(&config_path, 2.0);
        assert_eq!(geometry, FINAL_GEOMETRY);
        assert_eq!(settings::load_config(&config_path).unwrap(), changed);
        assert_eq!(file_names_in(dir.path()), ["config.json", WINDOW_FILE]);
    }

    /// The geometry is not converted a second time if the configuration file is not
    /// saved between two starts (it still holds the physical geometry then).
    #[test]
    fn the_window_file_wins_over_a_geometry_that_is_still_in_the_config_file() {
        let (_dir, config_path, _) = config_dir();
        fs::write(
            &config_path,
            legacy_config(
                json!({ "window_size": [2400.0, 1600.0], "window_position": [200.0, 100.0] }),
            ),
        )
        .unwrap();

        let (_, first) = GeometryStore::open(&config_path, 2.0);
        let (store, second) = GeometryStore::open(&config_path, 2.0);
        assert_eq!(first.size, (1200.0, 800.0));
        assert_eq!(second, first);

        store.save(FINAL_GEOMETRY).unwrap();
        assert_eq!(GeometryStore::open(&config_path, 2.0).1, FINAL_GEOMETRY);
    }

    /// A configuration file of the version that stored logical units and said so.
    #[test]
    fn a_logical_geometry_in_the_config_file_is_taken_over_as_it_is() {
        let (_dir, config_path, window_path) = config_dir();
        fs::write(
            &config_path,
            legacy_config(json!({
                "window_size": [1000.0, 700.0],
                "window_position": [-7.0, 8.0],
                "geometry_units": "logical",
            })),
        )
        .unwrap();

        let (_, geometry) = GeometryStore::open(&config_path, 2.0);

        assert_eq!(geometry, FINAL_GEOMETRY);
        assert!(window_path.is_file());
    }

    /// A configuration file without a geometry (this version's, or a fresh one):
    /// nothing to take over, and no file is written for the defaults.
    #[test]
    fn a_config_file_without_a_geometry_is_not_migrated() {
        let (dir, config_path, _) = config_dir();
        let (_, config) = ConfigStore::open(config_path.clone());
        assert_eq!(config, AppConfig::default());

        let (_, geometry) = GeometryStore::open(&config_path, 2.0);

        assert_eq!(geometry, DEFAULT_GEOMETRY);
        assert_eq!(file_names_in(dir.path()), ["config.json"]);
    }

    /// A window file that cannot be read does not cost the geometry the configuration
    /// file still holds.
    #[test]
    fn a_damaged_window_file_is_replaced_by_the_geometry_of_the_config_file() {
        let (_dir, config_path, window_path) = config_dir();
        fs::write(
            &config_path,
            legacy_config(json!({ "window_size": [2400.0, 1600.0] })),
        )
        .unwrap();
        fs::write(&window_path, "{ \"size\": [12").unwrap();

        let (_, geometry) = GeometryStore::open(&config_path, 2.0);

        assert_eq!(geometry.size, (1200.0, 800.0));
        assert_eq!(GeometryStore::open(&config_path, 1.0).1, geometry);
    }

    /// If the geometry that was taken over cannot be written, the session still
    /// starts with it.
    #[test]
    fn a_migration_that_cannot_be_written_still_yields_the_geometry() {
        let (_dir, config_path, window_path) = config_dir();
        fs::write(
            &config_path,
            legacy_config(json!({ "window_size": [2400.0, 1600.0] })),
        )
        .unwrap();
        fs::create_dir(&window_path).unwrap();

        let (store, geometry) = GeometryStore::open(&config_path, 2.0);

        assert_eq!(geometry.size, (1200.0, 800.0));
        assert!(store.save(geometry).is_err());
    }

    // --- The end of a session ---

    /// Geometry and configuration are saved independently: neither file is touched by
    /// the save of the other one.
    #[test]
    fn saving_the_geometry_and_saving_the_config_do_not_touch_each_other() {
        let (_dir, config_path, window_path) = config_dir();
        let (config_store, config) = ConfigStore::open(config_path.clone());
        let (geometry_store, _) = GeometryStore::open(&config_path, 1.0);

        let config_before = fs::read(&config_path).unwrap();
        geometry_store.save(FINAL_GEOMETRY).unwrap();
        assert_eq!(fs::read(&config_path).unwrap(), config_before);

        let window_before = fs::read(&window_path).unwrap();
        let changed = AppConfig {
            output_filename: "changed.txt".into(),
            ..config
        };
        config_store.snapshot(&changed).save().unwrap();
        config_store.close().unwrap();
        assert_eq!(fs::read(&window_path).unwrap(), window_before);
        assert_eq!(settings::load_config(&config_path).unwrap(), changed);
    }

    /// Quitting must not hang behind a worker that holds the application state during
    /// a large scan/filter: the last saves of a session do not ask for the state.
    #[test]
    fn the_last_saves_of_a_session_do_not_wait_for_the_application_state() {
        let (_dir, config_path, _) = config_dir();
        let (config_store, config) = ConfigStore::open(config_path.clone());
        let (geometry_store, _) = GeometryStore::open(&config_path, 1.0);
        let state = SharedState::new(AppState::new(config, config_store.clone()));

        // A change whose background save has not had its turn, and a busy worker.
        let queued = {
            let mut guard = state.lock();
            guard.config.output_filename = "changed.txt".into();
            guard.config_snapshot()
        };
        let busy = state.lock();
        geometry_store.save(FINAL_GEOMETRY).unwrap();
        config_store.close().unwrap();
        drop(busy);
        queued.save().unwrap();

        let saved = settings::load_config(&config_path).unwrap();
        assert_eq!(saved.output_filename, "changed.txt");
        assert_eq!(saved, state.lock().config);
        assert_eq!(GeometryStore::open(&config_path, 1.0).1, FINAL_GEOMETRY);
    }

    /// The race the ordered path exists for: a command changed the configuration and
    /// its background save has not run yet when the application quits. Whenever that
    /// save gets its turn, it must not put its older content over the final one.
    #[test]
    fn the_last_config_save_cannot_be_overtaken_by_one_that_is_still_queued() {
        let (_dir, config_path, _) = config_dir();
        let (config_store, config) = ConfigStore::open(config_path.clone());
        let state = SharedState::new(AppState::new(config, config_store.clone()));
        let queued = {
            let mut guard = state.lock();
            guard.config.output_filename = "first-change.txt".into();
            guard.config_snapshot()
        };
        let also_queued = {
            let mut guard = state.lock();
            guard.config.output_filename = "second-change.txt".into();
            guard.config_snapshot()
        };

        config_store.close().unwrap();
        queued.save().unwrap();
        also_queued.save().unwrap();

        let saved = settings::load_config(&config_path).unwrap();
        assert_eq!(saved.output_filename, "second-change.txt");
    }
}
