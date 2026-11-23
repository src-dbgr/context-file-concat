//! Tests for the cached tree: equivalence with the original algorithm and invalidation.
//!
//! The original algorithm builds the complete, nested tree. The rows the UI can show
//! are that tree with the subtrees of collapsed directories cut off, listed top to
//! bottom. So the oracle is the reference output pruned by exactly this rule, and it
//! is compared with the rows put back into nested form (`reference::nest`).

use super::reference::{self, snapshot, snapshot_tree, BuildTreeArgs, TreeNode};
use super::{generate_ui_state, rows};
use crate::app::filtering;
use crate::app::selection::{self, SelectionState};
use crate::app::state::AppState;
use crate::config::AppConfig;
use crate::core::FileItem;
use proptest::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;

fn test_state() -> AppState {
    let mut state = AppState::default();
    *state.config_mut() = AppConfig {
        ignore_patterns: HashSet::new(),
        remove_empty_directories: false,
        ..Default::default()
    };
    state.current_path = "/root".to_string();
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

/// The tree as the original, uncached algorithm builds it for the given state.
fn reference_tree(state: &AppState) -> Vec<TreeNode> {
    reference::build_tree_nodes(BuildTreeArgs {
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
    })
}

/// Cuts off everything below nodes that are not expanded.
fn prune_collapsed(nodes: Vec<TreeNode>) -> Vec<TreeNode> {
    nodes
        .into_iter()
        .map(|mut node| {
            node.children = if node.is_expanded {
                prune_collapsed(std::mem::take(&mut node.children))
            } else {
                Vec::new()
            };
            node
        })
        .collect()
}

/// What the UI is expected to receive: the reference tree, pruned to the visible part.
fn expected_tree(state: &AppState) -> Vec<TreeNode> {
    prune_collapsed(reference_tree(state))
}

/// The number of (files, directories) in a complete tree.
fn count_nodes(nodes: &[TreeNode]) -> (usize, usize) {
    nodes.iter().fold((0, 0), |(files, dirs), node| {
        let (child_files, child_dirs) = count_nodes(&node.children);
        if node.is_directory {
            (files + child_files, dirs + 1 + child_dirs)
        } else {
            (files + child_files + 1, dirs + child_dirs)
        }
    })
}

/// `true` if no node below a collapsed node is part of the tree.
fn is_pruned(nodes: &[TreeNode]) -> bool {
    nodes
        .iter()
        .all(|node| (node.is_expanded || node.children.is_empty()) && is_pruned(&node.children))
}

fn builds(state: &AppState) -> usize {
    state.file_tree_build_count()
}

// --- Equivalence -------------------------------------------------------------------

/// A path to act on: usually one of the currently listed items, sometimes an unrelated one.
#[derive(Debug, Clone)]
enum Target {
    Listed(prop::sample::Index),
    Unlisted(PathBuf),
}

impl Target {
    fn resolve(self, state: &AppState) -> PathBuf {
        let items = state.filtered_file_list();
        match self {
            Target::Listed(index) if !items.is_empty() => {
                items[index.index(items.len())].path.clone()
            }
            Target::Listed(_) => PathBuf::from("/root"),
            Target::Unlisted(path) => path,
        }
    }
}

/// One user-visible change to the state between two UI updates.
#[derive(Debug, Clone)]
enum Op {
    SetList(Vec<FileItem>),
    SetRoot(&'static str),
    ToggleSelected(Target),
    SelectAllFiles,
    DeselectAll,
    ToggleExpanded(Target),
    ToggleLoaded(Target),
    ToggleContentMatch(Target),
    Preview(Option<Target>),
    FilenameQuery(&'static str),
    ExtensionFilter(&'static str),
    CaseSensitive(bool),
}

/// Paths from a small pool so that parents, siblings, duplicates and orphans all occur.
fn path_strategy() -> impl Strategy<Value = PathBuf> {
    let segment = prop::sample::select(vec![
        "a",
        "b",
        "C",
        "src",
        "Main.RS",
        "main.rs",
        "lib.rs",
        "notes.txt",
        "Makefile",
        "ä.md",
    ]);
    (
        prop::sample::select(vec!["/root", "/root", "/root", "/other"]),
        prop::collection::vec(segment, 0..4),
    )
        .prop_map(|(base, segments)| {
            let mut path = PathBuf::from(base);
            path.extend(segments);
            path
        })
}

fn file_item(path: PathBuf, is_directory: bool, is_binary: bool, size: u64) -> FileItem {
    FileItem {
        parent: path.parent().map(|p| p.to_path_buf()),
        depth: path.components().count(),
        path,
        is_directory,
        is_binary,
        size,
    }
}

fn item_strategy() -> impl Strategy<Value = FileItem> {
    (path_strategy(), any::<bool>(), any::<bool>(), 0u64..5000).prop_map(
        |(path, is_directory, is_binary, size)| file_item(path, is_directory, is_binary, size),
    )
}

/// Adds every missing ancestor (below the filesystem root) as a directory, the way a
/// real scan lists them, so that the generated items form deep, connected trees.
fn with_ancestors(items: Vec<FileItem>) -> Vec<FileItem> {
    let mut known: HashSet<PathBuf> = items.iter().map(|item| item.path.clone()).collect();
    let mut ancestors = Vec::new();
    for item in &items {
        let mut current = item.path.parent();
        while let Some(dir) = current.filter(|dir| dir.parent().is_some()) {
            if known.insert(dir.to_path_buf()) {
                ancestors.push(file_item(dir.to_path_buf(), true, false, 0));
            }
            current = dir.parent();
        }
    }
    ancestors.extend(items);
    ancestors
}

/// Mostly connected trees, but also raw lists with orphans, duplicates and files that
/// have children.
fn list_strategy() -> impl Strategy<Value = Vec<FileItem>> {
    (
        prop::collection::vec(item_strategy(), 0..40),
        prop::bool::weighted(0.75),
    )
        .prop_map(|(items, connected)| {
            if connected {
                with_ancestors(items)
            } else {
                items
            }
        })
}

fn target_strategy() -> impl Strategy<Value = Target> {
    prop_oneof![
        5 => any::<prop::sample::Index>().prop_map(Target::Listed),
        1 => path_strategy().prop_map(Target::Unlisted),
    ]
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => list_strategy().prop_map(Op::SetList),
        1 => prop::sample::select(vec!["/root", "/root/a", "/other", ""]).prop_map(Op::SetRoot),
        5 => target_strategy().prop_map(Op::ToggleSelected),
        1 => Just(Op::SelectAllFiles),
        1 => Just(Op::DeselectAll),
        2 => target_strategy().prop_map(Op::ToggleExpanded),
        2 => target_strategy().prop_map(Op::ToggleLoaded),
        1 => target_strategy().prop_map(Op::ToggleContentMatch),
        1 => prop::option::of(target_strategy()).prop_map(Op::Preview),
        1 => prop::sample::select(vec!["", "main", "RS", "a", "Ä"]).prop_map(Op::FilenameQuery),
        1 => prop::sample::select(vec!["", "rs", ".txt", "no extension"])
            .prop_map(Op::ExtensionFilter),
        1 => any::<bool>().prop_map(Op::CaseSensitive),
    ]
}

fn toggle(set: &mut HashSet<PathBuf>, path: PathBuf) {
    if !set.remove(&path) {
        set.insert(path);
    }
}

fn toggle_selected(state: &mut AppState, path: PathBuf) {
    if !state.selected_files.remove(&path) {
        state.selected_files.insert(path);
    }
}

fn apply(state: &mut AppState, op: Op) {
    match op {
        Op::SetList(items) => state.set_filtered_file_list(items),
        Op::SetRoot(root) => state.current_path = root.to_string(),
        Op::ToggleSelected(target) => {
            let path = target.resolve(state);
            toggle_selected(state, path);
        }
        Op::SelectAllFiles => {
            state.selected_files = state
                .filtered_file_list()
                .iter()
                .filter(|item| !item.is_directory)
                .map(|item| item.path.clone())
                .collect();
        }
        Op::DeselectAll => state.selected_files.clear(),
        Op::ToggleExpanded(target) => {
            let path = target.resolve(state);
            toggle(&mut state.expanded_dirs, path);
        }
        Op::ToggleLoaded(target) => {
            let path = target.resolve(state);
            toggle(&mut state.loaded_dirs, path);
        }
        Op::ToggleContentMatch(target) => {
            let path = target.resolve(state);
            let mut matches = state.content_search_results().clone();
            toggle(&mut matches, path);
            state.set_content_search_results(matches);
        }
        Op::Preview(target) => {
            state.previewed_file_path = target.map(|target| target.resolve(state));
        }
        Op::FilenameQuery(query) => state.search_query = query.to_string(),
        Op::ExtensionFilter(filter) => state.extension_filter = filter.to_string(),
        Op::CaseSensitive(value) => state.config_mut().case_sensitive_search = value,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Across arbitrary sequences of state changes, the rows are identical to the
    /// pruned tree of the original algorithm, the snapshot carries exactly these
    /// rows, and its counters describe the complete tree. The selection changes go
    /// through the journal of `SelectedFiles`, so this also holds the incrementally
    /// kept selection counts against a count from scratch, for lists with orphans,
    /// duplicates and files that have children.
    #[test]
    fn cached_tree_equals_pruned_reference_tree(
        initial in list_strategy(),
        ops in prop::collection::vec(op_strategy(), 1..25),
    ) {
        let mut state = test_state();
        state.set_filtered_file_list(initial);

        for op in ops {
            apply(&mut state, op.clone());

            let complete = reference_tree(&state);
            let (files, dirs) = count_nodes(&complete);
            let expected = prune_collapsed(complete);
            let ui_state = generate_ui_state(&state);
            let actual = &snapshot_tree(&state);

            prop_assert_eq!(actual, &expected, "after {:?}", op);
            prop_assert!(is_pruned(actual));
            // The snapshot carries these rows (the lists generated here fit into
            // the default window).
            let all_rows = rows::all_rows(&state);
            prop_assert_eq!(ui_state.rows_total, all_rows.len());
            prop_assert_eq!(ui_state.rows_start, 0);
            prop_assert_eq!(&ui_state.rows, &all_rows);
            prop_assert_eq!(
                (ui_state.tree_files_count, ui_state.tree_dirs_count),
                (files, dirs),
                "after {:?}", op
            );
        }

        // With everything expanded nothing is pruned: the snapshot is the complete
        // tree of the original algorithm.
        state.expanded_dirs = state
            .filtered_file_list()
            .iter()
            .map(|item| item.path.clone())
            .collect();
        prop_assert_eq!(snapshot_tree(&state), reference_tree(&state));
    }

    /// Pruning never changes what is reported about a node that is part of the
    /// snapshot: apart from the cut-off children it equals the node of the full tree.
    #[test]
    fn expansion_only_decides_which_nodes_are_sent(
        list in list_strategy(),
        selected in prop::collection::vec(target_strategy(), 0..8),
        expanded in prop::collection::vec(target_strategy(), 0..8),
    ) {
        let mut state = test_state();
        state.set_filtered_file_list(list);
        for target in selected {
            let path = target.resolve(&state);
            state.selected_files.insert(path);
        }
        for target in expanded {
            let path = target.resolve(&state);
            state.expanded_dirs.insert(path);
        }

        let complete = reference_tree(&state);
        let snapshot = snapshot_tree(&state);

        fn check(snapshot: &[TreeNode], complete: &[TreeNode]) -> Result<(), TestCaseError> {
            prop_assert_eq!(snapshot.len(), complete.len());
            for (sent, full) in snapshot.iter().zip(complete) {
                prop_assert_eq!(&sent.path, &full.path);
                prop_assert_eq!(&sent.selection_state, &full.selection_state);
                prop_assert_eq!(sent.has_children, !full.children.is_empty());
                prop_assert_eq!(sent.children_loaded, full.children_loaded);
                if sent.is_expanded {
                    check(&sent.children, &full.children)?;
                } else {
                    prop_assert!(sent.children.is_empty());
                }
            }
            Ok(())
        }
        check(&snapshot, &complete)?;
    }

    /// What a click on a checkbox acts on is what the checkbox shows: for every node of
    /// the tree, the subtree the selection commands work with has the node's selection
    /// state, and a directory is "full" exactly if all files of that subtree are selected.
    #[test]
    fn subtree_selection_agrees_with_the_tree(
        list in list_strategy(),
        selected in prop::collection::vec(target_strategy(), 0..8),
    ) {
        let mut state = test_state();
        state.set_filtered_file_list(list);
        for target in selected {
            let path = target.resolve(&state);
            state.selected_files.insert(path);
        }
        state.expanded_dirs = state
            .filtered_file_list()
            .iter()
            .map(|item| item.path.clone())
            .collect();

        let index = state.file_tree();
        let mut pending = snapshot_tree(&state);
        while let Some(node) = pending.pop() {
            let subtree = selection::subtree_selection(
                state.filtered_file_list(),
                &index,
                &state.selected_files,
                &node.path,
            )
            .expect("every node of the tree is listed");

            prop_assert_eq!(subtree.state, node.selection_state, "{:?}", &node.path);
            if node.is_directory {
                let all_selected = !subtree.files.is_empty()
                    && subtree.files.iter().all(|f| state.selected_files.contains(f));
                prop_assert_eq!(
                    subtree.state == SelectionState::Full,
                    all_selected,
                    "{:?}", &node.path
                );
            }
            pending.extend(node.children);
        }
    }
}

// --- Invalidation ------------------------------------------------------------------

fn scanned_state() -> AppState {
    let mut state = test_state();
    state.set_full_file_list(vec![
        item("/root/src", true),
        item("/root/src/main.rs", false),
        item("/root/readme.md", false),
    ]);
    filtering::apply_filters(&mut state);
    state
}

fn root_names(state: &AppState) -> Vec<String> {
    snapshot(state)
        .tree
        .into_iter()
        .map(|node| node.name)
        .collect()
}

#[test]
fn index_is_reused_while_only_dynamic_state_changes() {
    let mut state = scanned_state();
    generate_ui_state(&state);
    assert_eq!(builds(&state), 1);

    state
        .selected_files
        .insert(PathBuf::from("/root/src/main.rs"));
    state.expanded_dirs.insert(PathBuf::from("/root/src"));
    state.loaded_dirs.insert(PathBuf::from("/root/src"));
    state.previewed_file_path = Some(PathBuf::from("/root/readme.md"));
    let ui_state = snapshot(&state);

    assert_eq!(builds(&state), 1);
    let src = &ui_state.tree[0];
    assert_eq!(src.selection_state, SelectionState::Full);
    assert!(src.is_expanded && src.children_loaded);
    assert!(ui_state.tree[1].is_previewed);
    assert_eq!(ui_state.tree, expected_tree(&state));
}

#[test]
fn index_is_rebuilt_after_apply_filters() {
    let mut state = scanned_state();
    assert_eq!(root_names(&state), ["src", "readme.md"]);
    assert_eq!(builds(&state), 1);

    state.search_query = "main".to_string();
    filtering::apply_filters(&mut state);

    assert_eq!(root_names(&state), ["src"]);
    assert_eq!(builds(&state), 2);
}

#[test]
fn index_is_rebuilt_after_scan_results_replace_the_list() {
    let mut state = scanned_state();
    generate_ui_state(&state);

    // A (deep) scan result replaces the full list and re-applies the filters.
    state.set_full_file_list(vec![item("/root/docs", true), item("/root/zz.txt", false)]);
    filtering::apply_filters(&mut state);

    assert_eq!(root_names(&state), ["docs", "zz.txt"]);
    assert_eq!(builds(&state), 2);
}

#[test]
fn index_is_rebuilt_after_lazy_load_merge() {
    let mut state = scanned_state();
    generate_ui_state(&state);

    // A lazy load appends the children of one directory and re-applies the filters.
    state.edit_full_file_list(|list| list.push(item("/root/src/lib.rs", false)));
    state.loaded_dirs.insert(PathBuf::from("/root/src"));
    state.expanded_dirs.insert(PathBuf::from("/root/src"));
    filtering::apply_filters(&mut state);
    let ui_state = snapshot(&state);

    let children: Vec<_> = ui_state.tree[0].children.iter().map(|n| &n.name).collect();
    assert_eq!(children, ["lib.rs", "main.rs"]);
    assert_eq!(builds(&state), 2);
}

#[test]
fn index_is_rebuilt_after_reset_directory_state() {
    let mut state = scanned_state();
    generate_ui_state(&state);

    state.reset_directory_state();
    assert!(snapshot(&state).tree.is_empty());

    // Same list and root as before the reset: the old index must not be picked up
    // unseen, the revision has moved on.
    state.current_path = "/root".to_string();
    state.set_full_file_list(vec![item("/root/other.rs", false)]);
    filtering::apply_filters(&mut state);

    assert_eq!(root_names(&state), ["other.rs"]);
    assert_eq!(builds(&state), 3);
}

#[test]
fn index_is_rebuilt_when_root_path_changes() {
    let mut state = scanned_state();
    assert_eq!(root_names(&state), ["src", "readme.md"]);

    state.current_path = "/root/src".to_string();

    assert_eq!(root_names(&state), ["main.rs"]);
    assert_eq!(builds(&state), 2);
}

#[test]
fn every_list_replacement_gets_a_new_revision() {
    let mut state = test_state();
    let before = state.filtered_revision();

    state.set_filtered_file_list(vec![item("/root/a.rs", false)]);
    let after_set = state.filtered_revision();
    state.reset_directory_state();
    let after_reset = state.filtered_revision();

    assert!(before < after_set && after_set < after_reset);
}
