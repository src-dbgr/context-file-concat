<script lang="ts">
  import { onMount } from 'svelte';
  import { appState, editorInstance, editorDecorations, previewedPath, getState } from '$lib/stores/app';
  import { get } from 'svelte/store';
  import type * as monaco from 'monaco-editor/esm/vs/editor/editor.api';
  import StatusBar from '$lib/components/StatusBar.svelte';

  // Keep Editor-Decorations in synch
  onMount(() => {
    const unsubscribeAppState = appState.subscribe(() => {
      updateEditorDecorations();
    });
    return () => unsubscribeAppState();
  });

  function updateEditorDecorations() {
    const editor = get(editorInstance);
    const state = getState();
    if (!editor || !get(previewedPath)) return;

    const model = editor.getModel();
    if (!model) return;

    const searchTerm = state.content_search_query;
    const matchCase = state.config.case_sensitive_search;
    let newDecorations: monaco.editor.IModelDeltaDecoration[] = [];
    if (searchTerm && searchTerm.trim() !== '') {
      const matches = model.findMatches(searchTerm, true, false, matchCase, null, true);
      newDecorations = matches.map((match: monaco.editor.FindMatch) => ({
        range: match.range,
        options: { inlineClassName: 'search-highlight' }
      }));
    }
    const currentDecorations = get(editorDecorations);
    const newDecorationIds = editor.deltaDecorations(currentDecorations, newDecorations);
    editorDecorations.set(newDecorationIds);
  }
</script>

<StatusBar />
