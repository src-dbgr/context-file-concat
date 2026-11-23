<script lang="ts">
  import { onMount, onDestroy, tick, untrack } from "svelte";
  import { appState } from "$lib/stores/app";
  import { post } from "$lib/services/backend";
  import { activatesNatively } from "$lib/modules/nativeActivation";
  import type { AppState, TreeRow } from "$lib/types";
  import type { RowTarget } from "$lib/ipc/schema";
  import TreeItem from "./TreeItem.svelte";
  import {
    collapseDirectory,
    expandDirectory,
    setAllExpanded,
    toggleDirectory,
  } from "$lib/modules/treeExpansion";
  import {
    ITEM_HEIGHT,
    renderRange,
    windowToRequest,
    type RowWindow,
  } from "$lib/modules/treeWindow";
  import ScanProgress from "$lib/components/ScanProgress.svelte";
  import { t } from "$lib/i18n";

  // Local virtualization state (Runes)
  let scrollEl = $state<HTMLDivElement | null>(null);
  let scrollTop = $state(0);
  let viewportHeight = $state(0);

  // --- The rows ---------------------------------------------------------------
  // The host owns the list of all rows. The UI knows its length (the height of
  // the scroller) and holds the window of it that the host sent last; a row is
  // addressed by its index in the whole list.
  const totalRows = $derived($appState.rows_total);

  /** The row at `index` of the list, if the window at hand holds it. */
  function rowAt(index: number): TreeRow | undefined {
    const state = $appState;
    return state.rows[index - state.rows_start];
  }

  // Total virtual height
  const totalHeight = $derived(totalRows * ITEM_HEIGHT);

  // The rows to render: the visible ones (plus overscan) that have arrived.
  // A row the host has not sent yet leaves its place empty until it has.
  type RenderedRow = { row: TreeRow; index: number };
  const visibleSlice = $derived(
    (() => {
      const state = $appState;
      const range = renderRange(scrollTop, viewportHeight, state.rows_total);
      const from = Math.max(range.start, state.rows_start);
      const to = Math.min(range.end, state.rows_start + state.rows.length);
      const rendered: RenderedRow[] = [];
      for (let index = from; index < to; index++) {
        rendered.push({ row: state.rows[index - state.rows_start], index });
      }
      return rendered;
    })()
  );

  // Tell the host which rows the viewport needs. The answer is the next
  // snapshot; until then the request is not repeated.
  let asked: (RowWindow & { answeredBy: AppState }) | null = null;
  $effect(() => {
    const state = $appState;
    const request = windowToRequest(
      scrollTop,
      viewportHeight,
      { start: state.rows_start, count: state.rows.length },
      state.rows_total
    );
    if (!request) return;
    if (
      asked &&
      asked.answeredBy === state &&
      asked.start === request.start &&
      asked.count === request.count
    ) {
      return;
    }
    asked = { ...request, answeredBy: state };
    post("setViewport", request);
  });

  // --- Roving tabindex / keyboard a11y ----------------------------------------
  // The focused row is known by its index in the list and, once the row has
  // arrived, by its path. The index moves the focus; the path finds the row
  // again when the list changes.
  let focusedIndex = $state<number>(-1);
  let focusedPath: string | null = null;
  let typeaheadBuffer = $state<string>("");
  let typeaheadTimer: ReturnType<typeof setTimeout> | null = null;

  /** The row whose element is to get the DOM focus once it is rendered. */
  let domFocusWanted: { index: number; onlyIfUnfocused: boolean } | null =
    null;

  function applyDomFocus() {
    const wanted = domFocusWanted;
    if (!wanted) return;
    if (wanted.index !== focusedIndex) {
      domFocusWanted = null;
      return;
    }
    const activeEl = document.activeElement;
    const isInputFocused =
      activeEl?.tagName === "INPUT" && activeEl.closest(".sidebar");
    // An unrequested focus (the tree has just appeared) must not take the
    // focus away from wherever the user has put it in the meantime.
    const userHasFocusElsewhere =
      wanted.onlyIfUnfocused && activeEl && activeEl !== document.body;
    if (isInputFocused || userHasFocusElsewhere) {
      domFocusWanted = null;
      return;
    }
    const node = document.querySelector<HTMLElement>(
      `.tree [data-index="${wanted.index}"]`
    );
    if (node) {
      domFocusWanted = null;
      node.focus();
    } else if (rowAt(wanted.index)) {
      // The row is here but not on screen: the user has scrolled on.
      domFocusWanted = null;
    }
    // Otherwise the row is on its way from the host; see the effect below.
  }

  function setFocusByIndex(
    next: number,
    opts: { ensureView?: boolean; onlyIfUnfocused?: boolean } = {
      ensureView: true,
    }
  ) {
    if (!totalRows) {
      focusedIndex = -1;
      focusedPath = null;
      return;
    }
    const clamped = Math.max(0, Math.min(totalRows - 1, next | 0));
    focusedIndex = clamped;
    // Unknown until the row arrives, if it is outside the window.
    focusedPath = rowAt(clamped)?.path ?? null;
    if (opts.ensureView) ensureItemVisible(clamped);
    domFocusWanted = {
      index: clamped,
      onlyIfUnfocused: !!opts.onlyIfUnfocused,
    };
    // Focus DOM target after the next frame (virtualization might re-render)
    requestAnimationFrame(applyDomFocus);
  }

  // A row that was to be focused before the host had sent it gets the focus
  // when it has been rendered.
  $effect(() => {
    void visibleSlice;
    if (domFocusWanted) void tick().then(applyDomFocus);
  });

  // --- Asking the host for a row ------------------------------------------------
  // The UI cannot know where a row outside its window is. It asks
  // (`locateRow`); the answer is `located` of a later snapshot.
  let locateRequests = 0;
  let awaitedLocate: { request: number; purpose: "focus" | "restore" } | null =
    null;

  function locate(target: RowTarget, purpose: "focus" | "restore") {
    locateRequests += 1;
    awaitedLocate = { request: locateRequests, purpose };
    post("locateRow", { request: locateRequests, target });
  }

  function takeLocateAnswer(state: AppState) {
    const answer = state.located;
    if (!awaitedLocate || !answer || answer.request !== awaitedLocate.request) {
      return;
    }
    const { purpose } = awaitedLocate;
    awaitedLocate = null;
    if (purpose === "focus") {
      if (answer.row !== null) setFocusByIndex(answer.row);
    } else if (answer.row !== null) {
      // The focused row has moved; it keeps the focus where it is now.
      focusedIndex = answer.row;
    } else {
      // The focused row is gone (collapsed, filtered out).
      focusedPath = null;
    }
  }

  // --- Following the snapshots ---------------------------------------------------
  let lastPath: string | null = null;
  let lastFilterKey = "";
  let lastHadItems = false;
  let lastRowsRevision = -1;

  function filterKeyOf(s: AppState): string {
    return (
      (s.search_query ?? "") +
      "|" +
      (s.extension_filter ?? "") +
      "|" +
      (s.content_search_query ?? "")
    );
  }

  function followSnapshot(state: AppState) {
    const total = state.rows_total;
    // Nothing had the focus and there are rows now: the tree has appeared.
    const appeared = total > 0 && focusedIndex === -1;

    // Another directory (handles Clear → Select Directory) or other filters:
    // start at the top, with the focus on the first row.
    const filterKey = filterKeyOf(state);
    if (state.current_path !== lastPath || filterKey !== lastFilterKey) {
      lastPath = state.current_path;
      lastFilterKey = filterKey;
      resetScroll();
      focusedIndex = total ? 0 : -1;
      focusedPath = null;
      awaitedLocate = null;
      domFocusWanted = null;
    }

    // If the list goes from 0 → >0 rows (e.g., filters cleared), scroll to start
    if (total > 0 && !lastHadItems) resetScroll();
    lastHadItems = total > 0;

    const listChanged = state.rows_revision !== lastRowsRevision;
    lastRowsRevision = state.rows_revision;

    takeLocateAnswer(state);

    if (!total) {
      focusedIndex = -1;
      focusedPath = null;
      return;
    }

    if (appeared) {
      // Focus the first row, unless the user has the focus somewhere else.
      setFocusByIndex(0, { ensureView: false, onlyIfUnfocused: true });
      return;
    }
    if (focusedIndex >= total) focusedIndex = total - 1;

    const windowEnd = state.rows_start + state.rows.length;
    const focusedRowIsHere =
      focusedIndex >= state.rows_start && focusedIndex < windowEnd;

    if (focusedPath === null) {
      // The focused row is known by its index only; take its path when it
      // has arrived.
      if (focusedRowIsHere) focusedPath = state.rows[focusedIndex - state.rows_start].path;
      return;
    }

    // Keep the focus on the same item when the list has changed around it.
    const offset = state.rows.findIndex((row) => row.path === focusedPath);
    if (offset !== -1) {
      focusedIndex = state.rows_start + offset;
      return;
    }
    // It is not in the window. If the list has changed (or another row is at
    // its index), only the host knows where it is now.
    const alreadyAsked = awaitedLocate?.purpose === "restore" && !listChanged;
    if ((listChanged || focusedRowIsHere) && !alreadyAsked) {
      locate({ kind: "path", path: focusedPath }, "restore");
    }
  }

  $effect(() => {
    const state = $appState;
    untrack(() => followSnapshot(state));
  });

  // Typeahead search within the tree. It covers all rows, so the host looks.
  function pushTypeahead(char: string) {
    const c = char.toLowerCase();
    if (!/^[a-z0-9._-]$/i.test(c)) return;
    if (typeaheadTimer) clearTimeout(typeaheadTimer);
    typeaheadBuffer = (typeaheadBuffer + c).slice(0, 40);
    typeaheadTimer = setTimeout(() => (typeaheadBuffer = ""), 800);

    if (!totalRows) return;
    locate(
      {
        kind: "typeahead",
        prefix: typeaheadBuffer,
        from: Math.max(0, focusedIndex) + 1,
      },
      "focus"
    );
  }

  /** Clamp and synchronize the scroll position with the DOM. */
  function clampAndSyncScroll() {
    const el = scrollEl;
    if (!el) return;
    const maxTop = Math.max(0, el.scrollHeight - el.clientHeight);
    const clamped = Math.min(Math.max(0, scrollTop), maxTop);
    if (el.scrollTop !== clamped) el.scrollTop = clamped;
    if (scrollTop !== el.scrollTop) scrollTop = el.scrollTop;
  }

  function onScroll() {
    if (!scrollEl) return;
    scrollTop = scrollEl.scrollTop;
  }

  // Helper: reset both internal & DOM scroll position
  function resetScroll() {
    scrollTop = 0;
    if (scrollEl) scrollEl.scrollTop = 0;
  }

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

  // Re-measure when the list changes its length
  $effect(() => {
    void totalRows;
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
    if (typeaheadTimer) clearTimeout(typeaheadTimer);
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

  function onExpandAll() {
    setAllExpanded(true);
  }
  function onCollapseAll() {
    setAllExpanded(false);
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

  // Totals of the filtered tree. They come from the backend: the UI holds a
  // window of the rows and cannot count the tree.
  const visibleFiles = $derived($appState.tree_files_count);
  const visibleFolders = $derived($appState.tree_dirs_count);

  // Baseline = last unfiltered counts. Update only when there is no active filter.
  let baselineCounts = $state<TreeCounts>({ files: 0, folders: 0 });
  $effect(() => {
    if (!hasActiveFilters($appState)) {
      baselineCounts = { files: visibleFiles, folders: visibleFolders };
    }
  });

  // Stats texts
  const statsTextMain = $derived(
    `${$t("filetree.stats.files")}: ${$appState.selected_files_count} ${$t(
      "filetree.stats.selectedOf"
    )} ${baselineCounts.files} • ${$t("filetree.stats.folders")}: ${
      baselineCounts.folders
    }`
  );
  const statsTextSecondary = $derived(
    hasActiveFilters($appState)
      ? ` • ${$t("filetree.stats.visible")}: ${visibleFiles}`
      : ""
  );

  // --- Helpers ---------------------------------------------------------------
  // A filter text that is empty or whitespace-only is not set. Same rule as the
  // host (`core::is_blank`), which ignores such a text entirely.
  function hasActiveFilters(s: AppState): boolean {
    return !!(
      s.search_query?.trim() ||
      s.extension_filter?.trim() ||
      s.content_search_query?.trim()
    );
  }

  // Center only the body (not the header) when showing placeholder/empty/scan states
  const centerBody = $derived(!$appState.current_path || totalRows === 0);

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

  // Move the focus to the directory a row lies in: the nearest row above it
  // that is less deep. If the window does not reach that far, the host knows.
  function focusParentOf(index: number, row: TreeRow) {
    if (row.depth === 0) return;
    for (let i = index - 1; i >= 0; i--) {
      const above = rowAt(i);
      if (!above) break;
      if (above.depth < row.depth) {
        setFocusByIndex(i);
        return;
      }
    }
    locate({ kind: "parent_of", path: row.path }, "focus");
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
      if (Number.isFinite(idx)) {
        focusedIndex = idx;
        focusedPath = rowAt(idx)?.path ?? null;
        // The user has put the focus somewhere; a pending one gives way.
        if (domFocusWanted && domFocusWanted.index !== idx) {
          domFocusWanted = null;
        }
      }
    }
  }

  // Central key handler for roving navigation & item actions
  function onTreeKeyDown(e: KeyboardEvent) {
    // A key on the checkbox or the ignore button of a row that activates the
    // control by itself (its click handler runs once). Handling it here as
    // well would act on the row instead, or a second time. All other keys on
    // a control, and every key on the arrow and the name, are the tree's.
    if (activatesNatively(e.target, e.key)) return;

    if (focusedIndex < 0 || focusedIndex >= totalRows) {
      // allow starting typeahead even if nothing focused
      if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
        pushTypeahead(e.key);
        e.preventDefault();
      }
      return;
    }

    // Navigation: by index, so it also reaches rows the host has not sent.
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
      setFocusByIndex(totalRows - 1);
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

    // Typeahead
    const isActionKey =
      e.key === "ArrowRight" ||
      e.key === "ArrowLeft" ||
      e.key === "Enter" ||
      e.key === " ";
    if (!isActionKey) {
      if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
        pushTypeahead(e.key);
        e.preventDefault();
      }
      return;
    }

    // The remaining keys act on the focused row itself.
    e.preventDefault();
    const node = rowAt(focusedIndex);
    if (!node) {
      // It is outside the window (scrolled far away): bring it back first.
      setFocusByIndex(focusedIndex);
      return;
    }

    // Expand/Collapse per WAI-ARIA Tree pattern
    if (e.key === "ArrowRight") {
      if (node.is_directory) {
        if (!node.is_expanded) {
          expandDirectory(node);
        } else {
          // Move to the first child: the row right below an expanded
          // directory, one level deeper. If that row is outside the window,
          // the directory's own flag says whether it exists.
          const below = rowAt(focusedIndex + 1);
          const hasChildRow = below
            ? below.depth === node.depth + 1
            : node.has_children && focusedIndex + 1 < totalRows;
          if (hasChildRow) setFocusByIndex(focusedIndex + 1);
        }
      } else {
        post("loadFilePreview", node.path);
      }
      return;
    }

    if (e.key === "ArrowLeft") {
      if (node.is_directory && node.is_expanded) {
        collapseDirectory(node);
      } else {
        focusParentOf(focusedIndex, node);
      }
      return;
    }

    // Activate / select
    if (e.key === "Enter") {
      if (node.is_directory) {
        toggleDirectory(node);
      } else {
        post("loadFilePreview", node.path);
      }
      return;
    }

    if (node.is_directory) post("toggleDirectorySelection", node.path);
    else post("toggleSelection", node.path);
  }
