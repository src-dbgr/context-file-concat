<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { appState, patternFilter } from "$lib/stores/app";
  import { HostDraft } from "$lib/modules/hostDraft";
  import { post } from "$lib/services/backend";
  import { COMMON_IGNORE_PATTERNS } from "$lib/config";
  import { t } from "$lib/i18n";

  // Runes: derived flags/collections
  const searchEnabled = $derived(
    Boolean($appState.current_path && !$appState.is_scanning)
  );
  const availableCommon = $derived(
    COMMON_IGNORE_PATTERNS.filter(
      (p) => !$appState.config.ignore_patterns.includes(p)
    )
  );
  const allPatterns = $derived(
    Array.from(new Set($appState.config.ignore_patterns || []))
  );
  const activeSet = $derived(new Set($appState.active_ignore_patterns || []));

  // --- Filters -------------------------------------------------------------
  // The inputs show a local draft, not the app state: a snapshot that arrives
  // while the user is typing (scan progress, the answer to another command)
  // would otherwise reset the text. The host's values are adopted whenever
  // the draft allows it, see HostDraft.
  type Filters = {
    searchQuery: string;
    extensionFilter: string;
    contentSearchQuery: string;
  };
  const FILTER_DEBOUNCE_MS = 300;

  function hostFilters(): Filters {
    return {
      searchQuery: $appState.search_query,
      extensionFilter: $appState.extension_filter,
      contentSearchQuery: $appState.content_search_query,
    };
  }
  // Shows the host's filters unless the draft holds them back.
  function syncFiltersFromHost() {
    if (filterDraft.accepts($appState.last_processed_seq)) {
      filters = hostFilters();
    }
  }
  const filterDraft = new HostDraft();
  let filters = $state(untrack(hostFilters));
  let filterTimer: ReturnType<typeof setTimeout> | null = null;
  let filtersPath = untrack(() => $appState.current_path);

  function cancelPendingFilters() {
    if (filterTimer) clearTimeout(filterTimer);
    filterTimer = null;
  }

  // Runs for every snapshot of the host.
  $effect(() => {
    const path = $appState.current_path;
    hostFilters();
    untrack(() => {
      if (path !== filtersPath) {
        // Another directory (or none): filters belong to the directory they
        // were typed for. Text that has not been sent is not sent to the new
        // one, and a sent text is no longer waited for.
        filtersPath = path;
        cancelPendingFilters();
        filterDraft.reset();
      }
      syncFiltersFromHost();
    });
  });

  function pushFilters() {
    filterTimer = null;
    if (!$appState.current_path) {
      // Nothing to filter anymore: the host's (reset) filters apply.
      filterDraft.reset();
      filters = hostFilters();
      return;
    }
    filterDraft.sent(post("updateFilters", { ...filters }));
  }
  function onFiltersInput() {
    filterDraft.edit();
    cancelPendingFilters();
    filterTimer = setTimeout(pushFilters, FILTER_DEBOUNCE_MS);
  }
  onDestroy(cancelPendingFilters);

  // --- Config ---------------------------------------------------------------
  // Every edit is an intent: it names the one thing the user changed. The
  // host applies it to its configuration and answers with a snapshot.
  function setFlag(
    option: "case_sensitive_search" | "remove_empty_directories",
    e: Event
  ) {
    const value = (e.currentTarget as HTMLInputElement).checked;
    post("setConfigOption", { option, value });
  }

  let newPattern = $state("");

  function addPattern(pattern: string) {
    post("addIgnorePattern", { pattern });
  }
  function addNewPattern() {
    const p = newPattern.trim();
    if (!p) return;
    addPattern(p);
    newPattern = "";
  }
  function removePattern(pattern: string) {
    post("removeIgnorePattern", { pattern });
  }
  function deleteAllPatterns() {
    post("clearIgnorePatterns");
  }

  function handleRescan() {
    post("rescanDirectory");
  }

  function onPatternFilterInput(e: Event) {
    const v = (e.currentTarget as HTMLInputElement).value.toLowerCase();
    patternFilter.set(v);
  }

  const filteredPatterns = $derived(
    (() => {
      const pf = ($patternFilter || "").trim();
      const base = [...allPatterns].sort((a, b) => a.localeCompare(b));
      const filtered = pf
        ? base.filter((p) => p.toLowerCase().includes(pf))
        : base;
      return filtered.sort(
        (a, b) => Number(activeSet.has(b)) - Number(activeSet.has(a))
      );
    })()
  );
</script>

<!-- IMPORTANT: do not hide the resizer mount; the action will size/style it. -->
<div class="sidebar-resize-handle-left" aria-hidden="true"></div>

