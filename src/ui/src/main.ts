import "../style.css";

import { mount } from "svelte";
import { post } from "$lib/services/backend";
import { setupEventListeners } from "$lib/modules/eventListeners";
import { setupGlobalKeyboardListeners } from "$lib/modules/keyboard";
import App from "./App.svelte";
import {
  isBudgetMode,
  markScriptStart,
  scheduleEarlyReadyFallback,
  markInitStart,
  markReadyAndMeasureOnce,
} from "$lib/dev/budget";
import { installE2EBridge } from "$lib/dev/e2eBridge";
import { installWindowIPCHandlers } from "$lib/ipc/handlers";

/* ----------------------------- Budget switches ----------------------------- */

const budgetMode = isBudgetMode();
if (budgetMode) {
  markScriptStart();
  // Early microtask fallback (keeps tests deterministic without coupling to the full init)
  scheduleEarlyReadyFallback();
}

/* ------------------------------ E2E test hook ------------------------------ */

// `window.__e2e` exists in dev and e2e builds only. `__CFC_TEST_HOOKS__` is a
// build-time constant (vite.config.ts): in a production build this call is
// dead code and the bridge module, which has no side effects, is dropped from
// the bundle with it. Installed before anything else of the app runs, so the
// tests have it even where there is no host to talk to.
if (__CFC_TEST_HOOKS__) installE2EBridge();

/* ----------------------------- Install IPC API ----------------------------- */

installWindowIPCHandlers();

/* ------------------------------- Mount UI ---------------------------------- */

mount(App, { target: document.body });

function initialize() {
  if (budgetMode) {
    markInitStart();
  }

  console.log("App initializing with Svelte 5 & TypeScript...");

  setupEventListeners();
  setupGlobalKeyboardListeners();

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
