document.addEventListener("DOMContentLoaded", () => {
  const selectDirBtn = document.getElementById("select-dir-btn");
  const generateBtn = document.getElementById("generate-btn");
  const saveBtn = document.getElementById("save-btn");
  const selectAllBtn = document.getElementById("select-all-btn");
  const deselectAllBtn = document.getElementById("deselect-all-btn");
  const expandAllBtn = document.getElementById("expand-all-btn");
  const collapseAllBtn = document.getElementById("collapse-all-btn");
  const currentPathEl = document.getElementById("current-path");
  const fileTreeContainer = document.getElementById("file-tree-container");
  const editorContainer = document.getElementById("editor-container");
  const statusBar = document.getElementById("status-bar");
  let editor;

  require.config({
    paths: { vs: "https://cdn.jsdelivr.net/npm/monaco-editor@0.45.0/min/vs" },
  });
  require(["vs/editor/editor.main"], () => {
    editor = monaco.editor.create(editorContainer, {
      value:
        "// 1. Wähle ein Verzeichnis aus.\n// 2. Wähle Dateien aus.\n// 3. Klicke 'Preview generieren'.",
      language: "plaintext",
      theme: "vs-dark",
      readOnly: true,
    });
  });

  function postMessage(command, payload = null) {
    window.ipc.postMessage(JSON.stringify({ command, payload }));
  }

  selectDirBtn.addEventListener("click", () => postMessage("selectDirectory"));
  generateBtn.addEventListener("click", () => {
    const selected = getSelectedFiles();
    if (selected.length > 0) {
      statusBar.textContent = "Status: Generiere Inhalt...";
      setButtonsDisabled(true);
      postMessage("generateContent", selected);
    }
  });
  saveBtn.addEventListener("click", () => {
    if (editor && !saveBtn.disabled) {
      statusBar.textContent = "Status: Speichere Datei...";
      setButtonsDisabled(true);
      postMessage("saveFile", editor.getValue());
    }
  });
  selectAllBtn.addEventListener("click", () => toggleAllCheckboxes(true));
  deselectAllBtn.addEventListener("click", () => toggleAllCheckboxes(false));
  expandAllBtn.addEventListener("click", () => toggleAllDetails(true));
  collapseAllBtn.addEventListener("click", () => toggleAllDetails(false));

  window.setScannedPath = (path) => {
    currentPathEl.textContent = path;
    currentPathEl.title = path;
    statusBar.textContent = `Status: Scanne Verzeichnis...`;
    fileTreeContainer.innerHTML =
      '<p class="placeholder">Lade Baumstruktur...</p>';
    postMessage("scanDirectory", path);
  };

  window.updateFileTree = (tree) => {
    fileTreeContainer.innerHTML = "";
    if (!tree || tree.length === 0) {
      fileTreeContainer.innerHTML =
        '<p class="placeholder">Keine anzeigbaren Dateien gefunden.</p>';
      return;
    }
    const treeRoot = document.createElement("div");
    treeRoot.className = "tree";
    treeRoot.appendChild(createTreeLevel(tree));
    fileTreeContainer.appendChild(treeRoot);
    statusBar.textContent = "Status: Bereit.";
  };

  window.showGeneratedContent = (content) => {
    if (editor) editor.setValue(content);
    statusBar.textContent = "Status: Preview generiert.";
    setButtonsDisabled(false);
    saveBtn.disabled = false;
    editor.updateOptions({ readOnly: false });
  };

  window.fileSaveStatus = (success, path) => {
    if (path === "cancelled") {
      statusBar.textContent = "Status: Speichern abgebrochen.";
    } else {
      statusBar.textContent = success
        ? `Datei erfolgreich gespeichert: ${path}`
        : `Fehler beim Speichern.`;
    }
    setButtonsDisabled(false);
    updateButtonStates(); // Re-evaluate generate button state
  };

  window.showError = (message) =>
    (statusBar.textContent = `Fehler: ${message}`);

  function setButtonsDisabled(disabled) {
    generateBtn.disabled = disabled;
    saveBtn.disabled = disabled;
  }

  function updateButtonStates() {
    const anySelected = getSelectedFiles().length > 0;
    generateBtn.disabled = !anySelected;
  }

  function getSelectedFiles() {
    return Array.from(
      fileTreeContainer.querySelectorAll(".tree-item-file input:checked")
    ).map((cb) => cb.dataset.path);
  }

  function toggleAllCheckboxes(checked) {
    fileTreeContainer
      .querySelectorAll(".tree-item-file input")
      .forEach((cb) => (cb.checked = checked));
    updateButtonStates();
  }

  function toggleAllDetails(open) {
    fileTreeContainer
      .querySelectorAll("details")
      .forEach((d) => (d.open = open));
  }

  function createTreeLevel(nodes) {
    const ul = document.createElement("ul");
    for (const node of nodes) {
      if (node.children && node.children.length > 0) {
        // It's a directory
        const details = document.createElement("details");
        details.open = true;
        const summary = document.createElement("summary");
        summary.innerHTML = `<span class="tree-item-label">📁 ${node.name}</span>`;
        details.appendChild(summary);
        details.appendChild(createTreeLevel(node.children));
        ul.appendChild(details);
      } else {
        // It's a file
        const li = document.createElement("li");
        li.className = "tree-item tree-item-file";
        const checkbox = document.createElement("input");
        checkbox.type = "checkbox";
        checkbox.dataset.path = node.path;
        checkbox.addEventListener("change", updateButtonStates);
        const icon = node.is_binary ? "🔧" : "📄";
        li.innerHTML = `<label class="tree-item-label" title="${node.path}">${checkbox.outerHTML} ${icon} ${node.name}</label>`;
        ul.appendChild(li);
      }
    }
    return ul;
  }
});
