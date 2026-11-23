//! Tests for what is a row: only expanded directories show their children, and
//! everything the UI needs for the rest is part of the collapsed row or a counter.
//!
//! The rows are looked at in nested form (see `reference::nest`), which states the
//! rule directly: a collapsed directory has no children.

use super::reference::{self, snapshot, BuildTreeArgs, Snapshot, TreeNode};
use crate::app::filtering;
use crate::app::filtering::auto_expand_for_matches;
use crate::app::selection::SelectionState;
use crate::app::state::AppState;
use crate::config::AppConfig;
use crate::core::FileItem;
use std::collections::HashSet;
use std::path::PathBuf;

fn test_state(paths: &[(&str, bool)]) -> AppState {
    let mut state = AppState::default();
    *state.config_mut() = AppConfig {
        ignore_patterns: HashSet::new(),
        remove_empty_directories: false,
        ..Default::default()
    };
    state.current_path = "/root".to_string();
    state.set_full_file_list(
        paths
            .iter()
            .map(|&(path, is_directory)| item(path, is_directory))
            .collect(),
    );
    state.loaded_dirs = state
        .full_file_list()
        .iter()
        .filter(|item| item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    state.is_fully_scanned = true;
    filtering::apply_filters(&mut state);
    state
}

fn item(path: &str, is_directory: bool) -> FileItem {
    FileItem {
        path: PathBuf::from(path),
        is_directory,
        size: if is_directory { 0 } else { 100 },
        ..Default::default()
    }
}

/// A project with two levels of directories below the root.
fn project() -> AppState {
    test_state(&[
        ("/root/src", true),
        ("/root/src/core", true),
        ("/root/src/core/a.rs", false),
        ("/root/src/core/b.rs", false),
        ("/root/src/main.rs", false),
        ("/root/docs", true),
        ("/root/docs/guide.md", false),
        ("/root/empty", true),
        ("/root/readme.md", false),
    ])
}

fn expand(state: &mut AppState, paths: &[&str]) {
    state.expanded_dirs.extend(paths.iter().map(PathBuf::from));
}

fn select(state: &mut AppState, paths: &[&str]) {
    state.selected_files = paths.iter().map(PathBuf::from).collect();
}

/// Like [`select`], but file by file, the way clicks change the selection.
fn click(state: &mut AppState, path: &str) {
    let path = PathBuf::from(path);
    if !state.selected_files.remove(&path) {
        state.selected_files.insert(path);
    }
}

fn find<'a>(nodes: &'a [TreeNode], path: &str) -> Option<&'a TreeNode> {
    nodes.iter().find_map(|node| {
        if node.path.as_path() == std::path::Path::new(path) {
            Some(node)
        } else {
            find(&node.children, path)
        }
    })
}

fn node<'a>(ui_state: &'a Snapshot, path: &str) -> &'a TreeNode {
    find(&ui_state.tree, path).unwrap_or_else(|| panic!("{path} is not part of the snapshot"))
}

fn names(nodes: &[TreeNode]) -> Vec<&str> {
    nodes.iter().map(|node| node.name.as_str()).collect()
}

fn node_count(nodes: &[TreeNode]) -> usize {
    nodes
        .iter()
        .map(|node| 1 + node_count(&node.children))
        .sum()
}

#[test]
fn collapsed_directories_are_sent_without_children() {
    let ui_state = snapshot(&project());

    assert_eq!(names(&ui_state.tree), ["docs", "empty", "src", "readme.md"]);
    for dir in ["/root/src", "/root/docs", "/root/empty"] {
        let dir = node(&ui_state, dir);
        assert!(!dir.is_expanded);
        assert!(dir.children.is_empty());
    }
    assert_eq!(node_count(&ui_state.tree), 4);
}

#[test]
fn has_children_tells_collapsed_directories_with_content_from_empty_ones() {
    let mut state = project();
    let collapsed = snapshot(&state);

    assert!(node(&collapsed, "/root/src").has_children);
    assert!(node(&collapsed, "/root/docs").has_children);
    assert!(!node(&collapsed, "/root/empty").has_children);
    assert!(!node(&collapsed, "/root/readme.md").has_children);

    // Expansion does not change the flag.
    expand(&mut state, &["/root/src", "/root/empty"]);
    let expanded = snapshot(&state);
    assert!(node(&expanded, "/root/src").has_children);
    assert!(!node(&expanded, "/root/empty").has_children);
    assert!(node(&expanded, "/root/src/core").has_children);
    assert!(!node(&expanded, "/root/src/main.rs").has_children);
}

