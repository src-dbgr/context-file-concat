import { elements } from "../dom.js";
import { post } from "../services/backend.js";
import { getState, editorInstance } from "../stores/app.js";
import { clearPreview } from "./editor.js";
import { handleCopy } from "./clipboard.js";
import { getUndoManagerForElement } from "./undo.js";
import { get } from "svelte/store";

export function setupEventListeners() {
  // File-list actions (still imperative until FileTree component)
  elements.selectAllBtn.addEventListener("click", () => post("selectAll"));
  elements.expandAllBtn.addEventListener("click", () =>
    post("expandCollapseAll", true)
  );
  elements.deselectAllBtn.addEventListener("click", () => post("deselectAll"));
  elements.collapseAllBtn.addEventListener("click", () =>
    post("expandCollapseAll", false)
  );

  // Generate / Cancel
  elements.generateBtn.addEventListener("click", () => {
    if (getState().is_generating) {
      post("cancelGeneration");
    } else {
      post("generatePreview");
    }
  });

  // Save
  elements.saveBtn.addEventListener("click", () => {
    const editor = get(editorInstance);
    if (editor) {
      post("saveFile", editor.getValue());
    }
  });

  // Preview helpers
  elements.clearPreviewBtn.addEventListener("click", clearPreview);
  elements.copyBtn.addEventListener("click", () =>
    handleCopy({
      isEditorFocused: true,
      activeEl: document.activeElement as HTMLElement,
      isInNormalInputField: false,
    })
  );

  // Input undo/redo support for remaining plain inputs (e.g., file-list header if any)
  document.body.addEventListener("focusin", (e: Event) => {
    const target = e.target as HTMLElement;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement
    ) {
      getUndoManagerForElement(target);
    }
  });

  document.body.addEventListener("input", (e: Event) => {
    const target = e.target as HTMLElement;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement
    ) {
      getUndoManagerForElement(target).recordState();
    }
  });
}
