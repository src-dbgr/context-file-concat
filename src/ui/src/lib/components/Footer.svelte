<script lang="ts">
  import { editorInstance } from "$lib/stores/app";
  import { post } from "$lib/services/backend";
  import { canGenerate, canSave, isGenerating } from "$lib/stores/uiStores";
  import LogoMark from "$lib/components/LogoMark.svelte";

  // Local UI state for the animated "Concat…" dots
  let generatingDots = $state("");

  // Drive the dots from $isGenerating via a Runes effect
  $effect(() => {
    let intervalId: ReturnType<typeof setInterval> | null = null;
    if ($isGenerating) {
      generatingDots = "";
      intervalId = setInterval(() => {
        generatingDots = generatingDots.length >= 3 ? "" : generatingDots + ".";
      }, 500);
    } else {
      generatingDots = "";
    }
    return () => { if (intervalId) clearInterval(intervalId); };
  });

  function onGenerateClick() {
    if ($isGenerating) post("cancelGeneration");
    else post("generatePreview");
  }

  function onSaveClick() {
    const editor = $editorInstance;
    if (editor) post("saveFile", editor.getValue());
  }
</script>

<div class="main-actions">
  <button
    id="generate-btn"
    class:is-generating={$isGenerating}
    class:button-cta={!$isGenerating}
    onclick={onGenerateClick}
    disabled={!$canGenerate && !$isGenerating}
    aria-busy={$isGenerating}
    aria-live="polite"
  >
    {#if $isGenerating}
      <span class="generating-content">
        <LogoMark effect="bolt" ariaHidden startWithStrike strikeMin={2} strikeMax={4} color="#FFD400" />
        <span class="generating-text">Concat{generatingDots}</span>
      </span>
      <span class="cancel-content">
        <svg class="icon" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        Cancel
      </span>
    {:else}
      <LogoMark size={16} ariaHidden effect="none" />
      Generate
    {/if}
  </button>

  <button
    id="save-btn"
    class="button-secondary"
    onclick={onSaveClick}
    disabled={!$canSave}
    aria-disabled={!$canSave}
  >
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
  #generate-btn.is-generating .generating-content { display:flex; align-items:center; gap:6px; color: var(--color-accent); font-weight:600; }
  #generate-btn.is-generating .cancel-content { display:none; }
  #generate-btn.is-generating:hover .generating-content { display:none; }
  #generate-btn.is-generating:hover .cancel-content { display:flex; align-items:center; justify-content:center; gap:6px; }
</style>
