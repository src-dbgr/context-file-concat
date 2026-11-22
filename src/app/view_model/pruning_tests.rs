//! Tests for the pruned snapshot: only expanded directories carry their children, and
//! everything the UI needs for the rest is part of the collapsed node or a counter.

use super::reference::{self, BuildTreeArgs};
use super::{auto_expand_for_matches, generate_ui_state, TreeNode, UiState};
use crate::app::filtering;
use crate::app::state::AppState;
use crate::config::AppConfig;
use crate::core::FileItem;
use std::collections::HashSet;
use std::path::PathBuf;

fn test_state(paths: &[(&str, bool)]) -> AppState {
    let mut state = AppState::default();
    state.config = AppConfig {
        ignore_patterns: HashSet::new(),
        remove_empty_directories: false,
        ..Default::default()
    };
    state.current_path = "/root".to_string();
    state.full_file_list = paths
        .iter()
        .map(|&(path, is_directory)| item(path, is_directory))
        .collect();
    state.loaded_dirs = state
        .full_file_list
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

fn find<'a>(nodes: &'a [TreeNode], path: &str) -> Option<&'a TreeNode> {
    nodes.iter().find_map(|node| {
        if node.path.as_path() == std::path::Path::new(path) {
            Some(node)
        } else {
            find(&node.children, path)
        }
    })
}

fn node<'a>(ui_state: &'a UiState, path: &str) -> &'a TreeNode {
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
    let ui_state = generate_ui_state(&project());

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
    let collapsed = generate_ui_state(&state);

    assert!(node(&collapsed, "/root/src").has_children);
    assert!(node(&collapsed, "/root/docs").has_children);
    assert!(!node(&collapsed, "/root/empty").has_children);
    assert!(!node(&collapsed, "/root/readme.md").has_children);

    // Expansion does not change the flag.
    expand(&mut state, &["/root/src", "/root/empty"]);
    let expanded = generate_ui_state(&state);
    assert!(node(&expanded, "/root/src").has_children);
    assert!(!node(&expanded, "/root/empty").has_children);
    assert!(node(&expanded, "/root/src/core").has_children);
    assert!(!node(&expanded, "/root/src/main.rs").has_children);
}

#[test]
fn collapsed_directory_reports_selection_of_its_hidden_descendants() {
    let mut state = project();
    let selection_of_src = |state: &AppState| {
        let ui_state = generate_ui_state(state);
        let src = node(&ui_state, "/root/src");
        assert!(src.children.is_empty(), "src must stay collapsed");
        src.selection_state.clone()
    };

    assert_eq!(selection_of_src(&state), "none");

    // A file two levels below the collapsed directory.
    select(&mut state, &["/root/src/core/a.rs"]);
    assert_eq!(selection_of_src(&state), "partial");

    select(
        &mut state,
        &[
            "/root/src/core/a.rs",
            "/root/src/core/b.rs",
            "/root/src/main.rs",
        ],
    );
    assert_eq!(selection_of_src(&state), "full");

    // Selections elsewhere do not leak in.
    select(&mut state, &["/root/docs/guide.md"]);
    assert_eq!(selection_of_src(&state), "none");
    assert_eq!(
        node(&generate_ui_state(&state), "/root/docs").selection_state,
        "full"
    );
}

#[test]
fn collapsed_directory_inside_an_expanded_one_keeps_its_selection_state() {
    let mut state = project();
    expand(&mut state, &["/root/src"]);
    select(&mut state, &["/root/src/core/a.rs"]);

    let ui_state = generate_ui_state(&state);

    assert_eq!(node(&ui_state, "/root/src").selection_state, "partial");
    let core = node(&ui_state, "/root/src/core");
    assert_eq!(core.selection_state, "partial");
    assert!(core.children.is_empty() && core.has_children);
    assert_eq!(node(&ui_state, "/root/src/main.rs").selection_state, "none");
}

#[test]
fn expanded_chain_is_complete() {
    let mut state = project();
    expand(&mut state, &["/root/src", "/root/src/core"]);

    let ui_state = generate_ui_state(&state);

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

    let ui_state = generate_ui_state(&state);

    let complete = reference::build_tree_nodes(BuildTreeArgs {
        items: state.filtered_file_list(),
        root_path: &PathBuf::from(&state.current_path),
        selected: &state.selected_files,
        expanded: &state.expanded_dirs,
        loaded_dirs: &state.loaded_dirs,
        content_search_matches: &state.content_search_results,
        filename_query: &state.search_query,
        extension_filter: &state.extension_filter,
        case_sensitive: state.config.case_sensitive_search,
        previewed_path: &state.previewed_file_path,
    });
    assert_eq!(ui_state.tree, complete);
    assert_eq!(node_count(&ui_state.tree), 9);
}

