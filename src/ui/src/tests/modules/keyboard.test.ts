/* @vitest-environment jsdom */

import { describe, it, expect, beforeAll, beforeEach, afterEach } from "vitest";
import { setupGlobalKeyboardListeners } from "$lib/modules/keyboard";
import { commands } from "$lib/modules/commands";
import type { FocusContext } from "$lib/types";

type TestCommand = (e: KeyboardEvent, ctx: FocusContext) => void;

function focusInput(): HTMLInputElement {
  const el = document.createElement("input");
  document.body.appendChild(el);
  el.focus();
  return el;
}

function focusEditor(): HTMLInputElement {
  const wrapper = document.createElement("div");
  wrapper.className = "monaco-editor";
  const el = document.createElement("input");
  wrapper.appendChild(el);
  document.body.appendChild(wrapper);
  el.focus();
  return el;
}

let originalCommands: typeof commands extends Array<infer T> ? T[] : never;

beforeAll(() => {
  setupGlobalKeyboardListeners();
});

beforeEach(() => {
  // Snapshot original command objects
  originalCommands = commands.slice();
});

afterEach(() => {
  // Restore original commands array (same reference, mutated in-place)
  commands.length = 0;
  commands.push(...originalCommands);
  document.body.innerHTML = "";
});

function replaceCommandsWithSingle(
  matcher: (e: KeyboardEvent) => boolean,
  executor: TestCommand,
  worksInEditor: boolean,
) {
  commands.length = 0;
  commands.push({
    matches: matcher,
    execute: executor,
    isUndoable: false,
    worksInEditor,
  });
}

describe("keyboard.ts – global handler integration", () => {
  it("executes a matching command in input context and prevents default", () => {
    focusInput();
    let called = false;

    replaceCommandsWithSingle(
      (e) => e.key === "K",
      (_e, _ctx) => {
        called = true;
      },
      true,
    );

    const ev = new KeyboardEvent("keydown", { key: "K", cancelable: true });
    const allowed = document.dispatchEvent(ev);
    expect(called).toBe(true);
    expect(allowed).toBe(false); // preventDefault() was called
  });

  it("skips commands with worksInEditor=false in editor context", () => {
    focusEditor();
    let called = false;

    replaceCommandsWithSingle(
      () => true,
      () => {
        called = true;
      },
      false, // not allowed in editor
    );

    const ev = new KeyboardEvent("keydown", { key: "X", cancelable: true });
    const allowed = document.dispatchEvent(ev);

    expect(called).toBe(false);
    expect(allowed).toBe(true); // no preventDefault when nothing handled in editor
  });

  it("blocks unhandled non-modifier keys when focus is not in input/editor", () => {
    // Ensure nothing is focused inside .monaco-editor or an input
    (document.activeElement as HTMLElement | null)?.blur?.();

    // No commands -> array empty
    commands.length = 0;

    const ev = new KeyboardEvent("keydown", { key: "F2", cancelable: true });
    const allowed = document.dispatchEvent(ev);
    expect(allowed).toBe(false); // blocked by safety guard
  });

  it("leaves Tab alone wherever the focus is, so it can move on", () => {
    commands.length = 0;
    const button = document.createElement("button");
    document.body.appendChild(button);
    button.focus();

    for (const shiftKey of [false, true]) {
      const tab = new KeyboardEvent("keydown", {
        key: "Tab",
        shiftKey,
        bubbles: true,
        cancelable: true,
      });
      expect(button.dispatchEvent(tab)).toBe(true);
    }
    button.blur();
    const tab = new KeyboardEvent("keydown", {
      key: "Tab",
      bubbles: true,
      cancelable: true,
    });
    expect(document.body.dispatchEvent(tab)).toBe(true);
    button.remove();
  });

  it("leaves Enter to the summary of a details element, which opens it", () => {
    commands.length = 0;
    const details = document.createElement("details");
    const summary = document.createElement("summary");
    details.appendChild(summary);
    document.body.appendChild(details);
    summary.focus();

    const enter = new KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true,
      cancelable: true,
    });
    expect(summary.dispatchEvent(enter)).toBe(true);
    details.remove();
  });

  it("leaves Enter to a focused button, which acts on it itself", () => {
    commands.length = 0;
    const button = document.createElement("button");
    document.body.appendChild(button);
    button.focus();

    const enter = new KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true,
      cancelable: true,
    });
    expect(button.dispatchEvent(enter)).toBe(true);

    // Keys the button does nothing with are still blocked.
    const f2 = new KeyboardEvent("keydown", {
      key: "F2",
      bubbles: true,
      cancelable: true,
    });
    expect(button.dispatchEvent(f2)).toBe(false);
    button.remove();
  });

  it("still blocks Enter where nothing acts on it", () => {
    commands.length = 0;
    const div = document.createElement("div");
    div.tabIndex = 0;
    document.body.appendChild(div);
    div.focus();

    const enter = new KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true,
      cancelable: true,
    });
    expect(div.dispatchEvent(enter)).toBe(false);
    div.remove();
  });

  it("allows printable characters in inputs to pass through when no command matches", () => {
    focusInput();
    commands.length = 0;

    const ev = new KeyboardEvent("keydown", { key: "x", cancelable: true });
    const allowed = document.dispatchEvent(ev);
    expect(allowed).toBe(true);
  });

  it("allows ArrowLeft in inputs (whitelisted) when no command matches", () => {
    focusInput();
    commands.length = 0;

    const ev = new KeyboardEvent("keydown", {
      key: "ArrowLeft",
      cancelable: true,
    });
    const allowed = document.dispatchEvent(ev);
    expect(allowed).toBe(true);
  });
});
