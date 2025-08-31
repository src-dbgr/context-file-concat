/**
 * Lightweight performance/budget instrumentation, extracted from main.ts.
 * No external deps; TS strict; no "any".
 */

type W = Window & { __APP_READY?: boolean };

const hasMark = (name: string): boolean =>
  performance.getEntriesByName(name).length > 0;

/** Returns true when URL contains ?budget=1 */
export function isBudgetMode(): boolean {
  try {
    const u = new URL(window.location.href);
    return u.searchParams.get("budget") === "1";
  } catch {
    return false;
  }
}

/** Mark script start as early as possible (used as a fallback start). */
export function markScriptStart(): void {
  try {
    performance.mark("app-script-start");
  } catch {
    /* no-op */
  }
}

/**
 * Schedules a microtask that sets __APP_READY and records a one-time "app-init" measure
 * if the regular init path doesn't do it first. Safe & idempotent.
 */
export function scheduleEarlyReadyFallback(): void {
  queueMicrotask(() => {
    const w = window as W;
    if (!w.__APP_READY) {
      try {
        w.__APP_READY = true;
        if (!hasMark("app-ready")) performance.mark("app-ready");

        if (!hasMark("app-init")) {
          const start = hasMark("app-init-start")
            ? "app-init-start"
            : "app-script-start";
          performance.measure("app-init", start, "app-ready");
        }
      } catch {
        /* no-op */
      }
    }
  });
}

/** Marks the beginning of app initialization (closer to DOM-ready). */
export function markInitStart(): void {
  try {
    performance.mark("app-init-start");
  } catch {
    /* no-op */
  }
}

/**
 * Marks app ready and measures app-init if not measured yet.
 * Intended to be called at the end of initialize().
 */
export function markReadyAndMeasureOnce(): void {
  try {
    (window as W).__APP_READY = true;
    if (!hasMark("app-ready")) performance.mark("app-ready");
    if (!hasMark("app-init")) {
      performance.measure("app-init", "app-init-start", "app-ready");
    }
  } catch {
    /* no-op */
  }
}
