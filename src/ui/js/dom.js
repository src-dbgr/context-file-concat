// Collects all DOM-Element-References at one place
export const elements = {
  // Top bar
  selectDirBtn: document.getElementById("select-dir-btn"),
  clearDirBtn: document.getElementById("clear-dir-btn"),
  currentPath: document.getElementById("current-path"),
  currentConfigFilename: document.getElementById("current-config-filename"),
  importConfigBtn: document.getElementById("import-config-btn"),
  exportConfigBtn: document.getElementById("export-config-btn"), // Sidebar
  searchQuery: document.getElementById("search-query"),
  extensionFilter: document.getElementById("extension-filter"),
  contentSearchQuery: document.getElementById("content-search-query"),
  caseSensitive: document.getElementById("case-sensitive"),
  rescanBtn: document.getElementById("rescan-btn"),
  newIgnorePattern: document.getElementById("new-ignore-pattern"),
  addPatternBtn: document.getElementById("add-pattern-btn"),
  commonPatternsContainer: document.getElementById("common-patterns-container"),
  deleteAllPatternsBtn: document.getElementById("delete-all-patterns-btn"),
  removeEmptyDirs: document.getElementById("remove-empty-dirs"),
  filterPatterns: document.getElementById("filter-patterns"),
  currentPatternsContainer: document.getElementById(
    "current-patterns-container"
  ), // File List
  fileStats: document.getElementById("file-stats"),
  selectAllBtn: document.getElementById("select-all-btn"),
  deselectAllBtn: document.getElementById("deselect-all-btn"),
  expandAllBtn: document.getElementById("expand-all-btn"),
  collapseAllBtn: document.getElementById("collapse-all-btn"),
  fileTreeContainer: document.getElementById("file-tree-container"), // Preview/Editor
  previewTitle: document.getElementById("preview-title"),
  copyBtn: document.getElementById("copy-btn"),
  clearPreviewBtn: document.getElementById("clear-preview-btn"),
  editorContainer: document.getElementById("editor-container"), // Bottom Panel
  generateBtn: document.getElementById("generate-btn"),
  saveBtn: document.getElementById("save-btn"),
  includeTree: document.getElementById("include-tree"),
  relativePaths: document.getElementById("relative-paths"),
  outputDir: document.getElementById("output-dir"),
  browseOutputDirBtn: document.getElementById("browse-output-dir"),
  outputFilename: document.getElementById("output-filename"), // Other
  statusBar: document.getElementById("status-bar"),
  indexingStatus: document.getElementById("indexing-status"),
  resizer: document.getElementById("resizer"),
  fileListPanel: document.getElementById("file-list-panel"),
  previewPanel: document.getElementById("preview-panel"),
  contentSplitter: document.querySelector(".content-splitter"),
};
