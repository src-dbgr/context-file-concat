// Collects all DOM-Element-References at one place with proper types
export const elements = {
  // Top bar
  selectDirBtn: document.getElementById("select-dir-btn") as HTMLButtonElement,
  clearDirBtn: document.getElementById("clear-dir-btn") as HTMLButtonElement,
  currentPath: document.getElementById("current-path") as HTMLSpanElement,
  currentConfigFilename: document.getElementById(
    "current-config-filename"
  ) as HTMLSpanElement,
  importConfigBtn: document.getElementById(
    "import-config-btn"
  ) as HTMLButtonElement,
  exportConfigBtn: document.getElementById(
    "export-config-btn"
  ) as HTMLButtonElement,

  // Sidebar
  searchQuery: document.getElementById("search-query") as HTMLInputElement,
  extensionFilter: document.getElementById(
    "extension-filter"
  ) as HTMLInputElement,
  contentSearchQuery: document.getElementById(
    "content-search-query"
  ) as HTMLInputElement,
  caseSensitive: document.getElementById("case-sensitive") as HTMLInputElement,
  rescanBtn: document.getElementById("rescan-btn") as HTMLButtonElement,
  newIgnorePattern: document.getElementById(
    "new-ignore-pattern"
  ) as HTMLInputElement,
  addPatternBtn: document.getElementById(
    "add-pattern-btn"
  ) as HTMLButtonElement,
  commonPatternsContainer: document.getElementById(
    "common-patterns-container"
  ) as HTMLDivElement,
  deleteAllPatternsBtn: document.getElementById(
    "delete-all-patterns-btn"
  ) as HTMLButtonElement,
  removeEmptyDirs: document.getElementById(
    "remove-empty-dirs"
  ) as HTMLInputElement,
  filterPatterns: document.getElementById(
    "filter-patterns"
  ) as HTMLInputElement,
  currentPatternsContainer: document.getElementById(
    "current-patterns-container"
  ) as HTMLDivElement,

  // File List
  fileStats: document.getElementById("file-stats") as HTMLDivElement,
  selectAllBtn: document.getElementById("select-all-btn") as HTMLButtonElement,
  deselectAllBtn: document.getElementById(
    "deselect-all-btn"
  ) as HTMLButtonElement,
  expandAllBtn: document.getElementById("expand-all-btn") as HTMLButtonElement,
  collapseAllBtn: document.getElementById(
    "collapse-all-btn"
  ) as HTMLButtonElement,
  fileTreeContainer: document.getElementById(
    "file-tree-container"
  ) as HTMLDivElement,

  // Preview/Editor
  previewTitle: document.getElementById("preview-title") as HTMLHeadingElement,
  copyBtn: document.getElementById("copy-btn") as HTMLButtonElement,
  clearPreviewBtn: document.getElementById(
    "clear-preview-btn"
  ) as HTMLButtonElement,
  editorContainer: document.getElementById(
    "editor-container"
  ) as HTMLDivElement,

  // Bottom Panel
  generateBtn: document.getElementById("generate-btn") as HTMLButtonElement,
  saveBtn: document.getElementById("save-btn") as HTMLButtonElement,
  includeTree: document.getElementById("include-tree") as HTMLInputElement,
  relativePaths: document.getElementById("relative-paths") as HTMLInputElement,
  outputDir: document.getElementById("output-dir") as HTMLInputElement,
  browseOutputDirBtn: document.getElementById(
    "browse-output-dir"
  ) as HTMLButtonElement,
  outputFilename: document.getElementById(
    "output-filename"
  ) as HTMLInputElement,

  // Other
  statusBar: document.getElementById("status-bar") as HTMLDivElement,
  indexingStatus: document.getElementById("indexing-status") as HTMLDivElement,
  resizer: document.getElementById("resizer") as HTMLDivElement,
  fileListPanel: document.getElementById("file-list-panel") as HTMLDivElement,
  previewPanel: document.getElementById("preview-panel") as HTMLDivElement,
  contentSplitter: document.querySelector(
    ".content-splitter"
  ) as HTMLDivElement,
};
