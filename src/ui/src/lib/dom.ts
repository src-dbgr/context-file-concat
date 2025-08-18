/**
 * Minimal DOM accessors that remain after Svelte migration.
 */
const getEl = (id: string) => document.getElementById(id);

export const elements = {
  // Preview/Editor
  get editorContainer() {
    return getEl("editor-container") as HTMLDivElement;
  },

  get copyBtn() {
    return getEl("copy-btn") as HTMLButtonElement;
  },
  get clearPreviewBtn() {
    return getEl("clear-preview-btn") as HTMLButtonElement;
  },
};
