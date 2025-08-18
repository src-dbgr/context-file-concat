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
import { verticalResizer, sidebarResizer } from "$lib/actions/resizer";
import App from "./App.svelte";
import Header from "$lib/components/Header.svelte";
import Sidebar from "$lib/components/Sidebar.svelte";
import PreviewPanel from "$lib/components/PreviewPanel.svelte";
import FileTree from "$lib/components/FileTree.svelte";
import Footer from "$lib/components/Footer.svelte";
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
mount(Footer, { target: document.getElementById("footer-root")! });

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

window.render = (incoming: AppState) => {
  try {
    const prev = getState();
    const nextPath = incoming?.current_path ?? null;
    const safeTree = Array.isArray(incoming?.tree) ? incoming.tree : [];

    if (incoming?.status_message) {
      incoming.status_message = incoming.status_message.startsWith("Status:")
        ? incoming.status_message
        : `Status: ${incoming.status_message}`;
    } else {
      incoming.status_message = "Status: Ready.";
    }

    if (lastPath !== nextPath) clearExpansionMemory();

    const patched: AppState = {
      ...incoming,
      tree: applyExpansionMemory(safeTree),
    };
    appState.set(patched);

    if (prev.current_path && !patched.current_path) clearPreview();

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
  const msg =
    path === "cancelled"
      ? "Status: Save cancelled."
      : success
        ? `Status: Saved to ${path}`
        : `Error: Failed to save file.`;

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

  // Bind actions
  const resizerEl = document.getElementById("resizer") as HTMLElement | null;
  const sidebarEl = document.querySelector(".sidebar") as HTMLElement | null;
  const resizerAction = resizerEl ? verticalResizer(resizerEl) : undefined;
  const sidebarAction = sidebarEl ? sidebarResizer(sidebarEl) : undefined;

  setupEventListeners();

  initEditor(() => {
    console.log("Monaco Editor is ready.");
    setupGlobalKeyboardListeners();
  });

  // Cleanup
  window.addEventListener("beforeunload", () => {
    resizerAction?.destroy?.();
    sidebarAction?.destroy?.();
  });

  post("initialize");
}

document.addEventListener("DOMContentLoaded", initialize);
