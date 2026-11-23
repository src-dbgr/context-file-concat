import { writable, derived, get } from "svelte/store";
import type { AppState } from "../types";
import type * as monaco from "monaco-editor/esm/vs/editor/editor.api";

/**
 * The primary store holding the entire application state: the last snapshot
 * the host has sent. It mirrors the host and is not edited to change a setting;
 * settings are changed with intent commands (see `ipc/schema.ts`).
 */
export const appState = writable<AppState>({
  is_scanning: false,
  is_generating: false,
  is_fully_scanned: false,
  patterns_need_rescan: false,
  rows: [],
  rows_start: 0,
  rows_total: 0,
  rows_revision: 0,
  located: null,
  last_processed_session: 0,
  last_processed_seq: 0,
  current_path: "",
  current_config_filename: null,
  status_message: "Ready.",
  total_files_found: 0,
  visible_files_count: 0,
  selected_files_count: 0,
  tree_files_count: 0,
  tree_dirs_count: 0,
  search_query: "",
  extension_filter: "",
  content_search_query: "",
  scan_progress: {
    files_scanned: 0,
    large_files_skipped: 0,
    current_scanning_path: "",
  },
  active_ignore_patterns: [],
  config: {
    ignore_patterns: [],
    case_sensitive_search: false,
    include_tree_by_default: false,
    use_relative_paths: false,
    remove_empty_directories: false,
    output_directory: null,
    output_filename: "output.txt",
  },
});

// --- Derived Stores for computed values ---

export const isIdle = derived(
  appState,
  ($appState) => !$appState.is_scanning && !$appState.is_generating,
);

export const hasSelection = derived(
  appState,
  ($appState) => $appState.selected_files_count > 0,
);

export const hasVisibleItems = derived(
  appState,
  ($appState) => $appState.rows_total > 0,
);

// --- Global State for UI elements not part of the backend state ---

export const editorInstance =
  writable<monaco.editor.IStandaloneCodeEditor | null>(null);
export const previewedPath = writable<string | null>(null);
export const patternFilter = writable<string>("");

// --- Helper function to easily access the current state ---
export function getState(): AppState {
  return get(appState);
}
