<script lang="ts">
  import { appState, editorInstance, previewedPath } from '$lib/stores/app';
  import { previewMode, generatedTokenCount } from '$lib/stores/preview';
  import { splitPathForDisplay, generateStatsString } from '$lib/utils';
  import { handleCopy } from '$lib/modules/clipboard';
  import { clearPreview } from '$lib/modules/editor';
  import { onMount } from 'svelte';
  import LinearProgress from '$lib/components/LinearProgress.svelte';
  import Skeleton from '$lib/components/Skeleton.svelte';

  // Live stats recomputation when editor content changes
  let contentVersion = 0;
  let modelDispose: { dispose: () => void } | null = null;

  onMount(() => {
    // Re-subscribe whenever the editor instance changes
    const unsub = editorInstance.subscribe((editor) => {
      if (modelDispose) {
        modelDispose.dispose();
        modelDispose = null;
      }
      if (editor && editor.getModel()) {
        modelDispose = editor.getModel()!.onDidChangeContent(() => {
          // trigger reactive recomputation
          contentVersion += 1;
        });
      }
    });
    return () => {
      unsub();
      if (modelDispose) {
        modelDispose.dispose();
        modelDispose = null;
      }
    };
  });

  function onCopyClick() {
    handleCopy({
      isEditorFocused: true,
      activeEl: document.activeElement as HTMLElement,
      isInNormalInputField: false
    });
  }

  // --- Reactive, component-scoped title + stats ---

  let pathPart = '';
  let filename = '';
  let statsText = 'Select a file to preview';

  $: {
    // Ensure this block depends on editor content changes
    void contentVersion;

    const mode = $previewMode;
    const editor = $editorInstance;

    if (!editor || mode === 'idle') {
      pathPart = '';
      filename = 'Preview';
      statsText = 'Select a file to preview';
    } else if (mode === 'file') {
      const content = editor.getValue();
      const fullPath = $previewedPath ?? '';
      const res = splitPathForDisplay(fullPath, $appState.current_path);
      pathPart = res.pathPart;
      filename = res.filename;
      statsText = generateStatsString(content, 'Read-only', undefined);
    } else {
      // generated
      const content = editor.getValue();
      pathPart = '';
      filename = '';
      statsText = generateStatsString(content, 'Editable', $generatedTokenCount ?? undefined);
    }
  }
</script>

<!--
  This component renders INTO the existing #preview-panel element.
  We add subtle, accessible feedback while generation is in progress.
-->
<div class="preview-fragment" style="display: contents">
  <div class="panel-header">
    <h3 id="preview-title">
      {#if $previewMode === 'generated'}
        <div class="preview-path-container">
          <span class="preview-filename">
            <svg class="icon icon-lightning" viewBox="0 0 24 24">
              <path d="M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z"></path>
            </svg>
            <span class="generated-preview-title">Generated Preview</span>
          </span>
        </div>
        <span class="preview-stats">{statsText}</span>
      {:else if $previewMode === 'file'}
        <div class="preview-path-container" title={$previewedPath ?? ''}>
          <span class="preview-path-part">{pathPart}</span
          ><span class="preview-filename">{filename}</span>
        </div>
        <span class="preview-stats">{statsText}</span>
      {:else}
        <div class="preview-path-container">
          <span class="preview-filename">Preview</span>
        </div>
        <span class="preview-stats">Select a file to preview</span>
      {/if}
    </h3>

    <div class="preview-header-buttons">
      <button
        id="copy-btn"
        style:display={$previewMode !== 'idle' ? 'inline-block' : 'none'}
        on:click={onCopyClick}
        disabled={$previewMode === 'idle'}
      >
        <svg
          class="icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
        </svg>
        Copy
      </button>

      <button
        id="clear-preview-btn"
        style:display={$previewMode !== 'idle' ? 'inline-block' : 'none'}
        on:click={clearPreview}
        disabled={$previewMode === 'idle'}
      >
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
        Clear
      </button>
    </div>
  </div>

  <!-- Generation progress hint (indeterminate) -->
  {#if $appState.is_generating}
    <div class="gen-progress" aria-hidden="false">
      <LinearProgress ariaLabel="Generating preview" indeterminate />
    </div>
  {/if}

  <!-- Monaco mounts here; flex rules are on #preview-panel in CSS -->
  <div id="editor-container">
    {#if $appState.is_generating && $previewMode === 'idle'}
      <!-- When there's nothing yet to show, hint content area -->
      <div class="editor-skeleton">
        <Skeleton width="70%" height="14px" />
        <Skeleton width="95%" height="12px" />
        <Skeleton width="92%" height="12px" />
        <Skeleton width="88%" height="12px" />
      </div>
    {/if}
  </div>
</div>

<style>
  .gen-progress { margin: var(--space-4) 0 var(--space-2); }
  .editor-skeleton { display: grid; gap: var(--space-4); padding: var(--space-6); }
</style>
