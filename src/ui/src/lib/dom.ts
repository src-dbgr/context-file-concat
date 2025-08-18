/**
 * Safe getter for elements by ID (resolved at access time).
 */
const getEl = (id: string) => document.getElementById(id);

/**
 * Central element accessors. Only keep what is still used after refactor.
 */
export const elements = {
  // Top bar
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

  // File list container (Svelte mounts inside)
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

  // Bottom panel
  get generateBtn() {
    return getEl("generate-btn") as HTMLButtonElement;
  },
  get saveBtn() {
    return getEl("save-btn") as HTMLButtonElement;
  },

  // Layout helpers
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
