import { elements } from "../dom.js";
import { state } from "../state.js";
import { post } from "../services/backend.js";
import { COMMON_IGNORE_PATTERNS } from "../config.js";
import { formatFileSize } from "../utils.js";

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
  const container = document.createElement("div");
  container.className = "scan-progress-container";

  const header = document.createElement("div");
  header.className = "scan-progress-header";

  const statusDiv = document.createElement("div");
  statusDiv.className = "scan-status";
  statusDiv.innerHTML = `<div class="scan-spinner"></div><span class="scan-text">Scanning directory...</span>`;

  const cancelBtn = document.createElement("button");
  cancelBtn.id = "cancel-scan-btn";
  cancelBtn.className = "cancel-scan-btn";
  cancelBtn.title = "Cancel current scan";
  cancelBtn.innerHTML = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg> Cancel`;

  header.appendChild(statusDiv);
  header.appendChild(cancelBtn);

  const progressBar = document.createElement("div");
  progressBar.className = "scan-progress-bar";
  progressBar.innerHTML = `<div class="scan-progress-fill" id="scan-progress-fill"></div>`;

  const details = document.createElement("div");
  details.className = "scan-details";
  details.innerHTML = `<span id="scan-files-count">0 files processed</span><span id="scan-current-path">Starting scan...</span><span id="scan-skipped-count"></span>`;

  container.appendChild(header);
  container.appendChild(progressBar);
  container.appendChild(details);

  return container;
}

/**
 * Creates a simple message display without drag & drop functionality
 * @param {string} message - The message to display
 * @param {string} iconSvg - Optional SVG icon to display
 * @returns {HTMLElement} The message element
 */
function createMessageDisplay(message, iconSvg = null) {
  const messageContainer = document.createElement("div");
  messageContainer.className = "message-display";

  if (iconSvg) {
    const iconElement = document.createElement("div");
    iconElement.className = "message-icon";
    iconElement.innerHTML = iconSvg;
    messageContainer.appendChild(iconElement);
  }

  const textElement = document.createElement("p");
  textElement.className = "message-text";
  textElement.textContent = message;
  messageContainer.appendChild(textElement);

  return messageContainer;
}

/**
 * Creates the directory selection placeholder with drag & drop functionality
 * @returns {HTMLElement} The placeholder element
 */
function createDirectorySelectionPlaceholder() {
  const placeholder = document.createElement("p");
  placeholder.className = "placeholder";
  placeholder.textContent = "Choose Directory";
  placeholder.style.cursor = "pointer";
  placeholder.addEventListener("click", () => post("selectDirectory"));
  return placeholder;
}

/**
 * Determines if any active filters are applied
 * @param {Object} appState - Current application state
 * @returns {boolean} True if filters are active
 */
function hasActiveFilters(appState) {
  return !!(
    appState.search_query?.trim() ||
    appState.extension_filter?.trim() ||
    appState.content_search_query?.trim()
  );
}

function createTreeLevel(nodes) {
  const ul = document.createElement("ul");
  if (!nodes) return ul;

  nodes.forEach((node) => {
    const li = document.createElement("li");

    if (node.is_directory) {
      const details = document.createElement("details");
      details.open = node.is_expanded;
      details.addEventListener("toggle", (e) => {
        // This check prevents sending redundant events on programmatic open/close
        if (e.target.open !== node.is_expanded) {
          post("toggleExpansion", node.path);
        }
      });

      const summary = document.createElement("summary");

      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.checked = node.selection_state === "full";
      checkbox.indeterminate = node.selection_state === "partial";
      checkbox.addEventListener("click", (e) => {
        e.preventDefault(); // Prevent default checkbox behavior
        post("toggleDirectorySelection", node.path);
      });

      const nameSpan = document.createElement("span");
      nameSpan.className = "file-name";
      if (node.is_match) nameSpan.classList.add("is-match");

      const icon = document.createElementNS(
        "http://www.w3.org/2000/svg",
        "svg"
      );
      icon.setAttribute("class", "icon");
      icon.setAttribute("viewBox", "0 0 24 24");
      icon.innerHTML = `<path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/>`;
      nameSpan.appendChild(icon);
      nameSpan.appendChild(document.createTextNode(` ${node.name}`));

      const ignoreBtn = document.createElement("button");
      ignoreBtn.className = "ignore-btn";
      ignoreBtn.title = "Add this directory to ignore patterns";
      ignoreBtn.innerHTML = `<svg class="icon ignore-icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" /></svg>`;
      ignoreBtn.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        post("addIgnorePath", node.path);
      });

      summary.appendChild(checkbox);
      summary.appendChild(nameSpan);
      summary.appendChild(ignoreBtn);
      details.appendChild(summary);

      details.appendChild(createTreeLevel(node.children));
      li.appendChild(details);
    } else {
      // It's a file
      const container = document.createElement("div");
      container.className = "tree-item-container";
      if (node.is_previewed) container.classList.add("previewed");

      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.checked = node.selection_state === "full";
      checkbox.addEventListener("change", () =>
        post("toggleSelection", node.path)
      );

      const nameSpan = document.createElement("span");
      nameSpan.className = "file-name";
      if (node.is_match) nameSpan.classList.add("is-match");
      const iconHTML = node.is_binary
        ? `<svg class="icon" viewBox="0 0 24 24"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>`
        : `<svg class="icon" viewBox="0 0 24 24"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14,2 14,8 20,8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10,9 9,9 8,9"/></svg>`;
      nameSpan.innerHTML = `${iconHTML} `;
      nameSpan.appendChild(document.createTextNode(node.name));
      nameSpan.setAttribute("data-path", node.path);
      nameSpan.addEventListener("click", () =>
        post("loadFilePreview", node.path)
      );

      const ignoreBtn = document.createElement("button");
      ignoreBtn.className = "ignore-btn";
      ignoreBtn.title = "Add this file to ignore patterns";
      ignoreBtn.innerHTML = `<svg class="icon ignore-icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" /></svg>`;
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

  // Sort active patterns first, then inactive, both alphabetically
  const active = allPatterns.filter((p) => activePatterns.has(p)).sort();
  const inactive = allPatterns.filter((p) => !activePatterns.has(p)).sort();
  let patterns = [...active, ...inactive];

  const currentPatternFilter = state.getPatternFilter();
  if (currentPatternFilter) {
    patterns = patterns.filter((pattern) =>
      pattern.toLowerCase().includes(currentPatternFilter)
    );
  }

  patterns.forEach((p) => {
    const chip = document.createElement("div");
    chip.className = "current-pattern-chip";
    if (activePatterns.has(p)) {
      chip.classList.add("active-pattern");
      chip.title = `This pattern was active and matched one or more files/directories.`;
    }

    const nameSpan = document.createElement("span");
    nameSpan.textContent = p;

    const removeBtn = document.createElement("button");
    removeBtn.className = "remove-pattern-btn";
    removeBtn.dataset.pattern = p;
    removeBtn.innerHTML = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;

    removeBtn.addEventListener("click", (e) => {
      const patternToRemove = e.currentTarget.dataset.pattern;
      const currentConfig = state.get().config;
      const newPatterns = currentConfig.ignore_patterns.filter(
        (pat) => pat !== patternToRemove
      );
      post("updateConfig", { ...currentConfig, ignore_patterns: newPatterns });
    });

    chip.appendChild(nameSpan);
    chip.appendChild(removeBtn);
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
    commonPatternsLabel.style.display =
      availablePatterns.length > 0 ? "block" : "none";
  }

  availablePatterns.forEach((pattern) => {
    const chip = document.createElement("button");
    chip.className = "common-pattern-chip";
    chip.textContent = pattern;
    chip.title = `Click to add "${pattern}" to ignore patterns`;
    chip.addEventListener("click", (e) => {
      e.preventDefault();
      const currentConfig = state.get().config;
      if (!currentConfig.ignore_patterns.includes(pattern)) {
        post("updateConfig", {
          ...currentConfig,
          ignore_patterns: [...currentConfig.ignore_patterns, pattern],
        });
      }
    });
    elements.commonPatternsContainer.appendChild(chip);
  });
}

