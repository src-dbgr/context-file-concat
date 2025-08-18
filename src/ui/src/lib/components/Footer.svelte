<script lang="ts">
  import { appState, editorInstance } from "$lib/stores/app";
  import { previewMode } from "$lib/stores/preview";
  import { post } from "$lib/services/backend";
  import { get } from "svelte/store";

  let generatingIntervalId: ReturnType<typeof setInterval> | null = null;
  let generatingDots = "";

  $: isGenerating = $appState.is_generating;
  $: isScanning = $appState.is_scanning;
  $: hasSelection = ($appState.selected_files_count ?? 0) > 0;
  $: canGenerate = !isScanning && hasSelection;
  $: canSave = $previewMode === "generated";

  // Dot-Animation
  $: {
    if (isGenerating) {
      if (generatingIntervalId) clearInterval(generatingIntervalId);
      generatingIntervalId = setInterval(() => {
        generatingDots = generatingDots.length >= 3 ? "" : generatingDots + ".";
      }, 500);
    } else {
      if (generatingIntervalId) clearInterval(generatingIntervalId);
      generatingIntervalId = null;
      generatingDots = "";
    }
  }

  function onGenerateClick() {
    if (isGenerating) post("cancelGeneration");
    else post("generatePreview");
  }

  function onSaveClick() {
    const editor = get(editorInstance);
    if (editor) post("saveFile", editor.getValue());
  }
</script>

<div class="main-actions">
  <button
    id="generate-btn"
    class:is-generating={isGenerating}
    class:button-cta={!isGenerating}
    on:click={onGenerateClick}
    disabled={!canGenerate && !isGenerating}
  >
    {#if isGenerating}
      <span class="generating-content">
        <svg class="icon icon-lightning-light" viewBox="0 0 24 24">
          <path d="M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z"></path>
        </svg>
        <span class="generating-text">Concat{generatingDots}</span>
      </span>
      <span class="cancel-content">
        <svg class="icon" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        Cancel
      </span>
    {:else}
      <svg class="icon icon-lightning-light" viewBox="0 0 24 24">
        <path d="M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z"></path>
      </svg>
      Generate
    {/if}
  </button>

  <button id="save-btn" class="button-secondary" on:click={onSaveClick} disabled={!canSave}>
    <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"/>
      <polyline points="17,21 17,13 7,13 7,21"/>
      <polyline points="7,3 7,8 15,8"/>
    </svg>
    Save to File
  </button>
</div>

<style>
  .main-actions { display: flex; flex-direction: row; gap: 5px; }
  #generate-btn { min-width: 120px; transition: all 0.2s ease-in-out; }
  #generate-btn.is-generating .generating-content { display:flex; align-items:center; gap:6px; color: var(--red-accent); font-weight:600; }
  #generate-btn.is-generating .cancel-content { display:none; }
  #generate-btn.is-generating:hover .generating-content { display:none; }
  #generate-btn.is-generating:hover .cancel-content { display:flex; align-items:center; justify-content:center; gap:6px; }
  .icon-lightning-light { fill: #fff !important; stroke: none !important; }
</style>
