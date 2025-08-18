import { elements } from "../dom.js";
import {
  editorInstance,
  editorDecorations,
  previewedPath,
  getState,
} from "../stores/app.js";
import { post } from "../services/backend.js";
import { get } from "svelte/store";
import { previewMode, generatedTokenCount } from "../stores/preview.js";

// --- Monaco Setup (workers) ---
import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";

self.MonacoEnvironment = {
  getWorker(_, label) {
    if (label === "json") return new jsonWorker();
    if (label === "css" || label === "scss" || label === "less")
      return new cssWorker();
    if (label === "html" || label === "handlebars" || label === "razor")
      return new htmlWorker();
    if (label === "typescript" || label === "javascript") return new tsWorker();
    return new editorWorker();
  },
};

let contentChangeListener: monaco.IDisposable | null = null;

export function layoutEditorSoon() {
  const editor = get(editorInstance);
  if (!editor) return;
  requestAnimationFrame(() => {
    try {
      editor.layout();
    } catch {}
    setTimeout(() => {
      try {
        editor.layout();
      } catch {}
    }, 0);
  });
}

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
  layoutEditorSoon();
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

  previewMode.set("file");
  generatedTokenCount.set(null);

  layoutEditorSoon();
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
  if (model) monaco.editor.setModelLanguage(model, "plaintext");
  editor.updateOptions({ readOnly: false });

  // State for UI
  previewMode.set("generated");
  generatedTokenCount.set(tokenCount);

  layoutEditorSoon();
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
  if (model) monaco.editor.setModelLanguage(model, "plaintext");

  previewMode.set("idle");
  generatedTokenCount.set(null);

  layoutEditorSoon();
}
