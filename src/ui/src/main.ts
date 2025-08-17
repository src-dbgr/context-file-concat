import "../style.css";

import { mount } from "svelte";
import { post } from "./lib/services/backend";
import {
  appState,
  editorDecorations,
  editorInstance,
  previewedPath,
  getState,
} from "./lib/stores/app";
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
import type { AppState } from "./lib/types";
import { get } from "svelte/store";
import type * as monaco from "monaco-editor";

const app = mount(App, {
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
    updateSearchInputsState?: () => void;
  }
}

window.render = (newState: AppState) => {
  // ==================================================================
  //                        **LOGGING-CODE**
  // ==================================================================
  console.log("----------- FROM_BACKEND: -----------");
  console.log("New state received from Rust backend.");
  console.table({
    "Config Patterns": newState.config.ignore_patterns.join(", "),
    "Active Patterns": newState.active_ignore_patterns.join(", "),
  });
  console.log("Full State Object:", newState);
  console.log("-------------------------------------");
  // ==================================================================

  const previousState = getState();
  const scrollContainer = document.querySelector(".virtual-scroll-container");
  const scrollPosition = scrollContainer ? scrollContainer.scrollTop : 0;
  const wasScanning = previousState.is_scanning && !newState.is_scanning;

  appState.set(newState);
  renderUI();

  const newScrollContainer = document.querySelector(
    ".virtual-scroll-container"
  );
  if (newScrollContainer) {
    requestAnimationFrame(() => {
      newScrollContainer.scrollTop = scrollPosition;
    });
  }

  if (previousState.current_path && !newState.current_path) {
    clearPreview();
  }

  if (
    previousState.current_path !== newState.current_path &&
    window.updateSearchInputsState
  ) {
    window.updateSearchInputsState();
  }

  const editor = get(editorInstance);
  if (editor && get(previewedPath)) {
    const model = editor.getModel();
    if (!model) return;

    const searchTerm = newState.content_search_query;
    const matchCase = newState.config.case_sensitive_search;
    let newDecorations: monaco.editor.IModelDeltaDecoration[] = [];
    if (searchTerm && searchTerm.trim() !== "") {
      const matches = model.findMatches(
        searchTerm,
        true,
        false,
        matchCase,
        null,
        true
      );
      newDecorations = matches.map((match: monaco.editor.FindMatch) => ({
        range: match.range,
        options: { inlineClassName: "search-highlight" },
      }));
    }
    const currentDecorations = get(editorDecorations);
    const newDecorationIds = editor.deltaDecorations(
      currentDecorations,
      newDecorations
    );
    editorDecorations.set(newDecorationIds);
  }

  if (wasScanning) {
    const progressFill = document.getElementById("scan-progress-fill");
    if (progressFill) {
      progressFill.style.width = "100%";
      progressFill.classList.add("scan-complete");
    }
    setTimeout(renderUI, 500);
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

export default app;