let generatingIntervalId = null;

export function renderUI() {
  const appState = state.get();
  const { config, is_scanning, is_generating } = appState;

  elements.currentPath.textContent =
    appState.current_path || "No directory selected.";
  elements.currentPath.title = appState.current_path;
  elements.clearDirBtn.style.display = appState.current_path
    ? "inline-block"
    : "none";
  elements.currentConfigFilename.textContent =
    appState.current_config_filename || "";

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

  const iconFolder = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>`;
  const iconScan = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M3 21v-5h5"/></svg>`;
  const iconScanning = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><polyline points="12,6 12,12 16,14"/></svg>`;
  const iconGenerate = `<svg class="icon icon-lightning-light" viewBox="0 0 24 24"><path d="M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z"></path></svg>`;
  const iconCancel = `<svg class="icon" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>`;

  if (is_scanning) {
    elements.selectDirBtn.innerHTML = `${iconScanning} Scanning...`;
    elements.rescanBtn.innerHTML = `${iconScanning} Scanning...`;
  } else {
    elements.selectDirBtn.innerHTML = `${iconFolder} Select Directory`;
    elements.rescanBtn.innerHTML = `${iconScan} Re-Scan`;
  }

  const wasGenerating =
    elements.generateBtn.classList.contains("is-generating");

  if (is_generating) {
    // State is generating.
    if (!wasGenerating) {
      // This is the first render in the generating state. Set up the interval.
      clearInterval(generatingIntervalId); // Clear any old timers just in case.

      elements.generateBtn.classList.remove("button-cta");
      elements.generateBtn.classList.add("is-generating");

      elements.generateBtn.innerHTML = `
          <span class="generating-content">
              ${iconGenerate}
              <span class="generating-text">Concat</span>
          </span>
          <span class="cancel-content">${iconCancel} Cancel</span>
      `;

      const textElement =
        elements.generateBtn.querySelector(".generating-text");
      let dotCount = 0;
      generatingIntervalId = setInterval(() => {
        dotCount = (dotCount + 1) % 4; // Cycle 0, 1, 2, 3

        const dots = ".".repeat(dotCount);
        const spaces = "\u00A0".repeat(3 - dotCount); // \u00A0 -> whitespace
        if (textElement) {
          textElement.textContent = `Concat${dots}${spaces}`;
        }
      }, 500);
    }
    elements.generateBtn.disabled = false; // Enable button for cancellation click
  } else {
    // State is NOT generating.
    if (wasGenerating) {
      // This is the first render after generating has stopped. Clean up.
      clearInterval(generatingIntervalId);
      generatingIntervalId = null;

      elements.generateBtn.classList.remove("is-generating");
      elements.generateBtn.classList.add("button-cta");
      elements.generateBtn.innerHTML = `${iconGenerate} Generate`;
    }
    elements.generateBtn.disabled = !hasSelection || is_scanning;
  }

  // Clear the file tree container
  elements.fileTreeContainer.innerHTML = "";

  if (is_scanning) {
    // Show scanning progress
    elements.fileTreeContainer.appendChild(createScanProgressUI());
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
  } else if (!appState.current_path) {
    // No directory selected - show directory selection placeholder with drag & drop
    elements.fileTreeContainer.appendChild(
      createDirectorySelectionPlaceholder()
    );
  } else if (appState.tree.length > 0) {
    // Directory selected and files found - show the tree
    const treeRoot = document.createElement("div");
    treeRoot.className = "tree";
    treeRoot.appendChild(createTreeLevel(appState.tree));
    elements.fileTreeContainer.appendChild(treeRoot);
  } else {
    // Directory selected but no files found
    const hasFilters = hasActiveFilters(appState);

    if (hasFilters) {
      // Filters are active but no results - show filter message without drag & drop
      const noResultsIcon = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/><line x1="11" y1="8" x2="11" y2="14"/><line x1="8" y1="11" x2="14" y2="11"/></svg>`;
      elements.fileTreeContainer.appendChild(
        createMessageDisplay("No files found matching filters.", noResultsIcon)
      );
    } else {
      // No filters but still no files - might be empty directory or all ignored
      const emptyIcon = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/><path d="M12 10v6"/><path d="M9 13h6"/></svg>`;
      elements.fileTreeContainer.appendChild(
        createMessageDisplay("No files found in this directory.", emptyIcon)
      );
    }
  }

  document.querySelector(
    ".status-text"
  ).textContent = `Status: ${appState.status_message}`;
  const { totalFiles, totalFolders } = countTreeItems(appState.tree);
  elements.fileStats.textContent = `Files: ${appState.selected_files_count} selected of ${totalFiles} • Folders: ${totalFolders}`;

  let statusMessage = appState.status_message;
  if (statusMessage.startsWith("Scan complete.")) {
    const totalItemsInTree = totalFiles + totalFolders;
    statusMessage = `Scan complete. Found ${totalItemsInTree} visible items.`;
  }
  document.querySelector(
    ".status-text"
  ).textContent = `Status: ${statusMessage}`;

  setupCommonPatterns();
  renderIgnorePatterns();

  // Update search inputs state if the function exists (from eventListeners.js)
  if (window.updateSearchInputsState) {
    window.updateSearchInputsState();
  }
}
