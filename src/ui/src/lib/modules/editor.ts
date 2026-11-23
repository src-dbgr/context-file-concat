/**
 * Monaco Editor integration – the imperative glue between the stores and the
 * editor instance.
 * - Monaco is loaded on first use (see `monacoLoader`); nothing here imports
 *   it statically.
 * - This module owns the editor instance and its search highlighting.
 */

import { elements } from "../dom.js";
import {
  appState,
  editorInstance,
  previewedPath,
  getState,
} from "../stores/app.js";
import { post } from "../services/backend.js";
import { get } from "svelte/store";
import { previewMode, generatedTokenCount } from "../stores/preview.js";
import { theme, type Theme } from "../stores/theme.js";
import { translate } from "../i18n/index.js";
import { loadMonaco, type MonacoNS } from "./monacoLoader.js";

// Types only – no runtime cost
import type * as MonacoTypes from "monaco-editor/esm/vs/editor/editor.api";

type Editor = MonacoTypes.editor.IStandaloneCodeEditor;

let monaco: MonacoNS | null = null;

/** The one creation of the editor; shared by every caller that needs it. */
let editorReady: Promise<Editor> | null = null;

const monacoTheme = (t: Theme) => (t === "light" ? "vs" : "vs-dark");

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

/* --------------------------- Search highlighting --------------------------- */

type SearchQuery = { term: string; matchCase: boolean };

/** What is highlighted in the previewed file; `null` when no file is shown. */
let highlightQuery: SearchQuery | null = null;
/** The decorations of the highlighting – the only bookkeeping of them. */
let highlights: MonacoTypes.editor.IEditorDecorationsCollection | null = null;

function renderSearchHighlights(editor: Editor): void {
  const model = editor.getModel();
  const query = highlightQuery;
  const decorations: MonacoTypes.editor.IModelDeltaDecoration[] =
    model && query && query.term.trim() !== ""
      ? model
          .findMatches(query.term, true, false, query.matchCase, null, false)
          .map((match) => ({
            range: match.range,
            options: {
              inlineClassName: "search-highlight",
              hoverMessage: { value: translate("preview.searchMatch") },
            },
          }))
      : [];

  highlights ??= editor.createDecorationsCollection();
  highlights.set(decorations);
}

function setSearchHighlight(editor: Editor, query: SearchQuery | null): void {
  highlightQuery = query;
  renderSearchHighlights(editor);
}

/**
 * Keeps the highlighting of a previewed file in line with the content search
 * of the app state. Only a change of the search is acted on, so the term the
 * host passes along with a preview stays until the search itself changes.
 */
function followContentSearch(editor: Editor): void {
  let seen: SearchQuery | null = null;
  appState.subscribe((s) => {
    const next: SearchQuery = {
      term: s.content_search_query,
      matchCase: s.config.case_sensitive_search,
    };
    const changed =
      seen !== null &&
      (seen.term !== next.term || seen.matchCase !== next.matchCase);
    seen = next;
    if (changed && highlightQuery) setSearchHighlight(editor, next);
  });
}

/* ------------------------------ Editor instance ---------------------------- */

async function createEditor(): Promise<Editor> {
  const ns = await loadMonaco();
  monaco = ns;

  const editor = ns.editor.create(elements.editorContainer, {
    value: "// Select a directory to begin.",
    language: "plaintext",
    theme: monacoTheme(get(theme)),
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
  theme.subscribe((t) => {
    try {
      ns.editor.setTheme(monacoTheme(t));
    } catch {
      /* noop */
    }
  });
  followContentSearch(editor);

  layoutEditorSoon();
  return editor;
}

/**
 * The editor, created on first use. The creation is memoized as a promise, so
 * callers that arrive while Monaco is still loading share one editor instead
 * of each creating their own in the same container. A failed creation is not
 * kept: the next caller tries again.
 */
function ensureEditor(): Promise<Editor> {
  editorReady ??= createEditor().catch((err: unknown) => {
    editorReady = null;
    throw err;
  });
  return editorReady;
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

function setLanguage(editor: Editor, language: string): void {
  const model = editor.getModel();
  if (model && monaco) monaco.editor.setModelLanguage(model, language);
}

export async function showPreviewContent(
  content: string,
  language: string,
  searchTerm: string,
  path: string,
): Promise<void> {
  const editor = await ensureEditor();

  previewedPath.set(path);

  editor.setValue(content);
  // Normalize language (enables .svelte → html highlighting)
  setLanguage(editor, resolveMonacoLanguage(language, path));
  setSearchHighlight(editor, {
    term: searchTerm,
    matchCase: getState().config.case_sensitive_search,
  });

  editor.updateOptions({ readOnly: true });
  editor.setPosition({ lineNumber: 1, column: 1 });
  editor.revealLine(1);

  previewMode.set("file");
  generatedTokenCount.set(null);

  layoutEditorSoon();
}

export async function showGeneratedContent(
  content: string,
  tokenCount: number,
): Promise<void> {
  const editor = await ensureEditor();

  previewedPath.set(null);
  editor.setValue(content);
  setSearchHighlight(editor, null);
  setLanguage(editor, "plaintext");
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
  setSearchHighlight(editor, null);
  editor.updateOptions({ readOnly: true });
  setLanguage(editor, "plaintext");

  previewMode.set("idle");
  generatedTokenCount.set(null);

  layoutEditorSoon();
}
