import { elements } from '../dom.js';
import { post } from '../services/backend.js';
import { state } from '../state.js';
import { clearPreview } from './editor.js';
import { copyToClipboard } from './clipboard.js';
import { renderUI } from './renderer.js';

let filterDebounceTimeout;

function onFilterChange() {
  clearTimeout(filterDebounceTimeout);
  filterDebounceTimeout = setTimeout(() => {
    post("updateFilters", {
      searchQuery: elements.searchQuery.value,
      extensionFilter: elements.extensionFilter.value,
      contentSearchQuery: elements.contentSearchQuery.value,
    });
  }, 300);
}

function onConfigChange() {
  const appState = state.get();
  const newConfig = {
    ...appState.config,
    case_sensitive_search: elements.caseSensitive.checked,
    include_tree_by_default: elements.includeTree.checked,
    use_relative_paths: elements.relativePaths.checked,
    remove_empty_directories: elements.removeEmptyDirs.checked,
    output_filename: elements.outputFilename.value,
    output_directory: elements.outputDir.value,
  };
  post("updateConfig", newConfig);
}

function addIgnorePattern() {
    const pattern = elements.newIgnorePattern.value.trim();
    if (pattern) {
        const appState = state.get();
        if (!appState.config.ignore_patterns.includes(pattern)) {
             const newConfig = {
                ...appState.config,
                ignore_patterns: [...appState.config.ignore_patterns, pattern],
            };
            post("updateConfig", newConfig);
        }
        elements.newIgnorePattern.value = "";
    }
}

export function setupEventListeners() {
  elements.selectDirBtn.addEventListener("click", () => post("selectDirectory"));
  elements.clearDirBtn.addEventListener("click", () => post("clearDirectory"));
  elements.rescanBtn.addEventListener("click", () => post("rescanDirectory"));
  elements.importConfigBtn.addEventListener("click", () => post("importConfig"));
  elements.exportConfigBtn.addEventListener("click", () => post("exportConfig"));
  elements.selectAllBtn.addEventListener("click", () => post("selectAll"));
  elements.deselectAllBtn.addEventListener("click", () => post("deselectAll"));
  elements.expandAllBtn.addEventListener("click", () => post("expandCollapseAll", true));
  elements.collapseAllBtn.addEventListener("click", () => post("expandCollapseAll", false));
  elements.generateBtn.addEventListener("click", () => post("generatePreview"));
  elements.saveBtn.addEventListener("click", () => post("saveFile", state.getEditor().getValue()));
  elements.browseOutputDirBtn.addEventListener("click", () => post("pickOutputDirectory"));
  elements.clearPreviewBtn.addEventListener("click", clearPreview);
  elements.copyBtn.addEventListener("click", copyToClipboard);

  // --- Change/Input Listeners ---
  ['change', 'input'].forEach(evt => {
    elements.includeTree.addEventListener(evt, onConfigChange);
    elements.relativePaths.addEventListener(evt, onConfigChange);
    elements.outputFilename.addEventListener(evt, onConfigChange);
    elements.caseSensitive.addEventListener(evt, onConfigChange);
    elements.removeEmptyDirs.addEventListener(evt, onConfigChange);
    elements.outputDir.addEventListener(evt, onConfigChange);
  });
  
  ['input'].forEach(evt => {
    elements.searchQuery.addEventListener(evt, onFilterChange);
    elements.extensionFilter.addEventListener(evt, onFilterChange);
    elements.contentSearchQuery.addEventListener(evt, onFilterChange);
    elements.filterPatterns.addEventListener(evt, (e) => {
        state.setPatternFilter(e.target.value.toLowerCase());
        renderUI(); // Re-render to apply filter
    });
  });

  elements.addPatternBtn.addEventListener("click", addIgnorePattern);
  elements.newIgnorePattern.addEventListener("keydown", (e) => {
    if (e.key === "Enter") addIgnorePattern();
  });
  
  elements.deleteAllPatternsBtn.addEventListener("click", () => {
    post("updateConfig", { ...state.get().config, ignore_patterns: [] });
  });
}
