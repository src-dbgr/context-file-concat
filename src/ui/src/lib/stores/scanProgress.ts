import { writable } from "svelte/store";
import type { AppState } from "../types";

/** Progress of the running directory scan, as the host reports it. */
export type ScanProgress = AppState["scan_progress"];

const NO_PROGRESS: ScanProgress = {
  files_scanned: 0,
  large_files_skipped: 0,
  current_scanning_path: "",
};

const { subscribe, set } = writable<ScanProgress>({ ...NO_PROGRESS });

/**
 * The progress the host pushes between state snapshots while it scans.
 * Kept apart from `appState`, so a snapshot does not overwrite it.
 */
export const scanProgress = {
  subscribe,
  /** A progress report of the host. */
  report: (progress: ScanProgress): void => set({ ...progress }),
  /** No scan is running, or a new one has just started. */
  reset: (): void => set({ ...NO_PROGRESS }),
};
