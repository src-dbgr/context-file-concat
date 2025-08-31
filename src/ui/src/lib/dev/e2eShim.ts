/**
 * E2E shims (idempotent) + conditional bridge loader.
 * - Aligns with the E2EApi type from e2eBridge (no duplicate window typings).
 * - Keeps tests deterministic without dev-only code in prod by default.
 */

import type { AppState } from "$lib/types";
import type { E2EApi } from "$lib/dev/e2eBridge"; // type-only, no runtime import

declare global {
  interface Window {
    __PW_E2E?: boolean;
    __e2e?: E2EApi;
  }
}

type SetState = (s: AppState) => void;
type GetState = () => AppState;

/**
 * Provide a minimal __e2e shim if none exists.
 * Matches E2EApi: { store.setAppState, debug.dump() -> { app, dom } }
 */
export function ensureE2EShim(setAppState: SetState, getState: GetState): void {
  if (window.__e2e) return;

  const dump = () => {
    const app = getState();
    const fileNames = Array.from(
      document.querySelectorAll(".file-item .file-name")
    ).map((el) => (el.textContent || "").trim());

    return {
      app, // full AppState
      dom: {
        currentPath: app.current_path,
        fileNames,
        fileCount: fileNames.length,
      },
    };
  };

  const api: E2EApi = {
    store: { setAppState },
    debug: { dump },
  };

  window.__e2e = api;
}

/** Decide if the dev e2eBridge should be installed (safe in production preview). */
function shouldInstallE2EBridge(): boolean {
  if (import.meta.env.MODE !== "production") return true;

  // In production bundles enable only via URL or Playwright flag
  try {
    const u = new URL(window.location.href);
    if (u.searchParams.get("e2e") === "1") return true;
  } catch {
    /* ignore */
  }
  if ((window as Window & { __PW_E2E?: boolean }).__PW_E2E === true)
    return true;

  return false;
}

/** Dynamically import the dev bridge if conditions allow it. */
export async function installE2EBridgeIfAllowed(): Promise<void> {
  if (!shouldInstallE2EBridge()) return;
  try {
    const mod = await import("$lib/dev/e2eBridge");
    const fn = (mod as unknown as { installE2EBridge?: () => void })
      .installE2EBridge;
    if (typeof fn === "function") fn();
  } catch {
    // swallow silently in preview/prod to avoid console noise
  }
}