#[test]
fn expanded_directory_below_a_collapsed_one_is_not_sent() {
    let mut state = project();
    expand(&mut state, &["/root/src/core"]);

    let ui_state = generate_ui_state(&state);

    assert!(node(&ui_state, "/root/src").children.is_empty());
    assert!(find(&ui_state.tree, "/root/src/core").is_none());

    // Expanding the parent reveals the remembered expansion of the child.
    expand(&mut state, &["/root/src"]);
    let ui_state = generate_ui_state(&state);
    assert_eq!(
        names(&node(&ui_state, "/root/src/core").children),
        ["a.rs", "b.rs"]
    );
}

#[test]
fn unloaded_directory_is_marked_for_lazy_loading() {
    let mut state = project();
    state.loaded_dirs.remove(&PathBuf::from("/root/docs"));

    let ui_state = generate_ui_state(&state);

    assert!(!node(&ui_state, "/root/docs").children_loaded);
    assert!(node(&ui_state, "/root/src").children_loaded);
    assert!(node(&ui_state, "/root/readme.md").children_loaded);
}

#[test]
fn counters_describe_the_complete_tree_regardless_of_expansion() {
    let mut state = project();
    let collapsed = generate_ui_state(&state);

    assert_eq!(collapsed.tree_files_count, 5);
    assert_eq!(collapsed.tree_dirs_count, 4);
    assert_eq!(collapsed.visible_files_count, 9);

    expand(&mut state, &["/root/src", "/root/src/core"]);
    let expanded = generate_ui_state(&state);
    assert_eq!(expanded.tree_files_count, 5);
    assert_eq!(expanded.tree_dirs_count, 4);
}

#[test]
fn counters_follow_the_filters() {
    let mut state = project();
    state.search_query = "a.rs".to_string();
    filtering::apply_filters(&mut state);

    let ui_state = generate_ui_state(&state);

    // /root/src/core/a.rs and its two ancestors.
    assert_eq!(ui_state.tree_files_count, 1);
    assert_eq!(ui_state.tree_dirs_count, 2);
}

#[test]
fn counters_are_zero_when_the_tree_is_suppressed() {
    let mut state = project();
    // A content search without results shows an empty tree.
    state.content_search_query = "needle".to_string();

    let ui_state = generate_ui_state(&state);

    assert!(ui_state.tree.is_empty());
    assert_eq!(ui_state.tree_files_count, 0);
    assert_eq!(ui_state.tree_dirs_count, 0);
}

#[test]
fn search_auto_expansion_makes_matches_part_of_the_snapshot() {
    let mut state = project();
    assert!(find(&generate_ui_state(&state).tree, "/root/src/core/b.rs").is_none());

    state.search_query = "b.rs".to_string();
    filtering::apply_filters(&mut state);
    auto_expand_for_matches(&mut state);

    let ui_state = generate_ui_state(&state);
    let hit = node(&ui_state, "/root/src/core/b.rs");
    assert!(hit.is_match);
    assert!(node(&ui_state, "/root/src").is_expanded);
    assert!(node(&ui_state, "/root/src/core").is_expanded);
}

// --- Payload size --------------------------------------------------------------

/// 50 modules with 10 packages of 100 files each: 50,000 files in 550 directories.
fn large_project() -> AppState {
    let mut paths: Vec<(String, bool)> = Vec::new();
    for module in 0..50 {
        let module_path = format!("/root/module_{module:02}");
        paths.push((module_path.clone(), true));
        for package in 0..10 {
            let package_path = format!("{module_path}/package_{package}");
            paths.push((package_path.clone(), true));
            for file in 0..100 {
                paths.push((format!("{package_path}/source_file_{file:03}.rs"), false));
            }
        }
    }
    let borrowed: Vec<(&str, bool)> = paths.iter().map(|(p, d)| (p.as_str(), *d)).collect();
    test_state(&borrowed)
}

fn payload_bytes(state: &AppState) -> usize {
    serde_json::to_string(&generate_ui_state(state))
        .expect("UiState is serializable")
        .len()
}

/// Guards the point of pruning and prints the numbers (`cargo test payload -- --nocapture`).
#[test]
fn payload_of_a_large_project_is_proportional_to_what_is_expanded() {
    let mut state = large_project();

    let collapsed = payload_bytes(&state);

    expand(
        &mut state,
        &["/root/module_07", "/root/module_07/package_3"],
    );
    let one_expanded = payload_bytes(&state);

    // What used to be sent on every update: the complete tree.
    state.expanded_dirs = state
        .filtered_file_list()
        .iter()
        .filter(|item| item.is_directory)
        .map(|item| item.path.clone())
        .collect();
    let complete = payload_bytes(&state);

    println!(
        "UiState payload for 50,000 files: complete tree {complete} B, \
         all collapsed {collapsed} B, one directory chain expanded {one_expanded} B"
    );
    let ui_state = generate_ui_state(&state);
    assert_eq!(ui_state.tree_files_count, 50_000);
    assert_eq!(ui_state.tree_dirs_count, 550);
    assert!(complete > 10_000_000);
    assert!(collapsed < 50_000);
    assert!(one_expanded < 100_000);
}
