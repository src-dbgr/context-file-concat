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
    let unmarked = json!({ "window_size": [2400.0, 1600.0], "window_position": [200.0, -100.0] });
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

    assert_eq!(store.shared.path.as_deref(), Some(window_path.as_path()));
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
        guard.config_mut().output_filename = "changed.txt".into();
        guard.config_snapshot()
    };
    let busy = state.lock();
    geometry_store.save(FINAL_GEOMETRY).unwrap();
    config_store.close().unwrap();
    drop(busy);
    queued.save().unwrap();

    let saved = settings::load_config(&config_path).unwrap();
    assert_eq!(saved.output_filename, "changed.txt");
    assert_eq!(saved, *state.lock().config());
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
        guard.config_mut().output_filename = "first-change.txt".into();
        guard.config_snapshot()
    };
    let also_queued = {
        let mut guard = state.lock();
        guard.config_mut().output_filename = "second-change.txt".into();
        guard.config_snapshot()
    };

    config_store.close().unwrap();
    queued.save().unwrap();
    also_queued.save().unwrap();

    let saved = settings::load_config(&config_path).unwrap();
    assert_eq!(saved.output_filename, "second-change.txt");
}

// --- Saving while the application runs ---

#[test]
fn recording_tells_whether_the_geometry_has_changed() {
    let mut g = geometry();
    assert!(g.record_size((2000.0, 1400.0), 2.0));
    assert!(!g.record_size((2000.0, 1400.0), 2.0), "the same size again");
    assert!(!g.record_size((0.0, 0.0), 2.0), "a size that is ignored");

    assert!(g.record_position((-400.0, 60.0), 2.0));
    assert!(!g.record_position((-400.0, 60.0), 2.0));
    assert!(!g.record_position((-32000.0, -32000.0), 2.0));
    assert!(!g.record_position((10.0, 10.0), f64::NAN));
}

const DELAY: Duration = Duration::from_millis(750);

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

#[test]
fn nothing_is_due_without_a_change() {
    let start = Instant::now();
    let mut debounce = SaveDebounce::new(DELAY);

    assert_eq!(debounce.due_at(), None);
    assert!(!debounce.take_due(start));
    assert!(!debounce.take_due(start + ms(10_000)));
}

#[test]
fn a_change_is_due_once_the_window_has_rested_for_the_delay() {
    let start = Instant::now();
    let mut debounce = SaveDebounce::new(DELAY);

    debounce.changed(start);

    assert_eq!(debounce.due_at(), Some(start + DELAY));
    assert!(!debounce.take_due(start));
    assert!(!debounce.take_due(start + ms(749)));
    assert!(debounce.take_due(start + ms(750)));
}

/// A drag is a run of changes: the save waits for the last one, and all of them
/// are one save.
#[test]
fn changes_in_quick_succession_are_saved_once_after_the_last_one() {
    let start = Instant::now();
    let mut debounce = SaveDebounce::new(DELAY);

    for step in 0..100 {
        debounce.changed(start + ms(step * 16));
        assert!(
            !debounce.take_due(start + ms(step * 16 + 15)),
            "step {step}"
        );
    }
    let last_change = start + ms(99 * 16);

    assert_eq!(debounce.due_at(), Some(last_change + DELAY));
    assert!(!debounce.take_due(last_change + ms(749)));
    assert!(debounce.take_due(last_change + ms(751)));
    assert!(!debounce.take_due(last_change + ms(752)), "saved only once");
    assert_eq!(debounce.due_at(), None);
}

#[test]
fn a_change_after_a_save_is_due_again() {
    let start = Instant::now();
    let mut debounce = SaveDebounce::new(DELAY);
    debounce.changed(start);
    assert!(debounce.take_due(start + ms(1000)));

    debounce.changed(start + ms(2000));

    assert!(!debounce.take_due(start + ms(2100)));
    assert!(debounce.take_due(start + ms(2750)));
}

/// At exit the event loop saves by itself; a save that was pending is dropped.
#[test]
fn a_cancelled_save_is_not_due() {
    let start = Instant::now();
    let mut debounce = SaveDebounce::new(DELAY);
    debounce.changed(start);

    debounce.cancel();

    assert_eq!(debounce.due_at(), None);
    assert!(!debounce.take_due(start + ms(10_000)));
}

/// What the event loop does with the two: the geometry that is saved when the window
/// rests is on disk before the application ends, so a crash does not cost it.
#[test]
fn the_geometry_is_on_disk_once_the_window_has_rested() {
    let (_dir, config_path, _) = config_dir();
    let (store, mut geometry) = GeometryStore::open(&config_path, 2.0);
    let mut debounce = SaveDebounce::new(DELAY);
    let start = Instant::now();

    if geometry.record_size((2000.0, 1400.0), 2.0) {
        debounce.changed(start);
    }
    assert!(debounce.take_due(start + DELAY));
    store.save(geometry).unwrap();

    // No save at exit: the next start still finds the geometry.
    assert_eq!(GeometryStore::open(&config_path, 2.0).1, geometry);
}

#[test]
fn a_geometry_the_store_has_written_is_not_written_again() {
    let (_dir, config_path, window_path) = config_dir();
    let (store, _) = GeometryStore::open(&config_path, 1.0);
    store.save(FINAL_GEOMETRY).unwrap();

    // Seen by what is on disk afterwards: nothing has replaced the file.
    fs::write(&window_path, "untouched").unwrap();
    store.save(FINAL_GEOMETRY).unwrap();
    store.clone().save(FINAL_GEOMETRY).unwrap();
    assert_eq!(fs::read_to_string(&window_path).unwrap(), "untouched");

    store.save(geometry()).unwrap();
    assert_eq!(GeometryStore::open(&config_path, 1.0).1, geometry());
}

/// A background save that gets its turn late (here: after a newer geometry has been
/// handed to the store, as by the save at exit) must not put its older geometry on
/// disk.
#[test]
fn a_save_that_runs_late_does_not_overwrite_a_newer_geometry() {
    let (_dir, config_path, _) = config_dir();
    let (store, _) = GeometryStore::open(&config_path, 1.0);

    // A writer holds the store exactly as `save` does while it writes the file.
    let writing = lock(&store.shared.written);
    let older = {
        let store = store.clone();
        std::thread::spawn(move || store.save(geometry()))
    };
    while lock(&store.shared.latest).is_none() {
        std::thread::yield_now();
    }
    let newer = {
        let store = store.clone();
        std::thread::spawn(move || store.save(FINAL_GEOMETRY))
    };
    while *lock(&store.shared.latest) != Some(FINAL_GEOMETRY) {
        std::thread::yield_now();
    }
    drop(writing);

    older.join().unwrap().unwrap();
    newer.join().unwrap().unwrap();
    assert_eq!(GeometryStore::open(&config_path, 1.0).1, FINAL_GEOMETRY);
}
