use super::*;

// --- Configuration ---

#[test]
fn test_default_state_has_the_default_config_and_is_not_bound_to_a_file() {
    let state = AppState::default();

    assert_eq!(state.config, AppConfig::default());
    assert_eq!(state.config_store().path(), None);
}

#[test]
fn test_config_snapshots_are_taken_from_the_store_of_the_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let (store, config) = ConfigStore::open(path.clone());
    let mut state = AppState::new(config, store);

    state.config.output_filename = "older.txt".to_string();
    let older = state.config_snapshot();
    state.config.output_filename = "newer.txt".to_string();
    let newer = state.config_snapshot();
    newer.save().unwrap();
    older.save().unwrap();

    let saved = crate::config::settings::load_config(&path).unwrap();
    assert_eq!(saved.output_filename, "newer.txt");
    assert_eq!(state.config_store().latest(), state.config);
}

/// The one way the configuration is changed hands out the snapshot to save, so no
/// change can be made without one.
#[test]
fn test_edit_config_hands_out_a_snapshot_of_the_change() {
    let mut state = AppState::default();

    let snapshot = state
        .edit_config(|config| config.output_filename = "changed.txt".to_string())
        .expect("a change is to be saved");

    assert_eq!(state.config().output_filename, "changed.txt");
    assert_eq!(snapshot.config(), state.config());
    assert_eq!(state.config_store().latest(), *state.config());
}

/// An edit that leaves the configuration as it is has nothing to save.
#[test]
fn test_edit_config_without_a_change_hands_out_nothing() {
    let mut state = AppState::default();
    let before = state.config().clone();

    assert!(state.edit_config(|_| {}).is_none());
    let same = before.output_filename.clone();
    assert!(state
        .edit_config(|config| config.output_filename = same)
        .is_none());

    assert_eq!(*state.config(), before);
}
