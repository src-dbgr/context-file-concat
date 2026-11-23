/**
 * The editor bundle – everything of Monaco the app uses, set up for use.
 * Never import this module statically: `monacoLoader` loads it on first use,
 * which keeps Monaco out of the initial bundle.
 *
 * What is included:
 * - the editor core (`edcore.main`), without the language services of the
 *   full `monaco-editor` entry;
 * - the Monarch grammars (`basic-languages`), each a chunk of its own that
 *   Monaco fetches when a model of that language is shown;
 * - the JSON contribution reduced to its tokenizer – JSON has no Monarch
 *   grammar, its tokenizer runs on the main thread.
 *
 * Highlighting therefore needs no language worker; the only worker shipped is
 * the base editor worker, which `MonacoEnvironment.getWorker` returns for
 * every label.
 */

import * as monaco from "monaco-editor/esm/vs/editor/edcore.main";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import "monaco-editor/esm/vs/language/json/monaco.contribution";
import "monaco-editor/esm/vs/basic-languages/monaco.contribution";

// Everything but the tokenizer of the JSON mode talks to the JSON worker,
// which is not shipped. (The contribution adds `languages.json`.)
const { jsonDefaults } = monaco.languages.json;
jsonDefaults.setDiagnosticsOptions({ validate: false });
jsonDefaults.setModeConfiguration({
  tokens: true,
  documentFormattingEdits: false,
  documentRangeFormattingEdits: false,
  completionItems: false,
  hovers: false,
  documentSymbols: false,
  colors: false,
  foldingRanges: false,
  diagnostics: false,
  selectionRanges: false,
});

// In place before the first editor is created.
self.MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};

export { monaco };
