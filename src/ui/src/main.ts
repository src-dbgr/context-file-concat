import "../style.css";

import { mount } from "svelte";
import { post } from "$lib/services/backend";
import { appState, getState } from "$lib/stores/app";
import {
  showPreviewContent,
  showGeneratedContent,
  clearPreview,
  initEditor,
} from "$lib/modules/editor";
import { setupEventListeners } from "$lib/modules/eventListeners";
import { setupGlobalKeyboardListeners } from "$lib/modules/keyboard";
import { setupResizerListeners } from "$lib/modules/resizer";
import App from "./App.svelte";
import Header from "$lib/components/Header.svelte";
import Sidebar from "$lib/components/Sidebar.svelte";
import type { AppState } from "$lib/types";

// Mount the main App component (bridge + StatusBar)
mount(App, {
  target: document.getElementById("svelte-root")!,
});

// Mount Header
mount(Header, {
  target: document.getElementById("header-root")!,
});

// Mount Sidebar
mount(Sidebar, {
  target: document.getElementById("sidebar-root")!,
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
 * Main entry point for backend updates — updates central Svelte store.
 */
window.render = (newState: AppState) => {
  const previousPath = getState().current_path;

  // Uniform status prefix
  newState.status_message = `Status: ${newState.status_message}`;

  appState.set(newState);

  // If directory was cleared, clear editor preview
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
  if (scanTextEl)
    (scanTextEl as HTMLElement).textContent = "Scanning directory...";

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
    (fillEl as HTMLElement).style.width =
      `${Math.min(90, (files_scanned / 1000) * 100)}%`;
  }
};

window.showPreviewContent = showPreviewContent;
window.showGeneratedContent = showGeneratedContent;

window.showError = (msg: string) => {
  appState.update((s: AppState) => {
    s.status_message = `Error: ${msg}`;
    return s;
  });
};

window.showStatus = (msg: string) => {
  appState.update((s: AppState) => {
    s.status_message = `Status: ${msg}`;
    return s;
  });
};

window.fileSaveStatus = (success: boolean, path: string) => {
  let msg = "";
  if (path === "cancelled") {
    msg = "Status: Save cancelled.";
  } else {
    msg = success ? `Status: Saved to ${path}` : `Error: Failed to save file.`;
  }
  appState.update((s: AppState) => {
    s.status_message = msg;
    return s;
  });
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
