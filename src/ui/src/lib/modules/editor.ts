/**
 * Monaco Editor integration – lazily code-split & loaded on first use.
 * - No top-level Monaco imports → smaller initial bundle & faster TTI.
 * - Workers are prepared before editor.create so getWorker is synchronous.
 * - Public API kept, but preview/generate functions are async for first-load safety.
 */

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
import { theme } from "../stores/theme.js";

// Types only – no runtime cost
import type * as MonacoTypes from "monaco-editor";

type MonacoNS = typeof import("monaco-editor");

// Lazy-loaded module singleton
let monaco: MonacoNS | null = null;

// Worker constructors captured before editor.create()
type WorkerCtor = new () => Worker;
let workers: {
  editorWorker: WorkerCtor;
  jsonWorker: WorkerCtor;
  cssWorker: WorkerCtor;
  htmlWorker: WorkerCtor;
  tsWorker: WorkerCtor;
} | null = null;

let contentChangeListener: MonacoTypes.IDisposable | null = null;
let themeUnsub: (() => void) | null = null;

/** Load Monaco and wire workers exactly once. */
async function loadMonacoAndWorkers(): Promise<MonacoNS> {
  if (monaco) return monaco;

  // Parallelize all imports
  const [monacoMod, editorW, jsonW, cssW, htmlW, tsW] = await Promise.all([
    import("monaco-editor"),
    import("monaco-editor/esm/vs/editor/editor.worker?worker"),
    import("monaco-editor/esm/vs/language/json/json.worker?worker"),
    import("monaco-editor/esm/vs/language/css/css.worker?worker"),
    import("monaco-editor/esm/vs/language/html/html.worker?worker"),
    import("monaco-editor/esm/vs/language/typescript/ts.worker?worker"),
  ]);

  workers = {
    editorWorker: editorW.default as unknown as WorkerCtor,
    jsonWorker: jsonW.default as unknown as WorkerCtor,
    cssWorker: cssW.default as unknown as WorkerCtor,
    htmlWorker: htmlW.default as unknown as WorkerCtor,
    tsWorker: tsW.default as unknown as WorkerCtor,
  };

  // Install synchronous getWorker hook before editor.create()
  (self as unknown as Window).MonacoEnvironment = {
    getWorker(_moduleId: string, label: string): Worker {
      const w = workers!;
      if (label === "json") return new w.jsonWorker();
      if (label === "css" || label === "scss" || label === "less")
        return new w.cssWorker();
      if (label === "html" || label === "handlebars" || label === "razor")
        return new w.htmlWorker();
      if (label === "typescript" || label === "javascript")
        return new w.tsWorker();
      return new w.editorWorker();
    },
  };

  monaco = monacoMod as unknown as MonacoNS;
  return monaco;
}

function applyMonacoThemeFromAppTheme() {
  if (!monaco) return;
  const t = get(theme);
  monaco.editor.setTheme(t === "light" ? "vs" : "vs-dark");
}

export function layoutEditorSoon() {
  const editor = get(editorInstance);
  if (!editor) return;
  requestAnimationFrame(() => {
    try {
      editor.layout();
    } catch {
      /* noop */
    }
    setTimeout(() => {
      try {
        editor.layout();
      } catch {
        /* noop */
      }
    }, 0);
  });
}

/** Ensure there is an editor instance; lazily create if missing. */
async function ensureEditor(): Promise<void> {
  if (get(editorInstance)) return;

  await loadMonacoAndWorkers();
  applyMonacoThemeFromAppTheme();

  const ed = monaco!.editor.create(elements.editorContainer, {
    value: "// Select a directory to begin.",
    language: "plaintext",
    theme: get(theme) === "light" ? "vs" : "vs-dark",
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
  editorInstance.set(ed);

  // Live switch Monaco when app theme toggles.
  if (!themeUnsub) {
    themeUnsub = theme.subscribe((t) => {
      try {
        monaco!.editor.setTheme(t === "light" ? "vs" : "vs-dark");
      } catch {
        /* noop */
      }
    });
  }

  layoutEditorSoon();
}

/** Public init hook kept for compatibility (now just ensures the editor). */
export async function initEditor(onFinished?: () => void) {
  await ensureEditor();
  if (onFinished) onFinished();
}

/** Map a file path / hint to a Monaco language id. */
function resolveMonacoLanguage(hint: string | null | undefined, path: string) {
  const h = (hint ?? "").toLowerCase();
  const p = path.toLowerCase();

  if (h === "svelte" || p.endsWith(".svelte")) return "html";

  if (
    h === "js" ||
    h === "javascript" ||
    p.endsWith(".mjs") ||
    p.endsWith(".cjs")
  )
    return "javascript";
  if (
    h === "ts" ||
    h === "typescript" ||
    p.endsWith(".mts") ||
    p.endsWith(".cts")
  )
    return "typescript";
  if (h === "json" || p.endsWith(".json")) return "json";
  if (h === "css" || p.endsWith(".css")) return "css";
  if (h === "html" || p.endsWith(".html") || p.endsWith(".htm")) return "html";
  if (h === "md" || h === "markdown" || p.endsWith(".md")) return "markdown";
  if (h === "toml" || p.endsWith(".toml")) return "toml";
  if (h === "yaml" || h === "yml" || p.endsWith(".yaml") || p.endsWith(".yml"))
    return "yaml";
  if (h === "rust" || h === "rs" || p.endsWith(".rs")) return "rust";
  if (h === "shell" || h === "bash" || p.endsWith(".sh")) return "shell";
  if (h) return h;

  return "plaintext";
}

export async function showPreviewContent(
  content: string,
  language: string,
  searchTerm: string,
  path: string
): Promise<void> {
  await ensureEditor();
  const editor = get(editorInstance);
  if (!editor) return;

  if (contentChangeListener) {
    contentChangeListener.dispose();
    contentChangeListener = null;
  }

  previewedPath.set(path);

  editor.setValue(content);
  const model = editor.getModel();

  if (model && monaco) {
    // Normalize language (enables .svelte → html highlighting)
    const lang = resolveMonacoLanguage(language, path);
    monaco.editor.setModelLanguage(model, lang);

    let newDecorations: MonacoTypes.editor.IModelDeltaDecoration[] = [];
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
      newDecorations = matches.map((match) => ({
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

export async function showGeneratedContent(
  content: string,
  tokenCount: number
): Promise<void> {
  await ensureEditor();
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
  if (model && monaco) monaco.editor.setModelLanguage(model, "plaintext");
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
  if (model && monaco) monaco.editor.setModelLanguage(model, "plaintext");

  previewMode.set("idle");
  generatedTokenCount.set(null);

  layoutEditorSoon();
}
