use super::*;

#[tokio::test]
async fn test_newer_content_search_supersedes_older_one() {
    let mut state = AppState::default();
    let older = state.begin_content_search();
    let newer = state.begin_content_search();
    let older_matches = HashSet::from([PathBuf::from("/p/older.txt")]);
    let newer_matches = HashSet::from([PathBuf::from("/p/newer.txt")]);

    // The older search finishes last and must be ignored, whatever the order.
    assert!(state.finish_content_search(&newer, newer_matches.clone()));
    assert!(!state.finish_content_search(&older, older_matches));

    assert_eq!(*state.content_search_results(), newer_matches);
    assert!(
        !state.finish_content_search(&newer, HashSet::new()),
        "A search can only publish once"
    );
    assert_eq!(*state.content_search_results(), newer_matches);
}

#[tokio::test]
async fn test_clear_content_search_discards_running_search() {
    let mut state = AppState::default();
    state.set_content_search_results(HashSet::from([PathBuf::from("/p/old.txt")]));
    let running = state.begin_content_search();

    state.clear_content_search();

    assert!(state.content_search_results().is_empty());
    assert!(!state.finish_content_search(&running, HashSet::from([PathBuf::from("/p/a")])));
    assert!(state.content_search_results().is_empty());
}

#[tokio::test]
async fn test_reset_directory_state_discards_running_content_search() {
    let mut state = AppState::default();
    let running = state.begin_content_search();

    state.reset_directory_state();

    assert!(!state.finish_content_search(&running, HashSet::from([PathBuf::from("/p/a")])));
    assert!(state.content_search_results().is_empty());
}

#[tokio::test]
async fn test_content_search_is_told_to_stop_when_superseded_or_cleared() {
    let mut state = AppState::default();

    let first = state.begin_content_search();
    let first_flag = first.cancel_flag();
    assert!(!first.is_cancelled());

    let second = state.begin_content_search();
    assert!(first.is_cancelled(), "A newer search stops the older one");
    assert!(first_flag.load(Ordering::SeqCst));
    assert!(!second.is_cancelled());

    state.clear_content_search();
    assert!(second.is_cancelled(), "Clearing the query stops the search");

    let third = state.begin_content_search();
    assert!(state.finish_content_search(&third, HashSet::new()));
    assert!(
        !third.is_cancelled(),
        "A search that came to its end was never cancelled"
    );
}

// --- Content filter ---

#[test]
fn test_content_filter_applies_once_a_search_has_answered() {
    let mut state = AppState::default();
    assert_eq!(state.content_filter_matches(), None, "no query");

    state.content_search_query = "needle".to_string();
    let first = state.begin_content_search();
    assert_eq!(state.content_filter_matches(), None, "first search running");

    assert!(state.finish_content_search(&first, HashSet::new()));
    assert_eq!(state.content_filter_matches(), Some(&HashSet::new()));

    // The next search runs: the previous answer applies until it delivers.
    let second = state.begin_content_search();
    assert_eq!(state.content_filter_matches(), Some(&HashSet::new()));
    let found = HashSet::from([PathBuf::from("/p/a.txt")]);
    assert!(state.finish_content_search(&second, found.clone()));
    assert_eq!(state.content_filter_matches(), Some(&found));

    // Clearing the query starts over.
    state.content_search_query.clear();
    state.clear_content_search();
    state.content_search_query = "other".to_string();
    let _third = state.begin_content_search();
    assert_eq!(state.content_filter_matches(), None);
}

/// Matches that are simply there (no search is running) are the filter.
#[test]
fn test_content_filter_uses_the_matches_in_place_when_no_search_runs() {
    let state = AppState {
        content_search_query: "needle".to_string(),
        ..Default::default()
    };
    assert_eq!(state.content_filter_matches(), Some(&HashSet::new()));
}

#[test]
fn test_abandoned_content_search_keeps_the_previous_matches() {
    let mut state = AppState {
        content_search_query: "needle".to_string(),
        ..Default::default()
    };
    let found = HashSet::from([PathBuf::from("/p/a.txt")]);
    let first = state.begin_content_search();
    state.finish_content_search(&first, found.clone());

    let failing = state.begin_content_search();
    let newer = state.begin_content_search();
    assert!(!state.abandon_content_search(&failing), "superseded");
    assert!(state.abandon_content_search(&newer));
    assert!(!state.abandon_content_search(&newer), "only once");

    assert_eq!(*state.content_search_results(), found);
    assert_eq!(state.content_filter_matches(), Some(&found));
}
