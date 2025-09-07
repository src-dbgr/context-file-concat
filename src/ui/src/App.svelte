<script lang="ts">
  import {
    appState,
    editorInstance,
    editorDecorations,
    previewedPath,
  } from "$lib/stores/app";
  import type * as monaco from "monaco-editor/esm/vs/editor/editor.api";
  import StatusBar from "$lib/components/StatusBar.svelte";
  import ToastHost from "$lib/components/ToastHost.svelte";

  // Non reactive store – avoids Self-trigger
  let lastDecorationIds: string[] = [];
  let lastModel: monaco.editor.ITextModel | null = null;

  function clearDecorations(
    editor: monaco.editor.IStandaloneCodeEditor | null
  ) {
    if (!editor) return;
    if (lastDecorationIds.length) {
      try {
        // removes existing deco in current editor
        editor.deltaDecorations(lastDecorationIds, []);
      } catch {
        // in case IDs do not match to the model (quick change) - ignore
      }
      lastDecorationIds = [];
      // only publish - no read - in order to avoid effekt re-trigger
      editorDecorations.set([]);
    }
  }

  // keep monaco highlights in sync - runes-only, no self-trigger
  $effect(() => {
    const editor = $editorInstance;
    const hasPath = !!$previewedPath;
    const model = editor?.getModel();

    const searchTerm = $appState.content_search_query;
    const matchCase = $appState.config.case_sensitive_search;

    // no editor/model/preview -> clean up everything
    if (!editor || !model || !hasPath) {
      clearDecorations(editor ?? null);
      lastModel = null;
      return;
    }

    // new model (different file) -> remove old decos securely
    if (model !== lastModel) {
      clearDecorations(editor);
      lastModel = model;
    }

    // calcuate new decos
    let newDecorations: monaco.editor.IModelDeltaDecoration[] = [];
    if (searchTerm && searchTerm.trim() !== "") {
      const matches = model.findMatches(
        searchTerm,
        true,
        false,
        matchCase,
        null,
        true
      );
      newDecorations = matches.map((m) => ({
        range: m.range,
        options: { inlineClassName: "search-highlight" },
      }));
    }

    // apply deltas - important: no read of $editorDecorations here
    const newIds = editor.deltaDecorations(lastDecorationIds, newDecorations);
    lastDecorationIds = newIds;

    // make available for different observer (without feeding the effect)
    editorDecorations.set(newIds);
  });
</script>

<StatusBar />
<ToastHost />
