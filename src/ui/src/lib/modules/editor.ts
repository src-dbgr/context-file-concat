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
let themeUnsub: (() => void) | null = null;

/** Map file path / hint -> a Monaco language id.
 *  NOTE: For Svelte we fall back to 'html' to get solid base highlighting
 *  (markup + <script>/<style> blocks) without extra deps. */
function resolveMonacoLanguage(hint: string | null | undefined, path: string) {
  const h = (hint ?? "").toLowerCase();
  const p = path.toLowerCase();

  // ---- Svelte quick win: use HTML tokenizer (no extra packages needed)
  if (h === "svelte" || p.endsWith(".svelte")) return "html";

  // Reasonable normalizations (helps if backend sends mime-ish values)
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

  // Fallback
  return "plaintext";
}

function applyMonacoThemeFromAppTheme() {
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

export function initEditor(onFinished?: () => void) {
  // Ensure Monaco theme matches the current app theme at creation time.
  applyMonacoThemeFromAppTheme();

  const editor = monaco.editor.create(elements.editorContainer, {
    value: "// Select a directory to begin.",
    language: "plaintext",
    // Theme is also set globally via setTheme; keep here for HMR safety.
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
  editorInstance.set(editor);

  // Live switch Monaco when app theme toggles.
  if (!themeUnsub) {
    themeUnsub = theme.subscribe((t) => {
      monaco.editor.setTheme(t === "light" ? "vs" : "vs-dark");
    });
  }

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
    // Normalize language (enables .svelte → html highlighting)
    const lang = resolveMonacoLanguage(language, path);
    monaco.editor.setModelLanguage(model, lang);

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
