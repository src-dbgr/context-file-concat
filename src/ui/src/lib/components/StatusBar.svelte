<script lang="ts">
  import { appState } from '$lib/stores/app';
  import { post } from '$lib/services/backend';
  import Spinner from '$lib/components/Spinner.svelte';

  // Reactive derived state for the indexing indicator
  $: isIndexingInProgress = $appState.is_scanning && !$appState.is_fully_scanned;

  // This function is now component-local and handles all config changes from this component.
  // It's triggered whenever a bound value changes.
  function handleConfigChange() {
    // The `bind:checked` and `bind:value` directives have already updated the store.
    // We just need to notify the backend of the new configuration state.
    post('updateConfig', $appState.config);
  }
</script>

<div id="status-bar" class:indexing={isIndexingInProgress}>
  <details class="status-output-settings">
    <summary>
      <svg
        class="icon icon-closed"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path
          d="M12.52 20.924c-.87 .262 -1.93 -.152 -2.195 -1.241a1.724 1.724 0 0 0 -2.573 -1.066c-1.543 .94 -3.31 -.826 -2.37 -2.37a1.724 1.724 0 0 0 -1.065 -2.572c-1.756 -.426 -1.756 -2.924 0 -3.35a1.724 1.724 0 0 0 1.066 -2.573c-.94 -1.543 .826 -3.31 2.37 -2.37c1 .608 2.296 .07 2.572 -1.065c.426 -1.756 2.924 -1.756 3.35 0a1.724 1.724 0 0 0 2.573 1.066c1.543 -.94 3.31 .826 2.37 2.37a1.724 1.724 0 0 0 1.065 2.572c1.088 .264 1.502 1.323 1.242 2.192"
        />
        <path d="M19 16v6" />
        <path d="M22 19l-3 3l-3 -3" />
        <path d="M9 12a3 3 0 1 0 6 0a3 3 0 0 0 -6 0" />
      </svg>
      <svg
        class="icon icon-opened"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path
          d="M12.501 20.93c-.866 .25 -1.914 -.166 -2.176 -1.247a1.724 1.724 0 0 0 -2.573 -1.066c-1.543 .94 -3.31 -.826 -2.37 -2.37a1.724 1.724 0 0 0 -1.065 -2.572c-1.756 -.426 -1.756 -2.924 0 -3.35a1.724 1.724 0 0 0 1.066 -2.573c-.94 -1.543 .826 -3.31 2.37 -2.37c1 .608 2.296 .07 2.572 -1.065c.426 -1.756 2.924 -1.756 3.35 0a1.724 1.724 0 0 0 2.573 1.066c1.543 -.94 3.31 .826 2.37 2.37a1.724 1.724 0 0 0 1.065 2.572c1.074 .26 1.49 1.296 1.252 2.158"
        />
        <path d="M19 22v-6" />
        <path d="M22 19l-3 -3l-3 3" />
        <path d="M9 12a3 3 0 1 0 6 0a3 3 0 0 0 -6 0" />
      </svg>
      Output
    </summary>
    <div class="settings-content">
      <div class="setting-row">
        <label for="output-dir">Directory:</label>
        <input
          type="text"
          id="output-dir"
          bind:value={$appState.config.output_directory}
          on:change={handleConfigChange}
        />
        <button on:click={() => post('pickOutputDirectory')}>Browse</button>
      </div>
      <div class="setting-row">
        <label for="output-filename">Filename:</label>
        <input
          type="text"
          id="output-filename"
          bind:value={$appState.config.output_filename}
          on:change={handleConfigChange}
        />
      </div>
      <div class="setting-row">
        <label>
          <input
            type="checkbox"
            id="include-tree"
            bind:checked={$appState.config.include_tree_by_default}
            on:change={handleConfigChange}
          />
          Include directory tree
        </label>
        <label>
          <input
            type="checkbox"
            id="relative-paths"
            bind:checked={$appState.config.use_relative_paths}
            on:change={handleConfigChange}
          />
          Use relative file paths
        </label>
      </div>
    </div>
  </details>

  <span class="status-text" role="status" aria-live="polite">{$appState.status_message}</span>

  <div class="indexing-status" style:display={isIndexingInProgress ? 'flex' : 'none'}>
    <Spinner size={16} ariaLabel="Indexing…" />
    <span>Indexing...</span>
  </div>
</div>
