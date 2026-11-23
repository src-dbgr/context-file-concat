// E2E bridge for deterministic UI testing: `window.__e2e`.
// Not part of production builds – main.ts installs it only when the build-time
// constant `__CFC_TEST_HOOKS__` is set (dev server, e2e build). Keep this
// module free of top-level side effects, so that it is tree-shaken otherwise.

import { appState, getState } from "$lib/stores/app";
import type { AppState, TreeRow } from "$lib/types";
import { TreeHostModel, type TreeLessState } from "./treeHostModel";

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
   * The snapshot is delivered through `window.render`, validation included.
   */
  setTree: (state: TreeLessState, rows: TreeRow[]) => void;
  /**
   * How long the stand-in host takes to answer a command, in ms (0 by
   * default: the next task). For tests of what the UI does in the meantime.
   */
  setLatency: (milliseconds: number) => void;
  /**
   * Keep back every answer from now on, until `release`. For tests of what
   * the UI does while the host has not answered: nothing arrives in between,
   * however long the test takes (no race against a timer).
   */
  hold: () => void;
  /** Deliver the answers kept back since `hold`, in order, and go on. */
  release: () => void;
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

/**
 * The stand-in for the host's row windowing: the shared model of it
 * (`treeHostModel.ts`, held against the real host by a contract test), wired
 * to the two channels of the IPC. It listens to what the UI posts and
 * delivers its snapshots through `window.render`, so they pass the same
 * validation as the snapshots of the host. It answers asynchronously, as the
 * host does: a command never changes the state while it is being posted.
 */
function createTreeHost(): E2EHostAPI {
  const model = new TreeHostModel();
  let latencyMs = 0;
  /** The number of answers kept back; `null` while the host is not held. */
  let held: number | null = null;
  let intercepting = false;

  function deliver() {
    const snapshot = model.snapshot();
    if (snapshot) window.render(snapshot);
  }

  function handle(message: string) {
    let posted: { command?: unknown; payload?: unknown };
    try {
      posted = JSON.parse(message) as typeof posted;
    } catch {
      return;
    }
    if (typeof posted.command !== "string") return;
    if (model.receive(posted.command, posted.payload ?? null)) {
      if (held !== null) held += 1;
      else setTimeout(deliver, latencyMs);
    }
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
    setTree(state: TreeLessState, rows: TreeRow[]) {
      intercept();
      model.show(state, rows);
      deliver();
    },
    setLatency(milliseconds: number) {
      latencyMs = Math.max(0, milliseconds);
    },
    hold() {
      held ??= 0;
    },
    release() {
      const answers = held ?? 0;
      held = null;
      // One snapshot per answered command, each of the state as it is now:
      // what a timer would have delivered as well, had it fired this late.
      for (let i = 0; i < answers; i++) deliver();
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
