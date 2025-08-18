<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import { appState } from "$lib/stores/app";
  import { post } from "$lib/services/backend";
  import type { AppState, TreeNode } from "$lib/types";
  import TreeItem from "./TreeItem.svelte";
  import { recordBulkExpanded } from "$lib/modules/treeExpansion";
  import Spinner from "$lib/components/Spinner.svelte";
  import LinearProgress from "$lib/components/LinearProgress.svelte";

  // Virtualization constants
  const ITEM_HEIGHT = 28;
  const OVERSCAN = 5;

  // Local virtualization state
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

  // Reactive flatten
  let flatTree: FlatItem[] = [];
  $: flatTree = flattenTree($appState.tree);

  // Total virtual height
  $: totalHeight = flatTree.length * ITEM_HEIGHT;

  // Visible slice
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

  // Viewport measurement
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

  $: if (flatTree) {
    requestAnimationFrame(() => measureViewportDeferred(2));
  }

  onMount(() => {
    measureViewportDeferred();
    ro = new ResizeObserver(() => measureViewportDeferred(2));
    if (scrollEl) ro.observe(scrollEl);
    window.addEventListener("resize", onWindowResize, { passive: true });
  });

  function onWindowResize() {
    measureViewportDeferred(2);
  }

  onDestroy(() => {
    if (ro && scrollEl) ro.unobserve(scrollEl);
    ro = null;
    window.removeEventListener("resize", onWindowResize);
  });

  // --- Toolbar actions -------------------------------------------------------

  function onSelectAll() {
    post("selectAll");
  }

  function onDeselectAll() {
    post("deselectAll");
  }

  /** Collect all directory paths that need a toggle to reach the target expansion state. */
  function collectToggleTargets(nodes: TreeNode[], expand: boolean, acc: string[] = []): string[] {
    for (const n of nodes) {
      if (n.is_directory) {
        if (n.is_expanded !== expand) acc.push(n.path);
        if (n.children?.length) collectToggleTargets(n.children, expand, acc);
      }
    }
    return acc;
  }

  /** Bulk set expansion by sending toggleExpansion for each directory that differs. */
  function bulkSetExpansion(expand: boolean) {
    // Optimistically remember desired state to avoid flicker on next render.
    recordBulkExpanded($appState.tree, expand);
    const targets = collectToggleTargets($appState.tree, expand);
    for (const path of targets) post("toggleExpansion", path);
  }

  function onExpandAll()  { bulkSetExpansion(true); }
  function onCollapseAll(){ bulkSetExpansion(false); }

  // Keyboard activation for the placeholder
  function activateSelectDir(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      post("selectDirectory");
    }
  }

  // --- Stats ------------------------------------------------
  type TreeCounts = { files: number; folders: number };

  function countFilesAndFolders(nodes: TreeNode[]): TreeCounts {
    let files = 0;
    let folders = 0;
    const stack = [...(nodes || [])];
    while (stack.length) {
      const n = stack.pop()!;
      if (n.is_directory) {
        folders++;
        if (n.children?.length) stack.push(...n.children);
      } else {
        files++;
      }
    }
    return { files, folders };
  }

  // Always count visible (filtered) tree
  $: visibleCounts = countFilesAndFolders($appState.tree);

  // Baseline = last unfiltered counts. Update only when there is no active filter.
  let baselineCounts: TreeCounts = { files: 0, folders: 0 };
  $: if (!hasActiveFilters($appState)) {
    baselineCounts = visibleCounts;
  }

  // Stats texts
  $: statsTextMain =
    `Files: ${$appState.selected_files_count} selected of ${baselineCounts.files} • ` +
    `Folders: ${baselineCounts.folders}`;
  $: statsTextSecondary =
    hasActiveFilters($appState) ? ` • Files Visible: ${visibleCounts.files}` : "";

  // --- Helpers ---------------------------------------------------------------
  function hasActiveFilters(s: AppState): boolean {
    return !!(s.search_query?.trim() || s.extension_filter?.trim() || s.content_search_query?.trim());
  }

  // Center only the body (not the header) when showing placeholder/empty/scan states
  $: centerBody =
    !$appState.current_path ||
    $appState.tree.length === 0 ||
    ($appState.is_scanning && $appState.tree.length === 0);
