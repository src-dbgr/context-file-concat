import { elements } from "../dom.js";
import { post } from "../services/backend.js";
import { getState, editorInstance } from "../stores/app.js";
import { get } from "svelte/store";

export function setupEventListeners() {
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

  // Plain input undo/redo helpers
  document.body.addEventListener("focusin", (e: Event) => {
    const target = e.target as HTMLElement;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement
    ) {
      import("./undo.js").then(({ getUndoManagerForElement }) =>
        getUndoManagerForElement(target)
      );
    }
  });

  document.body.addEventListener("input", (e: Event) => {
    const target = e.target as HTMLElement;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement
    ) {
      import("./undo.js").then(({ getUndoManagerForElement }) =>
        getUndoManagerForElement(target).recordState()
      );
    }
  });
}
