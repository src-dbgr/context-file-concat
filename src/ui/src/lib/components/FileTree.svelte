<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import { appState } from "$lib/stores/app";
  import { post } from "$lib/services/backend";
  import type { AppState, TreeNode } from "$lib/types";
  import TreeItem from "./TreeItem.svelte";
  import {
    recordBulkExpanded,
    recordDirExpanded,
  } from "$lib/modules/treeExpansion";
  import Spinner from "$lib/components/Spinner.svelte";
  import LinearProgress from "$lib/components/LinearProgress.svelte";

  // Virtualization constants
  const ITEM_HEIGHT = 28;
  const OVERSCAN = 5;

  // Local virtualization state (Runes)
  let scrollEl = $state<HTMLDivElement | null>(null);
  let scrollTop = $state(0);
  let viewportHeight = $state(0);

  // Track last path & filters to reset scroll when needed
  let lastPath = $state<string | null>(null);
  let lastFilterKey = $state("");

  // Roving tabindex / keyboard a11y -------------------------------------------
  let focusedIndex = $state<number>(-1); // index in flatTree
  let typeaheadBuffer = $state<string>("");
  let typeaheadTimer: ReturnType<typeof setTimeout> | null = null;

  function setFocusByIndex(
    next: number,
    opts: { ensureView?: boolean } = { ensureView: true }
  ) {
    if (!flatTree.length) {
      focusedIndex = -1;
      return;
    }
    const clamped = Math.max(0, Math.min(flatTree.length - 1, next | 0));
    focusedIndex = clamped;
    if (opts.ensureView) ensureItemVisible(clamped);
    // Focus DOM target after the next frame (virtualization might re-render)
    requestAnimationFrame(() => {
      const node = document.querySelector<HTMLElement>(
        `.tree [data-index="${clamped}"]`
      );
      node?.focus();
    });
  }

  // When tree content appears for the first time, focus the first item
  $effect(() => {
    if (flatTree.length && focusedIndex === -1)
      setFocusByIndex(0, { ensureView: false });
  });

  // Preserve focus by path across re-renders if possible
  let lastFocusedPath = $state<string | null>(null);
  $effect(() => {
    const idx = focusedIndex;
    if (idx >= 0 && idx < flatTree.length) {
      lastFocusedPath = flatTree[idx].node.path;
    }
  });
  $effect(() => {
    // Try to restore focus to the same path after tree changes
    const path = lastFocusedPath;
    if (!path) return;
    const idx = flatTree.findIndex((it) => it.node.path === path);
    if (idx !== -1) {
      focusedIndex = idx;
    } else if (flatTree.length) {
      focusedIndex = Math.min(focusedIndex, flatTree.length - 1);
    } else {
      focusedIndex = -1;
    }
  });

  // Typeahead search within the tree
  function pushTypeahead(char: string) {
    const c = char.toLowerCase();
    if (!/^[a-z0-9._-]$/i.test(c)) return;
    if (typeaheadTimer) clearTimeout(typeaheadTimer);
    typeaheadBuffer = (typeaheadBuffer + c).slice(0, 40);
    typeaheadTimer = setTimeout(() => (typeaheadBuffer = ""), 800);

    const start = Math.max(0, focusedIndex) + 1;
    const matchFrom = (from: number, to: number) => {
      for (let i = from; i < to; i++) {
        const name = flatTree[i].node.name.toLowerCase();
        if (name.startsWith(typeaheadBuffer)) return i;
      }
      return -1;
    };
    let idx = matchFrom(start, flatTree.length);
    if (idx === -1) idx = matchFrom(0, start);
    if (idx !== -1) setFocusByIndex(idx);
  }

  type FlatItem = { node: TreeNode; level: number; index: number };

  function flattenTree(
    nodes: TreeNode[],
    level = 0,
    acc: FlatItem[] = []
  ): FlatItem[] {
    if (!nodes) return acc;
    for (const n of nodes) {
      acc.push({ node: n, level, index: acc.length });
      if (n.is_directory && n.is_expanded && n.children?.length) {
        flattenTree(n.children, level + 1, acc);
      }
    }
    return acc;
  }

  // Reactive flatten (Runes)
  const flatTree = $derived(flattenTree($appState.tree));

  // Total virtual height
  const totalHeight = $derived(flatTree.length * ITEM_HEIGHT);

  /** Clamp and synchronize the scroll position with the DOM. */
  function clampAndSyncScroll() {
    const el = scrollEl;
    if (!el) return;
    const maxTop = Math.max(0, el.scrollHeight - el.clientHeight);
    const clamped = Math.min(Math.max(0, scrollTop), maxTop);
    if (el.scrollTop !== clamped) el.scrollTop = clamped;
    if (scrollTop !== el.scrollTop) scrollTop = el.scrollTop;
  }

  // Visible slice (robust: disable virtualization when viewport >= content)
  const visibleSlice = $derived(
    (() => {
      const contentPx = totalHeight;
      const vp = Math.max(0, viewportHeight);

      if (vp >= contentPx) {
        return flatTree.map((it, i) => ({
          node: it.node,
          level: it.level,
          index: i,
        }));
      }

      const rawStart = Math.floor(scrollTop / ITEM_HEIGHT) - OVERSCAN;
      const rawEnd = Math.ceil((scrollTop + vp) / ITEM_HEIGHT) + OVERSCAN;

      const maxIndex = Math.max(0, flatTree.length - 1);
      const startIndex = Math.max(0, Math.min(maxIndex, rawStart | 0));
      const endIndex = Math.max(startIndex, Math.min(maxIndex, rawEnd | 0));

      return flatTree.slice(startIndex, endIndex + 1).map((it, i) => ({
        node: it.node,
        level: it.level,
        index: startIndex + i,
      }));
    })()
  );

  function onScroll() {
    if (!scrollEl) return;
    scrollTop = scrollEl.scrollTop;
  }

  // Helper: reset both internal & DOM scroll position
  function resetScroll() {
    scrollTop = 0;
    if (scrollEl) scrollEl.scrollTop = 0;
  }

  // Reset scroll when directory path changes (handles Clear → Select Directory)
  $effect(() => {
    const p = $appState.current_path;
    if (p !== lastPath) {
      lastPath = p;
      resetScroll();
      // place focus at the top when path changes
      focusedIndex = flatTree.length ? 0 : -1;
    }
  });

  // Reset scroll whenever filters change (filename/extension/content)
  const currentFilterKey = $derived(
    ($appState.search_query ?? "") +
      "|" +
      ($appState.extension_filter ?? "") +
      "|" +
      ($appState.content_search_query ?? "")
  );
  $effect(() => {
    const k = currentFilterKey;
    if (k !== lastFilterKey) {
      lastFilterKey = k;
      resetScroll();
      // Reset roving focus to the first result when filters change
      focusedIndex = flatTree.length ? 0 : -1;
    }
  });

  // If result list goes from 0 → >0 (e.g., filters cleared), scroll to start
  let lastHadItems = $state(false);
  $effect(() => {
    const hasItems = flatTree.length > 0;
    if (hasItems && !lastHadItems) resetScroll();
    lastHadItems = hasItems;
  });

  // Keep DOM scroller in sync if we programmatically change scrollTop
  $effect(() => {
    const el = scrollEl;
    const top = scrollTop;
    if (el && el.scrollTop !== top) el.scrollTop = top;
  });

  // Viewport measurement
  let ro: ResizeObserver | null = null;

  async function measureViewport() {
    await tick();
    viewportHeight = scrollEl?.clientHeight ?? 0;
    clampAndSyncScroll();
  }

  function measureViewportDeferred(attempts = 8) {
    const step = () => {
      measureViewport().then(() => {
        if ((scrollEl?.clientHeight ?? 0) <= 1 && attempts > 0) {
          requestAnimationFrame(() =>
            requestAnimationFrame(() =>
              setTimeout(() => measureViewportDeferred(attempts - 1), 0)
            )
          );
        }
      });
    };
    step();
  }

  // Re-measure on tree changes
  $effect(() => {
    void flatTree.length;
    requestAnimationFrame(() => measureViewportDeferred(2));
  });

  function onWindowResize() {
    measureViewportDeferred(2);
  }
  function onLayout() {
    measureViewportDeferred(1);
  }

  onMount(() => {
    measureViewportDeferred();
    ro = new ResizeObserver(() => measureViewportDeferred(2));
    if (scrollEl) ro.observe(scrollEl);
    window.addEventListener("resize", onWindowResize, { passive: true });
    window.addEventListener("cfc:layout", onLayout, { passive: true });
  });

  onDestroy(() => {
    if (ro && scrollEl) ro.unobserve(scrollEl);
    ro = null;
    window.removeEventListener("resize", onWindowResize);
    window.removeEventListener("cfc:layout", onLayout);
  });

  // --- Toolbar actions -------------------------------------------------------
  function onSelectAll() {
    post("selectAll");
  }
  function onDeselectAll() {
    post("deselectAll");
  }

  /** Collect all directory paths that need a toggle to reach the target expansion state. */
  function collectToggleTargets(
    nodes: TreeNode[],
    expand: boolean,
    acc: string[] = []
  ): string[] {
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
    recordBulkExpanded($appState.tree, expand);
    const targets = collectToggleTargets($appState.tree, expand);
    for (const path of targets) post("toggleExpansion", path);
  }

  function onExpandAll() {
    bulkSetExpansion(true);
  }
  function onCollapseAll() {
    bulkSetExpansion(false);
  }

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
  const visibleCounts = $derived(countFilesAndFolders($appState.tree));

  // Baseline = last unfiltered counts. Update only when there is no active filter.
  let baselineCounts = $state<TreeCounts>({ files: 0, folders: 0 });
  $effect(() => {
    if (!hasActiveFilters($appState)) {
      baselineCounts = visibleCounts;
    }
  });

  // Stats texts
  const statsTextMain = $derived(
    `Files: ${$appState.selected_files_count} selected of ${baselineCounts.files} • Folders: ${baselineCounts.folders}`
  );
  const statsTextSecondary = $derived(
    hasActiveFilters($appState)
      ? ` • Files Visible: ${visibleCounts.files}`
      : ""
  );

  // --- Helpers ---------------------------------------------------------------
  function hasActiveFilters(s: AppState): boolean {
    return !!(
      s.search_query?.trim() ||
      s.extension_filter?.trim() ||
      s.content_search_query?.trim()
    );
  }

  // Center only the body (not the header) when showing placeholder/empty/scan states
  const centerBody = $derived(
    !$appState.current_path ||
      $appState.tree.length === 0 ||
      ($appState.is_scanning && $appState.tree.length === 0)
  );

  // Ensure an item is visible in the viewport (adjust scrollTop if needed)
  function ensureItemVisible(index: number) {
    const el = scrollEl;
    if (!el) return;
    const itemTop = index * ITEM_HEIGHT;
    const itemBottom = itemTop + ITEM_HEIGHT;
    const viewTop = el.scrollTop;
    const viewBottom = el.scrollTop + el.clientHeight;

    if (itemTop < viewTop) {
      el.scrollTop = itemTop;
      scrollTop = el.scrollTop;
    } else if (itemBottom > viewBottom) {
      el.scrollTop = itemBottom - el.clientHeight;
      scrollTop = el.scrollTop;
    }
  }

  // Find parent index by walking backwards to the first node with a smaller level
  function findParentIndex(idx: number): number {
    if (idx <= 0 || idx >= flatTree.length) return -1;
    const level = flatTree[idx].level;
    for (let i = idx - 1; i >= 0; i--) {
      if (flatTree[i].level < level) return i;
    }
    return -1;
  }

  function itemsPerPage(): number {
    return Math.max(1, Math.floor(viewportHeight / ITEM_HEIGHT) || 1);
  }

  // Delegated focus tracking (when user clicks or tabs into an item)
  function onTreeFocusIn(e: FocusEvent) {
    const target = (e.target as HTMLElement).closest<HTMLElement>(
      ".tree-item-container"
    );
    if (!target) return;
    const idxAttr = target.getAttribute("data-index");
    if (idxAttr) {
      const idx = Number(idxAttr);
      if (Number.isFinite(idx)) focusedIndex = idx;
    }
  }

  // Central key handler for roving navigation & item actions
  function onTreeKeyDown(e: KeyboardEvent) {
    if (focusedIndex < 0 || focusedIndex >= flatTree.length) {
      // allow starting typeahead even if nothing focused
      if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
        pushTypeahead(e.key);
        e.preventDefault();
      }
      return;
    }

    const item = flatTree[focusedIndex];
    const node = item.node;

    // Navigation
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setFocusByIndex(focusedIndex + 1);
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setFocusByIndex(focusedIndex - 1);
      return;
    }
    if (e.key === "Home") {
      e.preventDefault();
      setFocusByIndex(0);
      return;
    }
    if (e.key === "End") {
      e.preventDefault();
      setFocusByIndex(flatTree.length - 1);
      return;
    }
    if (e.key === "PageDown") {
      e.preventDefault();
      setFocusByIndex(focusedIndex + itemsPerPage());
      return;
    }
    if (e.key === "PageUp") {
      e.preventDefault();
      setFocusByIndex(focusedIndex - itemsPerPage());
      return;
    }

    // Expand/Collapse per WAI-ARIA Tree pattern
    if (e.key === "ArrowRight") {
      e.preventDefault();
      if (node.is_directory) {
        if (!node.is_expanded) {
          // keep memory in sync so backend render won't override
          recordDirExpanded(node.path, true);
          post("toggleExpansion", node.path);
        } else {
          // move to first child
          const next = focusedIndex + 1;
          if (
            next < flatTree.length &&
            flatTree[next].level === item.level + 1
          ) {
            setFocusByIndex(next);
          }
        }
      } else {
        post("loadFilePreview", node.path);
      }
      return;
    }

    if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (node.is_directory && node.is_expanded) {
        // mirror collapse into memory
        recordDirExpanded(node.path, false);
        post("toggleExpansion", node.path);
      } else {
        const parent = findParentIndex(focusedIndex);
        if (parent !== -1) setFocusByIndex(parent);
      }
      return;
    }

    // Activate / select
    if (e.key === "Enter") {
      e.preventDefault();
      if (node.is_directory) {
        recordDirExpanded(node.path, !node.is_expanded);
        post("toggleExpansion", node.path);
      } else {
        post("loadFilePreview", node.path);
      }
      return;
    }

    if (e.key === " ") {
      e.preventDefault();
      if (node.is_directory) post("toggleDirectorySelection", node.path);
      else post("toggleSelection", node.path);
      return;
    }

    // Typeahead
    if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
      pushTypeahead(e.key);
      e.preventDefault();
    }
  }
