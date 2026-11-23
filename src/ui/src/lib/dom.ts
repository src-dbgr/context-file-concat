/**
 * Minimal DOM accessors that remain after Svelte migration.
 */
export const elements = {
  /** The element Monaco mounts into (rendered by PreviewPanel). */
  get editorContainer() {
    return document.getElementById("editor-container") as HTMLDivElement;
  },
};
