import "../style.css";

import { mount } from "svelte";
import { post } from "./lib/services/backend";
import { appState, getState } from "./lib/stores/app";
import {
  showPreviewContent,
  showGeneratedContent,
  clearPreview,
} from "./lib/modules/editor";
import { setupEventListeners } from "./lib/modules/eventListeners";
import { setupGlobalKeyboardListeners } from "./lib/modules/keyboard";
import { setupResizerListeners } from "./lib/modules/resizer";
import App from "./App.svelte";
import type { AppState } from "./lib/types";
import { initEditor } from "./lib/modules/editor";

mount(App, {
  target: document.getElementById("svelte-root")!,
});

declare global {
  interface Window {
    render: (newState: AppState) => void;
    updateScanProgress: (progress: any) => void;
    showPreviewContent: (
      content: string,
      language: string,
      searchTerm: string,
      path: string
    ) => void;
    showGeneratedContent: (content: string, tokenCount: number) => void;
    showError: (msg: string) => void;
    showStatus: (msg: string) => void;
    fileSaveStatus: (success: boolean, path: string) => void;
    setDragState: (isDragging: boolean) => void;
  }
}

/**
 * The main entry point for updates from the Rust backend.
 * This function now has a single responsibility: updating the central Svelte store.
 * All rendering and side effects are handled reactively within the App.svelte component.
 */
window.render = (newState: AppState) => {
  const previousPath = getState().current_path;

  appState.set(newState);

  // If the directory was cleared, explicitly clear the editor preview.
  if (previousPath && !newState.current_path) {
    clearPreview();
  }
};

window.updateScanProgress = (progress: {
  files_scanned: number;
  current_scanning_path: string;
  large_files_skipped: number;
}) => {
  if (!getState().is_scanning) return;
  const { files_scanned, current_scanning_path, large_files_skipped } =
    progress;

  const scanTextEl = document.querySelector(".scan-text");
  if (scanTextEl) scanTextEl.textContent = "Scanning directory...";

  const filesCountEl = document.getElementById("scan-files-count");
  if (filesCountEl)
    filesCountEl.textContent = `${files_scanned} files processed`;

  const currentPathEl = document.getElementById("scan-current-path");
  if (currentPathEl)
    currentPathEl.textContent = current_scanning_path || "Processing...";

  const skippedEl = document.getElementById("scan-skipped-count");
  if (skippedEl) {
    skippedEl.textContent =
      large_files_skipped > 0
        ? `${large_files_skipped} large files skipped`
        : "";
    (skippedEl as HTMLElement).style.display =
      large_files_skipped > 0 ? "inline" : "none";
  }

  const fillEl = document.getElementById("scan-progress-fill");
  if (fillEl && files_scanned > 0) {
    (fillEl as HTMLElement).style.width = `${Math.min(
      90,
      (files_scanned / 1000) * 100
    )}%`;
  }
};

window.showPreviewContent = showPreviewContent;
window.showGeneratedContent = showGeneratedContent;

window.showError = (msg: string) => {
  const statusEl = document.querySelector(".status-text");
  if (statusEl) statusEl.textContent = `Error: ${msg}`;
};

window.showStatus = (msg: string) => {
  const statusEl = document.querySelector(".status-text");
  if (statusEl) statusEl.textContent = `Status: ${msg}`;
};

window.fileSaveStatus = (success: boolean, path: string) => {
  const status = document.querySelector(".status-text");
  if (!status) return;
  if (path === "cancelled") {
    status.textContent = "Status: Save cancelled.";
  } else {
    status.textContent = success
      ? `Status: Saved to ${path}`
      : `Error: Failed to save file.`;
  }
};

window.setDragState = (isDragging: boolean) => {
  const container = document.getElementById("file-tree-container");
  if (isDragging) container?.classList.add("drag-over");
  else container?.classList.remove("drag-over");
};

function initialize() {
  console.log("App initializing with Svelte 5 & TypeScript...");
  setupEventListeners();
  setupResizerListeners();
  initEditor(() => {
    console.log("Monaco Editor is ready.");
    setupGlobalKeyboardListeners();
  });

  post("initialize");
}

document.addEventListener("DOMContentLoaded", initialize);
