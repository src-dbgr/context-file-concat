/* @vitest-environment jsdom */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { handleCopy, handlePaste, handleCut } from "$lib/modules/clipboard";
import { editorInstance } from "$lib/stores/app";
import type { FocusContext } from "$lib/types";
import type * as monaco from "monaco-editor/esm/vs/editor/editor.api";

type Edit = { range: FakeSelection; text: string };
type ExecuteEditsFn = (src: string, edits: Edit[]) => void;

type FakeSelection = {
  isEmpty: () => boolean;
};
type FakeModel = {
  getValue: () => string;
  getValueInRange: (_sel: FakeSelection) => string;
};
type FakeEditor = {
  getModel: () => FakeModel;
  getSelection: () => FakeSelection;
  executeEdits: (
    src: string,
    edits: Array<{ range: FakeSelection; text: string }>
  ) => void;
};

function makeStatusAndCopyBtn() {
  const status = document.createElement("div");
  status.className = "status-text";
  document.body.appendChild(status);

  const copyBtn = document.createElement("button");
  copyBtn.id = "copy-btn";
  document.body.appendChild(copyBtn);
}

function ctxEditor(): FocusContext {
  return {
    activeEl: document.body,
    isEditorFocused: true,
    isInNormalInputField: false,
  };
}

function setupEditor({
  content,
  selectionText,
}: {
  content: string;
  selectionText: string | null;
}) {
  const model: FakeModel = {
    getValue: () => content,
    getValueInRange: () => selectionText ?? "",
  };

  const selection: FakeSelection = {
    isEmpty: () => selectionText === null,
  };

  // Strictly typed spy for later call inspection
  const executeEdits = vi.fn<ExecuteEditsFn>();

  const ed: FakeEditor = {
    getModel: () => model,
    getSelection: () => selection,
    executeEdits,
  };

  // Cast to the Monaco editor interface without leaking `any`
  editorInstance.set(ed as unknown as monaco.editor.IStandaloneCodeEditor);
  return { executeEdits };
}

function mockClipboard(opts: {
  writeOk?: boolean;
  readOk?: boolean;
  readText?: string | null;
}) {
  const { writeOk = true, readOk = true, readText = "" } = opts;
  const clip = {
    writeText: writeOk
      ? vi.fn().mockResolvedValue(undefined)
      : vi.fn().mockRejectedValue(new Error("no-permission")),
    readText: readOk
      ? vi.fn().mockResolvedValue(readText ?? "")
      : vi.fn().mockRejectedValue(new Error("no-permission")),
  } as Pick<Clipboard, "writeText" | "readText">;

  Object.defineProperty(globalThis.navigator, "clipboard", {
    configurable: true,
    get: () => clip,
  });
  return clip;
}

describe("clipboard.* – editor-focused paths", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = "";
    makeStatusAndCopyBtn();
  });

  it("handleCopy copies selected range when editor has selection; otherwise full model", async () => {
    const clip = mockClipboard({ writeOk: true });

    // Case 1: selected range
    setupEditor({ content: "AAA BBB", selectionText: "AAA" });
    await handleCopy(ctxEditor());
    expect(clip.writeText).toHaveBeenCalledWith("AAA");

    // Case 2: no selection -> full model
    setupEditor({ content: "CCC DDD", selectionText: null });
    await handleCopy(ctxEditor());
    expect(clip.writeText).toHaveBeenCalledWith("CCC DDD");
  });

  it("handlePaste (editor) – cancelled via prompt when clipboard read fails", async () => {
    mockClipboard({ readOk: false });
    (
      globalThis as unknown as { prompt: (msg: string) => string | null }
    ).prompt = vi.fn().mockReturnValue(null);

    setupEditor({ content: "X", selectionText: "ignored" });
    await handlePaste(ctxEditor());

    const status = document.querySelector(".status-text")!;
    expect(status.textContent || "").toMatch(/cancelled/i);
  });

  it("handlePaste zeigt 'not supported' außerhalb input/editor", async () => {
    document.body.innerHTML = ""; // wichtig: keine alten .status-text Reste
    const status = document.createElement("div");
    status.className = "status-text";
    document.body.appendChild(status);

    await handlePaste({
      activeEl: document.body as unknown as HTMLElement,
      isEditorFocused: false,
      isInNormalInputField: false,
    });

    expect(status.textContent).toMatch(/not supported/i);
  });

  it("handlePaste (editor) – empty string via prompt shows 'Clipboard is empty'", async () => {
    mockClipboard({ readOk: false });
    (
      globalThis as unknown as { prompt: (msg: string) => string | null }
    ).prompt = vi.fn().mockReturnValue("");

    setupEditor({ content: "X", selectionText: "ignored" });
    await handlePaste(ctxEditor());

    const status = document.querySelector(".status-text")!;
    expect(status.textContent || "").toMatch(/Clipboard is empty/i);
  });

  it("handleCut (editor) – copies selection and applies edit", async () => {
    const clip = mockClipboard({ writeOk: true });

    const { executeEdits } = setupEditor({
      content: "HELLO",
      selectionText: "EL",
    });
    await handleCut(ctxEditor());

    expect(clip.writeText).toHaveBeenCalledWith("EL");
    // Inspect recorded edits with strict, typed calls
    const calls = executeEdits.mock.calls as ReadonlyArray<
      Parameters<ExecuteEditsFn>
    >;
    expect(calls.length).toBeGreaterThan(0);
    const [, edits] = calls[0];
    expect(Array.isArray(edits)).toBe(true);
    expect(edits[0]?.text).toBe("");
  });
});
