<script lang="ts">
	import { onMount } from 'svelte';
	import {
		appState,
		patternFilter,
		editorInstance,
		editorDecorations,
		previewedPath,
		getState
	} from '$lib/stores/app';
	import { get } from 'svelte/store';
	import type { AppState, TreeNode } from '$lib/types';
	import type * as monaco from 'monaco-editor';
	import { elements } from '$lib/dom';
	import { post } from '$lib/services/backend';
	import { COMMON_IGNORE_PATTERNS } from '$lib/config';
	import { formatFileSize } from '$lib/utils';
	import StatusBar from '$lib/components/StatusBar.svelte';

	// --- STATE & LIFECYCLE MANAGEMENT ---

	onMount(() => {
		let previousState: AppState | null = null;

		/**
		 * Wraps a render function with logic to preserve the scroll position of the file tree.
		 * This is a robust solution for the current imperative rendering bridge.
		 * @param renderFn The function that performs the DOM update.
		 */
		function preserveScroll(renderFn: () => void) {
			const container = document.querySelector<HTMLDivElement>('.virtual-scroll-container');
			const scroll = container ? container.scrollTop : 0;

			renderFn();

			const newContainer = document.querySelector<HTMLDivElement>('.virtual-scroll-container');
			if (newContainer) {
				requestAnimationFrame(() => {
					newContainer.scrollTop = scroll;
				});
			}
		}

		// Subscribe to the main state store to drive all UI updates
		const unsubscribeAppState = appState.subscribe((newState) => {
			if (!previousState) {
				// Initial render on mount, no scroll preservation needed
				renderUI();
				updateSearchInputsState();
				previousState = newState;
				return;
			}

			// Subsequent renders triggered by state changes
			preserveScroll(() => {
				renderUI();
				updateSearchInputsState();
				updateEditorDecorations();
			});

			const wasScanning = previousState.is_scanning && !newState.is_scanning;
			if (wasScanning) {
				const progressFill = document.getElementById('scan-progress-fill');
				if (progressFill) {
					progressFill.style.width = '100%';
					progressFill.classList.add('scan-complete');
				}
				setTimeout(() => preserveScroll(renderUI), 500); // Allow animation to finish
			}

			previousState = newState;
		});

		// Subscribe to the pattern filter store for live filtering of ignore patterns
		const unsubscribePatternFilter = patternFilter.subscribe(() => {
			if (previousState) {
				// Only re-render if the component is already mounted and has state
				preserveScroll(renderUI);
			}
		});

		// Return a cleanup function to prevent memory leaks
		return () => {
			unsubscribeAppState();
			unsubscribePatternFilter();
		};
	});

	// --- IMPERATIVE RENDER LOGIC (BRIDGE TO LEGACY CODE) ---
	// This entire section is the legacy render code, now controlled by Svelte's reactivity.
	// It will be replaced by Svelte components in the next migration stage.

	interface TreeNodeWithLevel {
		node: TreeNode;
		level: number;
	}

	let flatTree: TreeNodeWithLevel[] = [];
	let virtualScrollContainer: HTMLDivElement | null = null;
	let generatingIntervalId: ReturnType<typeof setInterval> | null = null;

	function flattenTree(nodes: TreeNode[], level = 0): TreeNodeWithLevel[] {
		let result: TreeNodeWithLevel[] = [];
		if (!nodes) return result;

		for (const node of nodes) {
			result.push({ node, level });
			if (
				node.is_directory &&
				node.is_expanded &&
				node.children &&
				node.children.length > 0
			) {
				result = result.concat(flattenTree(node.children, level + 1));
			}
		}
		return result;
	}

	function renderVirtualTree() {
		if (!virtualScrollContainer) return;

		const ITEM_HEIGHT = 28;
		const totalHeight = flatTree.length * ITEM_HEIGHT;
		const sizer = virtualScrollContainer.querySelector<HTMLDivElement>('.virtual-scroll-sizer');
		if (sizer) {
			sizer.style.height = `${totalHeight}px`;
		}

		const scrollTop = virtualScrollContainer.scrollTop;
		const viewportHeight = virtualScrollContainer.offsetHeight;

		let startIndex = Math.floor(scrollTop / ITEM_HEIGHT);
		let endIndex = Math.min(
			flatTree.length - 1,
			Math.ceil((scrollTop + viewportHeight) / ITEM_HEIGHT)
		);

		startIndex = Math.max(0, startIndex - 5);
		endIndex = Math.min(flatTree.length - 1, endIndex + 5);

		let html = '';
		for (let i = startIndex; i <= endIndex; i++) {
			const item = flatTree[i];
			if (item) {
				html += `<div class="virtual-scroll-item" style="top: ${
					i * ITEM_HEIGHT
				}px; height: ${ITEM_HEIGHT}px;">
							${createNodeHtml(item.node, item.level)}
						</div>`;
			}
		}

		const contentDiv =
			virtualScrollContainer.querySelector<HTMLDivElement>('.virtual-scroll-content');
		if (contentDiv) {
			contentDiv.innerHTML = html;
			contentDiv
				.querySelectorAll<HTMLInputElement>('[data-indeterminate="true"]')
				.forEach((el) => {
					el.indeterminate = true;
				});
		}
	}

	function createNodeHtml(node: TreeNode, level: number): string {
		const indentWidth = level * 21;

		if (node.is_directory) {
			const arrowClass = node.is_expanded ? 'expanded' : '';
			const checkboxState = node.selection_state === 'full' ? 'checked' : '';
			const indeterminateState =
				node.selection_state === 'partial' ? 'data-indeterminate="true"' : '';
			const matchClass = node.is_match ? 'is-match' : '';

			return `
				<div class="tree-item-container directory-item" data-path="${
					node.path
				}" data-type="directory">
					<span style="width: ${indentWidth}px; flex-shrink: 0;"></span>
					<span class="arrow ${arrowClass}" data-type="directory"></span>
					<input type="checkbox" ${checkboxState} ${indeterminateState} data-path="${
				node.path
			}" data-type="dir-checkbox">
					<div class="name-and-button">
						<span class="file-name ${matchClass}" data-path="${
				node.path
			}" data-type="label">
							<svg class="icon" viewBox="0 0 24 24"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>
							${node.name}
						</span>
						<button class="ignore-btn" title="Add this directory to ignore patterns" data-path="${
							node.path
						}" data-type="ignore">
							<svg class="icon ignore-icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" /></svg>
						</button>
					</div>
				</div>`;
		} else {
			const checkboxState = node.selection_state === 'full' ? 'checked' : '';
			const previewedClass = node.is_previewed ? 'previewed' : '';
			const matchClass = node.is_match ? 'is-match' : '';
			const iconHTML = node.is_binary
				? `<svg class="icon" viewBox="0 0 24 24"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>`
				: `<svg class="icon" viewBox="0 0 24 24"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14,2 14,8 20,8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10,9 9,9 8,9"/></svg>`;

			return `
				<div class="tree-item-container file-item ${previewedClass}" data-path="${node.path}">
					<span style="width: ${indentWidth}px; flex-shrink: 0;"></span>
					<span class="spacer"></span>
					<input type="checkbox" ${checkboxState} data-path="${node.path}" data-type="file-checkbox">
					<div class="name-and-button">
						<span class="file-name ${matchClass}" data-path="${node.path}" data-type="label">
							${iconHTML}
							${node.name}
						</span>
						<button class="ignore-btn" title="Add this file to ignore patterns" data-path="${
							node.path
						}" data-type="ignore">
							<svg class="icon ignore-icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" /></svg>
						</button>
					</div>
					<span class="file-size">${formatFileSize(node.size)}</span>
				</div>`;
		}
	}

	function handleTreeClick(event: MouseEvent) {
		const target = event.target as HTMLElement;
		const actionElement = target.closest<HTMLElement>('[data-type]');
		if (!actionElement) return;
		const itemContainer = target.closest<HTMLElement>('.tree-item-container');
		if (!itemContainer) return;

		const path = itemContainer.dataset.path;
		const type = actionElement.dataset.type;

		if (path) {
			switch (type) {
				case 'directory':
				case 'label':
					if (itemContainer.dataset.type === 'directory') {
						post('toggleExpansion', path);
					} else {
						post('loadFilePreview', path);
					}
					break;
				case 'dir-checkbox':
				case 'file-checkbox':
					event.preventDefault();
					post(
						type === 'dir-checkbox' ? 'toggleDirectorySelection' : 'toggleSelection',
						path
					);
					break;
				case 'ignore':
					event.stopPropagation();
					post('addIgnorePath', path);
					break;
			}
		}
	}

	function countTreeItems(nodes: TreeNode[]): {
		totalFiles: number;
		totalFolders: number;
	} {
		let totalFiles = 0;
		let totalFolders = 0;
		function traverse(items: TreeNode[]) {
			for (const item of items) {
				if (item.is_directory) {
					totalFolders++;
					if (item.children && item.children.length > 0) traverse(item.children);
				} else {
					totalFiles++;
				}
			}
		}
		traverse(nodes);
		return { totalFiles, totalFolders };
	}

	function createScanProgressUI(): HTMLDivElement {
		const container = document.createElement('div');
		container.className = 'scan-progress-container';
		container.innerHTML = `
		<div class="scan-progress-header">
			<div class="scan-status">
				<div class="scan-spinner"></div>
				<span class="scan-text">Scanning directory...</span>
			</div>
			<button id="cancel-scan-btn" class="cancel-scan-btn" title="Cancel current scan">
				<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg> Cancel
			</button>
		</div>
		<div class="scan-progress-bar">
			<div class="scan-progress-fill" id="scan-progress-fill"></div>
		</div>
		<div class="scan-details">
			<span id="scan-files-count">0 files processed</span>
			<span id="scan-current-path">Starting scan...</span>
			<span id="scan-skipped-count"></span>
		</div>`;
		return container;
	}

	function createMessageDisplay(
		message: string,
		iconSvg: string | null = null
	): HTMLDivElement {
		const messageContainer = document.createElement('div');
		messageContainer.className = 'message-display';

		if (iconSvg) {
			const iconElement = document.createElement('div');
			iconElement.className = 'message-icon';
			iconElement.innerHTML = iconSvg;
			messageContainer.appendChild(iconElement);
		}

		const textElement = document.createElement('p');
		textElement.className = 'message-text';
		textElement.textContent = message;
		messageContainer.appendChild(textElement);

		return messageContainer;
	}

	function createDirectorySelectionPlaceholder(): HTMLParagraphElement {
		const placeholder = document.createElement('p');
		placeholder.className = 'placeholder';
		placeholder.textContent = 'Choose Directory';
		placeholder.style.cursor = 'pointer';
		placeholder.addEventListener('click', () => post('selectDirectory'));
		return placeholder;
	}

	function hasActiveFilters(appState: AppState): boolean {
		return !!(
			appState.search_query?.trim() ||
			appState.extension_filter?.trim() ||
			appState.content_search_query?.trim()
		);
	}

	function renderIgnorePatterns() {
		elements.currentPatternsContainer.innerHTML = '';
		const appState = getState();
		const allPatterns = Array.from(new Set(appState.config.ignore_patterns || []));
		const activePatterns = new Set(appState.active_ignore_patterns || []);

		const active = allPatterns.filter((p) => activePatterns.has(p)).sort();
		const inactive = allPatterns.filter((p) => !activePatterns.has(p)).sort();
		let patternsToRender = [...active, ...inactive];

		const currentPatternFilter = get(patternFilter);
		if (currentPatternFilter) {
			patternsToRender = patternsToRender.filter((pattern) =>
				pattern.toLowerCase().includes(currentPatternFilter)
			);
		}

		patternsToRender.forEach((p) => {
			const chip = document.createElement('div');
			chip.className = 'current-pattern-chip';
			if (activePatterns.has(p)) {
				chip.classList.add('active-pattern');
				chip.title = `This pattern was active and matched one or more files/directories.`;
			}

			const nameSpan = document.createElement('span');
			nameSpan.textContent = p;

			const removeBtn = document.createElement('button');
			removeBtn.className = 'remove-pattern-btn';
			removeBtn.dataset.pattern = p;
			removeBtn.innerHTML = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;

			removeBtn.addEventListener('click', () => {
				const patternToRemove = removeBtn.dataset.pattern;
				const currentConfig = getState().config;
				const newPatterns = currentConfig.ignore_patterns.filter(
					(pat) => pat !== patternToRemove
				);
				post('updateConfig', { ...currentConfig, ignore_patterns: newPatterns });
			});

			chip.appendChild(nameSpan);
			chip.appendChild(removeBtn);
			elements.currentPatternsContainer.appendChild(chip);
		});
	}
	function setupCommonPatterns() {
		elements.commonPatternsContainer.innerHTML = '';
		const appState = getState();
		const availablePatterns = COMMON_IGNORE_PATTERNS.filter(
			(pattern) => !appState.config.ignore_patterns.includes(pattern)
		);

		const commonPatternsLabel =
			document.querySelector<HTMLLabelElement>('.common-patterns-label');
		if (commonPatternsLabel) {
			commonPatternsLabel.style.display = availablePatterns.length > 0 ? 'block' : 'none';
		}

		availablePatterns.forEach((pattern) => {
			const chip = document.createElement('button');
			chip.className = 'common-pattern-chip';
			chip.textContent = pattern;
			chip.title = `Click to add "${pattern}" to ignore patterns`;
			chip.addEventListener('click', (e) => {
				e.preventDefault();
				const currentConfig = getState().config;
				if (!currentConfig.ignore_patterns.includes(pattern)) {
					post('updateConfig', {
						...currentConfig,
						ignore_patterns: [...currentConfig.ignore_patterns, pattern]
					});
				}
			});
			elements.commonPatternsContainer.appendChild(chip);
		});
	}

	function renderUI() {
		const appState = getState();
		const { config, is_scanning, is_generating, tree } = appState;

		elements.currentPath.textContent = appState.current_path || 'No directory selected.';
		elements.currentPath.title = appState.current_path ?? '';
		elements.clearDirBtn.style.display = appState.current_path ? 'inline-block' : 'none';
		elements.currentConfigFilename.textContent = appState.current_config_filename || '';

		elements.caseSensitive.checked = config.case_sensitive_search;
		elements.removeEmptyDirs.checked = config.remove_empty_directories || false;
		elements.searchQuery.value = appState.search_query;
		elements.extensionFilter.value = appState.extension_filter;
		elements.contentSearchQuery.value = appState.content_search_query;

		const hasSelection = appState.selected_files_count > 0;
		const hasVisibleItems = tree.length > 0;

		elements.selectDirBtn.disabled = is_scanning;
		elements.rescanBtn.disabled = is_scanning || !appState.current_path;
		elements.importConfigBtn.disabled = is_scanning;
		elements.exportConfigBtn.disabled = is_scanning || !appState.current_path;

		elements.expandAllBtn.disabled = is_scanning || !hasVisibleItems;
		elements.selectAllBtn.disabled = is_scanning || !hasVisibleItems;
		elements.collapseAllBtn.disabled = is_scanning || !hasVisibleItems;
		elements.deselectAllBtn.disabled = is_scanning || !hasSelection;

		const iconFolder = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>`;
		const iconScan = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M3 21v-5h5"/></svg>`;
		const iconScanning = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><polyline points="12,6 12,12 16,14"/></svg>`;
		const iconGenerate = `<svg class="icon icon-lightning-light" viewBox="0 0 24 24"><path d="M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z"></path></svg>`;
		const iconCancel = `<svg class="icon" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>`;

		if (is_scanning) {
			elements.selectDirBtn.innerHTML = `${iconScanning} Scanning...`;
			elements.rescanBtn.innerHTML = `${iconScanning} Scanning...`;
		} else {
			elements.selectDirBtn.innerHTML = `${iconFolder} Select Directory`;
			elements.rescanBtn.innerHTML = `${iconScan} Re-Scan`;
			if (appState.patterns_need_rescan) {
				elements.rescanBtn.classList.add('needs-rescan');
				elements.rescanBtn.title =
					'Ignore patterns were removed - Re-scan recommended to find previously hidden files';
				const iconPulse = `<svg class="icon pulse" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
		<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/>
		<path d="M21 3v5h-5"/>
		<path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/>
		<path d="M3 21v-5h5"/>
	</svg>`;
				elements.rescanBtn.innerHTML = `${iconPulse} Re-Scan`;
			} else {
				elements.rescanBtn.classList.remove('needs-rescan');
				elements.rescanBtn.title = 'Re-scan with current ignore patterns';
			}
		}

		const wasGenerating = elements.generateBtn.classList.contains('is-generating');

		if (is_generating) {
			if (!wasGenerating) {
				if (generatingIntervalId) clearInterval(generatingIntervalId);
				elements.generateBtn.classList.remove('button-cta');
				elements.generateBtn.classList.add('is-generating');
				elements.generateBtn.innerHTML = `
				<span class="generating-content">
					${iconGenerate}
					<span class="generating-text">Concat</span>
				</span>
				<span class="cancel-content">${iconCancel} Cancel</span>
			`;
				const textElement =
					elements.generateBtn.querySelector<HTMLSpanElement>('.generating-text');
				let dotCount = 0;
				generatingIntervalId = setInterval(() => {
					dotCount = (dotCount + 1) % 4;
					const dots = '.'.repeat(dotCount);
					const spaces = '\u00A0'.repeat(3 - dotCount);
					if (textElement) {
						textElement.textContent = `Concat${dots}${spaces}`;
					}
				}, 500);
			}
			elements.generateBtn.disabled = false;
		} else {
			if (wasGenerating) {
				if (generatingIntervalId) clearInterval(generatingIntervalId);
				generatingIntervalId = null;
				elements.generateBtn.classList.remove('is-generating');
				elements.generateBtn.classList.add('button-cta');
				elements.generateBtn.innerHTML = `${iconGenerate} Generate`;
			}
			elements.generateBtn.disabled = !hasSelection || is_scanning;
		}

		elements.fileTreeContainer.innerHTML = '';

		if (is_scanning && tree.length === 0) {
			elements.fileTreeContainer.appendChild(createScanProgressUI());
			const cancelBtn = document.getElementById('cancel-scan-btn');
			if (cancelBtn) {
				cancelBtn.addEventListener('click', () => {
					post('cancelScan');
					(cancelBtn as HTMLButtonElement).disabled = true;
					cancelBtn.innerHTML = `${iconScanning} Cancelling...`;
				});
			}
		} else if (!appState.current_path) {
			elements.fileTreeContainer.appendChild(createDirectorySelectionPlaceholder());
		} else if (tree.length > 0) {
			virtualScrollContainer = document.createElement('div');
			virtualScrollContainer.className = 'virtual-scroll-container tree';
			virtualScrollContainer.innerHTML =
				'<div class="virtual-scroll-sizer"><div class="virtual-scroll-content"></div></div>';

			elements.fileTreeContainer.appendChild(virtualScrollContainer);

			virtualScrollContainer.addEventListener('scroll', renderVirtualTree);
			virtualScrollContainer.addEventListener('click', handleTreeClick);

			flatTree = flattenTree(tree);
			renderVirtualTree();
		} else {
			const hasFilters = hasActiveFilters(appState);
			if (hasFilters) {
				const noResultsIcon = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/><line x1="11" y1="8" x2="11" y2="14"/><line x1="8" y1="11" x2="14" y2="11"/></svg>`;
				elements.fileTreeContainer.appendChild(
					createMessageDisplay('No files found matching filters.', noResultsIcon)
				);
			} else {
				const emptyIcon = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/><path d="M12 10v6"/><path d="M9 13h6"/></svg>`;
				elements.fileTreeContainer.appendChild(
					createMessageDisplay('No files found in this directory.', emptyIcon)
				);
			}
		}

		const { totalFiles, totalFolders } = countTreeItems(tree);
		elements.fileStats.textContent = `Files: ${appState.selected_files_count} selected of ${totalFiles} • Folders: ${totalFolders}`;

		setupCommonPatterns();
		renderIgnorePatterns();
	}

	// --- LOGIC MOVED FROM eventListeners.ts ---
	function shouldEnableSearch(): boolean {
		const currentAppState = getState();
		return !!(currentAppState.current_path && !currentAppState.is_scanning);
	}

	function updateSearchInputsState() {
		const searchEnabled = shouldEnableSearch();

		elements.searchQuery.disabled = !searchEnabled;
		elements.extensionFilter.disabled = !searchEnabled;
		elements.contentSearchQuery.disabled = !searchEnabled;

		if (!searchEnabled) {
			elements.searchQuery.placeholder = 'Select a directory first...';
			elements.extensionFilter.placeholder = 'Select a directory first...';
			elements.contentSearchQuery.placeholder = 'Select a directory first...';
		} else {
			elements.searchQuery.placeholder = 'Search filenames...';
			elements.extensionFilter.placeholder = 'Filter by extension (e.g., rs, py)';
			elements.contentSearchQuery.placeholder = 'Search text inside files...';
		}
	}

	// --- LOGIC MOVED FROM main.ts ---
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