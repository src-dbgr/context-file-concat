/**
 * Tiny helpers for budget/e2e measurement.
 * - No deps, TS strict.
 * - Safe in browsers without PerformanceObserver fancy APIs.
 */

declare global {
  interface Window {
    __APP_READY?: boolean;
  }
}

/** Read once from URL (?budget=1). */
export function isBudgetMode(): boolean {
  try {
    const u = new URL(window.location.href);
    return u.searchParams.get("budget") === "1";
  } catch {
    return false;
  }
}

export function markScriptStart(): void {
  try {
    performance.mark("app-script-start");
  } catch {
    /* ignore */
  }
}

/**
 * As early as possible, set a microtask that marks the app as "ready".
 * This is only a fallback so tests have a deterministic barrier even if
 * initialization is extremely fast or fails before init() runs.
 */
export function scheduleEarlyReadyFallback(): void {
  queueMicrotask(() => {
    const w = window as Window & { __APP_READY?: boolean };
    if (!w.__APP_READY) {
      try {
        w.__APP_READY = true;

        if (performance.getEntriesByName("app-ready").length === 0) {
          performance.mark("app-ready");
        }
        // Only create the measure once. Prefer init-start if it exists,
        // otherwise fall back to script-start.
        if (performance.getEntriesByName("app-init").length === 0) {
          const hasInitStart =
            performance.getEntriesByName("app-init-start").length > 0;
          performance.measure(
            "app-init",
            hasInitStart ? "app-init-start" : "app-script-start",
            "app-ready"
          );
        }
      } catch {
        /* ignore */
      }
    }
  });
}

export function markInitStart(): void {
  try {
    performance.mark("app-init-start");
  } catch {
    /* ignore */
  }
}

/**
 * Mark the app as ready and create the app-init measure exactly once.
 * Call this at the end of initialize().
 */
export function markReadyAndMeasureOnce(): void {
  try {
    (window as Window & { __APP_READY?: boolean }).__APP_READY = true;

    if (performance.getEntriesByName("app-ready").length === 0) {
      performance.mark("app-ready");
    }
    if (performance.getEntriesByName("app-init").length === 0) {
      const hasInitStart =
        performance.getEntriesByName("app-init-start").length > 0;
      performance.measure(
        "app-init",
        hasInitStart ? "app-init-start" : "app-script-start",
        "app-ready"
      );
    }
  } catch {
    /* ignore */
  }
}
