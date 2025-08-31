import "../style.css";

import { mount } from "svelte";
import { post } from "$lib/services/backend";
import { appState, getState } from "$lib/stores/app";
import { setupEventListeners } from "$lib/modules/eventListeners";
import { setupGlobalKeyboardListeners } from "$lib/modules/keyboard";
import { verticalResizer, sidebarResizer } from "$lib/actions/resizer";
import App from "./App.svelte";
import Header from "$lib/components/Header.svelte";
import Sidebar from "$lib/components/Sidebar.svelte";
import PreviewPanel from "$lib/components/PreviewPanel.svelte";
import FileTree from "$lib/components/FileTree.svelte";
import Footer from "$lib/components/Footer.svelte";
import {
  isBudgetMode,
  markScriptStart,
  scheduleEarlyReadyFallback,
  markInitStart,
  markReadyAndMeasureOnce,
} from "$lib/dev/budget";
import { ensureE2EShim, installE2EBridgeIfAllowed } from "$lib/dev/e2eShim";
import { initEditor } from "$lib/modules/editor";
import { installWindowIPCHandlers } from "$lib/ipc/handlers";

/* ----------------------------- Budget switches ----------------------------- */

const budgetMode = isBudgetMode();
if (budgetMode) {
  markScriptStart();
  // Early microtask fallback (keeps tests deterministic without coupling to the full init)
  scheduleEarlyReadyFallback();
}

/* ------------------------- Deterministic E2E fallback ---------------------- */

ensureE2EShim(
  (s) => appState.set(s),
  () => getState()
);
void installE2EBridgeIfAllowed();

/* ----------------------------- Install IPC API ----------------------------- */

installWindowIPCHandlers();

/* ------------------------------- Mount UI ---------------------------------- */

mount(App, { target: document.getElementById("svelte-root")! });
mount(Header, { target: document.getElementById("header-root")! });
mount(Sidebar, { target: document.getElementById("sidebar-root")! });
mount(FileTree, { target: document.getElementById("file-tree-container")! });
mount(PreviewPanel, { target: document.getElementById("preview-panel")! });
mount(Footer, { target: document.getElementById("footer-root")! });

function initialize() {
  if (budgetMode) {
    markInitStart();
  }

  console.log("App initializing with Svelte 5 & TypeScript...");

  const resizerEl = document.getElementById("resizer") as HTMLElement | null;
  const sidebarEl = document.querySelector(".sidebar") as HTMLElement | null;
  const resizerAction = resizerEl ? verticalResizer(resizerEl) : undefined;
  const sidebarAction = sidebarEl ? sidebarResizer(sidebarEl) : undefined;

  setupEventListeners();

  initEditor(() => {
    console.log("Monaco Editor is ready.");
    setupGlobalKeyboardListeners();
  });

  window.addEventListener("beforeunload", () => {
    resizerAction?.destroy?.();
    sidebarAction?.destroy?.();
  });

  post("initialize");

  if (budgetMode) {
    markReadyAndMeasureOnce();
  }
}

/* -------------------- DOM ready (robust, idempotent) ---------------------- */
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", initialize, { once: true });
} else {
  initialize();
}