#[test]
fn collapsed_directory_reports_selection_of_its_hidden_descendants() {
    let mut state = project();
    let selection_of_src = |state: &AppState| {
        let ui_state = snapshot(state);
        let src = node(&ui_state, "/root/src");
        assert!(src.children.is_empty(), "src must stay collapsed");
        src.selection_state
    };

    assert_eq!(selection_of_src(&state), SelectionState::None);

    // A file two levels below the collapsed directory.
    select(&mut state, &["/root/src/core/a.rs"]);
    assert_eq!(selection_of_src(&state), SelectionState::Partial);

    select(
        &mut state,
        &[
            "/root/src/core/a.rs",
            "/root/src/core/b.rs",
            "/root/src/main.rs",
        ],
    );
    assert_eq!(selection_of_src(&state), SelectionState::Full);

    // Selections elsewhere do not leak in.
    select(&mut state, &["/root/docs/guide.md"]);
    assert_eq!(selection_of_src(&state), SelectionState::None);
    assert_eq!(
        node(&snapshot(&state), "/root/docs").selection_state,
        SelectionState::Full
    );
}

#[test]
fn collapsed_directory_inside_an_expanded_one_keeps_its_selection_state() {
    let mut state = project();
    expand(&mut state, &["/root/src"]);
    select(&mut state, &["/root/src/core/a.rs"]);

    let ui_state = snapshot(&state);

    assert_eq!(
        node(&ui_state, "/root/src").selection_state,
        SelectionState::Partial
    );
    let core = node(&ui_state, "/root/src/core");
    assert_eq!(core.selection_state, SelectionState::Partial);
    assert!(core.children.is_empty() && core.has_children);
    assert_eq!(
        node(&ui_state, "/root/src/main.rs").selection_state,
        SelectionState::None
    );
}

#[test]
fn expanded_chain_is_complete() {
    let mut state = project();
    expand(&mut state, &["/root/src", "/root/src/core"]);

    let ui_state = snapshot(&state);

    let src = node(&ui_state, "/root/src");
    assert!(src.is_expanded);
    assert_eq!(names(&src.children), ["core", "main.rs"]);
    let core = node(&ui_state, "/root/src/core");
    assert!(core.is_expanded);
    assert_eq!(names(&core.children), ["a.rs", "b.rs"]);
    // The siblings of the chain stay pruned.
    assert!(node(&ui_state, "/root/docs").children.is_empty());
    assert_eq!(node_count(&ui_state.tree), 8);
}

#[test]
fn fully_expanded_snapshot_is_the_complete_tree() {
    let mut state = project();
    expand(
        &mut state,
        &["/root/src", "/root/src/core", "/root/docs", "/root/empty"],
    );

    let ui_state = snapshot(&state);

    let complete = reference::build_tree_nodes(BuildTreeArgs {
        items: state.filtered_file_list(),
        root_path: &PathBuf::from(&state.current_path),
        selected: &state.selected_files,
        expanded: &state.expanded_dirs,
        loaded_dirs: &state.loaded_dirs,
        content_search_matches: state.content_search_results(),
        filename_query: &state.search_query,
        extension_filter: &state.extension_filter,
        case_sensitive: state.config().case_sensitive_search,
        previewed_path: &state.previewed_file_path,
    });
    assert_eq!(ui_state.tree, complete);
    assert_eq!(node_count(&ui_state.tree), 9);
}

#[test]
fn expanded_directory_below_a_collapsed_one_is_not_sent() {
    let mut state = project();
    expand(&mut state, &["/root/src/core"]);

    let ui_state = snapshot(&state);

    assert!(node(&ui_state, "/root/src").children.is_empty());
    assert!(find(&ui_state.tree, "/root/src/core").is_none());

    // Expanding the parent reveals the remembered expansion of the child.
    expand(&mut state, &["/root/src"]);
    let ui_state = snapshot(&state);
    assert_eq!(
        names(&node(&ui_state, "/root/src/core").children),
        ["a.rs", "b.rs"]
    );
}