</script>

<div class="file-tree-root">
  {#if $appState.current_path}
    <div class="panel-header files-header">
      <div class="files-title-section">
        <h3>{$t("filetree.title")}</h3>
        <div class="button-group">
          <button
            onclick={onSelectAll}
            disabled={$appState.is_scanning || !totalRows}
            >{$t("filetree.selectAll")}</button
          >
          <button
            onclick={onDeselectAll}
            disabled={$appState.is_scanning || !totalRows}
            >{$t("filetree.deselectAll")}</button
          >
          <button
            onclick={onExpandAll}
            disabled={$appState.is_scanning || !totalRows}
            >{$t("filetree.expandAll")}</button
          >
          <button
            onclick={onCollapseAll}
            disabled={$appState.is_scanning || !totalRows}
            >{$t("filetree.collapseAll")}</button
          >
        </div>
      </div>
      <div class="stats" aria-live="polite">
        {statsTextMain}<span class="stats-secondary">{statsTextSecondary}</span>
      </div>
    </div>
  {/if}

  <div class="file-tree-body" class:centered={centerBody}>
    {#if $appState.is_scanning && totalRows === 0}
      <ScanProgress />
    {:else if !$appState.current_path}
      <button
        type="button"
        class="placeholder"
        onclick={() => post("selectDirectory")}
        onkeydown={activateSelectDir}
      >
        {$t("filetree.placeholder.chooseDir")}
      </button>
    {:else if totalRows > 0}
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
          {#each visibleSlice as item (item.row.path)}
            <div
              class="virtual-scroll-item"
              style="position:absolute; left:0; right:0; top:{item.index *
                ITEM_HEIGHT}px; height:{ITEM_HEIGHT}px;"
            >
              <TreeItem
                node={item.row}
                index={item.index}
                focused={item.index === focusedIndex}
              />
            </div>
          {/each}
        </div>
      </div>
    {:else if hasActiveFilters($appState)}
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
        <p class="message-text">{$t("filetree.noMatch")}</p>
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
        <p class="message-text">{$t("filetree.emptyDir")}</p>
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
</style>
