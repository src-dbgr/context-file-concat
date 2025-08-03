import { post } from "./services/backend.js";
import { state } from "./state.js";
import {
  initEditor,
  showPreviewContent,
  showGeneratedContent,
  clearPreview,
} from "./modules/editor.js";
import { renderUI } from "./modules/renderer.js";
import { setupEventListeners } from "./modules/eventListeners.js";
import { setupGlobalKeyboardListeners } from "./modules/keyboard.js";
import { setupResizerListeners } from "./modules/resizer.js";

// ==================================================================
//  API for the Rust backend (global window.* functions)
// ==================================================================
window.render = (newState) => {
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
      newDecorations = matches.map((match) => ({
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

window.updateScanProgress = (progress) => {
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
    skippedEl.style.display = large_files_skipped > 0 ? "inline" : "none";
  }

  const fillEl = document.getElementById("scan-progress-fill");
  if (fillEl && files_scanned > 0) {
    fillEl.style.width = `${Math.min(90, (files_scanned / 100) * 100)}%`;
  }
};

window.showPreviewContent = showPreviewContent;
window.showGeneratedContent = showGeneratedContent;

window.showError = (msg) => {
  document.querySelector(".status-text").textContent = `Error: ${msg}`;
};
window.showStatus = (msg) => {
  document.querySelector(".status-text").textContent = `Status: ${msg}`;
};
window.fileSaveStatus = (success, path) => {
  const status = document.querySelector(".status-text");
  if (path === "cancelled") {
    status.textContent = "Status: Save cancelled.";
  } else {
    status.textContent = success
      ? `Status: Saved to ${path}`
      : `Error: Failed to save file.`;
  }
};
window.setDragState = (isDragging) => {
  const container = document.getElementById("file-tree-container");
  if (isDragging) container.classList.add("drag-over");
  else container.classList.remove("drag-over");
};

// ==================================================================
//  App Initialization
// ==================================================================
function initialize() {
  console.log("App initializing...");
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
