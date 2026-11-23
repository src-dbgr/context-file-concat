import type { Page } from "@playwright/test";

/**
 * Shared seeding for the E2E specs.
 *
 * There is no host in these tests. A spec writes a tree down nested and this
 * module turns it into what the host would send: the flat list of rows
 * (children of expanded directories only), of which a snapshot holds a window.
 *
 * - `setAppState` puts a snapshot with *all* rows into the store. That is a
 *   valid snapshot as long as the tree fits into one window.
 * - `setTree` hands the rows to the stand-in host of the E2E bridge, which
 *   answers the viewport reports of the UI with windows like the real host.
 */

/** A node of a tree as a spec writes it down. */
export type TreeNode = {
  path: string;
  name: string;
  is_directory: boolean;
  is_expanded: boolean;
  is_binary: boolean;
  is_match: boolean;
  is_previewed: boolean;
  selection_state: "none" | "full" | "partial";
  size: number;
  /** Listed as rows only if the directory is expanded. */
  children: TreeNode[];
  has_children: boolean;
  children_loaded: boolean;
};

/** A row as the host sends it. */
export type TreeRow = Omit<TreeNode, "children"> & { depth: number };

export type AppConfig = {
  ignore_patterns: string[];
  case_sensitive_search: boolean;
  include_tree_by_default: boolean;
  use_relative_paths: boolean;
  remove_empty_directories: boolean;
  output_directory: string;
  output_filename: string;
};

/** The state a spec seeds: a snapshot whose tree is still nested. */
export type AppState = {
  is_scanning: boolean;
  is_generating: boolean;
  is_fully_scanned: boolean;
  patterns_need_rescan: boolean;
  tree: TreeNode[];
  current_path: string | null;
  current_config_filename: string | null;
  status_message: string;
  selected_files_count: number;
  tree_files_count: number;
  tree_dirs_count: number;
  search_query: string;
  extension_filter: string;
  content_search_query: string;
  active_ignore_patterns: string[];
  config: AppConfig;
};

/** The rows of a tree: preorder, the children of expanded directories only. */
export function rowsOf(
  nodes: TreeNode[],
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

/** A file row or node with the usual flags. */
export function fileNode(path: string, size = 123): TreeNode {
  return {
    path,
    name: path.split("/").pop() ?? path,
    is_directory: false,
    is_expanded: false,
    is_binary: false,
    is_match: true,
    is_previewed: false,
    selection_state: "none",
    size,
    has_children: false,
    children_loaded: true,
    children: [],
  };
}

/** One expanded directory `/repo/src` with `filesCount` files. */
export function makeHugeTree(filesCount: number): TreeNode[] {
  const children: TreeNode[] = [];
  for (let i = 0; i < filesCount; i++) {
    children.push(fileNode(`/repo/src/file-${i}.txt`));
  }
  return [
    {
      ...fileNode("/repo/src", 0),
      is_directory: true,
      is_expanded: true,
      has_children: true,
      children,
    },
  ];
}

export async function waitForBridge(page: Page): Promise<void> {
  await page.waitForFunction(() => {
    const w = window as unknown as {
      __e2e?: { store?: { setAppState?: (s: unknown) => void } };
    };
    return typeof w.__e2e?.store?.setAppState === "function";
  });
}

/**
 * The fields of a snapshot no spec cares about. With them a seeded state is a
 * complete snapshot: `setTree` delivers it through `window.render`, which
 * validates it like a snapshot of the host.
 */
const SNAPSHOT_DEFAULTS = {
  total_files_found: 0,
  visible_files_count: 0,
  scan_progress: {
    files_scanned: 0,
    large_files_skipped: 0,
    current_scanning_path: "",
  },
};

let revisions = 0;

/** A complete host snapshot of `state` whose window holds every row. */
function snapshotOf(state: AppState) {
  const { tree, ...rest } = state;
  const rows = rowsOf(tree);
  return {
    ...SNAPSHOT_DEFAULTS,
    ...rest,
    rows,
    rows_start: 0,
    rows_total: rows.length,
    rows_revision: ++revisions,
    located: null,
  };
}

/**
 * Delivers `state` the way the host does, through `window.render`, so the
 * snapshot is validated and every reaction of the UI to a host snapshot runs.
 */
export async function renderAsHost(page: Page, state: AppState): Promise<void> {
  await page.evaluate((next) => {
    (window as unknown as { render: (s: unknown) => void }).render(next);
  }, snapshotOf(state));
}

/** Replaces the state of the UI with a snapshot that holds every row. */
export async function setAppState(page: Page, state: AppState): Promise<void> {
  const snapshot = snapshotOf(state);
  await page.evaluate((next) => {
    (
      window as unknown as {
        __e2e: { store: { setAppState: (s: unknown) => void } };
      }
    ).__e2e.store.setAppState(next);
  }, snapshot);
}

/**
 * Shows a tree of any size: the stand-in host keeps the rows and sends the
 * window the UI asks for.
 */
export async function setTree(page: Page, state: AppState): Promise<void> {
  const { tree, ...rest } = state;
  await page.evaluate(
    ([base, rows]) => {
      (
        window as unknown as {
          __e2e: { host: { setTree: (s: unknown, r: unknown) => void } };
        }
      ).__e2e.host.setTree(base, rows);
    },
    [{ ...SNAPSHOT_DEFAULTS, ...rest }, rowsOf(tree)] as const,
  );
}

/**
 * Makes the stand-in host take `milliseconds` to answer a command, like a
 * host that is busy. What the UI does in the meantime is what a test of this
 * is about.
 */
export async function setHostLatency(
  page: Page,
  milliseconds: number,
): Promise<void> {
  await page.evaluate((ms) => {
    (
      window as unknown as {
        __e2e: { host: { setLatency: (ms: number) => void } };
      }
    ).__e2e.host.setLatency(ms);
  }, milliseconds);
}
