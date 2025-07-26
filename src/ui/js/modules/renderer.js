import { elements } from '../dom.js';
import { state } from '../state.js';
import { post } from '../services/backend.js';
import { COMMON_IGNORE_PATTERNS } from '../config.js';
import { formatFileSize } from '../utils.js';

function countTreeItems(nodes) {
  let totalFiles = 0;
  let totalFolders = 0;
  function traverse(items) {
    for (const item of items) {
      if (item.is_directory) {
        totalFolders++;
        if (item.children && item.children.length > 0) traverse(item.children);
      } else {
        totalFiles++;
      }
    }
  }
  traverse(nodes);
  return { totalFiles, totalFolders };
}

function createScanProgressUI() {
    return `
    <div class="scan-progress-container">
      <div class="scan-progress-header">
        <div class="scan-status">
          <div class="scan-spinner"></div>
          <span class="scan-text">Scanning directory...</span>
        </div>
        <button id="cancel-scan-btn" class="cancel-scan-btn" title="Cancel current scan">
          <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
          Cancel
        </button>
      </div>
      <div class="scan-progress-bar"><div class="scan-progress-fill" id="scan-progress-fill"></div></div>
      <div class="scan-details">
        <span id="scan-files-count">0 files processed</span>
        <span id="scan-current-path">Starting scan...</span>
        <span id="scan-skipped-count"></span>
      </div>
    </div>`;
}

function createTreeLevel(nodes) {
  const ul = document.createElement("ul");
  nodes.forEach((node) => {
    const li = document.createElement("li");
    if (node.is_directory) {
      const details = document.createElement("details");
      details.open = node.is_expanded;
      details.addEventListener("toggle", (e) => {
        if (e.target.open !== node.is_expanded) post("toggleExpansion", node.path);
      });

      const summary = document.createElement("summary");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.checked = node.selection_state === "full";
      checkbox.indeterminate = node.selection_state === "partial";
      checkbox.addEventListener("click", (e) => {
        e.preventDefault();
        post("toggleDirectorySelection", node.path);
      });

      const nameSpan = document.createElement("span");
      nameSpan.className = "file-name";
      nameSpan.innerHTML = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg> ${node.name}`;
      if (node.is_match) nameSpan.classList.add("is-match");

      const ignoreBtn = document.createElement("button");
      ignoreBtn.className = "ignore-btn";
      ignoreBtn.title = "Add this directory to ignore patterns";
      ignoreBtn.innerHTML = `<svg class="icon ignore-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" /></svg>`;
      ignoreBtn.addEventListener("click", (e) => {
        e.preventDefault(); e.stopPropagation();
        post("addIgnorePath", node.path);
      });

      summary.appendChild(checkbox);
      summary.appendChild(nameSpan);
      summary.appendChild(ignoreBtn);
      details.appendChild(summary);
      details.appendChild(createTreeLevel(node.children));
      li.appendChild(details);
    } else {
      const container = document.createElement("div");
      container.className = "tree-item-container";
      if (node.is_previewed) container.classList.add("previewed");

      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.checked = node.selection_state === "full";
      checkbox.addEventListener("change", () => post("toggleSelection", node.path));

      const nameSpan = document.createElement("span");
      nameSpan.className = "file-name";
      if (node.is_match) nameSpan.classList.add("is-match");
      const iconSvg = node.is_binary ? `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>` : `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14,2 14,8 20,8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10,9 9,9 8,9"/></svg>`;
      nameSpan.innerHTML = `${iconSvg} ${node.name}`;
      nameSpan.setAttribute("data-path", node.path);
      nameSpan.addEventListener("click", () => post("loadFilePreview", node.path));

      const ignoreBtn = document.createElement("button");
      ignoreBtn.className = "ignore-btn";
      ignoreBtn.title = "Add this file to ignore patterns";
      ignoreBtn.innerHTML = `<svg class="icon ignore-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" /></svg>`;
      ignoreBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        post("addIgnorePath", node.path);
      });

      const sizeSpan = document.createElement("span");
      sizeSpan.className = "file-size";
      sizeSpan.textContent = formatFileSize(node.size);

      container.appendChild(checkbox);
      container.appendChild(nameSpan);
      container.appendChild(ignoreBtn);
      container.appendChild(sizeSpan);
      li.appendChild(container);
    }
    ul.appendChild(li);
  });
  return ul;
}

function renderIgnorePatterns() {
  elements.currentPatternsContainer.innerHTML = "";
  const appState = state.get();
  const allPatterns = Array.from(appState.config.ignore_patterns || []);
  const activePatterns = new Set(appState.active_ignore_patterns || []);
  const active = allPatterns.filter((p) => activePatterns.has(p)).sort();
  const inactive = allPatterns.filter((p) => !activePatterns.has(p)).sort();
  let patterns = [...active, ...inactive];

  const currentPatternFilter = state.getPatternFilter();
  if (currentPatternFilter) {
    patterns = patterns.filter((pattern) => pattern.toLowerCase().includes(currentPatternFilter));
  }

  patterns.forEach((p) => {
    const chip = document.createElement("div");
    chip.className = "current-pattern-chip";
    if (activePatterns.has(p)) {
      chip.classList.add("active-pattern");
      chip.title = `This pattern was active and matched one or more files/directories.`;
    }
    chip.innerHTML = `<span>${p}</span><button class="remove-pattern-btn" data-pattern="${p}"><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg></button>`;
    chip.querySelector("button").addEventListener("click", (e) => {
      const patternToRemove = e.target.closest("button").dataset.pattern;
      const newPatterns = appState.config.ignore_patterns.filter((pat) => pat !== patternToRemove);
      post("updateConfig", { ...appState.config, ignore_patterns: newPatterns });
    });
    elements.currentPatternsContainer.appendChild(chip);
  });
}

