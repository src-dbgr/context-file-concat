<script lang="ts">
  /**
   * Progress of the running directory scan.
   * The total number of files is unknown while scanning, so the bar is
   * indeterminate; the numbers come from the `scanProgress` store.
   *
   * Two forms: the panel that fills the empty tree until the first rows are
   * there, and (`compact`) a one-line bar above the tree for the rest of the
   * scan, so that the scan can be watched and cancelled until it ends
   * without covering the rows that are already listed.
   */
  import { post } from "$lib/services/backend";
  import { scanProgress } from "$lib/stores/scanProgress";
  import Spinner from "$lib/components/Spinner.svelte";
  import LinearProgress from "$lib/components/LinearProgress.svelte";
  import { t } from "$lib/i18n";

  type Props = { compact?: boolean };
  let { compact = false }: Props = $props();
</script>

{#if compact}
  <div class="scan-bar" role="group" aria-label={$t("aria.scanProgress")}>
    <!-- Announced once; the numbers beside it change too often to be read out. -->
    <div class="scan-bar-status" role="status" aria-live="polite">
      <Spinner size={14} ariaHidden />
      <span class="scan-bar-text">{$t("filetree.scanning")}</span>
    </div>
    <span id="scan-files-count" class="scan-bar-count">
      {$t("scan.filesScanned", { count: $scanProgress.files_scanned })}
    </span>
    {#if $scanProgress.current_scanning_path}
      <span
        id="scan-current-path"
        class="scan-bar-path"
        title={$scanProgress.current_scanning_path}
      >
        {$scanProgress.current_scanning_path}
      </span>
    {/if}
    <button
      id="cancel-scan-btn"
      class="cancel-scan-btn scan-bar-cancel"
      title={$t("scan.cancelTitle")}
      aria-label={$t("scan.cancelTitle")}
      onclick={() => post("cancelScan")}
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
      {$t("filetree.cancel")}
    </button>
  </div>
{:else}
  <div class="scan-progress-container">
    <div class="scan-progress-header">
      <div class="scan-status" role="status" aria-live="polite">
        <Spinner size={16} ariaLabel={$t("filetree.scanning")} />
        <span class="scan-text">{$t("filetree.scanning")}</span>
      </div>
      <button
        id="cancel-scan-btn"
        class="cancel-scan-btn"
        title={$t("filetree.cancel")}
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
        {$t("filetree.cancel")}
      </button>
    </div>

    <LinearProgress ariaLabel={$t("aria.scanProgress")} indeterminate />

    <div class="scan-details" aria-live="polite">
      <span id="scan-files-count">
        {$t("scan.filesScanned", { count: $scanProgress.files_scanned })}
      </span>
      <span
        id="scan-current-path"
        title={$scanProgress.current_scanning_path || undefined}
      >
        {$scanProgress.current_scanning_path || $t("scan.starting")}
      </span>
      {#if $scanProgress.large_files_skipped > 0}
        <span id="scan-skipped-count">
          {$t("scan.largeFilesSkipped", {
            count: $scanProgress.large_files_skipped,
          })}
        </span>
      {/if}
    </div>
  </div>
{/if}

<style>
  .scan-bar {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: var(--space-5);
    min-width: 0;
    padding: var(--space-3) var(--space-5);
    border-bottom: 1px solid var(--color-border);
    background: var(--surface-2);
    font-size: 0.85em;
  }
  .scan-bar-status {
    display: inline-flex;
    align-items: center;
    gap: var(--space-4);
    flex: 0 0 auto;
    color: var(--color-text);
    font-weight: 500;
  }
  .scan-bar-count {
    flex: 0 0 auto;
    font-variant-numeric: tabular-nums;
  }
  .scan-bar-path {
    flex: 1 1 0;
    min-width: 0;
  }
  .scan-bar-cancel {
    flex: 0 0 auto;
    margin-left: auto;
    min-height: 0;
    padding: var(--space-2) var(--space-5);
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
