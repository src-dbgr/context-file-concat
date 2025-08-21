<script lang="ts">
  import type { TreeNode } from '$lib/types';
  import { post } from '$lib/services/backend';
  import { formatFileSize } from '$lib/utils';
  import { recordDirExpanded } from '$lib/modules/treeExpansion';

  // Runes props (no `export let` in Svelte 5)
  type Props = { node: TreeNode; level?: number };
  let { node, level = 0 }: Props = $props();

  const indentWidth = () => level * 21;

  function toggleDir() {
    // Optimistically remember the desired state so a later backend render
    // cannot collapse the node again.
    recordDirExpanded(node.path, !node.is_expanded);
    post('toggleExpansion', node.path);
  }
  function openFile() {
    post('loadFilePreview', node.path);
  }
  function toggleDirCheckbox(e: Event) {
    e.preventDefault();
    post('toggleDirectorySelection', node.path);
  }
  function toggleFileCheckbox(e: Event) {
    e.preventDefault();
    post('toggleSelection', node.path);
  }
  function addIgnore(e: Event) {
    e.stopPropagation();
    post('addIgnorePath', node.path);
  }

  // keep checkbox indeterminate in sync
  function indeterminate(el: HTMLInputElement, value: boolean) {
    el.indeterminate = value;
    return {
      update(v: boolean) {
        el.indeterminate = v;
      }
    };
  }

  function onActivate(e: KeyboardEvent, action: () => void) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      action();
    }
  }
</script>

{#if node.is_directory}
	<div
		class="tree-item-container directory-item"
		data-path={node.path}
		data-type="directory"
		role="treeitem"
		aria-level={level + 1}
		aria-selected="false"
	>
		<span style="width: {indentWidth()}px; flex-shrink: 0;"></span>
		<span
			class="arrow {node.is_expanded ? 'expanded' : ''}"
			data-type="directory"
			role="button"
			tabindex="0"
			onclick={toggleDir}
			onkeydown={(e) => onActivate(e, toggleDir)}
		></span>
		<input
			type="checkbox"
			checked={node.selection_state === 'full'}
			use:indeterminate={node.selection_state === 'partial'}
			onclick={toggleDirCheckbox}
			data-path={node.path}
			data-type="dir-checkbox"
			aria-label="Toggle directory selection"
		/>
		<div class="name-and-button">
			<span
				class="file-name {node.is_match ? 'is-match' : ''}"
				data-path={node.path}
				data-type="label"
				role="button"
				tabindex="0"
				onclick={toggleDir}
				onkeydown={(e) => onActivate(e, toggleDir)}
			>
				<svg class="icon" viewBox="0 0 24 24"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>
				{node.name}
			</span>
			<button
				class="ignore-btn"
				title="Add this directory to ignore patterns"
				aria-label="Add directory to ignore patterns"
				onclick={addIgnore}
				data-path={node.path}
				data-type="ignore"
			>
				<svg class="icon ignore-icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" /></svg>
			</button>
		</div>
	</div>
{:else}
	<div
		class="tree-item-container file-item {node.is_previewed ? 'previewed' : ''}"
		data-path={node.path}
		role="treeitem"
		aria-level={level + 1}
		aria-selected={node.is_previewed ? 'true' : 'false'}
	>
		<span style="width: {indentWidth()}px; flex-shrink: 0;"></span>
		<span class="spacer"></span>
		<input
			type="checkbox"
			checked={node.selection_state === 'full'}
			onclick={toggleFileCheckbox}
			data-path={node.path}
			data-type="file-checkbox"
			aria-label="Toggle file selection"
		/>
		<div class="name-and-button">
			<span
				class="file-name {node.is_match ? 'is-match' : ''}"
				data-path={node.path}
				data-type="label"
				role="button"
				tabindex="0"
				onclick={openFile}
				onkeydown={(e) => onActivate(e, openFile)}
			>
				{#if node.is_binary}
					<svg class="icon" viewBox="0 0 24 24"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>
				{:else}
					<svg class="icon" viewBox="0 0 24 24"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14,2 14,8 20,8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10,9 9,9 8,9"/></svg>
				{/if}
				{node.name}
			</span>
			<button
				class="ignore-btn"
				title="Add this file to ignore patterns"
				aria-label="Add file to ignore patterns"
				onclick={addIgnore}
				data-path={node.path}
				data-type="ignore"
			>
				<svg class="icon ignore-icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><line x1="4.93" y1="4.93" x2="19.07" y2="19.07" /></svg>
			</button>
		</div>
		<span class="file-size">{formatFileSize(node.size)}</span>
	</div>
{/if}