</script>

<!-- Root wrapper keeps the header fixed at the top -->
<div class="file-tree-root">
  {#if $appState.current_path}
    <div class="panel-header files-header">
      <div class="files-title-section">
        <h3>Files</h3>
        <div class="button-group">
          <button on:click={onSelectAll} disabled={$appState.is_scanning || !$appState.tree.length}>Select All</button>
          <button on:click={onDeselectAll} disabled={$appState.is_scanning || !$appState.tree.length}>Deselect All</button>
          <button on:click={onExpandAll} disabled={$appState.is_scanning || !$appState.tree.length}>Expand All</button>
          <button on:click={onCollapseAll} disabled={$appState.is_scanning || !$appState.tree.length}>Collapse All</button>
        </div>
      </div>
      <div class="stats" aria-live="polite">{statsTextMain}<span class="stats-secondary">{statsTextSecondary}</span></div>
    </div>
  {/if}

  <div class="file-tree-body" class:centered={centerBody}>
    {#if $appState.is_scanning && $appState.tree.length === 0}
      <div class="scan-progress-container">
        <div class="scan-progress-header">
          <div class="scan-status" role="status" aria-live="polite">
            <Spinner size={16} ariaLabel="Scanning directory" />
            <span class="scan-text">Scanning directory...</span>
          </div>
          <button
            id="cancel-scan-btn"
            class="cancel-scan-btn"
            title="Cancel current scan"
            on:click={() => post("cancelScan")}
          >
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
            Cancel
          </button>
        </div>

        <!-- Keeps legacy #scan-progress-fill for IPC updates -->
        <LinearProgress idForFill="scan-progress-fill" ariaLabel="Scan progress" indeterminate />

        <div class="scan-details" aria-live="polite">
          <span id="scan-files-count">0 files processed</span>
          <span id="scan-current-path">Starting scan...</span>
          <span id="scan-skipped-count"></span>
        </div>
      </div>

    {:else if !$appState.current_path}
      <button
        type="button"
        class="placeholder"
        on:click={() => post("selectDirectory")}
        on:keydown={activateSelectDir}
      >
        Choose Directory
      </button>

    {:else if $appState.tree.length > 0}
      <div
        class="tree"
        bind:this={scrollEl}
        on:scroll={onScroll}
        style="overflow:auto; height:100%; min-height:0; flex:1 1 auto;"
        role="tree"
        aria-label="Project files"
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
        <div class="message-display" role="status" aria-live="polite">
          <div class="message-icon">
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="11" cy="11" r="8" />
              <path d="m21 21-4.35-4.35" />
              <line x1="11" y1="8" x2="11" y2="14" />
              <line x1="8" y1="11" x2="14" y2="11" />
            </svg>
          </div>
          <p class="message-text">No files found matching filters.</p>
        </div>
      {:else}
        <div class="message-display" role="status" aria-live="polite">
          <div class="message-icon">
            <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z" />
              <path d="M12 10v6" />
              <path d="M9 13h6" />
            </svg>
          </div>
          <p class="message-text">No files found in this directory.</p>
        </div>
      {/if}
    {/if}
  </div>
</div>

<style>
  /* Keep tree from forcing parent to grow */
  .tree { min-height: 0; height: 100%; overflow: auto; flex: 1 1 auto; }
  .virtual-scroll-item { will-change: transform; }

  /* Root wrapper keeps header fixed */
  .file-tree-root {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }

  /* Body can be centered independently of the header */
  .file-tree-body {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .file-tree-body.centered {
    justify-content: center;
    align-items: center;
  }

  /* Ensure outer slots don't collapse */
  :global(.file-list-panel),
  :global(.file-tree-container),
  .tree { min-height: 0; }

  /* Scan area spacing */
  .scan-progress-container { width: 100%; display: flex; flex-direction: column; gap: var(--space-6); max-width: 720px; }
  .scan-progress-header { display: flex; align-items: center; justify-content: space-between; gap: var(--space-6); }
  .scan-status { display: inline-flex; align-items: center; gap: var(--space-5); color: var(--color-text); }
</style>