</script>

<div class="file-tree-root">
  {#if $appState.current_path}
    <div class="panel-header files-header">
      <div class="files-title-section">
        <h3>Files</h3>
        <div class="button-group">
          <button
            onclick={onSelectAll}
            disabled={$appState.is_scanning || !$appState.tree.length}
            >Select All</button
          >
          <button
            onclick={onDeselectAll}
            disabled={$appState.is_scanning || !$appState.tree.length}
            >Deselect All</button
          >
          <button
            onclick={onExpandAll}
            disabled={$appState.is_scanning || !$appState.tree.length}
            >Expand All</button
          >
          <button
            onclick={onCollapseAll}
            disabled={$appState.is_scanning || !$appState.tree.length}
            >Collapse All</button
          >
        </div>
      </div>
      <div class="stats" aria-live="polite">
        {statsTextMain}<span class="stats-secondary">{statsTextSecondary}</span>
      </div>
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
            onclick={() => post("cancelScan")}
          >
            <svg
              class="icon"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
            >
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
            Cancel
          </button>
        </div>

        <LinearProgress
          idForFill="scan-progress-fill"
          ariaLabel="Scan progress"
          indeterminate
        />

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
        onclick={() => post("selectDirectory")}
        onkeydown={activateSelectDir}
      >
        Choose Directory
      </button>
    {:else if $appState.tree.length > 0}
      <div
        class="tree"
        bind:this={scrollEl}
        onscroll={onScroll}
        onfocusin={onTreeFocusIn}
        onkeydown={onTreeKeyDown}
        style="overflow:auto; height:100%; min-height:0; flex:1 1 auto;"
        role="tree"
        aria-label="Project files"
        aria-multiselectable="true"
        tabindex="0"
      >
        <div
          class="virtual-scroll-sizer"
          style="height:{totalHeight}px; position:relative;"
        >
          {#each visibleSlice as item (item.node.path)}
            <div
              class="virtual-scroll-item"
              style="position:absolute; left:0; right:0; top:{item.index *
                ITEM_HEIGHT}px; height:{ITEM_HEIGHT}px;"
            >
              <TreeItem
                node={item.node}
                level={item.level}
                index={item.index}
                focused={item.index === focusedIndex}
              />
            </div>
          {/each}
        </div>
      </div>
    {:else if $appState.search_query?.trim() || $appState.extension_filter?.trim() || $appState.content_search_query?.trim()}
      <div class="message-display" role="status" aria-live="polite">
        <div class="message-icon">
          <svg
            class="icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
          >
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
          <svg
            class="icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
          >
            <path
              d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"
            />
            <path d="M12 10v6" /><path d="M9 13h6" />
          </svg>
        </div>
        <p class="message-text">No files found in this directory.</p>
      </div>
    {/if}
  </div>
</div>

<style>
  .tree {
    min-height: 0;
    height: 100%;
    overflow: auto;
    flex: 1 1 auto;
  }
  .virtual-scroll-item {
    will-change: transform;
  }

  .file-tree-root {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }
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

  :global(.file-list-panel),
  :global(.file-tree-container),
  .tree {
    min-height: 0;
  }

  .scan-progress-container {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    max-width: 720px;
  }
  .scan-progress-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-6);
  }
  .scan-status {
    display: inline-flex;
    align-items: center;
    gap: var(--space-5);
    color: var(--color-text);
  }
</style>
