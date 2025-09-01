/* @vitest-environment jsdom */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { handleCopy, handlePaste, handleCut } from "$lib/modules/clipboard";
import { editorInstance } from "$lib/stores/app";
import { toast } from "$lib/stores/toast";

type FakeSelection = { isEmpty: () => boolean };
type FakeModel = {
  getValue: () => string;
  getValueInRange: (range: FakeSelection) => string;
};
type FakeEditor = {
  getModel: () => FakeModel | null;
  getSelection: () => FakeSelection | null;
  executeEdits: (
    source: string,
    edits: Array<{ range: FakeSelection; text: string }>
  ) => void;
};

function ensureStatus() {
  const status = document.createElement("div");
  status.className = "status-text";
  document.body.appendChild(status);
}

function ensureCopyBtn() {
  const btn = document.createElement("button");
  btn.id = "copy-btn";
  document.body.appendChild(btn);
  return btn;
}

function ctx(
  o: Partial<{
    activeEl: HTMLElement;
    isEditorFocused: boolean;
    isInNormalInputField: boolean;
  }> = {}
) {
  return {
    activeEl: o.activeEl ?? document.body,
    isEditorFocused: o.isEditorFocused ?? false,
    isInNormalInputField: o.isInNormalInputField ?? false,
  };
}

function mockNavigatorClipboard(opts?: {
  writeOk?: boolean;
  readOk?: boolean;
  readText?: string;
}) {
  const { writeOk = true, readOk = true, readText = "" } = opts ?? {};
  const clip = {
    writeText: writeOk
      ? vi.fn().mockResolvedValue(undefined)
      : vi.fn().mockRejectedValue(new Error("write-fail")),
    readText: readOk
      ? vi.fn().mockResolvedValue(readText)
      : vi.fn().mockRejectedValue(new Error("read-fail")),
  } as Pick<Clipboard, "writeText" | "readText">;

  Object.defineProperty(globalThis.navigator, "clipboard", {
    configurable: true,
    get: () => clip,
  });

  return clip;
}

function setEditor(e: FakeEditor | null) {
  (editorInstance as unknown as { set: (v: unknown) => void }).set(e);
}