#[test]
fn unloaded_directory_is_marked_for_lazy_loading() {
    let mut state = project();
    state.loaded_dirs.remove(&PathBuf::from("/root/docs"));

    let ui_state = snapshot(&state);

    assert!(!node(&ui_state, "/root/docs").children_loaded);
    assert!(node(&ui_state, "/root/src").children_loaded);
    assert!(node(&ui_state, "/root/readme.md").children_loaded);
}

#[test]
fn counters_describe_the_complete_tree_regardless_of_expansion() {
    let mut state = project();
    let collapsed = snapshot(&state);

    assert_eq!(collapsed.tree_files_count, 5);
    assert_eq!(collapsed.tree_dirs_count, 4);
    assert_eq!(collapsed.visible_files_count, 9);

    expand(&mut state, &["/root/src", "/root/src/core"]);
    let expanded = snapshot(&state);
    assert_eq!(expanded.tree_files_count, 5);
    assert_eq!(expanded.tree_dirs_count, 4);
}

#[test]
fn counters_follow_the_filters() {
    let mut state = project();
    state.search_query = "a.rs".to_string();
    filtering::apply_filters(&mut state);

    let ui_state = snapshot(&state);

    // /root/src/core/a.rs and its two ancestors.
    assert_eq!(ui_state.tree_files_count, 1);
    assert_eq!(ui_state.tree_dirs_count, 2);
}

#[test]
fn counters_are_zero_when_the_tree_is_suppressed() {
    let mut state = project();
    // A content search without results shows an empty tree.
    state.content_search_query = "needle".to_string();

    let ui_state = snapshot(&state);

    assert!(ui_state.tree.is_empty());
    assert_eq!(ui_state.tree_files_count, 0);
    assert_eq!(ui_state.tree_dirs_count, 0);
}

#[test]
fn search_auto_expansion_makes_matches_part_of_the_snapshot() {
    let mut state = project();
    assert!(find(&snapshot(&state).tree, "/root/src/core/b.rs").is_none());

    state.search_query = "b.rs".to_string();
    filtering::apply_filters(&mut state);
    auto_expand_for_matches(&mut state);

    let ui_state = snapshot(&state);
    let hit = node(&ui_state, "/root/src/core/b.rs");
    assert!(hit.is_match);
    assert!(node(&ui_state, "/root/src").is_expanded);
    assert!(node(&ui_state, "/root/src/core").is_expanded);
}

#[test]
fn selection_states_follow_single_clicks_without_counting_the_tree_again() {
    let mut state = project();
    expand(&mut state, &["/root/src"]);
    let selection_of = |state: &AppState, path: &str| node(&snapshot(state), path).selection_state;

    assert_eq!(selection_of(&state, "/root/src"), SelectionState::None);
    let counted = state.selection_count_count();

    click(&mut state, "/root/src/core/a.rs");
    assert_eq!(selection_of(&state, "/root/src"), SelectionState::Partial);
    assert_eq!(
        selection_of(&state, "/root/src/core"),
        SelectionState::Partial
    );
    assert_eq!(selection_of(&state, "/root/docs"), SelectionState::None);

    click(&mut state, "/root/src/core/b.rs");
    assert_eq!(selection_of(&state, "/root/src/core"), SelectionState::Full);
    click(&mut state, "/root/src/main.rs");
    assert_eq!(selection_of(&state, "/root/src"), SelectionState::Full);

    click(&mut state, "/root/src/core/a.rs");
    assert_eq!(selection_of(&state, "/root/src"), SelectionState::Partial);
    assert_eq!(
        selection_of(&state, "/root/src/core"),
        SelectionState::Partial
    );

    // A path that is not listed, and a directory, are not files of the tree.
    click(&mut state, "/root/src/unlisted.rs");
    click(&mut state, "/root/docs");
    assert_eq!(selection_of(&state, "/root/src"), SelectionState::Partial);
    assert_eq!(selection_of(&state, "/root/docs"), SelectionState::None);

    assert_eq!(
        state.selection_count_count(),
        counted,
        "single changes are followed, not recounted"
    );

    // Replacing the selection wholesale, or the list, counts again.
    state.selected_files.clear();
    assert_eq!(selection_of(&state, "/root/src"), SelectionState::None);
    assert_eq!(state.selection_count_count(), counted + 1);
    click(&mut state, "/root/src/main.rs");
    state.search_query = "main".to_string();
    filtering::apply_filters(&mut state);
    assert_eq!(selection_of(&state, "/root/src"), SelectionState::Full);
    assert_eq!(state.selection_count_count(), counted + 2);
}
