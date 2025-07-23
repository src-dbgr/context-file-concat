document.addEventListener("DOMContentLoaded", () => {
  let editor;
  let appState = {};
  let filterDebounceTimeout;
  let currentDecorations = [];
  let currentPreviewedPath = null;

  const elements = {
    // Top bar
    selectDirBtn: document.getElementById("select-dir-btn"),
    currentPath: document.getElementById("current-path"),
    currentConfigFilename: document.getElementById("current-config-filename"),
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
    commonPatternsContainer: document.getElementById(
      "common-patterns-container"
    ),
    deleteAllPatternsBtn: document.getElementById("delete-all-patterns-btn"),
    removeEmptyDirs: document.getElementById("remove-empty-dirs"),
    filterPatterns: document.getElementById("filter-patterns"),
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

  // Common ignore patterns
  const commonIgnorePatterns = [
    "node_modules",
    "target",
    ".git",
    ".idea",
    "__pycache__",
    "*.log",
    "*.tmp",
    ".DS_Store",
    "Thumbs.db",
    "*.pyc",
    "*.class",
    "*.o",
    "*.obj",
    "package-lock.json",
    "*.lock",
    ".gitignore",
    "*.png",
    "*.jpg",
    "*.jpeg",
    "*.gif",
    "*.bmp",
    "*.ico",
    "*.webp",
    "*.exe",
    "*.dll",
    "*.so",
    "*.dylib",
    "*.zip",
    "*.tar",
    "*.gz",
    "*.7z",
    "*.rar",
    "*.pdf",
    "*.doc",
    "*.docx",
    "*.mp3",
    "*.mp4",
    "dist",
    "build",
  ];

  let currentPatternFilter = "";

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

    // FIX 1: Komplette Blockierung aller problematischen Tastenkombinationen
    // Wir fangen Events auf mehreren Ebenen ab

    // Globaler Document Event Listener (höchste Priorität)
    document.addEventListener(
      "keydown",
      (e) => {
        // ERWEITERT: Alle potentiell problematischen Tasten abfangen
        const shouldBlock =
          // Cmd/Ctrl + Buchstaben/Zahlen (die über macOS Menu-System laufen könnten)
          ((e.metaKey || e.ctrlKey) &&
            (e.key.length === 1 || // Alle einzelnen Zeichen (a-z, 0-9, Sonderzeichen)
              [
                "Backspace",
                "Delete",
                "Enter",
                "Return",
                "Tab",
                "Escape",
              ].includes(e.key))) ||
          // Standalone-Navigationstasten die Probleme machen können
          ["Home", "End", "PageUp", "PageDown"].includes(e.key) ||
          // Funktionstasten
          (e.key.startsWith("F") && e.key.length <= 3); // F1-F12

        // ALLOW CMD+F for search functionality in editor
        const isEditorFocused =
          document.activeElement &&
          document.activeElement.closest(".monaco-editor");
        const isFindCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f";

        if (shouldBlock && !(isEditorFocused && isFindCommand)) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          // Spezielle Behandlung für nützliche Shortcuts
          if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c") {
            copyToClipboard();
          } else if (
            (e.metaKey || e.ctrlKey) &&
            e.key.toLowerCase() === "a" &&
            editor &&
            document.activeElement &&
            document.activeElement.closest(".monaco-editor")
          ) {
            const model = editor.getModel();
            if (model) {
              editor.setSelection(model.getFullModelRange());
            }
          }
          // Home-Taste: An Zeilenanfang springen (im Editor)
          else if (
            e.key === "Home" &&
            editor &&
            document.activeElement &&
            document.activeElement.closest(".monaco-editor")
          ) {
            const position = editor.getPosition();
            if (position) {
              editor.setPosition({
                lineNumber: position.lineNumber,
                column: 1,
              });
            }
          }
          // End-Taste: An Zeilenende springen (im Editor)
          else if (
            e.key === "End" &&
            editor &&
            document.activeElement &&
            document.activeElement.closest(".monaco-editor")
          ) {
            const position = editor.getPosition();
            if (position) {
              const model = editor.getModel();
              if (model) {
                const lineLength = model.getLineLength(position.lineNumber);
                editor.setPosition({
                  lineNumber: position.lineNumber,
                  column: lineLength + 1,
                });
              }
            }
          }

          return false;
        }
      },
      true
    ); // Capture phase für höchste Priorität

    // Editor-spezifische Event-Behandlung (zusätzliche Sicherheit)
    editor.getDomNode().addEventListener(
      "keydown",
      (e) => {
        // Gleiche Logik wie oben für doppelte Sicherheit
        const shouldBlock =
          ((e.metaKey || e.ctrlKey) &&
            (e.key.length === 1 ||
              [
                "Backspace",
                "Delete",
                "Enter",
                "Return",
                "Tab",
                "Escape",
              ].includes(e.key))) ||
          ["Home", "End", "PageUp", "PageDown"].includes(e.key) ||
          (e.key.startsWith("F") && e.key.length <= 3);

        // ALLOW CMD+F for search functionality in editor
        const isFindCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f";

        if (shouldBlock && !isFindCommand) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          // Custom-Implementierungen (gleich wie oben)
          if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c") {
            copyToClipboard();
          } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") {
            const model = editor.getModel();
            if (model) {
              editor.setSelection(model.getFullModelRange());
            }
          } else if (e.key === "Home") {
            const position = editor.getPosition();
            if (position) {
              editor.setPosition({
                lineNumber: position.lineNumber,
                column: 1,
              });
            }
          } else if (e.key === "End") {
            const position = editor.getPosition();
            if (position) {
              const model = editor.getModel();
              if (model) {
                const lineLength = model.getLineLength(position.lineNumber);
                editor.setPosition({
                  lineNumber: position.lineNumber,
                  column: lineLength + 1,
                });
              }
            }
          }

          return false;
        }
      },
      true
    );

    // Zusätzliche Sicherheit: Context Menu Events blockieren
    editor.getDomNode().addEventListener("contextmenu", (e) => {
      // Wir lassen das Context Menu zu, aber überschreiben Copy-Aktionen
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
      remove_empty_directories: elements.removeEmptyDirs.checked,
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

  // New ignore pattern event listeners
  elements.deleteAllPatternsBtn.addEventListener("click", () => {
    const newConfig = {
      ...appState.config,
      ignore_patterns: [],
    };
    post("updateConfig", newConfig);
  });

  elements.removeEmptyDirs.addEventListener("change", () => {
    const newConfig = {
      ...appState.config,
      remove_empty_directories: elements.removeEmptyDirs.checked,
    };
    post("updateConfig", newConfig);
  });

  elements.filterPatterns.addEventListener("input", (e) => {
    currentPatternFilter = e.target.value.toLowerCase();
    renderIgnorePatterns();
  });

  function addIgnorePattern() {
    const pattern = elements.newIgnorePattern.value.trim();
    if (pattern) {
      addIgnorePatternValue(pattern);
      elements.newIgnorePattern.value = "";
    }
  }

  function addIgnorePatternValue(pattern) {
    if (pattern && !appState.config.ignore_patterns.includes(pattern)) {
      const newConfig = {
        ...appState.config,
        ignore_patterns: [...appState.config.ignore_patterns, pattern],
      };
      post("updateConfig", newConfig);
    }
  }

  function setupCommonPatterns() {
    elements.commonPatternsContainer.innerHTML = "";

    // Filter out patterns that are already in use
    const availablePatterns = commonIgnorePatterns.filter(
      (pattern) => !appState.config.ignore_patterns.includes(pattern)
    );

    availablePatterns.forEach((pattern) => {
      const chip = document.createElement("button");
      chip.className = "common-pattern-chip";
      chip.textContent = pattern;
      chip.title = `Click to add "${pattern}" to ignore patterns`;
      chip.addEventListener("click", (e) => {
        e.preventDefault();
        addIgnorePatternValue(pattern);
      });
      elements.commonPatternsContainer.appendChild(chip);
    });
  }

  // --- Global Event Handlers from Rust ---
  window.render = (newState) => {
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

  window.showPreviewContent = (content, language, searchTerm, path) => {
    currentPreviewedPath = path;
    editor.setValue(content);
    const model = editor.getModel();

    if (model) {
      monaco.editor.setModelLanguage(model, language);
      let newDecorations = [];
      if (searchTerm && searchTerm.trim() !== "") {
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

    // Scroll to top of file
    editor.setPosition({ lineNumber: 1, column: 1 });
    editor.revealLine(1);

    // Updated: Show path, lines, and file size (not chars)
    const pathStr = path || "Unknown File";
    const lines = content.split("\n").length;
    const sizeBytes = new Blob([content], { type: "text/plain" }).size;
    const sizeFormatted = formatFileSize(sizeBytes);

    elements.previewTitle.innerHTML = `
      <span class="preview-main-title">${pathStr}</span>
      <span class="preview-stats">${lines} lines • ${sizeFormatted} • Read-only</span>
    `;

    elements.copyBtn.style.display = "inline-block";
    elements.clearPreviewBtn.style.display = "inline-block";
  };

  window.showGeneratedContent = (content) => {
    currentPreviewedPath = null;
    editor.setValue(content);
    currentDecorations = editor.deltaDecorations(currentDecorations, []);
    monaco.editor.setModelLanguage(editor.getModel(), "plaintext");
    editor.updateOptions({ readOnly: false });

    // Function to update stats
    const updateGeneratedStats = () => {
      const currentContent = editor.getValue();
      const lines = currentContent.split("\n").length;
      const sizeBytes = new Blob([currentContent], { type: "text/plain" }).size;
      const sizeFormatted = formatFileSize(sizeBytes);

      elements.previewTitle.innerHTML = `
        <span class="preview-main-title">🚀 Generated Preview</span>
        <span class="preview-stats">${lines} lines • ${sizeFormatted} • Editable</span>
      `;
    };

    // Initial stats
    updateGeneratedStats();

    // Add event listener for live updates
    const model = editor.getModel();
    if (model) {
      model.onDidChangeContent(() => {
        updateGeneratedStats();
      });
    }

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

    // Show current config filename if available
    if (appState.current_config_filename) {
      elements.currentConfigFilename.textContent =
        appState.current_config_filename;
    } else {
      elements.currentConfigFilename.textContent = "";
    }

    const { config } = appState;
    elements.caseSensitive.checked = config.case_sensitive_search;
    elements.includeTree.checked = config.include_tree_by_default;
    elements.relativePaths.checked = config.use_relative_paths;
    elements.removeEmptyDirs.checked = config.remove_empty_directories || false;
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

    // Calculate file statistics more clearly
    const { totalFiles, totalFolders } = countTreeItems(appState.tree);
    const visibleItems = appState.visible_files_count;
    const selectedFiles = appState.selected_files_count;

    elements.fileStats.textContent = `Files: ${selectedFiles} selected of ${totalFiles} • Folders: ${totalFolders} • Total visible: ${visibleItems}`;

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

    setupCommonPatterns();
    renderIgnorePatterns();
  }

  // Helper function to count files and folders recursively
  function countTreeItems(nodes) {
    let totalFiles = 0;
    let totalFolders = 0;

    function traverse(items) {
      for (const item of items) {
        if (item.is_directory) {
          totalFolders++;
          if (item.children && item.children.length > 0) {
            traverse(item.children);
          }
        } else {
          totalFiles++;
        }
      }
    }

    traverse(nodes);
    return { totalFiles, totalFolders };
  }

  function renderIgnorePatterns() {
    elements.currentPatternsContainer.innerHTML = "";

    let patterns = (appState.config.ignore_patterns || []).slice().sort();

    // Apply filter if active
    if (currentPatternFilter) {
      patterns = patterns.filter((pattern) =>
        pattern.toLowerCase().includes(currentPatternFilter)
      );
    }

    patterns.forEach((p) => {
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

        const ignoreBtn = document.createElement("button");
        ignoreBtn.className = "ignore-btn";
        ignoreBtn.title = "Add this directory to ignore patterns";
        ignoreBtn.textContent = "i";

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
        const container = document.createElement("div");
        container.className = "tree-item-container";

        const checkbox = document.createElement("input");
        checkbox.type = "checkbox";
        checkbox.checked = node.selection_state === "full";
        checkbox.addEventListener("change", () =>
          post("toggleSelection", node.path)
        );

        const nameSpan = document.createElement("span");
        nameSpan.className = "file-name";
        if (node.is_match) {
          nameSpan.classList.add("is-match");
        }
        nameSpan.textContent = `${node.is_binary ? "🔧" : "📄"} ${node.name}`;
        nameSpan.setAttribute("data-path", node.path);
        nameSpan.addEventListener("click", () => {
          post("loadFilePreview", node.path);
        });

        const ignoreBtn = document.createElement("button");
        ignoreBtn.className = "ignore-btn";
        ignoreBtn.title = "Add this file to ignore patterns";
        ignoreBtn.textContent = "i";
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

  function clearPreview() {
    currentPreviewedPath = null;
    editor.setValue("// Preview cleared.");
    currentDecorations = editor.deltaDecorations(currentDecorations, []);
    editor.updateOptions({ readOnly: true });
    monaco.editor.setModelLanguage(editor.getModel(), "plaintext");
    elements.previewTitle.innerHTML = `
      <span class="preview-main-title">👁️ Preview</span>
      <span class="preview-stats">Select a file to preview</span>
    `;
    elements.saveBtn.disabled = true;
    elements.clearPreviewBtn.style.display = "none";
    elements.copyBtn.style.display = "none";
  }

  // FIX 2: Vollständig überarbeitete Copy-Funktion mit garantierter Funktionalität
  function copyToClipboard() {
    if (!editor) {
      elements.statusBar.textContent = "Error: Editor not available.";
      return;
    }

    // Den aktuellen Editor-Inhalt holen (garantiert der neueste Stand)
    const model = editor.getModel();
    if (!model) {
      elements.statusBar.textContent = "Error: No content to copy.";
      return;
    }

    const contentToCopy = model.getValue();
    if (!contentToCopy || contentToCopy.trim().length === 0) {
      elements.statusBar.textContent = "Error: No content to copy.";
      return;
    }

    const button = elements.copyBtn;
    const originalText = button.textContent;
    const originalStyle = {
      backgroundColor: button.style.backgroundColor,
      color: button.style.color,
    };

    // UI-Feedback sofort anzeigen
    button.textContent = "⏳ Copying...";
    button.style.backgroundColor = "#FFA500";
    button.disabled = true;

    // Clipboard API mit mehreren Fallback-Methoden
    const copyMethods = [
      // Methode 1: Moderne Clipboard API
      () => navigator.clipboard.writeText(contentToCopy),

      // Methode 2: Fallback für ältere Browser
      () =>
        new Promise((resolve, reject) => {
          const textArea = document.createElement("textarea");
          textArea.value = contentToCopy;
          textArea.style.position = "fixed";
          textArea.style.left = "-999999px";
          textArea.style.top = "-999999px";
          document.body.appendChild(textArea);
          textArea.focus();
          textArea.select();

          try {
            const successful = document.execCommand("copy");
            document.body.removeChild(textArea);
            if (successful) {
              resolve();
            } else {
              reject(new Error("execCommand copy failed"));
            }
          } catch (err) {
            document.body.removeChild(textArea);
            reject(err);
          }
        }),
    ];

    // Versuche die Kopiermethoden der Reihe nach
    const tryCopyMethod = async (methodIndex = 0) => {
      if (methodIndex >= copyMethods.length) {
        throw new Error("All copy methods failed");
      }

      try {
        await copyMethods[methodIndex]();
        return true;
      } catch (error) {
        console.warn(`Copy method ${methodIndex + 1} failed:`, error);
        return tryCopyMethod(methodIndex + 1);
      }
    };

    tryCopyMethod()
      .then(() => {
        // Erfolg
        elements.statusBar.textContent = `✅ Content copied to clipboard! (${contentToCopy.length} characters)`;
        button.textContent = "✅ Copied!";
        button.style.backgroundColor = "#34a853";
        button.style.color = "#ffffff";
      })
      .catch((error) => {
        // Fehler
        console.error("Failed to copy to clipboard:", error);
        elements.statusBar.textContent =
          "❌ Failed to copy to clipboard. Try selecting and copying manually.";
        button.textContent = "❌ Failed";
        button.style.backgroundColor = "#dc3545";
        button.style.color = "#ffffff";
      })
      .finally(() => {
        // UI nach 3 Sekunden zurücksetzen
        setTimeout(() => {
          button.textContent = originalText;
          button.style.backgroundColor = originalStyle.backgroundColor;
          button.style.color = originalStyle.color;
          button.disabled = false;

          // Status zurücksetzen falls kein neuer Status gesetzt wurde
          if (
            elements.statusBar.textContent.includes("copied to clipboard") ||
            elements.statusBar.textContent.includes("Failed to copy")
          ) {
            if (appState && appState.status_message) {
              elements.statusBar.textContent = `Status: ${appState.status_message}`;
            } else {
              elements.statusBar.textContent = "Status: Ready.";
            }
          }
        }, 3000);
      });
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

  post("initialize");
});
