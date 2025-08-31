/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { getUndoManagerForElement } from "$lib/modules/undo";

function makeInput(initial = "") {
  const el = document.createElement("textarea");
  el.value = initial;
  document.body.appendChild(el);
  el.focus();
  return el;
}

describe("UndoManager (text inputs)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
  });

  it("push → undo → redo basic flow and fires input events on apply", () => {
    const el = makeInput("a");
    const manager = getUndoManagerForElement(el);

    let inputEvents = 0;
    el.addEventListener("input", () => inputEvents++);

    el.value = "ab";
    manager.recordState();
    vi.advanceTimersByTime(600);

    manager.undo();
    expect(el.value).toBe("a");
    expect(inputEvents).toBeGreaterThanOrEqual(1);

    manager.redo();
    expect(el.value).toBe("ab");
  });

  it("coalesces multiple rapid edits into a single stack entry", () => {
    const el = makeInput("a");
    const manager = getUndoManagerForElement(el);

    el.value = "ab";
    manager.recordState();

    el.value = "abc";
    manager.recordState();

    vi.advanceTimersByTime(500);

    manager.undo();
    expect(el.value).toBe("a");
  });

  it("redo stack is cleared after a new forced record", () => {
    const el = makeInput("start");
    const manager = getUndoManagerForElement(el);

    el.value = "mid";
    manager.recordState();
    vi.advanceTimersByTime(500);

    manager.undo();
    expect(el.value).toBe("start");

    el.value = "final";
    manager.recordState(true);

    manager.redo();
    expect(el.value).toBe("final");
  });

  it("separate elements get separate histories", () => {
    const a = makeInput("x");
    const b = makeInput("1");

    const ua = getUndoManagerForElement(a);
    const ub = getUndoManagerForElement(b);

    a.value = "xx";
    ua.recordState();
    b.value = "12";
    ub.recordState();

    vi.advanceTimersByTime(500);

    ua.undo();
    expect(a.value).toBe("x");
    ub.undo();
    expect(b.value).toBe("1");
  });
});
