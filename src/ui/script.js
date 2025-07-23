document.addEventListener("DOMContentLoaded", () => {
  let editor;
  let appState = {};
  let filterDebounceTimeout;
  let currentDecorations = [];
  let currentPreviewedPath = null; // Für Live-Highlight-Updates

  const elements = {
    // Top bar
    selectDirBtn: document.getElementById("select-dir-btn"),
    currentPath: document.getElementById("current-path"),
    importConfigBtn: document.getElementById("import-config-btn"),
    exportConfigBtn: document.getElementById("export-config-btn"),
    // Sidebar
    searchQuery: document.getElementById("search-query"),
    extensionFilter: document.getElementById("extension-filter"),
    contentSearchQuery: document.getElementById("content-search-query"),
    caseSensitive: document.getElementById("case-sensitive"),
    rescanBtn: document.getElementById("rescan-btn"),
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
    copyBtn: document.getElementById("copy-btn"),
    clearPreviewBtn: document.getElementById("clear-preview-btn"),
    editorContainer: document.getElementById("editor-container"),
    // Bottom Panel
    generateBtn: document.getElementById("generate-btn"),
    saveBtn: document.getElementById("save-btn"),
    includeTree: document.getElementById("include-tree"),
    relativePaths: document.getElementById("relative-paths"),
    outputDir: document.getElementById("output-dir"),
    browseOutputDirBtn: document.getElementById("browse-output-dir"),
    outputFilename: document.getElementById("output-filename"),
    // Other
    statusBar: document.getElementById("status-bar"),
    resizer: document.getElementById("resizer"),
    fileListPanel: document.getElementById("file-list-panel"),
    previewPanel: document.getElementById("preview-panel"),
    contentSplitter: document.querySelector(".content-splitter"),
  };

  const post = (command, payload = null) =>
    window.ipc.postMessage(JSON.stringify({ command, payload }));

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
    post("saveFile", editor.getValue())
  );
  elements.clearPreviewBtn.addEventListener("click", clearPreview);
  elements.copyBtn.addEventListener("click", copyToClipboard);
  elements.browseOutputDirBtn.addEventListener("click", () =>
    post("pickOutputDirectory")
  );

  const onConfigChange = () => {
    const newConfig = {
      ...appState.config,
      case_sensitive_search: elements.caseSensitive.checked,
      include_tree_by_default: elements.includeTree.checked,
      use_relative_paths: elements.relativePaths.checked,
      output_filename: elements.outputFilename.value,
    };
    post("updateConfig", newConfig);
  };

  const onFilterChange = () => {
    clearTimeout(filterDebounceTimeout);
    filterDebounceTimeout = setTimeout(() => {
      post("updateFilters", {
        searchQuery: elements.searchQuery.value,
        extensionFilter: elements.extensionFilter.value,
        contentSearchQuery: elements.contentSearchQuery.value,
      });
    }, 300);
  };

  elements.includeTree.addEventListener("change", onConfigChange);
  elements.relativePaths.addEventListener("change", onConfigChange);
  elements.outputFilename.addEventListener("change", onConfigChange);
  elements.caseSensitive.addEventListener("change", onConfigChange);
  elements.searchQuery.addEventListener("input", onFilterChange);
  elements.extensionFilter.addEventListener("input", onFilterChange);
  elements.contentSearchQuery.addEventListener("input", onFilterChange);

  elements.addPatternBtn.addEventListener("click", () => addIgnorePattern());
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
    // FIX 2: Re-apply highlighting if the content search term changes on an active preview
    if (
      currentPreviewedPath &&
      editor?.getModel() &&
      newState.content_search_query !== (appState.content_search_query || "")
    ) {
      const model = editor.getModel();
      const searchTerm = newState.content_search_query;
      const matchCase = newState.config.case_sensitive_search;

      let newDecorations = [];
      if (searchTerm && searchTerm.trim() !== "") {
        const matches = model.findMatches(
          searchTerm,
          true,
          false,
          matchCase,
          null,
          true
        );
        newDecorations = matches.map((match) => ({
          range: match.range,
          options: {
            inlineClassName: "search-highlight",
            hoverMessage: { value: "Search match" },
          },
        }));
      }
      currentDecorations = editor.deltaDecorations(
        currentDecorations,
        newDecorations
      );
    }

    appState = newState;
    renderUI();
  };

  // FIX 2: Update showPreviewContent to handle path and apply correct case-sensitivity
  window.showPreviewContent = (content, language, searchTerm, path) => {
    currentPreviewedPath = path; // Store path for live updates
    editor.setValue(content);
    const model = editor.getModel();

    if (model) {
      monaco.editor.setModelLanguage(model, language);
      let newDecorations = [];
      if (searchTerm && searchTerm.trim() !== "") {
        // Use case sensitivity from the current app state
        const matchCase = appState.config.case_sensitive_search;
        const matches = model.findMatches(
          searchTerm,
          true,
          false,
          matchCase,
          null,
          true
        );
        newDecorations = matches.map((match) => ({
          range: match.range,
          options: {
            inlineClassName: "search-highlight",
            hoverMessage: { value: "Search match" },
          },
        }));
      }
      currentDecorations = editor.deltaDecorations(
        currentDecorations,
        newDecorations
      );
    }

    editor.updateOptions({ readOnly: true });
    elements.previewTitle.textContent = "Preview (Read-only)";
    elements.copyBtn.style.display = "inline-block";
    elements.clearPreviewBtn.style.display = "inline-block";
  };

  window.showGeneratedContent = (content) => {
    currentPreviewedPath = null; // Generated content is not a specific file preview
    editor.setValue(content);
    currentDecorations = editor.deltaDecorations(currentDecorations, []);
    monaco.editor.setModelLanguage(editor.getModel(), "plaintext");
    editor.updateOptions({ readOnly: false });
    elements.previewTitle.textContent = "Generated Preview (Editable)";
    elements.saveBtn.disabled = false;
    elements.copyBtn.style.display = "inline-block";
    elements.clearPreviewBtn.style.display = "inline-block";
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
    elements.currentPath.textContent =
      appState.current_path || "No directory selected.";
    elements.currentPath.title = appState.current_path;

    const { config } = appState;
    elements.caseSensitive.checked = config.case_sensitive_search;
    elements.includeTree.checked = config.include_tree_by_default;
    elements.relativePaths.checked = config.use_relative_paths;
    elements.outputDir.value = config.output_directory?.toString() || "Not set";
    elements.outputFilename.value = config.output_filename;
    elements.searchQuery.value = appState.search_query;
    elements.extensionFilter.value = appState.extension_filter;
    elements.contentSearchQuery.value = appState.content_search_query;

    const hasSelection = appState.selected_files_count > 0;
    elements.generateBtn.disabled = !hasSelection || appState.is_scanning;
    elements.rescanBtn.disabled =
      !appState.current_path || appState.is_scanning;

    elements.statusBar.textContent = `Status: ${appState.status_message}`;
    elements.fileStats.textContent = `Visible: ${appState.visible_files_count} | Selected: ${appState.selected_files_count}`;

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
        '<p class="placeholder">No files found matching filters.</p>';
    } else {
      elements.fileTreeContainer.innerHTML =
        '<p class="placeholder">Select a directory to start.</p>';
    }

    renderIgnorePatterns();
  }

  function renderIgnorePatterns() {
    elements.currentPatternsContainer.innerHTML = "";
    (appState.config.ignore_patterns || [])
      .slice()
      .sort()
      .forEach((p) => {
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
          if (e.target.open !== node.is_expanded)
            post("toggleExpansion", node.path);
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
        nameSpan.textContent = `📁 ${node.name}`;
        if (node.is_match) {
          nameSpan.classList.add("is-match");
        }

        summary.appendChild(checkbox);
        summary.appendChild(nameSpan);
        const ignoreBtn = document.createElement("button");
        ignoreBtn.className = "ignore-btn";
        ignoreBtn.title = "Add this directory to ignore patterns";
        ignoreBtn.textContent = "i";
        summary.appendChild(ignoreBtn);

        ignoreBtn.addEventListener("click", (e) => {
          e.preventDefault();
          e.stopPropagation();
          post("addIgnorePath", node.path);
        });

        details.appendChild(summary);
        details.appendChild(createTreeLevel(node.children));
        li.appendChild(details);
      } else {
        const container = document.createElement("div");
        container.className = "tree-item-container";

        container.innerHTML = `
          <input type="checkbox" ${
            node.selection_state === "full" ? "checked" : ""
          }>
          <span class="file-name ${
            node.is_match ? "is-match" : ""
          }" data-path="${node.path}">${node.is_binary ? "🔧" : "📄"} ${
          node.name
        }</span>
          <span class="file-size">${formatFileSize(node.size)}</span>
          <button class="ignore-btn" title="Add this file to ignore patterns">i</button>
        `;

        container
          .querySelector("input")
          .addEventListener("change", () => post("toggleSelection", node.path));
        container.querySelector(".file-name").addEventListener("click", () => {
          post("loadFilePreview", node.path);
        });
        container
          .querySelector(".ignore-btn")
          .addEventListener("click", (e) => {
            e.stopPropagation();
            post("addIgnorePath", node.path);
          });
        li.appendChild(container);
      }
      ul.appendChild(li);
    });
    return ul;
  }

  function clearPreview() {
    currentPreviewedPath = null; // No file is being previewed
    editor.setValue("// Preview cleared.");
    currentDecorations = editor.deltaDecorations(currentDecorations, []);
    editor.updateOptions({ readOnly: true });
    monaco.editor.setModelLanguage(editor.getModel(), "plaintext");
    elements.previewTitle.textContent = "Preview";
    elements.saveBtn.disabled = true;
    elements.clearPreviewBtn.style.display = "none";
    elements.copyBtn.style.display = "none";
  }

  // FIX 1: Provide better visual feedback on copy
  function copyToClipboard() {
    const originalText = elements.copyBtn.textContent;
    navigator.clipboard.writeText(editor.getValue()).then(
      () => {
        elements.statusBar.textContent = "Status: Copied to clipboard!";
        elements.copyBtn.textContent = "✅ Copied!";
        setTimeout(() => {
          elements.copyBtn.textContent = originalText;
          if (appState && appState.status_message) {
            elements.statusBar.textContent = `Status: ${appState.status_message}`;
          }
        }, 2000);
      },
      () => {
        elements.statusBar.textContent = "Error: Could not copy to clipboard.";
      }
    );
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
  elements.resizer.addEventListener("mousedown", () => {
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

  // FIX 3: Global event listener to prevent crashes from keyboard shortcuts
  document.addEventListener("keydown", (e) => {
    // Check for Cmd (Mac) or Ctrl (Windows/Linux)
    if (!e.metaKey && !e.ctrlKey) {
      return;
    }

    // Check if focus is on an element that handles shortcuts itself
    const isTextField =
      e.target.tagName === "INPUT" && e.target.type === "text";
    const isEditorFocused = editor && editor.hasTextFocus();

    if (isTextField || isEditorFocused) {
      const key = e.key.toLowerCase();
      // Intercept common editing shortcuts
      if (
        [
          "a",
          "c",
          "v",
          "x",
          "z",
          "y",
          "arrowleft",
          "arrowright",
          "arrowup",
          "arrowdown",
          "home",
          "end",
        ].includes(key)
      ) {
        // Stop the event from bubbling up to the wry host, which prevents the crash.
        // The browser/editor's default action (like copying) will still execute.
        e.stopPropagation();
      }
    }
  });

  post("initialize");
});
