use super::*;

#[tokio::test]
async fn test_a_lazy_load_begun_during_a_scan_belongs_to_the_list_of_that_scan() {
    let mut state = AppState::default();
    let before = state.begin_lazy_load();

    let _token = state.begin_scan();
    let during = state.begin_lazy_load();

    assert!(!state.is_current_lazy_load(&before));
    assert!(state.is_current_lazy_load(&during));
    assert!(!during.is_cancelled());
}

#[tokio::test]
async fn test_lazy_load_token_goes_stale_when_file_list_is_replaced() {
    let mut state = AppState::default();

    let token = state.begin_lazy_load();
    assert!(state.is_current_lazy_load(&token));
    let scan = state.begin_scan();
    assert!(!state.is_current_lazy_load(&token), "stale after new scan");
    state.finish_scan(&scan);

    let token = state.begin_lazy_load();
    state.reset_directory_state();
    assert!(!state.is_current_lazy_load(&token), "stale after reset");
}

#[tokio::test]
async fn test_lazy_load_is_cancelled_when_a_new_scan_begins() {
    let mut state = AppState::default();
    let lazy = state.begin_lazy_load();
    let scanner_flag = lazy.cancel_flag();
    assert!(!lazy.is_cancelled());

    let scan = state.begin_scan();

    assert!(lazy.is_cancelled(), "Superseded lazy load must stop");
    assert!(scanner_flag.load(Ordering::SeqCst));
    assert!(
        !scan.is_cancelled(),
        "The new scan itself must keep running"
    );
}

#[tokio::test]
async fn test_lazy_load_is_cancelled_when_directory_is_reset() {
    let mut state = AppState::default();
    let lazy = state.begin_lazy_load();

    state.reset_directory_state();

    assert!(
        lazy.is_cancelled(),
        "Lazy load of a dropped directory must stop"
    );
}

#[tokio::test]
async fn test_lazy_load_started_for_the_new_file_list_is_not_cancelled() {
    let mut state = AppState::default();
    let outdated = state.begin_lazy_load();
    let scan = state.begin_scan();
    state.finish_scan(&scan);

    let current = state.begin_lazy_load();
    let sibling = state.begin_lazy_load();

    assert!(outdated.is_cancelled());
    assert!(!current.is_cancelled());
    assert!(state.is_current_lazy_load(&current));

    // Loads of the same file list are replaced together.
    state.reset_directory_state();
    assert!(current.is_cancelled());
    assert!(sibling.is_cancelled());
}

#[tokio::test]
async fn test_cancelling_a_scan_leaves_the_file_list_and_its_lazy_loads_alone() {
    let mut state = AppState::default();
    let scan = state.begin_scan();
    state.finish_scan(&scan);
    let lazy = state.begin_lazy_load();

    state.cancel_current_scan();
    state.cancel_current_generation();

    assert!(!lazy.is_cancelled());
    assert!(state.is_current_lazy_load(&lazy));
}

#[tokio::test]
async fn test_apply_ignore_patterns_invalidates_pending_lazy_loads() {
    let mut state = AppState::default();
    state.config.ignore_patterns.clear();
    state.current_path = "/project".to_string();
    let token = state.begin_lazy_load();

    // Without patterns nothing changes, so the lazy load stays valid.
    state.apply_ignore_patterns();
    assert!(state.is_current_lazy_load(&token));

    state.config.ignore_patterns.insert("target/".to_string());
    state.apply_ignore_patterns();

    assert!(!state.is_current_lazy_load(&token));
    assert!(token.is_cancelled(), "The lazy scan must be told to stop");
    let fresh = state.begin_lazy_load();
    assert!(state.is_current_lazy_load(&fresh) && !fresh.is_cancelled());
}