function setupCommonPatterns() {
  elements.commonPatternsContainer.innerHTML = "";
  const appState = state.get();
  const availablePatterns = COMMON_IGNORE_PATTERNS.filter(
    (pattern) => !appState.config.ignore_patterns.includes(pattern)
  );

  const commonPatternsLabel = document.querySelector(".common-patterns-label");
  if (commonPatternsLabel) {
    commonPatternsLabel.style.display = availablePatterns.length > 0 ? "block" : "none";
  }

  availablePatterns.forEach((pattern) => {
    const chip = document.createElement("button");
    chip.className = "common-pattern-chip";
    chip.textContent = pattern;
    chip.title = `Click to add "${pattern}" to ignore patterns`;
    chip.addEventListener("click", (e) => {
      e.preventDefault();
      if (!appState.config.ignore_patterns.includes(pattern)) {
        post("updateConfig", {
          ...appState.config,
          ignore_patterns: [...appState.config.ignore_patterns, pattern],
        });
      }
    });
    elements.commonPatternsContainer.appendChild(chip);
  });
}

export function renderUI() {
  const appState = state.get();
  const { config, is_scanning } = appState;

  elements.currentPath.textContent = appState.current_path || "No directory selected.";
  elements.currentPath.title = appState.current_path;
  elements.clearDirBtn.style.display = appState.current_path ? "inline-block" : "none";
  elements.currentConfigFilename.textContent = appState.current_config_filename || "";

  elements.caseSensitive.checked = config.case_sensitive_search;
  elements.includeTree.checked = config.include_tree_by_default;
  elements.relativePaths.checked = config.use_relative_paths;
  elements.removeEmptyDirs.checked = config.remove_empty_directories || false;
  elements.outputDir.value = config.output_directory?.toString() || "";
  elements.outputFilename.value = config.output_filename;
  elements.searchQuery.value = appState.search_query;
  elements.extensionFilter.value = appState.extension_filter;
  elements.contentSearchQuery.value = appState.content_search_query;

  const hasSelection = appState.selected_files_count > 0;
  elements.selectDirBtn.disabled = is_scanning;
  elements.rescanBtn.disabled = is_scanning;
  elements.importConfigBtn.disabled = is_scanning;
  elements.generateBtn.disabled = !hasSelection || is_scanning;

  const iconFolder = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>`;
  const iconScan = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M3 21v-5h5"/></svg>`;
  const iconScanning = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><polyline points="12,6 12,12 16,14"/></svg>`;

  if (is_scanning) {
    elements.selectDirBtn.innerHTML = `${iconScanning} Scanning...`;
    elements.rescanBtn.innerHTML = `${iconScanning} Scanning...`;
  } else {
    elements.selectDirBtn.innerHTML = `${iconFolder} Select Directory`;
    elements.rescanBtn.innerHTML = `${iconScan} Re-Scan`;
  }

  elements.fileTreeContainer.innerHTML = "";
  if (is_scanning) {
    elements.fileTreeContainer.innerHTML = createScanProgressUI();
    const cancelBtn = document.getElementById("cancel-scan-btn");
    if (cancelBtn) {
      cancelBtn.addEventListener("click", () => {
        post("cancelScan");
        cancelBtn.disabled = true;
        cancelBtn.innerHTML = `${iconScanning} Cancelling...`;
      });
    }
    const progressFill = document.getElementById("scan-progress-fill");
    if (progressFill) progressFill.style.width = "0%";
  } else if (appState.tree.length > 0) {
    const treeRoot = document.createElement("div");
    treeRoot.className = "tree";
    treeRoot.appendChild(createTreeLevel(appState.tree));
    elements.fileTreeContainer.appendChild(treeRoot);
  } else if (appState.current_path) {
    elements.fileTreeContainer.innerHTML = '<p class="placeholder">No files found matching filters.</p>';
  } else {
    elements.fileTreeContainer.innerHTML = '<p class="placeholder">Choose Directory</p>';
    const placeholder = elements.fileTreeContainer.querySelector(".placeholder");
    if (placeholder) {
      placeholder.style.cursor = "pointer";
      placeholder.addEventListener("click", () => post("selectDirectory"));
    }
  }

  document.querySelector(".status-text").textContent = `Status: ${appState.status_message}`;
  const { totalFiles, totalFolders } = countTreeItems(appState.tree);
  elements.fileStats.textContent = `Files: ${appState.selected_files_count} selected of ${totalFiles} • Folders: ${totalFolders}`;

  setupCommonPatterns();
  renderIgnorePatterns();
}
