// E2E bridge for deterministic UI testing: `window.__e2e`.
// Not part of production builds – main.ts installs it only when the build-time
// constant `__CFC_TEST_HOOKS__` is set (dev server, e2e build). Keep this
// module free of top-level side effects, so that it is tree-shaken otherwise.

import { appState, getState } from "$lib/stores/app";
import type { AppState } from "$lib/types";

type E2EStoreAPI = {
  /** Atomically replace the whole AppState. */
  setAppState: (s: AppState) => void;
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

export function installE2EBridge(): void {
  const api: E2EApi = {
    store: {
      setAppState: (s: AppState) => {
        appState.set(s);
      },
    },
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
