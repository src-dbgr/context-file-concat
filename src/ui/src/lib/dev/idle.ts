/**
 * Small idle helper using native requestIdleCallback when available.
 * No global type augmentation to avoid lib.dom conflicts.
 */

/** Schedule a callback for a browser idle slot (with simple timeout fallback). */
export function onIdle(cb: () => void, timeout = 1_500): number {
  if (
    "requestIdleCallback" in window &&
    typeof window.requestIdleCallback === "function"
  ) {
    // Run cb inside the idle callback to avoid dealing with IdleDeadline types.
    return window.requestIdleCallback(() => cb(), { timeout });
  }
  return window.setTimeout(cb, 0);
}

/** Cancel an idle callback handle produced by onIdle (no-op if unsupported). */
export function cancelOnIdle(handle: number): void {
  if (
    "cancelIdleCallback" in window &&
    typeof window.cancelIdleCallback === "function"
  ) {
    window.cancelIdleCallback(handle);
  } else {
    window.clearTimeout(handle);
  }
}
