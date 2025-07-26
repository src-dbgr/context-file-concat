import { post } from "./services/backend.js";
import { state } from "./state.js";
import {
  initEditor,
  showPreviewContent,
  showGeneratedContent,
} from "./modules/editor.js";
import { renderUI } from "./modules/renderer.js";
import { setupEventListeners } from "./modules/eventListeners.js";
import { setupGlobalKeyboardListeners } from "./modules/keyboard.js";
import { setupResizerListeners } from "./modules/resizer.js";

// ==================================================================
//  API für die Rust-Seite (globale window.* Funktionen)
// ==================================================================
window.render = (newState) => {
  const wasScanning = state.get().is_scanning;
  state.set(newState);
  renderUI();

  // Zusätzliche Logik nach dem Rendern
  const editor = state.getEditor();
  if (editor && state.getPreviewedPath()) {
    // Wenn sich Suchbegriffe geändert haben, Highlights im Editor aktualisieren
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
    setTimeout(renderUI, 500); // UI nach kurzer Zeit erneut rendern, um Buttons zurückzusetzen
  }
};

window.updateScanProgress = (progress) => {
  if (!state.get().is_scanning) return;
  const { files_scanned, current_scanning_path, large_files_skipped } =
    progress;

  // KORREKTUR: Erst Element suchen, dann prüfen, dann zuweisen.
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
    // Die Logik hier war schon korrekt und kann bleiben.
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
//  Initialisierung der App
// ==================================================================
function initialize() {
  console.log("App initializing...");
  setupEventListeners();
  setupResizerListeners();
  initEditor(() => {
    // Dieser Code wird ausgeführt, sobald der Monaco-Editor geladen und bereit ist.
    console.log("Monaco Editor is ready.");
    setupGlobalKeyboardListeners();
  });

  // Fordere den initialen Zustand vom Backend an.
  post("initialize");
}

document.addEventListener("DOMContentLoaded", initialize);
