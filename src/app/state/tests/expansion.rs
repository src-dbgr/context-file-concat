use super::*;

// --- Expansion while filtering ---

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}

#[test]
fn test_reveal_expands_what_the_user_has_not_collapsed_under_the_filter() {
    let mut state = AppState::default();
    state.set_filter_texts("main".to_string(), String::new(), String::new());

    state.reveal_directory(Path::new("/p/a"));
    state.collapse_directories(paths(&["/p/a"]));
    state.reveal_directory(Path::new("/p/a"));
    state.reveal_directory(Path::new("/p/b"));

    assert_eq!(state.expanded_dirs, HashSet::from([PathBuf::from("/p/b")]));
}

#[test]
fn test_collapse_without_a_filter_does_not_hold_back_a_later_filter() {
    let mut state = AppState::default();
    state.expand_directories(paths(&["/p/a"]));
    state.collapse_directories(paths(&["/p/a"]));
    assert!(state.expanded_dirs.is_empty());

    state.set_filter_texts(String::new(), "rs".to_string(), String::new());
    state.reveal_directory(Path::new("/p/a"));

    assert!(state.expanded_dirs.contains(Path::new("/p/a")));
}

#[test]
fn test_the_hold_on_a_collapsed_directory_ends_with_the_filters_or_an_expand() {
    let mut state = AppState::default();
    state.set_filter_texts(String::new(), String::new(), "needle".to_string());
    state.collapse_directories(paths(&["/p/a", "/p/b"]));

    // Another filter text is the same session.
    state.set_filter_texts("x".to_string(), String::new(), String::new());
    state.reveal_directory(Path::new("/p/a"));
    assert!(state.expanded_dirs.is_empty());

    // The user opens one of them: it is the host's to reveal again.
    state.expand_directories(paths(&["/p/a"]));
    state.expanded_dirs.clear();
    state.reveal_directory(Path::new("/p/a"));
    state.reveal_directory(Path::new("/p/b"));
    assert_eq!(state.expanded_dirs, HashSet::from([PathBuf::from("/p/a")]));

    // All filters blank (whitespace counts as blank): the session is over.
    state.set_filter_texts(" ".to_string(), String::new(), String::new());
    state.set_filter_texts("x".to_string(), String::new(), String::new());
    state.reveal_directory(Path::new("/p/b"));
    assert!(state.expanded_dirs.contains(Path::new("/p/b")));
}

#[test]
fn test_resetting_the_directory_ends_the_filter_session() {
    let mut state = AppState::default();
    state.set_filter_texts("x".to_string(), String::new(), String::new());
    state.collapse_directories(paths(&["/p/a"]));

    state.reset_directory_state();
    state.set_filter_texts("x".to_string(), String::new(), String::new());
    state.reveal_directory(Path::new("/p/a"));

    assert!(state.expanded_dirs.contains(Path::new("/p/a")));
}

#[test]
fn test_set_filter_texts_reports_a_changed_content_query() {
    let mut state = AppState::default();
    assert!(!state.set_filter_texts("a".to_string(), "rs".to_string(), String::new()));
    assert!(state.set_filter_texts("a".to_string(), "rs".to_string(), "x".to_string()));
    assert!(!state.set_filter_texts("b".to_string(), String::new(), "x".to_string()));
    assert!(state.set_filter_texts("b".to_string(), String::new(), String::new()));
    assert_eq!(state.search_query, "b");
    assert!(state.has_active_filter());
}

#[test]
fn test_changed_filter_texts_show_the_new_list_from_the_top() {
    use crate::app::view_model::Viewport;
    let mut state = AppState::default();
    state.set_filter_texts("a".to_string(), String::new(), String::new());
    let scrolled = Viewport::new(500, 80);
    state.tree_view.set_viewport(scrolled);

    // The same texts again: the UI has not moved.
    state.set_filter_texts("a".to_string(), String::new(), String::new());
    assert_eq!(state.tree_view.viewport(), scrolled);

    for texts in [("ab", "", ""), ("ab", "rs", ""), ("ab", "rs", "needle")] {
        state.tree_view.set_viewport(scrolled);
        state.set_filter_texts(
            texts.0.to_string(),
            texts.1.to_string(),
            texts.2.to_string(),
        );
        assert_eq!(
            state.tree_view.viewport(),
            Viewport::new(0, 80),
            "{texts:?}"
        );
    }
}
