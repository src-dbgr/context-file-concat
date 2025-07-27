/* global require, monaco */
import { elements } from "../dom.js";
import { state } from "../state.js";
import { post } from "../services/backend.js";
import { generateStatsString, splitPathForDisplay } from "../utils.js";
import { MONACO_VS_PATH } from "../config.js";

export function initEditor(onFinished) {
  require.config({ paths: { vs: MONACO_VS_PATH } });
  require(["vs/editor/editor.main"], () => {
    const editor = monaco.editor.create(elements.editorContainer, {
      value: "// Select a directory to begin.",
      language: "plaintext",
      theme: "vs-dark",
      readOnly: true,
      automaticLayout: true,
      wordWrap: "on",
    });
    state.setEditor(editor);
    if (onFinished) onFinished();
  });
}

export function showPreviewContent(content, language, searchTerm, path) {
  const editor = state.getEditor();
  if (!editor) return;

  state.setPreviewedPath(path);
  editor.setValue(content);
  const model = editor.getModel();

  if (model) {
    monaco.editor.setModelLanguage(model, language);
    let newDecorations = [];
    if (searchTerm && searchTerm.trim() !== "") {
      const matchCase = state.get().config.case_sensitive_search;
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
    const currentDecorations = state.getDecorations();
    const newCurrentDecorations = editor.deltaDecorations(
      currentDecorations,
      newDecorations
    );
    state.setDecorations(newCurrentDecorations);
  }

  editor.updateOptions({ readOnly: true });
  editor.setPosition({ lineNumber: 1, column: 1 });
  editor.revealLine(1);

  const { pathPart, filename } = splitPathForDisplay(
    path,
    state.get().current_path
  );
  const statsString = generateStatsString(content, "Read-only");

  const previewTitle = document.querySelector(".preview-panel #preview-title");
  if (previewTitle) {
    previewTitle.innerHTML = `
      <div class="preview-path-container" title="${path}">
        <span class="preview-path-part">${pathPart}</span><span class="preview-filename">${filename}</span>
      </div>
      <span class="preview-stats">${statsString}</span>`;
  }

  elements.copyBtn.style.display = "inline-block";
  elements.clearPreviewBtn.style.display = "inline-block";
}

export function showGeneratedContent(content) {
  const editor = state.getEditor();
  if (!editor) return;

  state.setPreviewedPath(null);
  editor.setValue(content);
  state.setDecorations(editor.deltaDecorations(state.getDecorations(), []));
  monaco.editor.setModelLanguage(editor.getModel(), "plaintext");
  editor.updateOptions({ readOnly: false });

  const updateStats = () => {
    const currentContent = editor.getValue();
    const statsString = generateStatsString(currentContent, "Editable");
    const previewTitle = document.querySelector(
      ".preview-panel #preview-title"
    );
    if (previewTitle) {
      previewTitle.innerHTML = `
              <div class="preview-path-container">
                <span class="preview-filename">
                  <svg class="icon icon-lightning" viewBox="0 0 24 24"><path d="M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z"></path></svg>
                  Generated Preview
                </span>
              </div>
              <span class="preview-stats">${statsString}</span>`;
    }
  };

  updateStats();
  const model = editor.getModel();
  if (model) {
    model.onDidChangeContent(updateStats);
  }

  elements.saveBtn.disabled = false;
  elements.copyBtn.style.display = "inline-block";
  elements.clearPreviewBtn.style.display = "inline-block";
}

export function clearPreview() {
  post("clearPreviewState");
  state.setPreviewedPath(null);

  const editor = state.getEditor();
  if (!editor) return;

  editor.setValue("// Preview cleared.");
  state.setDecorations(editor.deltaDecorations(state.getDecorations(), []));
  editor.updateOptions({ readOnly: true });
  monaco.editor.setModelLanguage(editor.getModel(), "plaintext");

  const previewTitle = document.querySelector(".preview-panel #preview-title");
  if (previewTitle) {
    previewTitle.innerHTML = `
      <div class="preview-path-container">
        <span class="preview-filename">Preview</span>
      </div>
      <span class="preview-stats">Select a file to preview</span>`;
  }

  elements.saveBtn.disabled = true;
  elements.clearPreviewBtn.style.display = "none";
  elements.copyBtn.style.display = "none";
}
