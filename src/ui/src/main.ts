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
import PreviewPanel from "$lib/components/PreviewPanel.svelte";
import FileTree from "$lib/components/FileTree.svelte";
import type { AppState } from "$lib/types";
import {
  applyExpansionMemory,
  clearExpansionMemory,
} from "$lib/modules/treeExpansion";

// Mount core UI fragments
mount(App, { target: document.getElementById("svelte-root")! });
mount(Header, { target: document.getElementById("header-root")! });
mount(Sidebar, { target: document.getElementById("sidebar-root")! });
mount(FileTree, { target: document.getElementById("file-tree-container")! });
mount(PreviewPanel, { target: document.getElementById("preview-panel")! });

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

let lastPath: string | null = null;

/**
 * Main entry point called by the backend. It:
 * 1) Preserves expansion state across renders.
 * 2) Resets expansion memory when the base directory changes.
 * 3) Avoids duplicating "Status:" prefixes.
 * 4) Clears the editor when the directory is cleared.
 */
window.render = (incoming: AppState) => {
  try {
    // Capture previous state BEFORE mutating the store
    const prev = getState();

    // Normalize/guard incoming payload
    const nextPath = incoming?.current_path ?? null;
    const safeTree = Array.isArray(incoming?.tree) ? incoming.tree : [];

    // Avoid double "Status:" prefixes
    if (incoming?.status_message) {
      incoming.status_message = incoming.status_message.startsWith("Status:")
        ? incoming.status_message
        : `Status: ${incoming.status_message}`;
    } else {
      incoming.status_message = "Status: Ready.";
    }

    // Reset expansion memory whenever the root directory path actually changes
    if (lastPath !== nextPath) {
      clearExpansionMemory();
    }

    // Apply remembered expand/collapse intent before committing to the store
    const patched: AppState = {
      ...incoming,
      tree: applyExpansionMemory(safeTree),
    };

    appState.set(patched);

    // If the directory was cleared, also clear the editor preview
    if (prev.current_path && !patched.current_path) {
      clearPreview();
    }

    // Update path snapshot after successful commit
    lastPath = nextPath;
  } catch (err) {
    console.error("render() failed:", err);
    appState.update((s) => {
      s.status_message = "Error: Failed to render state.";
      return s;
    });
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

  // Create Monaco AFTER the PreviewPanel exists in the DOM
  initEditor(() => {
    console.log("Monaco Editor is ready.");
    setupGlobalKeyboardListeners();
  });

  post("initialize");
}

document.addEventListener("DOMContentLoaded", initialize);
