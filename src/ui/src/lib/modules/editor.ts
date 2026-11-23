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
import type { SaveFilePayload } from "../ipc/schema.js";
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

/**
 * The generated document the editor shows: the id the host keeps it under, the
 * text as the host sent it and the version of the editor's text at the moment
 * it was shown. `null` while the editor shows something else.
 *
 * The text is kept because the editor's own value is not it: Monaco gives a
 * text one kind of line end, so an output of files with CRLF and LF line ends
 * comes back from `getValue()` with all of them rewritten.
 */
let shownDocument: { id: number; text: string; versionId: number } | null =
  null;

/** The generated document, if the editor still shows it as it came. */
function unchangedDocument() {
  const model = get(editorInstance)?.getModel();
  return shownDocument &&
    model &&
    model.getAlternativeVersionId() === shownDocument.versionId
    ? shownDocument
    : null;
}

/** The themes `monacoBundle` registers: Monaco's own, with readable colors. */
const monacoTheme = (t: Theme) => (t === "light" ? "cfc-light" : "cfc-dark");

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
    // Tab leaves an editor that cannot be typed in (no keyboard trap); in the
    // editable output it indents, and Ctrl+M (Mac: Ctrl+Shift+M) switches.
    tabFocusMode: true,
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

  editor.updateOptions({ readOnly: true, tabFocusMode: true });
  editor.setPosition({ lineNumber: 1, column: 1 });
  editor.revealLine(1);

  shownDocument = null;
  previewMode.set("file");
  generatedTokenCount.set(null);

  layoutEditorSoon();
}

export async function showGeneratedContent(
  content: string,
  tokenCount: number,
  documentId: number,
): Promise<void> {
  const editor = await ensureEditor();

  previewedPath.set(null);
  editor.setValue(content);
  setSearchHighlight(editor, null);
  setLanguage(editor, "plaintext");
  editor.updateOptions({ readOnly: false, tabFocusMode: false });

  const model = editor.getModel();
  shownDocument = model
    ? {
        id: documentId,
        text: content,
        versionId: model.getAlternativeVersionId(),
      }
    : null;

  // State for UI
  previewMode.set("generated");
  generatedTokenCount.set(tokenCount);

  layoutEditorSoon();
}

/**
 * What a save of the editor's content has to send to the host, or `null` if
 * there is no editor yet.
 *
 * The host keeps the document it has generated, so an output that is saved as
 * it was shown is only named (`generated`); its text does not cross the IPC
 * boundary a second time. Only an output the user has changed is sent
 * (`edited`). Whether it was changed is told by the version of the editor's
 * text: Monaco's alternative version id changes with every edit and returns
 * to its earlier value when the edits are undone, so an output that was
 * edited and restored with undo counts as unchanged.
 */
export function outputToSave(): SaveFilePayload | null {
  const editor = get(editorInstance);
  if (!editor) return null;

  const unchanged = unchangedDocument();
  if (unchanged) return { kind: "generated", documentId: unchanged.id };
  return { kind: "edited", content: editor.getValue() };
}

/**
 * The text of the generated output exactly as the host produced it, or `null`
 * if the editor shows something else or the user has changed it (by the same
 * rule as {@link outputToSave}).
 *
 * This is what a copy of the whole output puts on the clipboard, so that it is
 * the text a save writes, line ends included. Once the output has been edited,
 * the editor's value is the one text there is, for copying and for saving.
 */
export function unchangedOutput(): string | null {
  return unchangedDocument()?.text ?? null;
}

export function clearPreview() {
  post("clearPreviewState");
  previewedPath.set(null);
  shownDocument = null;

  const editor = get(editorInstance);
  if (!editor) return;

  editor.setValue("// Preview cleared.");
  setSearchHighlight(editor, null);
  editor.updateOptions({ readOnly: true, tabFocusMode: true });
  setLanguage(editor, "plaintext");

  previewMode.set("idle");
  generatedTokenCount.set(null);

  layoutEditorSoon();
}
