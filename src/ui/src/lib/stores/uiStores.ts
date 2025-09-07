import { derived } from "svelte/store";
import { appState } from "./app";
import { previewMode } from "./preview";

/**
 * Small, explicit derived stores for commonly-used UI flags.
 * Keeps components declarative and avoids duplicating logic.
 */

export const isScanning = derived(appState, (s) => s.is_scanning);
export const isGenerating = derived(appState, (s) => s.is_generating);
export const isBusy = derived(
  [isScanning, isGenerating],
  ([scan, gen]) => scan || gen
);

export const hasSelection = derived(
  appState,
  (s) => (s.selected_files_count ?? 0) > 0
);
export const canGenerate = derived(
  [isScanning, hasSelection],
  ([scan, sel]) => !scan && sel
);
export const canSave = derived(previewMode, (m) => m === "generated");

// Optional extras that are useful across the UI
export const isFullyScanned = derived(appState, (s) => s.is_fully_scanned);
export const patternsNeedRescan = derived(
  appState,
  (s) => s.patterns_need_rescan
);
