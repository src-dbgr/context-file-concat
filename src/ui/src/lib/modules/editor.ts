import { elements } from "../dom.js";
import {
  editorInstance,
  editorDecorations,
  previewedPath,
  getState,
} from "../stores/app.js";
import { post } from "../services/backend.js";
import { generateStatsString, splitPathForDisplay } from "../utils.js";
import { get } from "svelte/store";

// --- START: Direct Monaco Integration (Plugin-Free) ---
import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";

// Configure the Monaco environment to use the imported workers
// This must be done before the editor is created.
self.MonacoEnvironment = {
  getWorker(_, label) {
    if (label === "json") {
      return new jsonWorker();
    }
    if (label === "css" || label === "scss" || label === "less") {
      return new cssWorker();
    }
    if (label === "html" || label === "handlebars" || label === "razor") {
      return new htmlWorker();
    }
    if (label === "typescript" || label === "javascript") {
      return new tsWorker();
    }
    return new editorWorker();
  },
};
// --- END: Direct Monaco Integration ---

let contentChangeListener: monaco.IDisposable | null = null;

export function initEditor(onFinished?: () => void) {
  const editor = monaco.editor.create(elements.editorContainer, {
    value: "// Select a directory to begin.",
    language: "plaintext",
    theme: "vs-dark",
    readOnly: true,
    automaticLayout: true,
    wordWrap: "on",
    stickyScroll: { enabled: true },
    minimap: { enabled: true },
    renderLineHighlight: "line",
    padding: { top: 10 },
    bracketPairColorization: { enabled: true },
    formatOnPaste: true,
    smoothScrolling: true,
  });
  editorInstance.set(editor);
  if (onFinished) onFinished();
}

export function showPreviewContent(
  content: string,
  language: string,
  searchTerm: string,
  path: string
) {
  const editor = get(editorInstance);
  if (!editor) return;

  if (contentChangeListener) {
    contentChangeListener.dispose();
    contentChangeListener = null;
  }

  previewedPath.set(path);

  editor.setValue(content);
  const model = editor.getModel();

  if (model) {
    monaco.editor.setModelLanguage(model, language);
    let newDecorations: monaco.editor.IModelDeltaDecoration[] = [];
    if (searchTerm && searchTerm.trim() !== "") {
      const matchCase = getState().config.case_sensitive_search;
      const matches = model.findMatches(
        searchTerm,
        true,
        false,
        matchCase,
        null,
        true
      );
      newDecorations = matches.map((match: monaco.editor.FindMatch) => ({
        range: match.range,
        options: {
          inlineClassName: "search-highlight",
          hoverMessage: { value: "Search match" },
        },
      }));
    }
    const currentDecorations = get(editorDecorations);
    const newCurrentDecorations = editor.deltaDecorations(
      currentDecorations,
      newDecorations
    );
    editorDecorations.set(newCurrentDecorations);
  }

  editor.updateOptions({ readOnly: true });
  editor.setPosition({ lineNumber: 1, column: 1 });
  editor.revealLine(1);

  const { pathPart, filename } = splitPathForDisplay(
    path,
    getState().current_path
  );
  const statsString = generateStatsString(content, "Read-only", undefined);
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

export function showGeneratedContent(content: string, tokenCount: number) {
  const editor = get(editorInstance);
  if (!editor) return;

  if (contentChangeListener) {
    contentChangeListener.dispose();
    contentChangeListener = null;
  }

  previewedPath.set(null);
  editor.setValue(content);
  editorDecorations.set(editor.deltaDecorations(get(editorDecorations), []));
  const model = editor.getModel();
  if (model) {
    monaco.editor.setModelLanguage(model, "plaintext");
  }
  editor.updateOptions({ readOnly: false });

  const updateStats = () => {
    const currentContent = editor.getValue();
    const statsString = generateStatsString(
      currentContent,
      "Editable",
      tokenCount
    );
    const previewTitle = document.querySelector(
      ".preview-panel #preview-title"
    );
    if (previewTitle) {
      previewTitle.innerHTML = `
        <div class="preview-path-container">
          <span class="preview-filename">
            <svg class="icon icon-lightning" viewBox="0 0 24 24"><path d="M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z"></path></svg>
            <h3 class="generated-preview-title">Generated Preview</h3>
          </span>
        </div>
        <span class="preview-stats">${statsString}</span>`;
    }
  };

  updateStats();
  if (model) {
    contentChangeListener = model.onDidChangeContent(updateStats);
  }

  elements.saveBtn.disabled = false;
  elements.copyBtn.style.display = "inline-block";
  elements.clearPreviewBtn.style.display = "inline-block";
}

export function clearPreview() {
  post("clearPreviewState");
  previewedPath.set(null);

  const editor = get(editorInstance);
  if (!editor) return;

  editor.setValue("// Preview cleared.");
  editorDecorations.set(editor.deltaDecorations(get(editorDecorations), []));
  editor.updateOptions({ readOnly: true });
  const model = editor.getModel();
  if (model) {
    monaco.editor.setModelLanguage(model, "plaintext");
  }

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
