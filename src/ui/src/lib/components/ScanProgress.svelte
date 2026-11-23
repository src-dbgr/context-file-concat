<script lang="ts">
  /**
   * Progress of the running directory scan.
   * The total number of files is unknown while scanning, so the bar is
   * indeterminate; the numbers come from the `scanProgress` store.
   */
  import { post } from "$lib/services/backend";
  import { scanProgress } from "$lib/stores/scanProgress";
  import Spinner from "$lib/components/Spinner.svelte";
  import LinearProgress from "$lib/components/LinearProgress.svelte";
  import { t } from "$lib/i18n";
</script>

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

<style>
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
