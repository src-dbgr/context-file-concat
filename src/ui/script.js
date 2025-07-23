document.addEventListener("DOMContentLoaded", () => {
  let editor;
  let appState = {};

  const elements = {
    // Top bar
    selectDirBtn: document.getElementById("select-dir-btn"),
    currentPath: document.getElementById("current-path"),
    importConfigBtn: document.getElementById("import-config-btn"),
    exportConfigBtn: document.getElementById("export-config-btn"),
    // Sidebar
    searchQuery: document.getElementById("search-query"),
    extensionFilter: document.getElementById("extension-filter"),
    caseSensitive: document.getElementById("case-sensitive"),
    rescanBtn: document.getElementById("rescan-btn"),
    commonPatterns: document.getElementById("common-patterns"),
    newIgnorePattern: document.getElementById("new-ignore-pattern"),
    addPatternBtn: document.getElementById("add-pattern-btn"),
    currentPatternsContainer: document.getElementById(
      "current-patterns-container"
    ),
    // File List
    fileStats: document.getElementById("file-stats"),
    selectAllBtn: document.getElementById("select-all-btn"),
    deselectAllBtn: document.getElementById("deselect-all-btn"),
    expandAllBtn: document.getElementById("expand-all-btn"),
    collapseAllBtn: document.getElementById("collapse-all-btn"),
    fileTreeContainer: document.getElementById("file-tree-container"),
    // Preview/Editor
    previewTitle: document.getElementById("preview-title"),
    clearPreviewBtn: document.getElementById("clear-preview-btn"),
    editorContainer: document.getElementById("editor-container"),
    // Bottom Panel
    generateBtn: document.getElementById("generate-btn"),
    saveBtn: document.getElementById("save-btn"),
    includeTree: document.getElementById("include-tree"),
    // Other
    statusBar: document.getElementById("status-bar"),
    resizer: document.getElementById("resizer"),
    fileListPanel: document.getElementById("file-list-panel"),
  };

  // --- IPC Communication ---
  const post = (command, payload = null) =>
    window.ipc.postMessage(JSON.stringify({ command, payload }));

  // --- Monaco Editor Initialization ---
  require.config({
    paths: { vs: "https://cdn.jsdelivr.net/npm/monaco-editor@0.45.0/min/vs" },
  });
  require(["vs/editor/editor.main"], () => {
    editor = monaco.editor.create(elements.editorContainer, {
      value: "// Select a directory to begin.",
      language: "plaintext",
      theme: "vs-dark",
      readOnly: true,
      automaticLayout: true,
      wordWrap: "on",
    });
  });

  // --- Event Listeners ---
  elements.selectDirBtn.addEventListener("click", () =>
    post("selectDirectory")
  );
  elements.rescanBtn.addEventListener("click", () =>
    post("selectDirectory", appState.current_path)
  );
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
    post("saveFile", editor.getValue())
  );
  elements.clearPreviewBtn.addEventListener("click", clearPreview);

  const debounce = (func, delay) => {
    let timeout;
    return function (...args) {
      clearTimeout(timeout);
      timeout = setTimeout(() => func.apply(this, args), delay);
    };
  };

  const onFilterChange = debounce(() => {
    const newConfig = { ...appState.config };
    // This is a placeholder for full filtering via config. A more complex IPC message is needed for this.
    // For now, filtering is mainly on the Rust side via ignore patterns.
    // post("updateConfig", newConfig);
  }, 300);

  elements.searchQuery.addEventListener("input", onFilterChange);
  elements.extensionFilter.addEventListener("input", onFilterChange);
  elements.caseSensitive.addEventListener("change", onFilterChange);
  elements.includeTree.addEventListener("change", () => {
    const newConfig = {
      ...appState.config,
      include_tree_by_default: elements.includeTree.checked,
    };
    post("updateConfig", newConfig);
  });
  elements.addPatternBtn.addEventListener("click", addIgnorePattern);
  elements.newIgnorePattern.addEventListener("keydown", (e) => {
    if (e.key === "Enter") addIgnorePattern();
  });

  function addIgnorePattern() {
    const pattern = elements.newIgnorePattern.value.trim();
    if (pattern && !appState.config.ignore_patterns.includes(pattern)) {
      const newConfig = {
        ...appState.config,
        ignore_patterns: [...appState.config.ignore_patterns, pattern],
      };
      post("updateConfig", newConfig);
      elements.newIgnorePattern.value = "";
    }
  }

  // --- Global Event Handlers from Rust ---
  window.render = (newState) => {
    appState = newState;
    renderUI();
  };
  window.showPreviewContent = (content) => {
    editor.setValue(content);
    editor.updateOptions({ readOnly: true });
    elements.previewTitle.textContent = "Preview (Read-only)";
    elements.clearPreviewBtn.style.display = "block";
  };
  window.showGeneratedContent = (content) => {
    editor.setValue(content);
    editor.updateOptions({ readOnly: false });
    elements.previewTitle.textContent = "Generated Preview (Editable)";
    elements.saveBtn.disabled = false;
    elements.clearPreviewBtn.style.display = "block";
  };
  window.showError = (msg) => {
    elements.statusBar.textContent = `Error: ${msg}`;
  };
  window.showStatus = (msg) => {
    elements.statusBar.textContent = `Status: ${msg}`;
  };
  window.fileSaveStatus = (success, path) => {
    if (path === "cancelled") {
      elements.statusBar.textContent = "Status: Save cancelled.";
    } else {
      elements.statusBar.textContent = success
        ? `Status: Saved to ${path}`
        : `Error: Failed to save file.`;
    }
  };

  // --- UI Rendering ---
  function renderUI() {
    // Config & Path
    elements.currentPath.textContent =
      appState.current_path || "No directory selected.";
    elements.currentPath.title = appState.current_path;
    elements.includeTree.checked = appState.config.include_tree_by_default;

    // Buttons
    const hasSelection = appState.selected_files_count > 0;
    elements.generateBtn.disabled = !hasSelection || appState.is_scanning;
    elements.rescanBtn.disabled =
      !appState.current_path || appState.is_scanning;

    // Stats
    elements.statusBar.textContent = `Status: ${appState.status_message}`;
    elements.fileStats.textContent = `Visible: ${appState.visible_files_count} | Selected: ${appState.selected_files_count}`;

    // File Tree
    elements.fileTreeContainer.innerHTML = "";
    if (appState.is_scanning) {
      elements.fileTreeContainer.innerHTML =
        '<p class="placeholder">Scanning...</p>';
    } else if (appState.tree.length > 0) {
      const treeRoot = document.createElement("div");
      treeRoot.className = "tree";
      treeRoot.appendChild(createTreeLevel(appState.tree));
      elements.fileTreeContainer.appendChild(treeRoot);
    } else if (appState.current_path) {
      elements.fileTreeContainer.innerHTML =
        '<p class="placeholder">No files found.</p>';
    } else {
      elements.fileTreeContainer.innerHTML =
        '<p class="placeholder">Select a directory to start.</p>';
    }

    renderIgnorePatterns();
  }

  function renderIgnorePatterns() {
    // Current patterns
    elements.currentPatternsContainer.innerHTML = "";
    appState.config.ignore_patterns.sort().forEach((p) => {
      const chip = document.createElement("div");
      chip.className = "current-pattern-chip";
      chip.innerHTML = `<span>${p}</span><button class="remove-pattern-btn" data-pattern="${p}">&times;</button>`;
      chip.querySelector("button").addEventListener("click", (e) => {
        const patternToRemove = e.target.dataset.pattern;
        const newPatterns = appState.config.ignore_patterns.filter(
          (pat) => pat !== patternToRemove
        );
        post("updateConfig", {
          ...appState.config,
          ignore_patterns: newPatterns,
        });
      });
      elements.currentPatternsContainer.appendChild(chip);
    });
  }

  function createTreeLevel(nodes) {
    const ul = document.createElement("ul");
    nodes.forEach((node) => {
      const li = document.createElement("li");
      if (node.is_directory) {
        const details = document.createElement("details");
        details.open = node.is_expanded;
        details.addEventListener("toggle", (e) => {
          if (e.target.open !== node.is_expanded) {
            post("toggleExpansion", node.path);
          }
        });

        const summary = document.createElement("summary");
        summary.innerHTML = `
          <input type="checkbox" ${node.is_selected ? "checked" : ""}>
          <span class="file-name">📁 ${node.name}</span>
          <button class="ignore-btn" title="Add this directory to ignore patterns">i</button>`;
        summary.querySelector("input").addEventListener("click", (e) => {
          e.preventDefault(); // Prevent details toggling
          post("toggleDirectorySelection", node.path);
        });
        summary.querySelector(".ignore-btn").addEventListener("click", (e) => {
          e.preventDefault();
          const pattern = `${node.name}/`;
          if (!appState.config.ignore_patterns.includes(pattern)) {
            post("updateConfig", {
              ...appState.config,
              ignore_patterns: [...appState.config.ignore_patterns, pattern],
            });
          }
        });
        details.appendChild(summary);
        details.appendChild(createTreeLevel(node.children));
        li.appendChild(details);
      } else {
        li.innerHTML = `
          <label class="tree-item-label">
            <input type="checkbox" ${node.is_selected ? "checked" : ""}>
            <span class="file-name" data-path="${node.path}">${
          node.is_binary ? "🔧" : "📄"
        } ${node.name}</span>
            <span class="file-size">${formatFileSize(node.size)}</span>
            <button class="ignore-btn" title="Add this file to ignore patterns">i</button>
          </label>`;
        li.querySelector("input").addEventListener("change", () =>
          post("toggleSelection", node.path)
        );
        li.querySelector(".file-name").addEventListener("click", () =>
          post("loadFilePreview", node.path)
        );
        li.querySelector(".ignore-btn").addEventListener("click", (e) => {
          e.stopPropagation();
          if (!appState.config.ignore_patterns.includes(node.name)) {
            post("updateConfig", {
              ...appState.config,
              ignore_patterns: [...appState.config.ignore_patterns, node.name],
            });
          }
        });
      }
      ul.appendChild(li);
    });
    return ul;
  }

  function clearPreview() {
    editor.setValue("// Preview cleared.");
    editor.updateOptions({ readOnly: true });
    elements.previewTitle.textContent = "Preview";
    elements.saveBtn.disabled = true;
    elements.clearPreviewBtn.style.display = "none";
  }

  function formatFileSize(bytes) {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
  }

  // --- Resizer Logic ---
  let mouseDown = false;
  elements.resizer.addEventListener("mousedown", (e) => {
    mouseDown = true;
    document.body.style.cursor = "ns-resize";
  });
  document.addEventListener("mouseup", () => {
    mouseDown = false;
    document.body.style.cursor = "default";
  });
  document.addEventListener("mousemove", (e) => {
    if (!mouseDown) return;
    const totalHeight = elements.contentSplitter.offsetHeight;
    const newTopHeight = e.clientY - elements.fileListPanel.offsetTop;
    if (newTopHeight > 100 && newTopHeight < totalHeight - 100) {
      const newTopPercent = (newTopHeight / totalHeight) * 100;
      elements.fileListPanel.style.height = `${newTopPercent}%`;
      elements.previewPanel.style.height = `${100 - newTopPercent}%`;
    }
  });

  // --- Keyboard Shorcut Fix ---
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.metaKey) {
      if (
        e.key === "c" ||
        e.key === "x" ||
        e.key === "v" ||
        e.key === "a" ||
        e.key === "z" ||
        e.key === "y"
      ) {
        // If the editor has focus, let it handle the event
        if (editor && editor.hasTextFocus()) {
          e.stopPropagation();
        }
      }
    }
  });

  // --- Initial Load ---
  post("initialize");
});
