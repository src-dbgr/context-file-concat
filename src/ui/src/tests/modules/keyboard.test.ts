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
  worksInEditor: boolean
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
      true
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
      false // not allowed in editor
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
