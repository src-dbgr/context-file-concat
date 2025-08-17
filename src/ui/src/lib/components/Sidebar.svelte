<script lang="ts">
	import { appState, patternFilter } from '$lib/stores/app';
	import { post } from '$lib/services/backend';
	import { COMMON_IGNORE_PATTERNS } from '$lib/config';

	// --- Derived UI state ---
	$: searchEnabled = Boolean($appState.current_path && !$appState.is_scanning);

	// --- Filter inputs: debounce push to backend ---
	let filterTimer: ReturnType<typeof setTimeout> | null = null;
	function pushFilters() {
		if (!$appState.current_path) return;
		post('updateFilters', {
			searchQuery: $appState.search_query,
			extensionFilter: $appState.extension_filter,
			contentSearchQuery: $appState.content_search_query
		});
	}
	function onFiltersInput() {
		if (filterTimer) clearTimeout(filterTimer);
		filterTimer = setTimeout(pushFilters, 300);
	}

	// --- Config toggles ---
	function onCaseSensitiveChange() {
		post('updateConfig', $appState.config);
		// Parität mit Legacy: Filter nach Toggle erneut pushen
		pushFilters();
	}
	function onRemoveEmptyDirsChange() {
		post('updateConfig', $appState.config);
	}

	// --- Ignore patterns add/remove ---
	let newPattern = '';
	function addPattern() {
		const p = newPattern.trim();
		if (!p) return;
		if (!$appState.config.ignore_patterns.includes(p)) {
			post('updateConfig', {
				...$appState.config,
				ignore_patterns: [...$appState.config.ignore_patterns, p]
			});
		}
		newPattern = '';
	}
	function removePattern(p: string) {
		post('updateConfig', {
			...$appState.config,
			ignore_patterns: $appState.config.ignore_patterns.filter((x) => x !== p)
		});
	}
	function deleteAllPatterns() {
		post('updateConfig', { ...$appState.config, ignore_patterns: [] });
	}

	// Rescan
	function handleRescan() {
		post('rescanDirectory');
	}

	// --- Computed lists for rendering ---
	$: availableCommon = COMMON_IGNORE_PATTERNS.filter(
		(p) => !$appState.config.ignore_patterns.includes(p)
	);
	$: allPatterns = Array.from(new Set($appState.config.ignore_patterns || []));
	$: activeSet = new Set($appState.active_ignore_patterns || []);

	// patternFilter UI binding (Store enthält lowercase)
	function onPatternFilterInput(e: Event) {
		const v = (e.currentTarget as HTMLInputElement).value.toLowerCase();
		patternFilter.set(v);
	}

	$: filteredPatterns = (() => {
		const pf = ($patternFilter || '').trim();
		const base = [...allPatterns].sort((a, b) => a.localeCompare(b));
		const filtered = pf ? base.filter((p) => p.toLowerCase().includes(pf)) : base;
		// aktive Patterns zuerst
		return filtered.sort((a, b) => Number(activeSet.has(b)) - Number(activeSet.has(a)));
	})();
</script>

<!-- IMPORTANT: No outer <aside>. This content is mounted inside <aside.sidebar#sidebar-root> -->
<div class="panel">
	<div class="panel-header">
		<h3>
			<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<circle cx="11" cy="11" r="8" />
				<path d="m21 21-4.35-4.35" />
			</svg>
			Search & Filter
		</h3>
	</div>

	<input
		type="text"
		id="search-query"
		placeholder={searchEnabled ? 'Search filenames...' : 'Select a directory first...'}
		bind:value={$appState.search_query}
		disabled={!searchEnabled}
		on:input={onFiltersInput}
	/>

	<input
		type="text"
		id="extension-filter"
		placeholder={searchEnabled ? 'Filter by extension (e.g., rs, py)' : 'Select a directory first...'}
		bind:value={$appState.extension_filter}
		disabled={!searchEnabled}
		on:input={onFiltersInput}
	/>

	<input
		type="text"
		id="content-search-query"
		placeholder={searchEnabled ? 'Search text inside files...' : 'Select a directory first...'}
		bind:value={$appState.content_search_query}
		disabled={!searchEnabled}
		on:input={onFiltersInput}
	/>

	<label>
		<input
			type="checkbox"
			id="case-sensitive"
			bind:checked={$appState.config.case_sensitive_search}
			on:change={onCaseSensitiveChange}
		/>
		Case Sensitive
	</label>
