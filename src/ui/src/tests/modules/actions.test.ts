/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { selectAll } from "$lib/modules/actions";
import { editorInstance } from "$lib/stores/app";

type Editor = NonNullable<Parameters<typeof editorInstance.set>[0]>;

describe("actions.selectAll", () => {
  beforeEach(() => editorInstance.set(null));

  it("selects the whole content of the editor with a plain range", () => {
    const setSelection = vi.fn();
    editorInstance.set({
      getModel: () => ({
        getLineCount: () => 3,
        getLineMaxColumn: (line: number) => (line === 3 ? 8 : 1),
      }),
      setSelection,
    } as unknown as Editor);

    selectAll({
      activeEl: document.body,
      isEditorFocused: true,
      isInNormalInputField: false,
    });

    // No Monaco class involved: the module must not import Monaco.
    expect(setSelection).toHaveBeenCalledWith({
      startLineNumber: 1,
      startColumn: 1,
      endLineNumber: 3,
      endColumn: 8,
    });
  });

  it("selects the text of a focused input field", () => {
    const input = document.createElement("input");
    input.value = "hello";
    document.body.appendChild(input);

    selectAll({
      activeEl: input,
      isEditorFocused: false,
      isInNormalInputField: true,
    });

    expect(input.selectionStart).toBe(0);
    expect(input.selectionEnd).toBe(5);
    input.remove();
  });

  it("does nothing without an editor", () => {
    expect(() =>
      selectAll({
        activeEl: document.body,
        isEditorFocused: true,
        isInNormalInputField: false,
      }),
    ).not.toThrow();
  });
});
