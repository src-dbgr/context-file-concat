/**
 * A helper function to safely get an element by its ID.
 * The query is executed when the function is called, not on module load.
 */
const getEl = (id: string) => document.getElementById(id);

/**
 * An object that provides access to DOM elements using getters.
 * This ensures that `document.getElementById` is only called when an element
 * is actually accessed, preventing errors where the script executes before
 * the DOM is fully parsed.
 */
export const elements = {
  // Top bar (partially managed by Header.svelte, but IDs still exist)
  get selectDirBtn() {
    return getEl("select-dir-btn") as HTMLButtonElement;
  },
  get clearDirBtn() {
    return getEl("clear-dir-btn") as HTMLButtonElement;
  },
  get currentPath() {
    return getEl("current-path") as HTMLSpanElement;
  },
  get currentConfigFilename() {
    return getEl("current-config-filename") as HTMLSpanElement;
  },
  get importConfigBtn() {
    return getEl("import-config-btn") as HTMLButtonElement;
  },
  get exportConfigBtn() {
    return getEl("export-config-btn") as HTMLButtonElement;
  },

  // Sidebar
  get searchQuery() {
    return getEl("search-query") as HTMLInputElement;
  },
  get extensionFilter() {
    return getEl("extension-filter") as HTMLInputElement;
  },
  get contentSearchQuery() {
    return getEl("content-search-query") as HTMLInputElement;
  },
  get caseSensitive() {
    return getEl("case-sensitive") as HTMLInputElement;
  },
  get rescanBtn() {
    return getEl("rescan-btn") as HTMLButtonElement;
  },
  get newIgnorePattern() {
    return getEl("new-ignore-pattern") as HTMLInputElement;
  },
  get addPatternBtn() {
    return getEl("add-pattern-btn") as HTMLButtonElement;
  },
  get commonPatternsContainer() {
    return getEl("common-patterns-container") as HTMLDivElement;
  },
  get deleteAllPatternsBtn() {
    return getEl("delete-all-patterns-btn") as HTMLButtonElement;
  },
  get removeEmptyDirs() {
    return getEl("remove-empty-dirs") as HTMLInputElement;
  },
  get filterPatterns() {
    return getEl("filter-patterns") as HTMLInputElement;
  },
  get currentPatternsContainer() {
    return getEl("current-patterns-container") as HTMLDivElement;
  },

  // File List
  get fileStats() {
    return getEl("file-stats") as HTMLDivElement;
  },
  get selectAllBtn() {
    return getEl("select-all-btn") as HTMLButtonElement;
  },
  get deselectAllBtn() {
    return getEl("deselect-all-btn") as HTMLButtonElement;
  },
  get expandAllBtn() {
    return getEl("expand-all-btn") as HTMLButtonElement;
  },
  get collapseAllBtn() {
    return getEl("collapse-all-btn") as HTMLButtonElement;
  },
  get fileTreeContainer() {
    return getEl("file-tree-container") as HTMLDivElement;
  },

  // Preview/Editor
  get previewTitle() {
    return getEl("preview-title") as HTMLHeadingElement;
  },
  get copyBtn() {
    return getEl("copy-btn") as HTMLButtonElement;
  },
  get clearPreviewBtn() {
    return getEl("clear-preview-btn") as HTMLButtonElement;
  },
  get editorContainer() {
    return getEl("editor-container") as HTMLDivElement;
  },

  // Bottom Panel
  get generateBtn() {
    return getEl("generate-btn") as HTMLButtonElement;
  },
  get saveBtn() {
    return getEl("save-btn") as HTMLButtonElement;
  },

  // Other
  get resizer() {
    return getEl("resizer") as HTMLDivElement;
  },
  get fileListPanel() {
    return getEl("file-list-panel") as HTMLDivElement;
  },
  get previewPanel() {
    return getEl("preview-panel") as HTMLDivElement;
  },
  get contentSplitter() {
    return document.querySelector(".content-splitter") as HTMLDivElement;
  },
};
