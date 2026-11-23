use super::*;

// --- Preview requests ---

#[test]
fn test_only_the_most_recent_preview_request_may_show_its_content() {
    let mut state = AppState::default();
    let first = state.begin_preview(PathBuf::from("/project/a.rs"));
    let second = state.begin_preview(PathBuf::from("/project/b.rs"));

    assert!(!state.finish_preview(&first), "Superseded by the second");
    assert_eq!(
        state.previewed_file_path,
        Some(PathBuf::from("/project/b.rs"))
    );
    assert!(state.finish_preview(&second));
    assert!(!state.finish_preview(&second), "A read ends only once");
}

#[test]
fn test_closing_the_preview_invalidates_the_read_under_way() {
    let mut state = AppState::default();
    let token = state.begin_preview(PathBuf::from("/project/a.rs"));

    state.clear_preview();

    assert_eq!(state.previewed_file_path, None);
    assert!(!state.finish_preview(&token));
}

#[test]
fn test_reset_directory_state_invalidates_the_preview_read_under_way() {
    let mut state = AppState::default();
    let token = state.begin_preview(PathBuf::from("/project/a.rs"));

    state.reset_directory_state();

    assert!(!state.finish_preview(&token));
}
