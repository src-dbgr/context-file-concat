// src/tests/setup.ts
import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/svelte";

// 👉 Tests nach jedem Testlauf aufräumen (verhindert DOM-Duplikate zwischen Tests)
afterEach(() => cleanup());

// — ResizeObserver Polyfill ohne ungenutzte Property —
class RO implements ResizeObserver {
  constructor(_cb: ResizeObserverCallback) {} // absichtlich ungenutzt
  observe(_target: Element): void {}
  unobserve(_target: Element): void {}
  disconnect(): void {}
  takeRecords(): ResizeObserverEntry[] {
    return [];
  }
}

// Nur im DOM-Kontext polyfillen
const hasDOM =
  typeof window !== "undefined" &&
  typeof document !== "undefined" &&
  typeof Element !== "undefined";

if (hasDOM) {
  // Typ-sicheres Global-Assign
  type ROConstructor = new (cb: ResizeObserverCallback) => ResizeObserver;
  const g = globalThis as { ResizeObserver?: ROConstructor };
  if (!g.ResizeObserver) g.ResizeObserver = RO as unknown as ROConstructor;

  // RAF-Shims
  if (
    !(globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame
  ) {
    (
      globalThis as {
        requestAnimationFrame: (cb: FrameRequestCallback) => number;
      }
    ).requestAnimationFrame = (cb) =>
      setTimeout(() => cb(performance.now()), 0) as unknown as number;
  }
  if (
    !(globalThis as { cancelAnimationFrame?: unknown }).cancelAnimationFrame
  ) {
    (
      globalThis as { cancelAnimationFrame: (id: number) => void }
    ).cancelAnimationFrame = (id) => clearTimeout(id);
  }

  // scrollTo Shim
  if (typeof HTMLElement !== "undefined" && !HTMLElement.prototype.scrollTo) {
    HTMLElement.prototype.scrollTo = function (
      x?: number | ScrollToOptions,
      _y?: number
    ): void {
      if (
        typeof x === "object" &&
        typeof (x as ScrollToOptions).top === "number"
      ) {
        // jsdom erlaubt Assignment von scrollTop
        (this as HTMLElement & { scrollTop: number }).scrollTop =
          (x as ScrollToOptions).top ?? 0;
      }
    };
  }
}
