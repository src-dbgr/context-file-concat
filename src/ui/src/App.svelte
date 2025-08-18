<script lang="ts">
	import { onMount } from 'svelte';
	import {
		appState,
		editorInstance,
		editorDecorations,
		previewedPath,
		getState
	} from '$lib/stores/app';
	import { get } from 'svelte/store';
	import type { AppState, TreeNode } from '$lib/types';
	import type * as monaco from 'monaco-editor/esm/vs/editor/editor.api';

	import { elements } from '$lib/dom';
	import StatusBar from '$lib/components/StatusBar.svelte';

	// Only handles cross-cutting UI now (generate button, editor highlights, file stats)

	let generatingIntervalId: ReturnType<typeof setInterval> | null = null;

	onMount(() => {
		let previousState: AppState | null = null;

		const unsubscribeAppState = appState.subscribe((newState) => {
			if (!previousState) {
				renderUI();
				previousState = newState;
				return;
			}

			renderUI();
			updateEditorDecorations();

			previousState = newState;
		});

		return () => {
			unsubscribeAppState();
		};
	});

	function renderUI() {
		const s = getState();
		const { is_scanning, is_generating } = s;

		const hasSelection = s.selected_files_count > 0;

		// Restore/update file statistics in header
		updateFileStats(s);

		// Generate / Cancel button in bottom bar (unchanged behavior)
		const iconGenerate = `<svg class="icon icon-lightning-light" viewBox="0 0 24 24"><path d="M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z"></path></svg>`;
		const iconCancel = `<svg class="icon" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>`;

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
	}

	/** Counts files and folders in the current (filtered) tree. */
	function countFilesAndFolders(nodes: TreeNode[]): { files: number; folders: number } {
		let files = 0;
		let folders = 0;
		for (const n of nodes || []) {
			if (n.is_directory) {
				folders += 1;
				if (n.children?.length) {
					const sub = countFilesAndFolders(n.children);
					files += sub.files;
					folders += sub.folders;
				}
			} else {
				files += 1;
			}
		}
		return { files, folders };
	}

	/** Updates the header stats text (right side of the Files panel header). */
	function updateFileStats(state: AppState) {
		const el = elements.fileStats;
		if (!el) return;

		// When no directory is selected, clear the text.
		if (!state.current_path) {
			el.textContent = '';
			return;
		}

		// While scanning, still show current numbers from the visible tree snapshot.
		const { files, folders } = countFilesAndFolders(state.tree || []);
		const selected = state.selected_files_count ?? 0;

		// Matches the original wording from your screenshots
		el.textContent = `Files: ${selected} selected of ${files}  ·  Folders: ${folders}`;
	}

	// Keep Monaco search-highlights in sync with store state
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
