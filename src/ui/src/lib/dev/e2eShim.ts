/**
 * Minimal E2E shim & conditional dev bridge loader, extracted from main.ts.
 * Keeps __e2e.store.setAppState and __e2e.debug.dump available in tests.
 */

import type { AppState } from "$lib/types";

export type E2EStore = { setAppState: (s: AppState) => void };
export type E2EDebug = { dump: () => unknown };
export type E2EBridge = { store: E2EStore; debug: E2EDebug };

/** Installs a synchronous shim so tests have deterministic hooks even without the dev bridge. */
export function ensureE2EShim(
  set: (s: AppState) => void,
  get: () => AppState
): void {
  const w = window as unknown as { __e2e?: E2EBridge };
  if (!w.__e2e) {
    const shim: E2EBridge = {
      store: { setAppState: set },
      debug: {
        dump: () => ({
          state: get(),
          ts: Date.now(),
          href: window.location.href,
        }),
      },
    };
    (window as unknown as { __e2e: E2EBridge }).__e2e = shim;
  }
}

/** Decides if the heavier dev bridge should be installed. */
export function shouldInstallE2EBridge(): boolean {
  if (import.meta.env.MODE !== "production") return true;

  try {
    const u = new URL(window.location.href);
    if (u.searchParams.get("e2e") === "1") return true;
  } catch {
    /* no-op */
  }

  type PWFlag = Window & { __PW_E2E?: boolean };
  return (window as PWFlag).__PW_E2E === true;
}

/** Dynamically installs the real dev bridge if policy allows it. */
export async function installE2EBridgeIfAllowed(): Promise<void> {
  if (!shouldInstallE2EBridge()) return;
  try {
    const m = await import("$lib/dev/e2eBridge");
    if (
      typeof (m as { installE2EBridge?: () => unknown }).installE2EBridge ===
      "function"
    ) {
      (m as { installE2EBridge: () => unknown }).installE2EBridge();
    }
  } catch {
    /* swallow silently in preview/prod */
  }
}
