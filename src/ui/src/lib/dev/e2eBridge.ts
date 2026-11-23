// E2E bridge for deterministic UI testing: `window.__e2e`.
// Not part of production builds – main.ts installs it only when the build-time
// constant `__CFC_TEST_HOOKS__` is set (dev server, e2e build). Keep this
// module free of top-level side effects, so that it is tree-shaken otherwise.

import { appState, getState } from "$lib/stores/app";
import type { AppState, TreeRow } from "$lib/types";

/** A state without the fields that describe the window of rows. */
type TreeLessState = Omit<
  AppState,
  "rows" | "rows_start" | "rows_total" | "rows_revision" | "located"
>;

type E2EStoreAPI = {
  /**
   * Atomically replace the whole AppState. The state has to be a snapshot as
   * the host sends it: `rows` is the window, `rows_total` the whole list.
   */
  setAppState: (s: AppState) => void;
};

type E2EHostAPI = {
  /**
   * Show a tree that is larger than one window. There is no host in E2E
   * tests, so the bridge stands in for the one part of it the tree cannot do
   * without: it keeps the list of all rows and answers `setViewport` and
   * `locateRow` with a snapshot, like `src/app/view_model/rows.rs` does.
   */
  setTree: (state: TreeLessState, rows: TreeRow[]) => void;
};

type E2EDebugAPI = {
  /** Return a structured snapshot of state and key DOM facts. */
  dump: () => {
    app: AppState;
    dom: {
      currentPath: string | null;
      fileNames: string[];
      fileCount: number;
    };
  };
};

export type E2EApi = {
  store: E2EStoreAPI;
  host: E2EHostAPI;
  debug: E2EDebugAPI;
};

declare global {
  interface Window {
    __e2e?: E2EApi;
  }
}

function listVisibleFileNames(): string[] {
  // Collect all rendered file rows (not directories)
  const nodes = Array.from(
    document.querySelectorAll<HTMLElement>(".tree .file-item .file-name"),
  );
  return nodes
    .map((el) => (el.textContent || "").trim())
    .filter((t) => t.length > 0);
}

/** The window of a UI that has not reported its viewport (as on the host). */
const DEFAULT_WINDOW_ROWS = 100;
/** The largest window the host sends. */
const MAX_WINDOW_ROWS = 600;

type Located = AppState["located"];
type HostCommand = { command?: unknown; payload?: unknown };
type LocateTarget =
  | { kind: "path"; path: string }
  | { kind: "parent_of"; path: string }
  | { kind: "typeahead"; prefix: string; from: number };

/**
 * The stand-in for the host's row windowing. It answers asynchronously, as the
 * host does: a command never changes the state while it is being posted.
 */
function createTreeHost() {
  let base: TreeLessState | null = null;
  let rows: TreeRow[] = [];
  let viewport = { start: 0, count: DEFAULT_WINDOW_ROWS };
  let revision = 0;
  let located: Located = null;
  let intercepting = false;

  function send() {
    if (!base) return;
    const count = Math.min(viewport.count, rows.length);
    const start = Math.min(viewport.start, rows.length - count);
    appState.set({
      ...base,
      rows: rows.slice(start, start + count),
      rows_start: start,
      rows_total: rows.length,
      rows_revision: revision,
      located,
    });
  }

  function rowOf(target: LocateTarget): number | null {
    const found = (index: number) => (index === -1 ? null : index);
    if (target.kind === "path") {
      return found(rows.findIndex((row) => row.path === target.path));
    }
    if (target.kind === "parent_of") {
      const child = rows.findIndex((row) => row.path === target.path);
      if (child === -1) return null;
      for (let index = child - 1; index >= 0; index--) {
        if (rows[index].depth < rows[child].depth) return index;
      }
      return null;
    }
    const prefix = target.prefix.toLowerCase();
    const from = Math.min(target.from, rows.length);
    const matches = (row: TreeRow) => row.name.toLowerCase().startsWith(prefix);
    const after = rows.findIndex((row, index) => index >= from && matches(row));
    return found(after !== -1 ? after : rows.findIndex(matches));
  }

  function handle(message: string) {
    if (!base) return;
    let parsed: HostCommand;
    try {
      parsed = JSON.parse(message) as HostCommand;
    } catch {
      return;
    }
    if (parsed.command === "setViewport") {
      const { start, count } = parsed.payload as {
        start: number;
        count: number;
      };
      viewport = {
        start,
        count: Math.max(1, Math.min(count, MAX_WINDOW_ROWS)),
      };
    } else if (parsed.command === "locateRow") {
      const { request, target } = parsed.payload as {
        request: number;
        target: LocateTarget;
      };
      located = { request, row: rowOf(target) };
    } else {
      return;
    }
    setTimeout(send, 0);
  }

  /** Listen to what the UI posts; whoever listened before still does. */
  function intercept() {
    if (intercepting) return;
    intercepting = true;
    const previous = window.ipc as Window["ipc"] | undefined;
    window.ipc = {
      postMessage: (message: string) => {
        previous?.postMessage(message);
        handle(message);
      },
    };
  }

  return {
    setTree(state: TreeLessState, allRows: TreeRow[]) {
      intercept();
      base = state;
      rows = allRows;
      revision += 1;
      viewport = { start: 0, count: viewport.count };
      located = null;
      send();
    },
  };
}

export function installE2EBridge(): void {
  const api: E2EApi = {
    store: {
      setAppState: (s: AppState) => {
        appState.set(s);
      },
    },
    host: createTreeHost(),
    debug: {
      dump: () => {
        const currentPath =
          document.getElementById("current-path")?.textContent ?? null;
        const fileNames = listVisibleFileNames();
        return {
          app: getState(),
          dom: {
            currentPath,
            fileNames,
            fileCount: fileNames.length,
          },
        };
      },
    },
  };

  // Expose under window.__e2e (idempotent)
  window.__e2e = api;
}
