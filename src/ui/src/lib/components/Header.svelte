<script lang="ts">
  import { appState } from "$lib/stores/app";
  import { post } from "$lib/services/backend";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import LocaleToggle from "$lib/components/LocaleToggle.svelte";
  import { t } from "$lib/i18n";

  // Runes derived fields
  const current_path = $derived($appState.current_path);
  const current_config_filename = $derived($appState.current_config_filename);
  const is_scanning = $derived($appState.is_scanning);
</script>

<div class="top-bar">
  <div class="path-selection">
    <button
      id="clear-dir-btn"
      title={$t("action.clear")}
      style:display={current_path ? "inline-flex" : "none"}
      onclick={() => post("clearDirectory")}
      disabled={is_scanning}
    >
      <svg class="icon" viewBox="0 0 24 24">
        <line x1="18" y1="6" x2="6" y2="18" />
        <line x1="6" y1="6" x2="18" y2="18" />
      </svg>
      {$t("action.clear")}
    </button>
    <span id="current-path" title={current_path ?? ""}>
      {current_path || $t("action.noDirectorySelected")}
    </span>
  </div>
  <div class="config-buttons">
    <span id="current-config-filename" class="config-filename">
      {current_config_filename || ""}
    </span>
    <button
      id="import-config-btn"
      onclick={() => post("importConfig")}
      disabled={is_scanning}
      title={$t("action.importConfig")}
    >
      <svg class="icon" viewBox="0 0 24 24">
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
        <polyline points="14,2 14,8 20,8" />
        <line x1="16" y1="13" x2="8" y2="13" />
        <line x1="16" y1="17" x2="8" y2="17" />
        <polyline points="10,9 9,9 8,9" />
      </svg>
      {$t("action.importConfig")}
    </button>
    <button
      id="export-config-btn"
      onclick={() => post("exportConfig")}
      disabled={is_scanning || !current_path}
      title={$t("action.exportConfig")}
    >
      <svg class="icon" viewBox="0 0 24 24">
        <path
          d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"
        />
        <polyline points="17,21 17,13 7,13 7,21" />
        <polyline points="7,3 7,8 15,8" />
      </svg>
      {$t("action.exportConfig")}
    </button>
    <ThemeToggle />
    <LocaleToggle />
  </div>
</div>
