/* global require, monaco */
import { elements } from '../dom.js';
import { state } from '../state.js';
import { post } from '../services/backend.js';
import { generateStatsString, splitPathForDisplay } from '../utils.js';
import { MONACO_VS_PATH } from '../config.js';

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
      const matches = model.findMatches(searchTerm, true, false, matchCase, null, true);
      newDecorations = matches.map((match) => ({
        range: match.range,
        options: {
          inlineClassName: "search-highlight",
          hoverMessage: { value: "Search match" },
        },
      }));
    }
    const currentDecorations = state.getDecorations();
    const newCurrentDecorations = editor.deltaDecorations(currentDecorations, newDecorations);
    state.setDecorations(newCurrentDecorations);
  }

  editor.updateOptions({ readOnly: true });
  editor.setPosition({ lineNumber: 1, column: 1 });
  editor.revealLine(1);

  const { pathPart, filename } = splitPathForDisplay(path, state.get().current_path);
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
        const previewTitle = document.querySelector(".preview-panel #preview-title");
        if (previewTitle) {
            previewTitle.innerHTML = `
              <div class="preview-path-container">
                <span class="preview-filename">
                  <svg class="icon icon-lightning" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="13,2 3,14 12,14 11,22 21,10 12,10 13,2"/></svg>
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
  if(!editor) return;

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
