<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { appState } from '$lib/stores/app';
  import { post } from '$lib/services/backend';
  import type { AppState, TreeNode } from '$lib/types';
  import TreeItem from './TreeItem.svelte';

  // Virtualization constants
  const ITEM_HEIGHT = 28;
  const OVERSCAN = 5;

  // Virtual scroll state
  let scrollEl: HTMLDivElement | null = null;
  let scrollTop = 0;
  let viewportHeight = 0;

  type FlatItem = { node: TreeNode; level: number; index: number };

  function flattenTree(nodes: TreeNode[], level = 0, acc: FlatItem[] = []): FlatItem[] {
    if (!nodes) return acc;
    for (const n of nodes) {
      acc.push({ node: n, level, index: acc.length });
      if (n.is_directory && n.is_expanded && n.children?.length) {
        flattenTree(n.children, level + 1, acc);
      }
    }
    return acc;
  }

  // Reactive flattened list
  let flatTree: FlatItem[] = [];
  $: flatTree = flattenTree($appState.tree);

  // Total height for sizer
  $: totalHeight = flatTree.length * ITEM_HEIGHT;

  // Visible window
  let visibleSlice: FlatItem[] = [];
  $: {
    const startRaw = Math.floor(scrollTop / ITEM_HEIGHT) - OVERSCAN;
    const endRaw = Math.ceil((scrollTop + viewportHeight) / ITEM_HEIGHT) + OVERSCAN;
    const startIndex = Math.max(0, startRaw | 0);
    const endIndex = Math.min(flatTree.length - 1, endRaw | 0);
    visibleSlice = startIndex <= endIndex ? flatTree.slice(startIndex, endIndex + 1) : [];
    for (let i = 0; i < visibleSlice.length; i++) visibleSlice[i].index = startIndex + i;
  }

  function onScroll() {
    if (!scrollEl) return;
    scrollTop = scrollEl.scrollTop;
  }

  // Robust viewport measurement (handles late flex/layout)
  let ro: ResizeObserver | null = null;

  async function measureViewport() {
    await tick();
    viewportHeight = scrollEl?.clientHeight ?? 0;
  }

  function measureViewportDeferred(attempts = 8) {
    const step = () => {
      measureViewport().then(() => {
        if ((scrollEl?.clientHeight ?? 0) <= 1 && attempts > 0) {
          requestAnimationFrame(() =>
            requestAnimationFrame(() => setTimeout(() => measureViewportDeferred(attempts - 1), 0))
          );
        }
      });
    };
    step();
  }

  // Re-measure after tree changes (expand/collapse)
  $: if (flatTree) {
    requestAnimationFrame(() => measureViewportDeferred(2));
  }

  onMount(() => {
    measureViewportDeferred();

    ro = new ResizeObserver(() => measureViewportDeferred(2));
    if (scrollEl) ro.observe(scrollEl);

    window.addEventListener('resize', onWindowResize, { passive: true });
  });

  function onWindowResize() {
    measureViewportDeferred(2);
  }

  onDestroy(() => {
    if (ro && scrollEl) ro.unobserve(scrollEl);
    ro = null;
    window.removeEventListener('resize', onWindowResize);
  });

  function hasActiveFilters(s: AppState): boolean {
    return !!(s.search_query?.trim() || s.extension_filter?.trim() || s.content_search_query?.trim());
  }

  // Keyboard activation for “Choose Directory”
  function activateSelectDir(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      post('selectDirectory');
    }
  }
</script>

{#if $appState.is_scanning && $appState.tree.length === 0}
  <!-- Scan progress -->
  <div class="scan-progress-container">
    <div class="scan-progress-header">
      <div class="scan-status">
        <div class="scan-spinner"></div>
        <span class="scan-text">Scanning directory...</span>
      </div>
      <button id="cancel-scan-btn" class="cancel-scan-btn" title="Cancel current scan" on:click={() => post('cancelScan')}>
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        Cancel
      </button>
    </div>
    <div class="scan-progress-bar">
      <div class="scan-progress-fill" id="scan-progress-fill"></div>
    </div>
    <div class="scan-details">
      <span id="scan-files-count">0 files processed</span>
      <span id="scan-current-path">Starting scan...</span>
      <span id="scan-skipped-count"></span>
    </div>
  </div>
{:else if !$appState.current_path}
  <!-- Centered empty state (robust without :has()) -->
  <div class="empty-center" aria-hidden="false">
    <button
      type="button"
      class="placeholder"
      on:click={() => post('selectDirectory')}
      on:keydown={activateSelectDir}
      aria-label="Choose a directory"
    >
      Choose Directory
    </button>
  </div>
{:else if $appState.tree.length > 0}
  <!-- Scroll container -->
  <div
    class="tree"
    bind:this={scrollEl}
    on:scroll={onScroll}
    style="overflow:auto; height:100%; min-height:0; flex:1 1 auto;"
  >
    <div class="virtual-scroll-sizer" style="height:{totalHeight}px; position:relative;">
      {#each visibleSlice as item (item.node.path)}
        <div
          class="virtual-scroll-item"
          style="position:absolute; left:0; right:0; top:{item.index * ITEM_HEIGHT}px; height:{ITEM_HEIGHT}px;"
        >
          <TreeItem node={item.node} level={item.level} />
        </div>
      {/each}
    </div>
  </div>
{:else}
  {#if hasActiveFilters($appState)}
    <div class="message-display">
      <div class="message-icon">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/><line x1="11" y1="8" x2="11" y2="14"/><line x1="8" y1="11" x2="14" y2="11"/>
        </svg>
      </div>
      <p class="message-text">No files found matching filters.</p>
    </div>
  {:else}
    <div class="message-display">
      <div class="message-icon">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/><path d="M12 10v6"/><path d="M9 13h6"/>
        </svg>
      </div>
      <p class="message-text">No files found in this directory.</p>
    </div>
  {/if}
{/if}

<style>
  /* Keep container flexible */
  :global(.file-list-panel),
  :global(.file-tree-container),
  .tree { min-height: 0; }

  /* Dedicated centering wrapper for the empty state */
  .empty-center {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1 1 auto;
    width: 100%;
    height: 100%;
  }

  .tree {
    height: 100%;
    overflow: auto;
    flex: 1 1 auto;
  }

  .virtual-scroll-item { will-change: transform; }
</style>
