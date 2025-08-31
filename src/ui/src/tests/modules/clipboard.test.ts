/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { handleCopy, handlePaste, handleCut } from "$lib/modules/clipboard";
import type { FocusContext } from "$lib/types";

function makeInput(value: string, selStart: number, selEnd?: number) {
  const el = document.createElement("input");
  el.value = value;
  document.body.appendChild(el);
  el.focus();
  el.setSelectionRange(selStart, selEnd ?? selStart);
  return el;
}

function ctxFor(el: HTMLElement): FocusContext {
  return {
    activeEl: el,
    isEditorFocused: false,
    isInNormalInputField: true,
  };
}

function ensureStatusAndCopyBtn() {
  const status = document.createElement("div");
  status.className = "status-text";
  document.body.appendChild(status);

  const btn = document.createElement("button");
  btn.id = "copy-btn";
  document.body.appendChild(btn);
}

function mockNavigatorClipboard({
  writeOk = true,
  readText = "",
}: {
  writeOk?: boolean;
  readText?: string;
}) {
  const clip = {
    writeText: writeOk
      ? vi.fn().mockResolvedValue(undefined)
      : vi.fn().mockRejectedValue(new Error("nope")),
    readText: vi.fn().mockResolvedValue(readText),
  } as Pick<Clipboard, "writeText" | "readText">;

  Object.defineProperty(globalThis.navigator, "clipboard", {
    configurable: true,
    get: () => clip,
  });

  return clip;
}

function setExecCommand(
  fn: (cmd: string, arg2?: boolean, arg3?: string) => boolean
) {
  // Assign without altering global DOM typings
  (
    document as unknown as {
      execCommand: (cmd: string, arg2?: boolean, arg3?: string) => boolean;
    }
  ).execCommand = fn as unknown as (
    cmd: string,
    arg2?: boolean,
    arg3?: string
  ) => boolean;
}

describe("clipboard.* (input fields)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
    ensureStatusAndCopyBtn();
  });

  it("copies via navigator.clipboard when available", async () => {
    const el = makeInput("hello", 0, 5);
    const ctx = ctxFor(el);
    const clip = mockNavigatorClipboard({ writeOk: true });

    await handleCopy(ctx);
    expect(clip.writeText).toHaveBeenCalledWith("hello");

    vi.advanceTimersByTime(1100);
    const btn = document.getElementById("copy-btn") as HTMLButtonElement;
    expect(btn).toBeTruthy();
  });

  it("falls back to document.execCommand when navigator.clipboard fails", async () => {
    const el = makeInput("abc", 0, 3);
    const ctx = ctxFor(el);
    mockNavigatorClipboard({ writeOk: false });

    setExecCommand(vi.fn().mockReturnValue(true));
    await handleCopy(ctx);

    const exec = (document as unknown as { execCommand: unknown })
      .execCommand as unknown as (cmd: string) => boolean;
    expect(vi.isMockFunction(exec)).toBe(true);
  });

  it("reports failure if neither API works", async () => {
    const el = makeInput("xyz", 0, 3);
    const ctx = ctxFor(el);
    mockNavigatorClipboard({ writeOk: false });

    setExecCommand(() => {
      throw new Error("blocked");
    });

    await handleCopy(ctx);
    const status = document.querySelector(".status-text") as HTMLElement;
    expect(status.textContent || "").toMatch(/Failed/i);
  });

  it("handlePaste inserts via execCommand and updates status", async () => {
    const el = makeInput("", 0);
    const ctx = ctxFor(el);
    mockNavigatorClipboard({ readText: "PASTE" });

    const execSpy = vi.fn().mockReturnValue(true);
    setExecCommand(execSpy);

    await handlePaste(ctx);
    expect(execSpy).toHaveBeenCalledWith("insertText", false, "PASTE");

    const status = document.querySelector(".status-text") as HTMLElement;
    expect(status.textContent || "").toMatch(/pasted/i);
  });

  it("handleCut copies selection and deletes", async () => {
    const el = makeInput("CUTME", 0, 5);
    const ctx = ctxFor(el);
    const clip = mockNavigatorClipboard({ writeOk: true });

    const execSpy = vi.fn().mockReturnValue(true);
    setExecCommand(execSpy);

    await handleCut(ctx);
    expect(clip.writeText).toHaveBeenCalledWith("CUTME");
    expect(execSpy).toHaveBeenCalledWith("delete");
  });
});
