import { elements } from "../dom.js";
import { post } from "../services/backend.js";
import { getState, patternFilter, editorInstance } from "../stores/app.js";
import { clearPreview } from "./editor.js";
import { handleCopy } from "./clipboard.js";
import { getUndoManagerForElement } from "./undo.js";
import { get } from "svelte/store";

let filterDebounceTimeout: number | undefined;

function onFilterChange() {
  const currentAppState = getState();
  if (!currentAppState.current_path) {
    return;
  }

  clearTimeout(filterDebounceTimeout);
  filterDebounceTimeout = window.setTimeout(() => {
    post("updateFilters", {
      searchQuery: elements.searchQuery.value,
      extensionFilter: elements.extensionFilter.value,
      contentSearchQuery: elements.contentSearchQuery.value,
    });
  }, 300);
}

function onConfigChange() {
  const currentConfig = getState().config;
  const newConfig = {
    ...currentConfig,
    case_sensitive_search: elements.caseSensitive.checked,
    remove_empty_directories: elements.removeEmptyDirs.checked,
  };
  post("updateConfig", newConfig);
}

function onConfigAndFilterChange() {
  onConfigChange();
  onFilterChange();
}

function addIgnorePattern() {
  const pattern = elements.newIgnorePattern.value.trim();
  if (pattern) {
    const currentConfig = getState().config;
    if (!currentConfig.ignore_patterns.includes(pattern)) {
      const newPatterns = [...currentConfig.ignore_patterns, pattern];
      post("updateConfig", { ...currentConfig, ignore_patterns: newPatterns });
    }
    elements.newIgnorePattern.value = "";
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
  elements.expandAllBtn.addEventListener("click", () =>
    post("expandCollapseAll", true)
  );
  elements.deselectAllBtn.addEventListener("click", () => post("deselectAll"));
  elements.collapseAllBtn.addEventListener("click", () =>
    post("expandCollapseAll", false)
  );

  elements.generateBtn.addEventListener("click", () => {
    if (getState().is_generating) {
      post("cancelGeneration");
    } else {
      post("generatePreview");
    }
  });
  elements.saveBtn.addEventListener("click", () => {
    const editor = get(editorInstance);
    if (editor) {
      post("saveFile", editor.getValue());
    }
  });
  elements.clearPreviewBtn.addEventListener("click", clearPreview);
  elements.copyBtn.addEventListener("click", () =>
    handleCopy({
      isEditorFocused: true,
      activeEl: document.activeElement as HTMLElement,
      isInNormalInputField: false,
    })
  );

  ["change", "input"].forEach((evt) => {
    elements.removeEmptyDirs.addEventListener(evt, onConfigChange);
    elements.caseSensitive.addEventListener(evt, onConfigAndFilterChange);
  });

  ["input"].forEach((evt) => {
    elements.searchQuery.addEventListener(evt, onFilterChange);
    elements.extensionFilter.addEventListener(evt, onFilterChange);
    elements.contentSearchQuery.addEventListener(evt, onFilterChange);

    elements.filterPatterns.addEventListener(evt, (e: Event) => {
      const target = e.target as HTMLInputElement;
      // Simply update the store. The App component will react and re-render.
      patternFilter.set(target.value.toLowerCase());
    });
  });

  elements.addPatternBtn.addEventListener("click", addIgnorePattern);
  elements.newIgnorePattern.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") addIgnorePattern();
  });

  elements.deleteAllPatternsBtn.addEventListener("click", () => {
    const currentConfig = getState().config;
    post("updateConfig", { ...currentConfig, ignore_patterns: [] });
  });

  document.body.addEventListener("focusin", (e: Event) => {
    const target = e.target as HTMLElement;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement
    ) {
      getUndoManagerForElement(target);
    }
  });

  document.body.addEventListener("input", (e: Event) => {
    const target = e.target as HTMLElement;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement
    ) {
      getUndoManagerForElement(target).recordState();
    }
  });
}
