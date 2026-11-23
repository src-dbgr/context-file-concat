/* @vitest-environment jsdom */

import { describe, it, expect, vi, beforeEach } from "vitest";

// The generated output as the host sent it, while the editor shows it unchanged.
const { unchangedOutput } = vi.hoisted(() => ({
  unchangedOutput: vi.fn<() => string | null>(() => null),
}));
vi.mock("$lib/modules/editor", () => ({ unchangedOutput }));

import { handleCopy, handlePaste, handleCut } from "$lib/modules/clipboard";
import { editorInstance } from "$lib/stores/app";
import type { FocusContext } from "$lib/types";
import type * as monaco from "monaco-editor/esm/vs/editor/editor.api";

type Edit = { range: FakeSelection; text: string };
type ExecuteEditsFn = (src: string, edits: Edit[]) => void;

type FakeSelection = {
  isEmpty: () => boolean;
  equalsRange?: (range: unknown) => boolean;
};
type FakeModel = {
  getValue: () => string;
  getValueInRange: (_sel: FakeSelection) => string;
  getFullModelRange?: () => unknown;
};
type FakeEditor = {
  getModel: () => FakeModel;
  getSelection: () => FakeSelection;
  executeEdits: (
    src: string,
    edits: Array<{ range: FakeSelection; text: string }>,
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
  selectsEverything = false,
}: {
  content: string;
  selectionText: string | null;
  /** The selection reaches from the start of the text to its end. */
  selectsEverything?: boolean;
}) {
  const fullRange = { full: true };
  const model: FakeModel = {
    getValue: () => content,
    getValueInRange: () => selectionText ?? "",
    getFullModelRange: () => fullRange,
  };

  const selection: FakeSelection = {
    isEmpty: () => selectionText === null,
    equalsRange: (range) => selectsEverything && range === fullRange,
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
    unchangedOutput.mockReset();
    unchangedOutput.mockReturnValue(null);
  });

  describe("copy of a generated output", () => {
    // The host's text has the line ends of its files; the editor's value has
    // one kind of line end throughout.
    const HOST = "a.txt\r\nline\r\nb.txt\nline\n";
    const EDITOR = "a.txt\nline\nb.txt\nline\n";

    it("copies the text of the host while the output is unchanged", async () => {
      const clip = mockClipboard({ writeOk: true });
      unchangedOutput.mockReturnValue(HOST);
      setupEditor({ content: EDITOR, selectionText: null });

      await handleCopy(ctxEditor());

      expect(clip.writeText).toHaveBeenCalledWith(HOST);
    });

    it("does so as well when everything is selected", async () => {
      const clip = mockClipboard({ writeOk: true });
      unchangedOutput.mockReturnValue(HOST);
      setupEditor({
        content: EDITOR,
        selectionText: EDITOR,
        selectsEverything: true,
      });

      await handleCopy(ctxEditor());

      expect(clip.writeText).toHaveBeenCalledWith(HOST);
    });

    it("copies a part of the output from the editor", async () => {
      const clip = mockClipboard({ writeOk: true });
      unchangedOutput.mockReturnValue(HOST);
      setupEditor({ content: EDITOR, selectionText: "line" });

      await handleCopy(ctxEditor());

      expect(clip.writeText).toHaveBeenCalledWith("line");
    });

    it("copies the editor's text once the output has been edited", async () => {
      const clip = mockClipboard({ writeOk: true });
      unchangedOutput.mockReturnValue(null);
      setupEditor({ content: EDITOR + "note", selectionText: null });

      await handleCopy(ctxEditor());

      expect(clip.writeText).toHaveBeenCalledWith(EDITOR + "note");
    });

    it("copies an empty output as nothing, not as the editor's text", async () => {
      const clip = mockClipboard({ writeOk: true });
      unchangedOutput.mockReturnValue("");
      setupEditor({ content: "stale", selectionText: null });

      await handleCopy(ctxEditor());

      expect(clip.writeText).not.toHaveBeenCalled();
    });
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