<div class="panel">
  <div class="panel-header">
    <h2>
      <svg
        class="icon"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
      >
        <circle cx="11" cy="11" r="8" />
        <path d="m21 21-4.35-4.35" />
      </svg>
      {$t("sidebar.title")}
    </h2>
  </div>

  <input
    type="text"
    id="search-query"
    placeholder={searchEnabled
      ? $t("sidebar.ph.searchFileNames")
      : $t("sidebar.ph.selectDirFirst")}
    bind:value={filters.searchQuery}
    disabled={!searchEnabled}
    oninput={onFiltersInput}
  />

  <input
    type="text"
    id="extension-filter"
    placeholder={searchEnabled
      ? $t("sidebar.ph.extensionFilter")
      : $t("sidebar.ph.selectDirFirst")}
    bind:value={filters.extensionFilter}
    disabled={!searchEnabled}
    oninput={onFiltersInput}
  />

  <input
    type="text"
    id="content-search-query"
    placeholder={searchEnabled
      ? $t("sidebar.ph.contentSearch")
      : $t("sidebar.ph.selectDirFirst")}
    bind:value={filters.contentSearchQuery}
    disabled={!searchEnabled}
    oninput={onFiltersInput}
  />

  <label>
    <input
      type="checkbox"
      id="case-sensitive"
      checked={$appState.config.case_sensitive_search}
      onchange={(e) => setFlag("case_sensitive_search", e)}
    />
    {$t("sidebar.caseSensitive")}
  </label>
</div>

<div class="panel ignore-patterns-panel">
  <div class="panel-header">
    <h2>
      <svg
        class="icon"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
      >
        <circle cx="12" cy="12" r="10" />
        <line x1="4.93" y1="4.93" x2="19.07" y2="19.07" />
      </svg>
      {$t("sidebar.ignoreTitle")}
    </h2>

    <button
      id="rescan-btn"
      class:needs-rescan={$appState.patterns_need_rescan}
      disabled={$appState.is_scanning || !$appState.current_path}
      onclick={handleRescan}
      title={$appState.patterns_need_rescan
        ? $t("sidebar.rescanRecommended")
        : $t("action.rescan")}
    >
      {#if $appState.is_scanning}
        <svg
          class="icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <circle cx="12" cy="12" r="10" />
          <polyline points="12,6 12,12 16,14" />
        </svg>
        {$t("sidebar.scanning")}
      {:else}
        <svg
          class="icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-linecap="round"
          stroke-linejoin="round"
          stroke-width="2"
        >
          <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"></path>
          <path d="M21 3v5h-5"></path>
          <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"></path>
          <path d="M8 16H3v5"></path>
        </svg>
        {$t("sidebar.rescan")}
      {/if}
    </button>
  </div>

  <div class="input-group">
    <input
      type="text"
      id="new-ignore-pattern"
      placeholder={$t("sidebar.ph.addPattern")}
      bind:value={newPattern}
      onkeydown={(e) => e.key === "Enter" && addNewPattern()}
    />
    <button id="add-pattern-btn" onclick={addNewPattern}>{$t("action.add")}</button
    >
  </div>

  <div class="ignore-options">
    <div class="ignore-actions">
      <button
        id="delete-all-patterns-btn"
        title={$t("sidebar.removeAll")}
        onclick={deleteAllPatterns}
      >
        {$t("sidebar.removeAll")}
      </button>
      <label>
        <input
          type="checkbox"
          id="remove-empty-dirs"
          checked={$appState.config.remove_empty_directories}
          onchange={(e) => setFlag("remove_empty_directories", e)}
        />
        {$t("sidebar.removeEmptyDirs")}
      </label>
    </div>
  </div>

  <div class="common-patterns-section">
    <p
      id="common-patterns-heading"
      class="common-patterns-label"
      style:display={availableCommon.length > 0 ? "block" : "none"}
    >
      {$t("sidebar.commonPatterns")}
    </p>
    <div
      id="common-patterns-container"
      class="common-patterns-container"
      role="group"
      aria-labelledby="common-patterns-heading"
    >
      {#each availableCommon as pattern (pattern)}
        <button
          class="common-pattern-chip"
          onclick={() => addPattern(pattern)}
          title={$t("sidebar.addCommonPattern", { pattern })}
        >
          {pattern}
        </button>
      {/each}
    </div>
  </div>

  <input
    type="text"
    id="filter-patterns"
    placeholder={$t("sidebar.ph.filterAssigned")}
    value={$patternFilter}
    oninput={onPatternFilterInput}
  />

  <div id="current-patterns-container" class="current-patterns" role="list">
    {#each filteredPatterns as p (p)}
      <div
        class="current-pattern-chip {activeSet.has(p) ? 'active-pattern' : ''}"
        title={activeSet.has(p)
          ? $t("sidebar.patternActive")
          : ""}
        role="listitem"
      >
        <span>{p}</span>
        <button
          class="remove-pattern-btn"
          onclick={() => removePattern(p)}
          aria-label={$t("aria.removePattern", { pattern: p })}
        >
          <svg
            class="icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            aria-hidden="true"
          >
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>
    {/each}
  </div>
</div>