describe("clipboard – editor-focused paths", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
    ensureStatus();
    setEditor(null);

    // Make toast spies return a number (matches NormalizedProcedure signature)
    vi.spyOn(toast, "success").mockReturnValue(1);
    vi.spyOn(toast, "error").mockReturnValue(1);
    vi.spyOn(toast, "info").mockReturnValue(1);
    vi.spyOn(toast, "warning").mockReturnValue(1);
  });

  it("handleCopy (editor) copies selection when present", async () => {
    ensureCopyBtn();
    const clip = mockNavigatorClipboard();

    const sel: FakeSelection = { isEmpty: () => false };
    const model: FakeModel = {
      getValue: () => "ALL",
      getValueInRange: () => "SEL",
    };
    const exec = vi.fn();
    const ed: FakeEditor = {
      getModel: () => model,
      getSelection: () => sel,
      executeEdits: exec,
    };
    setEditor(ed);

    await handleCopy(ctx({ isEditorFocused: true }));

    expect(clip.writeText).toHaveBeenCalledWith("SEL");
  });

  it("handleCopy (editor) copies entire content when selection is empty", async () => {
    ensureCopyBtn();
    const clip = mockNavigatorClipboard();

    const sel: FakeSelection = { isEmpty: () => true };
    const model: FakeModel = {
      getValue: () => "ALL",
      getValueInRange: () => "SHOULD_NOT_BE_USED",
    };
    const ed: FakeEditor = {
      getModel: () => model,
      getSelection: () => sel,
      executeEdits: vi.fn(),
    };
    setEditor(ed);

    await handleCopy(ctx({ isEditorFocused: true }));
    expect(clip.writeText).toHaveBeenCalledWith("ALL");
  });

  it("handleCopy (editor) updates copy button UI and resets after timeout", async () => {
    const btn = ensureCopyBtn();
    mockNavigatorClipboard();

    const sel: FakeSelection = { isEmpty: () => true };
    const model: FakeModel = {
      getValue: () => "ALL",
      getValueInRange: () => "",
    };
    const ed: FakeEditor = {
      getModel: () => model,
      getSelection: () => sel,
      executeEdits: vi.fn(),
    };
    setEditor(ed);

    await handleCopy(ctx({ isEditorFocused: true }));

    expect(btn.innerHTML.toLowerCase()).toContain("copied");
    expect(btn.classList.contains("button-cta")).toBe(true);

    vi.advanceTimersByTime(1100);
    expect(btn.innerHTML.toLowerCase()).toContain("copy");
    expect(btn.classList.contains("button-cta")).toBe(false);
    expect(btn.classList.contains("button-cancel-action")).toBe(false);
  });

  it("handlePaste (editor) – prompt fallback: cancel (null) → status + info toast", async () => {
    mockNavigatorClipboard({ readOk: false }); // force prompt fallback
    vi.spyOn(window, "prompt").mockReturnValue(null);

    await handlePaste(ctx({ isEditorFocused: true }));

    const status = document.querySelector(".status-text") as HTMLElement;
    expect(status.textContent).toMatch(/paste cancelled/i);
    expect(toast.info).toHaveBeenCalled();
  });

  it("handlePaste (editor) – prompt fallback: empty string → status + warning toast", async () => {
    mockNavigatorClipboard({ readOk: false });
    vi.spyOn(window, "prompt").mockReturnValue("");

    await handlePaste(ctx({ isEditorFocused: true }));

    const status = document.querySelector(".status-text") as HTMLElement;
    expect(status.textContent).toMatch(/clipboard is empty/i);
    expect(toast.warning).toHaveBeenCalled();
  });

  it("handleCut (editor) – selection copied and removed", async () => {
    mockNavigatorClipboard();
    ensureCopyBtn();

    const sel: FakeSelection = { isEmpty: () => false };
    const model: FakeModel = {
      getValue: () => "ALL",
      getValueInRange: () => "CUTSEL",
    };
    const exec = vi.fn();
    const ed: FakeEditor = {
      getModel: () => model,
      getSelection: () => sel,
      executeEdits: exec,
    };
    setEditor(ed);

    await handleCut(ctx({ isEditorFocused: true }));

    expect(exec).toHaveBeenCalledWith("cut", [{ range: sel, text: "" }]);
  });

  it("handleCut (editor) – no selection → failure path", async () => {
    mockNavigatorClipboard();
    const sel: FakeSelection = { isEmpty: () => true };
    const model: FakeModel = {
      getValue: () => "ALL",
      getValueInRange: () => "SEL",
    };
    const ed: FakeEditor = {
      getModel: () => model,
      getSelection: () => sel,
      executeEdits: vi.fn(),
    };
    setEditor(ed);

    await handleCut(ctx({ isEditorFocused: true }));

    const status = document.querySelector(".status-text") as HTMLElement;
    expect(status.textContent).toMatch(/cut failed/i);
    expect(toast.error).toHaveBeenCalled();
  });
});

describe("clipboard – other branches", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
    ensureStatus();
    setEditor(null);
    vi.spyOn(toast, "success").mockReturnValue(1);
    vi.spyOn(toast, "error").mockReturnValue(1);
    vi.spyOn(toast, "info").mockReturnValue(1);
    vi.spyOn(toast, "warning").mockReturnValue(1);
  });

  it("handleCopy with neither input nor editor → fails gracefully", async () => {
    mockNavigatorClipboard(); // not used because content is empty
    await handleCopy(
      ctx({ isEditorFocused: false, isInNormalInputField: false })
    );

    const status = document.querySelector(".status-text") as HTMLElement;
    expect(status.textContent).toMatch(/failed to copy/i);
    expect(toast.error).toHaveBeenCalled();
  });

  it("handlePaste when unsupported context → error toast + status", async () => {
    const errSpy = vi.spyOn(toast, "error").mockReturnValue(1);
    await handlePaste(
      ctx({ isEditorFocused: false, isInNormalInputField: false })
    );

    const status = document.querySelector(".status-text") as HTMLElement;
    expect(status.textContent).toMatch(/not supported/i);
    expect(errSpy).toHaveBeenCalled();
  });
});
