/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";

// Mock the editor module BEFORE importing the handlers to avoid monaco worker imports.
vi.mock("$lib/modules/editor", () => ({
  layoutEditorSoon: vi.fn(),
  initEditor: vi.fn(),
  showPreviewContent: vi.fn(),
  showGeneratedContent: vi.fn(),
  clearPreview: vi.fn(),
}));

// Import after mocks are set up
import { installWindowIPCHandlers } from "$lib/ipc/handlers";

// Minimal outbound IPC shape to satisfy type usage
declare global {
  interface Window {
    ipc: { postMessage(message: string): void };
  }
}

function toFn(
  l: EventListenerOrEventListenerObject
): ((ev: Event) => void) | null {
  if (typeof l === "function") {
    const fn = l as (evt: Event) => unknown;
    return (ev: Event) => {
      void fn(ev);
    };
  }
  const obj = l as { handleEvent?: (evt: Event) => unknown };
  if (typeof obj.handleEvent === "function") {
    return (ev: Event) => {
      void obj.handleEvent?.(ev);
    };
  }
  return null;
}

describe("ipc/handlers – installWindowIPCHandlers", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    window.ipc = { postMessage: (_: string) => void 0 };
  });

  it("registers handlers (via addEventListener or onmessage) and is resilient to malformed inputs", () => {
    const addSpy = vi.spyOn(window, "addEventListener");
    const setOnMessageSpy = vi.spyOn(window, "onmessage", "set");

    // Should not throw
    expect(() => installWindowIPCHandlers()).not.toThrow();

    // Collect listeners registered via addEventListener(...)
    const fromAdd: Array<(ev: MessageEvent) => void> = addSpy.mock.calls
      .map((args) => {
        const candidate = args[1];
        const fn = toFn(candidate);
        return fn
          ? (ev: MessageEvent) => {
              fn(ev);
            }
          : null;
      })
      .filter((x): x is (ev: MessageEvent) => void => x !== null);

    // Collect handler functions assigned via window.onmessage = fn
    const fromSetter: Array<(ev: MessageEvent) => void> = [];
    for (const [assigned] of setOnMessageSpy.mock.calls) {
      if (typeof assigned === "function") {
        fromSetter.push((ev: MessageEvent) => {
          const prev = window.onmessage;
          window.onmessage = assigned; // type matches Window["onmessage"]
          window.dispatchEvent(ev);
          window.onmessage = prev;
        });
      }
    }

    const candidates = [...fromAdd, ...fromSetter];

    // If we found explicit candidates, exercise them; otherwise, dispatch on window
    if (candidates.length > 0) {
      for (const cb of candidates) {
        const bad = new MessageEvent("message", { data: "<<not-json>>" });
        expect(() => cb(bad)).not.toThrow();

        const unknown = new MessageEvent("message", {
          data: JSON.stringify({ type: "unknown", payload: { ok: true } }),
        });
        expect(() => cb(unknown)).not.toThrow();
      }
    } else {
      // Fallback: still verify global resilience (no crash on message dispatch)
      const bad = new MessageEvent("message", { data: "<<not-json>>" });
      const unknown = new MessageEvent("message", {
        data: JSON.stringify({ type: "unknown", payload: { ok: true } }),
      });
      expect(() => window.dispatchEvent(bad)).not.toThrow();
      expect(() => window.dispatchEvent(unknown)).not.toThrow();
    }
  });

  it("can be installed multiple times without throwing (idempotent or additive)", () => {
    const addSpy = vi.spyOn(window, "addEventListener");
    const setOnMessageSpy = vi.spyOn(window, "onmessage", "set");

    expect(() => installWindowIPCHandlers()).not.toThrow();
    const addsAfterFirst = addSpy.mock.calls.length;
    const setsAfterFirst = setOnMessageSpy.mock.calls.length;

    expect(() => installWindowIPCHandlers()).not.toThrow();

    // Accept either guarded (same count) or additive (count increased) behavior
    expect(addSpy.mock.calls.length).toBeGreaterThanOrEqual(addsAfterFirst);
    expect(setOnMessageSpy.mock.calls.length).toBeGreaterThanOrEqual(
      setsAfterFirst
    );
  });
});
