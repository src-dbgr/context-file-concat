import "../style.css";

import { mount } from "svelte";
import { post } from "./lib/services/backend";
import { state } from "./lib/state";
import {
  initEditor,
  showPreviewContent,
  showGeneratedContent,
  clearPreview,
} from "./lib/modules/editor";
import { renderUI } from "./lib/modules/renderer";
import { setupEventListeners } from "./lib/modules/eventListeners";
import { setupGlobalKeyboardListeners } from "./lib/modules/keyboard";
import { setupResizerListeners } from "./lib/modules/resizer";
import App from "./App.svelte";

// WICHTIGE ÄNDERUNG: 'new App' wird durch 'mount(App, ...)' ersetzt
const app = mount(App, {
  target: document.getElementById("svelte-root")!,
});

// ==================================================================
//  API for the Rust backend (global window.* functions)
// ==================================================================
declare global {
  interface Window {
    render: (newState: any) => void;
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
    updateSearchInputsState?: () => void;
  }
}

window.render = (newState: any) => {
  const previousState = state.get(); // Get the state *before* it's updated

  const scrollContainer = document.querySelector(".virtual-scroll-container");
  const scrollPosition = scrollContainer ? scrollContainer.scrollTop : 0;

  const wasScanning = state.get().is_scanning;

  state.set(newState);
  renderUI(); // Render the declarative parts of the UI

  const newScrollContainer = document.querySelector(
    ".virtual-scroll-container"
  );
  if (newScrollContainer) {
    requestAnimationFrame(() => {
      newScrollContainer.scrollTop = scrollPosition;
    });
  }

  // Detect a hard reset (e.g., after config import or "Clear Directory").
  if (previousState.current_path && !newState.current_path) {
    clearPreview();
  }

  // Update search inputs state when directory selection changes
  if (
    previousState.current_path !== newState.current_path &&
    window.updateSearchInputsState
  ) {
    window.updateSearchInputsState();
  }

  // Additional logic after rendering
  const editor = state.getEditor();
  if (editor && state.getPreviewedPath()) {
    // If search terms have changed, update highlights in the editor
    const model = editor.getModel();
    const searchTerm = newState.content_search_query;
    const matchCase = newState.config.case_sensitive_search;
    let newDecorations = [];
    if (searchTerm && searchTerm.trim() !== "") {
      const matches = model.findMatches(
        searchTerm,
        true,
        false,
        matchCase,
        null,
        true
      );
      newDecorations = matches.map((match: any) => ({
        range: match.range,
        options: { inlineClassName: "search-highlight" },
      }));
    }
    const newCurrentDecorations = editor.deltaDecorations(
      state.getDecorations(),
      newDecorations
    );
    state.setDecorations(newCurrentDecorations);
  }

  if (wasScanning && !newState.is_scanning) {
    const progressFill = document.getElementById("scan-progress-fill");
    if (progressFill) {
      progressFill.style.width = "100%";
      progressFill.classList.add("scan-complete");
    }
    // Re-render UI after a short delay to reset buttons
    setTimeout(renderUI, 500);
  }
};

window.updateScanProgress = (progress: any) => {
  if (!state.get().is_scanning) return;
  const { files_scanned, current_scanning_path, large_files_skipped } =
    progress;

  // Find all necessary DOM elements for progress updates.
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
      (files_scanned / 100) * 100
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

// ==================================================================
//  App Initialization
// ==================================================================
function initialize() {
  console.log("App initializing with Svelte 5 & TypeScript...");
  setupEventListeners();
  setupResizerListeners();
  initEditor(() => {
    // This code executes as soon as the Monaco editor is loaded and ready.
    console.log("Monaco Editor is ready.");
    setupGlobalKeyboardListeners();
  });

  // Request the initial state from the backend.
  post("initialize");
}

document.addEventListener("DOMContentLoaded", initialize);

export default app;
