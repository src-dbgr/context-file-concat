import { elements } from "../dom.js";
import { post } from "../services/backend.js";
import { state } from "../state.js";
import { clearPreview } from "./editor.js";
import { copyToClipboard } from "./clipboard.js";
import { renderUI } from "./renderer.js";

let filterDebounceTimeout;

function onFilterChange() {
  // Only apply filters if a directory is selected
  const appState = state.get();
  if (!appState.current_path) {
    return;
  }

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

/**
 * Validates if search operations should be enabled based on current state
 * @returns {boolean} True if search should be enabled
 */
function shouldEnableSearch() {
  const appState = state.get();
  return !!(appState.current_path && !appState.is_scanning);
}

/**
 * Updates the UI state of search inputs based on whether search should be enabled
 */
function updateSearchInputsState() {
  const searchEnabled = shouldEnableSearch();

  elements.searchQuery.disabled = !searchEnabled;
  elements.extensionFilter.disabled = !searchEnabled;
  elements.contentSearchQuery.disabled = !searchEnabled;

  if (!searchEnabled) {
    elements.searchQuery.placeholder = "Select a directory first...";
    elements.extensionFilter.placeholder = "Select a directory first...";
    elements.contentSearchQuery.placeholder = "Select a directory first...";
  } else {
    elements.searchQuery.placeholder = "Search filenames...";
    elements.extensionFilter.placeholder = "Filter by extension (e.g., rs, py)";
    elements.contentSearchQuery.placeholder = "Search text inside files...";
  }
}

export function setupEventListeners() {
  elements.selectDirBtn.addEventListener("click", () =>
    post("selectDirectory")
  );
  elements.clearDirBtn.addEventListener("click", () => post("clearDirectory"));
  elements.rescanBtn.addEventListener("click", () => post("rescanDirectory"));
  elements.importConfigBtn.addEventListener("click", () =>
    post("importConfig")
  );
  elements.exportConfigBtn.addEventListener("click", () =>
    post("exportConfig")
  );
  elements.selectAllBtn.addEventListener("click", () => post("selectAll"));
  elements.deselectAllBtn.addEventListener("click", () => post("deselectAll"));
  elements.expandAllBtn.addEventListener("click", () =>
    post("expandCollapseAll", true)
  );
  elements.collapseAllBtn.addEventListener("click", () =>
    post("expandCollapseAll", false)
  );
  elements.generateBtn.addEventListener("click", () => post("generatePreview"));
  elements.saveBtn.addEventListener("click", () =>
    post("saveFile", state.getEditor().getValue())
  );
  elements.browseOutputDirBtn.addEventListener("click", () =>
    post("pickOutputDirectory")
  );
  elements.clearPreviewBtn.addEventListener("click", clearPreview);
  elements.copyBtn.addEventListener("click", copyToClipboard);

  // --- Change/Input Listeners ---
  ["change", "input"].forEach((evt) => {
    elements.includeTree.addEventListener(evt, onConfigChange);
    elements.relativePaths.addEventListener(evt, onConfigChange);
    elements.outputFilename.addEventListener(evt, onConfigChange);
    elements.caseSensitive.addEventListener(evt, onConfigChange);
    elements.removeEmptyDirs.addEventListener(evt, onConfigChange);
    elements.outputDir.addEventListener(evt, onConfigChange);
  });

  // Enhanced filter event listeners with validation
  ["input"].forEach((evt) => {
    elements.searchQuery.addEventListener(evt, (e) => {
      if (shouldEnableSearch()) {
        onFilterChange();
      }
    });
    elements.extensionFilter.addEventListener(evt, (e) => {
      if (shouldEnableSearch()) {
        onFilterChange();
      }
    });
    elements.contentSearchQuery.addEventListener(evt, (e) => {
      if (shouldEnableSearch()) {
        onFilterChange();
      }
    });
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

  // Initialize search inputs state
  updateSearchInputsState();

  // Update search inputs state when app state changes (called from main.js render function)
  window.updateSearchInputsState = updateSearchInputsState;
}
