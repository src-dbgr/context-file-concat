// Test helpers for the rows of the file tree.
//
// The host sends the tree as a flat list of rows, and of that list a window.
// Tests are easier to read when they write a tree down as a tree, so these
// helpers do here what the host does (`src/app/view_model/rows.rs`): list the
// rows of a nested tree, descending into expanded directories only, and cut a
// window out of the list.

import type { AppState, TreeRow } from "$lib/types";

/** A tree as a test writes it down: every node with its children. */
export type TestNode = Omit<TreeRow, "depth"> & { children: TestNode[] };

export function file(
  path: string,
  overrides: Partial<TestNode> = {},
): TestNode {
  return {
    path,
    name: path.split(/[\\/]/).pop() || path,
    is_directory: false,
    is_expanded: false,
    is_binary: false,
    is_match: true,
    is_previewed: false,
    selection_state: "none",
    children: [],
    size: 123,
    has_children: false,
    children_loaded: true,
    ...overrides,
  };
}

export function dir(
  path: string,
  expanded = true,
  children: TestNode[] = [],
  overrides: Partial<TestNode> = {},
): TestNode {
  return {
    ...file(path),
    is_directory: true,
    is_expanded: expanded,
    children,
    size: 0,
    has_children: children.length > 0,
    ...overrides,
  };
}

/**
 * A collapsed directory with content: its children are no rows, only the
 * flags of its own row describe them.
 */
export function collapsedDir(
  path: string,
  overrides: Partial<TestNode> = {},
): TestNode {
  return dir(path, false, [], { has_children: true, ...overrides });
}

/** The rows of a tree: preorder, the children of expanded directories only. */
export function rowsOf(
  nodes: TestNode[],
  depth = 0,
  rows: TreeRow[] = [],
): TreeRow[] {
  for (const node of nodes) {
    const { children, ...row } = node;
    rows.push({ ...row, depth });
    if (node.is_directory && node.is_expanded) {
      rowsOf(children, depth + 1, rows);
    }
  }
  return rows;
}

/** The fields of a snapshot that describe the tree. */
export type TreeFields = Pick<
  AppState,
  "rows" | "rows_start" | "rows_total" | "rows_revision" | "located"
>;

let revisions = 0;

/**
 * The tree part of a snapshot that holds `count` of `rows` from row `start`
 * on; by default all of them. Every call is a new list (`rows_revision`),
 * unless a revision is given.
 */
export function treeFields(
  rows: TreeRow[],
  window: { start?: number; count?: number; revision?: number } = {},
): TreeFields {
  const count = Math.min(window.count ?? rows.length, rows.length);
  const start = Math.min(window.start ?? 0, rows.length - count);
  return {
    rows: rows.slice(start, start + count),
    rows_start: start,
    rows_total: rows.length,
    rows_revision: window.revision ?? ++revisions,
    located: null,
  };
}

/** `count` files `file-<n>.txt` directly below `/repo`: a long, flat list. */
export function manyRows(count: number): TreeRow[] {
  return rowsOf(
    Array.from({ length: count }, (_, i) =>
      file(`/repo/file-${String(i).padStart(5, "0")}.txt`),
    ),
  );
}

/** A complete, valid state without a tree; `overrides` come on top. */
export function baseAppState(overrides: Partial<AppState> = {}): AppState {
  return {
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    patterns_need_rescan: false,
    ...treeFields([], { revision: 0 }),
    current_path: "/repo",
    current_config_filename: null,
    status_message: "Status: Ready.",
    total_files_found: 0,
    visible_files_count: 0,
    scan_progress: {
      files_scanned: 0,
      large_files_skipped: 0,
      current_scanning_path: "",
    },
    selected_files_count: 0,
    tree_files_count: 0,
    tree_dirs_count: 0,
    search_query: "",
    extension_filter: "",
    content_search_query: "",
    active_ignore_patterns: [],
    config: {
      ignore_patterns: [],
      case_sensitive_search: false,
      include_tree_by_default: false,
      use_relative_paths: false,
      remove_empty_directories: false,
      output_directory: "",
      output_filename: "output.txt",
    },
    ...overrides,
  };
}