</div>

<div class="panel ignore-patterns-panel">
	<div class="panel-header">
		<h3>
			<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
				<circle cx="12" cy="12" r="10" />
				<line x1="4.93" y1="4.93" x2="19.07" y2="19.07" />
			</svg>
			Ignore Patterns
		</h3>

		<button
			id="rescan-btn"
			title={$appState.patterns_need_rescan
				? 'Ignore patterns were removed - Re-scan recommended to find previously hidden files'
				: 'Re-scan with current ignore patterns'}
			class:needs-rescan={$appState.patterns_need_rescan}
			disabled={$appState.is_scanning || !$appState.current_path}
			on:click={handleRescan}
		>
			{@html $appState.is_scanning
				? '<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><polyline points="12,6 12,12 16,14"/></svg>'
				: ($appState.patterns_need_rescan
					? `<svg class="icon pulse" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M3 21v-5h5"/></svg>`
					: '<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M3 21v-5h5"/></svg>')}
			{$appState.is_scanning ? 'Scanning...' : 'Re-Scan'}
		</button>
	</div>

	<div class="input-group">
		<input
			type="text"
			id="new-ignore-pattern"
			placeholder="Add pattern (*.log, build/)"
			bind:value={newPattern}
			on:keydown={(e) => e.key === 'Enter' && addPattern()}
		/>
		<button id="add-pattern-btn" on:click={addPattern}>Add</button>
	</div>

	<div class="ignore-options">
		<div class="ignore-actions">
			<button id="delete-all-patterns-btn" title="Remove all ignore patterns" on:click={deleteAllPatterns}>
				Delete All
			</button>
			<label>
				<input
					type="checkbox"
					id="remove-empty-dirs"
					bind:checked={$appState.config.remove_empty_directories}
					on:change={onRemoveEmptyDirsChange}
				/>
				Remove empty dirs
			</label>
		</div>
	</div>

	<div class="common-patterns-section">
		<!-- A11y: heading + labelled group instead of a <label> without control -->
		<p
			id="common-patterns-heading"
			class="common-patterns-label"
			style:display={availableCommon.length > 0 ? 'block' : 'none'}
		>
			Common Ignore Pattern:
		</p>
		<div
			id="common-patterns-container"
			class="common-patterns-container"
			role="group"
			aria-labelledby="common-patterns-heading"
		>
			{#each availableCommon as pattern}
				<button
					class="common-pattern-chip"
					on:click={() =>
						post('updateConfig', {
							...$appState.config,
							ignore_patterns: [...$appState.config.ignore_patterns, pattern]
						})}
					title={`Click to add "${pattern}" to ignore patterns`}
				>
					{pattern}
				</button>
			{/each}
		</div>
	</div>

	<input
		type="text"
		id="filter-patterns"
		placeholder="Filter currently assigned ignore patterns..."
		value={$patternFilter}
		on:input={onPatternFilterInput}
	/>

	<div id="current-patterns-container" class="current-patterns" role="list">
		{#each filteredPatterns as p}
			<div
				class="current-pattern-chip {activeSet.has(p) ? 'active-pattern' : ''}"
				title={activeSet.has(p)
					? 'This pattern was active and matched one or more files/directories.'
					: ''}
				role="listitem"
			>
				<span>{p}</span>
				<button
					class="remove-pattern-btn"
					on:click={() => removePattern(p)}
					aria-label={`Remove pattern ${p}`}
				>
					<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
						<line x1="18" y1="6" x2="6" y2="18" />
						<line x1="6" y1="6" x2="18" y2="18" />
					</svg>
				</button>
			</div>
		{/each}
	</div>
</div>
