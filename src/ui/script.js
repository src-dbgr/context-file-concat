document.addEventListener("DOMContentLoaded", () => {
  let editor;
  let appState = {};
  let filterDebounceTimeout;
  let currentDecorations = [];
  let currentPreviewedPath = null;

  const elements = {
    // Top bar
    selectDirBtn: document.getElementById("select-dir-btn"),
    clearDirBtn: document.getElementById("clear-dir-btn"), // NEU
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
    "venv",
    "*.ipynb",
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

    // DEFINIERE HILFSFUNKTIONEN ZUERST (vor Event Listeners!)

    // Verbesserte Monaco Find Widget Erkennung
    function isInMonacoFindWidget() {
      const activeEl = document.activeElement;
      if (!activeEl) {
        return false;
      }

      // Debug: Schaue nach allen möglichen Selektoren
      const possibleSelectors = [
        ".find-widget",
        ".find-part",
        ".monaco-findInput",
        ".find-box",
        ".editor-widget",
        ".find-input",
        ".monaco-find-input",
      ];

      for (const selector of possibleSelectors) {
        if (activeEl.closest(selector)) {
          return true;
        }
      }

      // Prüfe auch aria-label
      const ariaLabel =
        activeEl.getAttribute && activeEl.getAttribute("aria-label");
      if (ariaLabel && ariaLabel.toLowerCase().includes("find")) {
        return true;
      }

      // Prüfe Class-Namen des aktiven Elements
      if (activeEl.className && activeEl.className.includes("find")) {
        return true;
      }
      return false;
    }

    // ERSETZE die handleSafePaste Funktion mit dieser erweiterten Debug-Version:

    // ERSETZE die komplette handleSafePaste Funktion mit dieser direkteren Version:

    // ERSETZE die handleSafePaste Funktion mit dieser zuverlässigen Version:

    function handleSafePaste() {
      const activeEl = document.activeElement;

      // VERBESSERTE ERKENNUNG WO WIR SIND
      const isInNormalInputField =
        activeEl &&
        (activeEl.tagName === "INPUT" || activeEl.tagName === "TEXTAREA") &&
        !activeEl.closest(".monaco-editor");

      const isInMonacoFind = isInMonacoFindWidget();

      const isEditorFocused =
        activeEl && activeEl.closest(".monaco-editor") && !isInMonacoFind;

      // VERBESSERTE Legacy-Methode mit TEXTAREA für Multi-line Support
      const getClipboardViaLegacy = () => {
        return new Promise((resolve, reject) => {
          // WICHTIG: Verwende TEXTAREA statt INPUT um Zeilenumbrüche zu erhalten
          const tempTextarea = document.createElement("textarea");
          tempTextarea.style.position = "fixed";
          tempTextarea.style.left = "-9999px";
          tempTextarea.style.top = "-9999px";
          tempTextarea.style.width = "1px";
          tempTextarea.style.height = "1px";
          tempTextarea.style.opacity = "0";
          tempTextarea.style.pointerEvents = "none";

          document.body.appendChild(tempTextarea);

          // Focus und Select
          tempTextarea.focus();
          tempTextarea.select();

          // Versuche execCommand paste
          const success = document.execCommand("paste");
          const value = tempTextarea.value;

          // Cleanup
          document.body.removeChild(tempTextarea);

          if (success && value) {
            // Prüfe auf Multi-line
            const hasLineBreaks = value.includes("\n") || value.includes("\r");
            const lineCount = hasLineBreaks
              ? value.split(/\r\n|\r|\n/).length
              : 1;

            resolve(value);
          } else {
            reject(new Error("Legacy clipboard failed"));
          }
        });
      };

      // Führe zuverlässige Legacy-Methode aus
      getClipboardViaLegacy()
        .then((clipboardText) => {
          if (!clipboardText || clipboardText.trim() === "") {
            elements.statusBar.textContent = "Clipboard is empty.";
            return;
          }

          // Multi-line Analyse
          const hasLineBreaks =
            clipboardText.includes("\n") || clipboardText.includes("\r");
          const lineCount = hasLineBreaks
            ? clipboardText.split(/\r\n|\r|\n/).length
            : 1;

          if (isInNormalInputField) {
            insertTextIntoElement(activeEl, clipboardText);
            elements.statusBar.textContent = `✓ Text pasted into input field.`;
          } else if (isInMonacoFind) {
            // SIMPEL: Paste Rohtext direkt - Monaco kann Multi-line Suche!
            if (hasLineBreaks) {
              const lineCount = clipboardText.split(/\r\n|\r|\n/).length;
              elements.statusBar.textContent = `✓ Multi-line search text pasted (${lineCount} lines).`;
            } else {
              elements.statusBar.textContent = `✓ Text pasted into Monaco search field.`;
            }

            // Direkt den rohen Clipboard-Text einfügen
            insertTextIntoElement(activeEl, clipboardText, true);
          } else if (isEditorFocused && editor) {
            const selection = editor.getSelection();
            const range = selection || editor.getModel().getFullModelRange();

            editor.executeEdits("paste", [
              {
                range: range,
                text: clipboardText,
              },
            ]);

            elements.statusBar.textContent = `✓ Text pasted into editor.`;
          } else {
            elements.statusBar.textContent = `✗ Paste not supported here.`;
          }
        })
        .catch((error) => {
          // Fallback: User Prompt
          const userText = prompt(
            "Clipboard access failed. Please paste your text here:"
          );
          if (userText) {
            if (isInMonacoFind) {
              // Auch hier Multi-line behandeln
              const hasUserLineBreaks =
                userText.includes("\n") || userText.includes("\r");
              let processedUserText = userText;

              if (hasUserLineBreaks) {
                const userLineCount = userText.split(/\r\n|\r|\n/).length;
                processedUserText = userText
                  .replace(/\r\n/g, " ")
                  .replace(/\n/g, " ")
                  .replace(/\r/g, " ")
                  .replace(/\s+/g, " ")
                  .trim();
                elements.statusBar.textContent = `⚠ User text (${userLineCount} lines) converted to single line.`;
              } else {
                elements.statusBar.textContent = `✓ User text entered manually.`;
              }

              insertTextIntoElement(activeEl, processedUserText, true);
            } else {
              insertTextIntoElement(activeEl, userText);
              elements.statusBar.textContent = `✓ User text entered manually.`;
            }
          } else {
            elements.statusBar.textContent = "✗ No text provided.";
          }
        });
    }

    // BEHALTE die insertTextIntoElement Funktion (bleibt gleich)
    function insertTextIntoElement(element, text, triggerMonacoEvents = false) {
      if (!element) {
        return;
      }

      const start = element.selectionStart || 0;
      const end = element.selectionEnd || 0;
      const value = element.value || "";

      // Text einfügen
      element.value = value.slice(0, start) + text + value.slice(end);
      element.selectionStart = element.selectionEnd = start + text.length;

      // Standard Events triggern
      const standardEvents = ["input", "change"];
      standardEvents.forEach((eventType) => {
        try {
          element.dispatchEvent(new Event(eventType, { bubbles: true }));
        } catch (e) {}
      });

      // Monaco Events falls benötigt
      if (triggerMonacoEvents) {
        try {
          element.dispatchEvent(
            new InputEvent("beforeinput", {
              bubbles: true,
              data: text,
              inputType: "insertText",
            })
          );
        } catch (e) {}
      }

      element.focus();
    }

    // JETZT DIE EVENT LISTENERS (nach den Funktionsdefinitionen)

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

        // VERBESSERTE FOKUS-ERKENNUNG
        const activeEl = document.activeElement;

        const isEditorFocused = activeEl && activeEl.closest(".monaco-editor");
        const isInMonacoFind = isInMonacoFindWidget();
        const isInNormalInputField =
          activeEl &&
          (activeEl.tagName === "INPUT" || activeEl.tagName === "TEXTAREA") &&
          !isInMonacoFind && // Nicht das Monaco Find Widget
          !activeEl.closest(".monaco-editor"); // Nicht im Editor

        const isFindCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f";
        const isCopyCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c";
        const isPasteCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "v";
        const isSelectAllCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a";

        // KORRIGIERTE Prüfung ob Text im Editor selektiert ist
        const hasEditorSelection =
          editor &&
          isEditorFocused &&
          !isInMonacoFind && // WICHTIG: Nicht wenn wir im Find Widget sind
          !editor.getSelection().isEmpty();

        // COPY-Behandlung: Unterscheide zwischen Selektion und vollem File
        if (isCopyCommand) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          if (hasEditorSelection) {
            // Text ist selektiert - kopiere nur Selektion
            copySelectedTextToClipboard();
          } else {
            // Kein Text selektiert - kopiere das gesamte File
            copyToClipboard();
          }
          return false;
        }

        // PASTE-Behandlung: IMMER blockieren, aber manuelle Implementierung für sichere Bereiche
        if (isPasteCommand) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          // Manuelle Paste-Implementierung für sichere Bereiche
          handleSafePaste();
          return false;
        }

        // SELECT ALL-Behandlung: Kontextabhängig
        if (isSelectAllCommand) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          console.log("=== SELECT ALL DEBUG ===");
          console.log("Context detection:", {
            isInMonacoFind,
            isInNormalInputField,
            isEditorFocused,
            activeElementTag: activeEl?.tagName,
          });

          if (isInMonacoFind) {
            // Select All im Monaco Find Widget
            console.log("Selecting all in Monaco Find Widget");
            if (activeEl && activeEl.select) {
              activeEl.select();
            }
          } else if (isInNormalInputField) {
            // Select All in normalen Input-Feldern
            console.log("Selecting all in normal input field");
            if (activeEl && activeEl.select) {
              activeEl.select();
            }
          } else if (isEditorFocused && editor) {
            // Select All im Editor
            console.log("Selecting all in editor");
            const model = editor.getModel();
            if (model) {
              editor.setSelection(model.getFullModelRange());
            }
          }
          return false;
        }

        if (
          shouldBlock &&
          !(isEditorFocused && isFindCommand) &&
          !isCopyCommand &&
          !isPasteCommand &&
          !isSelectAllCommand
        ) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          // Home-Taste: An Zeilenanfang springen (im Editor)
          if (
            e.key === "Home" &&
            editor &&
            isEditorFocused &&
            !isInMonacoFind
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
            isEditorFocused &&
            !isInMonacoFind
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
    );

    // Editor-spezifische Event-Behandlung (zusätzliche Sicherheit)
    editor.getDomNode().addEventListener(
      "keydown",
      (e) => {
        // GLEICHE LOGIK WIE BEIM ERSTEN EVENT LISTENER
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

        const isInMonacoFind = isInMonacoFindWidget();
        const isFindCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f";
        const isCopyCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c";
        const isPasteCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "v";
        const isSelectAllCommand =
          (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a";

        // KORRIGIERTE Prüfung ob Text selektiert ist (nur wenn NICHT im Find Widget)
        const hasEditorSelection =
          editor && !isInMonacoFind && !editor.getSelection().isEmpty();

        // COPY-Behandlung: Unterscheide zwischen Selektion und vollem File
        if (isCopyCommand) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          if (hasEditorSelection) {
            copySelectedTextToClipboard();
          } else {
            copyToClipboard();
          }
          return false;
        }

        // PASTE-Behandlung: IMMER blockieren, aber manuelle Implementierung
        if (isPasteCommand) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          handleSafePaste();
          return false;
        }

        // SELECT ALL-Behandlung: Kontextabhängig (auch hier)
        if (isSelectAllCommand) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          const activeEl = document.activeElement;

          if (isInMonacoFind) {
            // Select All im Find Widget
            if (activeEl && activeEl.select) {
              activeEl.select();
            }
          } else if (editor) {
            // Select All im Editor
            const model = editor.getModel();
            if (model) {
              editor.setSelection(model.getFullModelRange());
            }
          }
          return false;
        }

        if (
          shouldBlock &&
          !isFindCommand &&
          !isCopyCommand &&
          !isPasteCommand &&
          !isSelectAllCommand
        ) {
          e.preventDefault();
          e.stopPropagation();
          e.stopImmediatePropagation();

          if (e.key === "Home" && !isInMonacoFind) {
            const position = editor.getPosition();
            if (position) {
              editor.setPosition({
                lineNumber: position.lineNumber,
                column: 1,
              });
            }
          } else if (e.key === "End" && !isInMonacoFind) {
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
  elements.clearDirBtn.addEventListener("click", () => post("clearDirectory")); // NEU
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
            <line x1="18" y1="6" x2="6" y2="18"/>
            <line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
          Cancel
        </button>
      </div>
      <div class="scan-progress-bar">
        <div class="scan-progress-fill" id="scan-progress-fill"></div>
      </div>
      <div class="scan-details">
        <span id="scan-files-count">0 files processed</span>
        <span id="scan-current-path">Starting scan...</span>
        <span id="scan-skipped-count"></span>
      </div>
    </div>
  `;
  }

  // --- Global Event Handlers from Rust ---

  // NEU: Handler für Drag-and-Drop-Zustand
  window.setDragState = (isDragging) => {
    const container = elements.fileTreeContainer;
    if (isDragging) {
      container.classList.add("drag-over");
    } else {
      container.classList.remove("drag-over");
    }
  };

  window.render = (newState) => {
    const wasScanning = appState.is_scanning;
    const isNowScanning = newState.is_scanning;

    // Behandelt die Such-Markierungen im Editor
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

    // Erkennt den Abschluss eines Scans, um die UI zu aktualisieren
    if (wasScanning && !isNowScanning) {
      const progressFill = document.getElementById("scan-progress-fill");
      if (progressFill) {
        progressFill.style.width = "100%";
        progressFill.classList.add("scan-complete");
      }
      elements.statusBar.textContent = `Status: Scan completed! Found ${newState.total_files_found} files.`;
      elements.statusBar.classList.remove("scanning");
      setTimeout(() => {
        elements.selectDirBtn.disabled = false;
        elements.rescanBtn.disabled = false;
        elements.selectDirBtn.innerHTML = `
          <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/>
          </svg>
          Select Directory
        `;
        elements.rescanBtn.innerHTML = `
          <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/>
            <path d="M21 3v5h-5"/>
            <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/>
            <path d="M3 21v-5h5"/>
          </svg>
          Re-Scan
        `;
      }, 500);
    }

    // Erkennt den Start eines Scans
    if (!wasScanning && isNowScanning) {
      elements.statusBar.textContent = "Status: Starting directory scan...";
      elements.statusBar.classList.add("scanning");
    }

    // Aktualisiert den globalen State und rendert die Haupt-UI
    appState = newState;
    renderUI();
  };

  // Enhanced updateScanProgress function
  window.updateScanProgress = (progress) => {
    if (!appState.is_scanning) return;

    const scanText = document.querySelector(".scan-text");
    const scanFilesCount = document.getElementById("scan-files-count");
    const scanCurrentPath = document.getElementById("scan-current-path");
    const scanSkippedCount = document.getElementById("scan-skipped-count");
    const scanProgressFill = document.getElementById("scan-progress-fill");

    if (scanText) {
      scanText.textContent = "Scanning directory...";
    }

    if (scanFilesCount) {
      scanFilesCount.textContent = `${progress.files_scanned} files processed`;
    }

    if (scanCurrentPath) {
      scanCurrentPath.textContent =
        progress.current_scanning_path || "Processing...";
    }

    if (scanSkippedCount) {
      if (progress.large_files_skipped > 0) {
        scanSkippedCount.textContent = `${progress.large_files_skipped} large files skipped`;
        scanSkippedCount.style.display = "inline";
      } else {
        scanSkippedCount.style.display = "none";
      }
    }

    // Animate progress bar (estimated progress)
    if (scanProgressFill && progress.files_scanned > 0) {
      const estimatedProgress = Math.min(
        90,
        (progress.files_scanned / 100) * 100
      );
      scanProgressFill.style.width = `${estimatedProgress}%`;
    }

    // Update status bar
    let statusText = `Scanning... ${progress.files_scanned} files processed`;
    if (progress.large_files_skipped > 0) {
      statusText += `, ${progress.large_files_skipped} large files skipped`;
    }
    if (progress.current_scanning_path) {
      statusText += ` (${progress.current_scanning_path})`;
    }

    elements.statusBar.textContent = `Status: ${statusText}`;
  };

  function splitPathForDisplay(fullPath) {
    if (!fullPath) return { pathPart: "", filename: "Unknown File" };

    // Berechne relativen Pfad zum selected directory
    const currentDir = appState.current_path;
    let relativePath = fullPath;

    if (currentDir && fullPath.startsWith(currentDir)) {
      relativePath = fullPath.substring(currentDir.length);
      // Entferne führende Slashes
      relativePath = relativePath.replace(/^[\/\\]+/, "");
    }

    const parts = relativePath.replace(/\\/g, "/").split("/");

    if (parts.length <= 1) {
      return { pathPart: "", filename: relativePath };
    }

    const filename = parts[parts.length - 1];
    const pathPart = parts.slice(0, -1).join("/") + "/";

    return { pathPart, filename };
  }

  window.showPreviewContent = (content, language, searchTerm, path) => {
    currentPreviewedPath = path;
    currentFullPath = path;

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
    editor.setPosition({ lineNumber: 1, column: 1 });
    editor.revealLine(1);

    const pathStr = path || "Unknown File";
    const { pathPart, filename } = splitPathForDisplay(pathStr);
    const lines = content.split("\n").length;
    const sizeBytes = new Blob([content], { type: "text/plain" }).size;
    const sizeFormatted = formatFileSize(sizeBytes);

    const previewTitle = document.querySelector(
      ".preview-panel #preview-title"
    );
    if (previewTitle) {
      // Relativer Pfad mit einheitlicher Schrift
      previewTitle.innerHTML = `
      <div class="preview-path-container" title="${pathStr}">
        <span class="preview-path-part">${pathPart}</span><span class="preview-filename">${filename}</span>
      </div>
      <span class="preview-stats">${lines} lines • ${sizeFormatted} • Read-only</span>
    `;
    }

    elements.copyBtn.style.display = "inline-block";
    elements.clearPreviewBtn.style.display = "inline-block";
  };

  window.showGeneratedContent = (content) => {
    currentPreviewedPath = null;
    currentFullPath = null;

    editor.setValue(content);
    currentDecorations = editor.deltaDecorations(currentDecorations, []);
    monaco.editor.setModelLanguage(editor.getModel(), "plaintext");
    editor.updateOptions({ readOnly: false });

    const updateGeneratedStats = () => {
      const currentContent = editor.getValue();
      const lines = currentContent.split("\n").length;
      const sizeBytes = new Blob([currentContent], { type: "text/plain" }).size;
      const sizeFormatted = formatFileSize(sizeBytes);

      const previewTitle = document.querySelector(
        ".preview-panel #preview-title"
      );
      if (previewTitle) {
        previewTitle.innerHTML = `
        <div class="preview-path-container">
          <span class="preview-filename">
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polygon points="13,2 3,14 12,14 11,22 21,10 12,10 13,2"/>
            </svg>
            Generated Preview
          </span>
        </div>
        <span class="preview-stats">${lines} lines • ${sizeFormatted} • Editable</span>
      `;
      }
    };

    updateGeneratedStats();

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

    // NEU: Sichtbarkeit des Clear-Buttons steuern
    if (appState.current_path) {
      elements.clearDirBtn.style.display = "inline-block";
    } else {
      elements.clearDirBtn.style.display = "none";
    }

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
    // ENHANCED: Scan state management
    const isScanning = appState.is_scanning;

    // Disable/Enable buttons based on scan state
    elements.selectDirBtn.disabled = isScanning;
    elements.rescanBtn.disabled = isScanning;
    elements.importConfigBtn.disabled = isScanning;
    elements.generateBtn.disabled = !hasSelection || isScanning;

    // Update button text during scan
    if (isScanning) {
      elements.selectDirBtn.innerHTML = `
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="10"/>
          <polyline points="12,6 12,12 16,14"/>
        </svg>
        Scanning...
      `;
      elements.rescanBtn.innerHTML = `
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="10"/>
          <polyline points="12,6 12,12 16,14"/>
        </svg>
        Scanning...
      `;
    } else {
      elements.selectDirBtn.innerHTML = `
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/>
        </svg>
        Select Directory
      `;
      elements.rescanBtn.innerHTML = `
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/>
          <path d="M21 3v5h-5"/>
          <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/>
          <path d="M3 21v-5h5"/>
        </svg>
        Re-Scan
      `;
    }

    // Enhanced file tree container with scan progress
    elements.fileTreeContainer.innerHTML = "";
    if (isScanning) {
      elements.fileTreeContainer.innerHTML = createScanProgressUI();

      // Add cancel scan event listener
      const cancelBtn = document.getElementById("cancel-scan-btn");
      if (cancelBtn) {
        cancelBtn.addEventListener("click", () => {
          post("cancelScan");
          cancelBtn.disabled = true;
          cancelBtn.innerHTML = `
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="12" cy="12" r="10"/>
              <polyline points="12,6 12,12 16,14"/>
            </svg>
            Cancelling...
          `;
        });
      }

      // Initialize progress bar
      const progressFill = document.getElementById("scan-progress-fill");
      if (progressFill) {
        progressFill.style.width = "0%";
      }
    } else if (appState.tree.length > 0) {
      const treeRoot = document.createElement("div");
      treeRoot.className = "tree";
      treeRoot.appendChild(createTreeLevel(appState.tree));
      elements.fileTreeContainer.appendChild(treeRoot);
    } else if (appState.current_path) {
      elements.fileTreeContainer.innerHTML =
        '<p class="placeholder">No files found matching filters.</p>';
    } else {
      // GEÄNDERT: Platzhaltertext für Drag & Drop
      elements.fileTreeContainer.innerHTML =
        '<p class="placeholder">Select or drop a directory to start.</p>';

      // NEU: Platzhalter klickbar machen
      const placeholder =
        elements.fileTreeContainer.querySelector(".placeholder");
      if (placeholder) {
        placeholder.style.cursor = "pointer";
        placeholder.addEventListener("click", () => post("selectDirectory"));
      }
    }

    elements.statusBar.textContent = `Status: ${appState.status_message}`;

    // Calculate file statistics more clearly
    const { totalFiles, totalFolders } = countTreeItems(appState.tree);
    const visibleItems = appState.visible_files_count;
    const selectedFiles = appState.selected_files_count;

    elements.fileStats.textContent = `Files: ${selectedFiles} selected of ${totalFiles} • Folders: ${totalFolders}`;

    setupCommonPatterns();
    renderIgnorePatterns();
  }

  // Enhanced post function with immediate UI feedback
  const originalPost = post;
  window.post = (command, payload = null) => {
    // Immediate UI feedback for scan operations
    if (command === "selectDirectory") {
      // Show immediate feedback
      elements.selectDirBtn.disabled = true;
      elements.selectDirBtn.innerHTML = `
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="10"/>
          <polyline points="12,6 12,12 16,14"/>
        </svg>
        Selecting...
      `;
      elements.statusBar.textContent = "Status: Selecting directory...";
    } else if (command === "rescanDirectory") {
      // Show immediate feedback
      elements.rescanBtn.disabled = true;
      elements.rescanBtn.innerHTML = `
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="10"/>
          <polyline points="12,6 12,12 16,14"/>
        </svg>
        Starting scan...
      `;
      elements.statusBar.textContent = "Status: Starting directory scan...";
    }

    // Call original post function
    originalPost(command, payload);
  };

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

    const allPatterns = Array.from(appState.config.ignore_patterns || []);
    const activePatterns = new Set(appState.active_ignore_patterns || []);

    // 1. Separate into active and inactive
    const active = allPatterns.filter((p) => activePatterns.has(p));
    const inactive = allPatterns.filter((p) => !activePatterns.has(p));

    // 2. Sort each list alphabetically
    active.sort();
    inactive.sort();

    // 3. Combine them: active first, then inactive
    let patterns = [...active, ...inactive];

    // Apply filter if active
    if (currentPatternFilter) {
      patterns = patterns.filter((pattern) =>
        pattern.toLowerCase().includes(currentPatternFilter)
      );
    }

    patterns.forEach((p) => {
      const chip = document.createElement("div");
      chip.className = "current-pattern-chip";

      // 4. Add 'active-pattern' class if the pattern was active
      if (activePatterns.has(p)) {
        chip.classList.add("active-pattern");
        chip.title = `This pattern was active and matched one or more files/directories.`;
      }

      chip.innerHTML = `<span>${p}</span><button class="remove-pattern-btn" data-pattern="${p}">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="18" y1="6" x2="6" y2="18"/>
          <line x1="6" y1="6" x2="18" y2="18"/>
        </svg>
      </button>`;
      chip.querySelector("button").addEventListener("click", (e) => {
        const patternToRemove = e.target.closest("button").dataset.pattern;
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
        nameSpan.innerHTML = `
          <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/>
          </svg>
          ${node.name}
        `;
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
        if (node.is_previewed) {
          container.classList.add("previewed");
        }
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

        const iconSvg = node.is_binary
          ? `
          <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>
          </svg>
        `
          : `
          <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
            <polyline points="14,2 14,8 20,8"/>
            <line x1="16" y1="13" x2="8" y2="13"/>
            <line x1="16" y1="17" x2="8" y2="17"/>
            <polyline points="10,9 9,9 8,9"/>
          </svg>
        `;

        nameSpan.innerHTML = `${iconSvg} ${node.name}`;
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
    post("clearPreviewState");
    currentPreviewedPath = null;
    currentFullPath = null;

    editor.setValue("// Preview cleared.");
    currentDecorations = editor.deltaDecorations(currentDecorations, []);
    editor.updateOptions({ readOnly: true });
    monaco.editor.setModelLanguage(editor.getModel(), "plaintext");

    const previewTitle = document.querySelector(
      ".preview-panel #preview-title"
    );
    if (previewTitle) {
      previewTitle.innerHTML = `
      <div class="preview-path-container">
        <span class="preview-filename">Preview</span>
      </div>
      <span class="preview-stats">Select a file to preview</span>
    `;
    }

    elements.saveBtn.disabled = true;
    elements.clearPreviewBtn.style.display = "none";
    elements.copyBtn.style.display = "none";
  }

  // MODIFIZIERTE VERSION der bestehenden copyToClipboard Funktion
  // (Diese Funktion bleibt größtenteils gleich, aber mit klarerer Dokumentation)
  function copyToClipboard() {
    if (!editor) {
      elements.statusBar.textContent = "Error: Editor not available.";
      return;
    }

    // WICHTIG: Diese Funktion kopiert IMMER das gesamte File, unabhängig von der Selektion
    const model = editor.getModel();
    if (!model) {
      elements.statusBar.textContent = "Error: No content to copy.";
      return;
    }

    const contentToCopy = model.getValue(); // Gesamter Inhalt, nicht nur Selektion
    if (!contentToCopy || contentToCopy.trim().length === 0) {
      elements.statusBar.textContent = "Error: No content to copy.";
      return;
    }

    const button = elements.copyBtn;
    const originalText = button.innerHTML;
    const originalStyle = {
      backgroundColor: button.style.backgroundColor,
      color: button.style.color,
    };

    // UI-Feedback sofort anzeigen
    button.innerHTML = `
      <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <circle cx="12" cy="12" r="10"/>
        <polyline points="12,6 12,12 16,14"/>
      </svg>
      Copying...
    `;
    button.style.backgroundColor = "#FF6B35";
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
        elements.statusBar.textContent = `✓ Complete file copied to clipboard! (${contentToCopy.length} characters)`;
        button.innerHTML = `
          <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polyline points="20,6 9,17 4,12"/>
          </svg>
          Copied!
        `;
        button.style.backgroundColor = "#22C55E";
        button.style.color = "#ffffff";
      })
      .catch((error) => {
        // Fehler
        console.error("Failed to copy to clipboard:", error);
        elements.statusBar.textContent =
          "✗ Failed to copy to clipboard. Try selecting and copying manually.";
        button.innerHTML = `
          <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <line x1="18" y1="6" x2="6" y2="18"/>
            <line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
          Failed
        `;
        button.style.backgroundColor = "#EF4444";
        button.style.color = "#ffffff";
      })
      .finally(() => {
        // UI nach 3 Sekunden zurücksetzen
        setTimeout(() => {
          button.innerHTML = originalText;
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
        }, 1000);
      });
  }

  function copySelectedTextToClipboard() {
    if (!editor) {
      elements.statusBar.textContent = "Error: Editor not available.";
      return;
    }

    const selection = editor.getSelection();
    if (!selection || selection.isEmpty()) {
      elements.statusBar.textContent = "No text selected.";
      return;
    }

    const model = editor.getModel();
    if (!model) {
      elements.statusBar.textContent = "Error: No content available.";
      return;
    }

    const selectedText = model.getValueInRange(selection);
    if (!selectedText || selectedText.trim().length === 0) {
      elements.statusBar.textContent = "No text selected.";
      return;
    }

    // Clipboard API mit Fallback-Methoden (gleiche Logik wie in copyToClipboard)
    const copyMethods = [
      // Methode 1: Moderne Clipboard API
      () => navigator.clipboard.writeText(selectedText),

      // Methode 2: Fallback für ältere Browser
      () =>
        new Promise((resolve, reject) => {
          const textArea = document.createElement("textarea");
          textArea.value = selectedText;
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
        const lines = selectedText.split("\n").length;
        elements.statusBar.textContent = `✓ Selected text copied to clipboard! (${selectedText.length} characters, ${lines} lines)`;
      })
      .catch((error) => {
        // Fehler
        console.error("Failed to copy selected text to clipboard:", error);
        elements.statusBar.textContent =
          "✗ Failed to copy selected text to clipboard.";
      });
  }

  function formatFileSize(bytes) {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
  }

  // --- Resizer Logic (Vertikal) ---
  let mouseDown = false;
  elements.resizer.addEventListener("mousedown", () => {
    mouseDown = true;

    // NEU: Visual Feedback hinzufügen
    document.body.classList.add("vertical-resizing");
    elements.resizer.classList.add("resizing");
    document.body.style.cursor = "ns-resize";
  });

  // --- Sidebar Horizontal Resizer Logic (bleibt gleich) ---
  let sidebarMouseDown = false;
  let startX = 0;
  let startWidth = 0;

  document.addEventListener("mousedown", (e) => {
    const sidebar = document.querySelector(".sidebar");
    if (!sidebar) return;

    const rect = sidebar.getBoundingClientRect();
    const rightEdge = rect.right;

    if (e.clientX >= rightEdge - 5 && e.clientX <= rightEdge + 5) {
      sidebarMouseDown = true;
      startX = e.clientX;
      startWidth = parseInt(getComputedStyle(sidebar).width, 10);

      document.body.classList.add("sidebar-resizing");
      sidebar.classList.add("resizing");

      e.preventDefault();
    }
  });

  // ERWEITERT: Mouseup für beide Resizer mit Visual Feedback
  document.addEventListener("mouseup", () => {
    // Vertikaler Resizer
    if (mouseDown) {
      mouseDown = false;

      // NEU: Visual Feedback entfernen
      document.body.classList.remove("vertical-resizing");
      elements.resizer.classList.remove("resizing");
    }

    // Horizontaler Sidebar Resizer
    if (sidebarMouseDown) {
      sidebarMouseDown = false;

      document.body.classList.remove("sidebar-resizing");
      const sidebar = document.querySelector(".sidebar");
      if (sidebar) {
        sidebar.classList.remove("resizing");
      }
    }

    // Cursor zurücksetzen
    document.body.style.cursor = "default";
  });

  // Mousemove bleibt gleich...
  document.addEventListener("mousemove", (e) => {
    const sidebar = document.querySelector(".sidebar");

    // Sidebar Horizontal Resize
    if (sidebarMouseDown && sidebar) {
      const dx = e.clientX - startX;
      let newWidth = startWidth + dx;

      const minWidth = 280;
      const maxWidth = 600;

      newWidth = Math.max(minWidth, Math.min(maxWidth, newWidth));

      sidebar.style.width = newWidth + "px";
      e.preventDefault();
      return;
    }

    // Vertikaler File Panel Resize
    if (mouseDown) {
      const totalHeight = elements.contentSplitter.offsetHeight;
      const newTopHeight = e.clientY - elements.fileListPanel.offsetTop;
      if (newTopHeight > 100 && newTopHeight < totalHeight - 100) {
        const newTopPercent = (newTopHeight / totalHeight) * 100;
        elements.fileListPanel.style.height = `${newTopPercent}%`;
        elements.previewPanel.style.height = `${100 - newTopPercent}%`;
      }
      return;
    }

    // Cursor ändern für Sidebar Resize Hit-Bereich
    if (sidebar) {
      const rect = sidebar.getBoundingClientRect();
      const rightEdge = rect.right;

      if (e.clientX >= rightEdge - 5 && e.clientX <= rightEdge + 5) {
        document.body.style.cursor = "ew-resize";
      } else if (document.body.style.cursor === "ew-resize") {
        document.body.style.cursor = "default";
      }
    }
  });

  // Cursor zurücksetzen wenn Maus die Sidebar verlässt
  document.querySelector(".sidebar")?.addEventListener("mouseleave", () => {
    if (!sidebarMouseDown && document.body.style.cursor === "ew-resize") {
      document.body.style.cursor = "default";
    }
  });

  post("initialize");
});
