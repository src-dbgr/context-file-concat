import { writable, derived, get } from "svelte/store";
import type { AppState, Config } from "../types";
import type * as monaco from "monaco-editor/esm/vs/editor/editor.api";

/**
 * Creates the central app state store with a custom method for updating config.
 * @param initialState The initial state for the store.
 */
function createAppStateStore(initialState: AppState) {
  const { subscribe, set, update } = writable(initialState);

  return {
    subscribe,
    set,
    update,
    /**
     * Updates properties within the config object.
     * @param partialConfig A partial config object with the properties to update.
     */
    updateConfig(partialConfig: Partial<Config>) {
      update((state) => {
        state.config = { ...state.config, ...partialConfig };
        return state;
      });
    },
  };
}

/**
 * The primary store holding the entire application state.
 */
export const appState = createAppStateStore({
  is_scanning: false,
  is_generating: false,
  is_fully_scanned: false,
  patterns_need_rescan: false,
  tree: [],
  current_path: null,
  current_config_filename: null,
  status_message: "Ready.",
  selected_files_count: 0,
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
});

// --- Derived Stores for computed values ---

export const isIdle = derived(
  appState,
  ($appState) => !$appState.is_scanning && !$appState.is_generating
);

export const hasSelection = derived(
  appState,
  ($appState) => $appState.selected_files_count > 0
);

export const hasVisibleItems = derived(
  appState,
  ($appState) => $appState.tree.length > 0
);

// --- Global State for UI elements not part of the backend state ---

export const editorInstance =
  writable<monaco.editor.IStandaloneCodeEditor | null>(null);
export const editorDecorations = writable<string[]>([]);
export const previewedPath = writable<string | null>(null);
export const patternFilter = writable<string>("");

// --- Helper function to easily access the current state ---
export function getState(): AppState {
  return get(appState);
}
