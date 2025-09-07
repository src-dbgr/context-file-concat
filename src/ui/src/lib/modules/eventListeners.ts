// Only Undo/Redo for native inputs (Generate/Save is in Footer.svelte).

export function setupEventListeners() {
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
