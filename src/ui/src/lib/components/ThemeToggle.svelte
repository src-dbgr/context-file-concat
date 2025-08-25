<script lang="ts">
  /**
   * ThemeToggle – light/dark theme switcher (Svelte 5 Runes).
   * - Uses the central `theme` store (persists to localStorage).
   * - Applies `data-theme="light" | "dark"` on <html>.
   * - Fully accessible: role="switch", aria-checked, keyboard operable.
   */
  import { theme, toggleTheme } from "$lib/stores/theme";

  const isLight = $derived($theme === "light");
  const label = $derived(
    isLight ? "Switch to dark mode" : "Switch to light mode"
  );

  function onKey(e: KeyboardEvent) {
    // Support Space/Enter on focused switch
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      toggleTheme();
    }
  }
</script>

<button
  type="button"
  class="theme-toggle"
  role="switch"
  aria-checked={isLight}
  aria-label={label}
  title={label}
  onclick={toggleTheme}
  onkeydown={onKey}
>
  <!-- Sun / Moon swap with a smooth crossfade. We keep paths simple and inline. -->
  <span class="icon-wrap">
    <svg
      class="icon sun"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      aria-hidden="true"
      focusable="false"
    >
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2" /><path d="M12 20v2" /><path d="M4.93 4.93l1.41 1.41" />
      <path d="M17.66 17.66l1.41 1.41" />
      <path d="M2 12h2" /><path d="M20 12h2" />
      <path d="M4.93 19.07l1.41-1.41" />
      <path d="M17.66 6.34l1.41-1.41" />
    </svg>
    <svg
      class="icon moon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M21 12.79A9 9 0 1 1 11.21 3a7 7 0 0 0 9.79 9.79z" />
    </svg>
  </span>
  <span class="label">{isLight ? "Light" : "Dark"}</span>
</button>

<style>
  /* Make the toggle match header buttons 1:1 (height/padding/shape). */
  .theme-toggle {
    min-height: var(--size-input-min-height); /* 36px, matches header buttons */
    padding: var(--space-4) var(--space-6);
    border-radius: var(--radius-md);
    background-color: var(--button-bg);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    display: inline-flex;
    align-items: center;
    gap: var(--space-4);
    cursor: pointer;
    transition: all 0.2s ease;
    font-size: var(--text-size-base);
    font-weight: 500;
  }
  .theme-toggle:hover {
    border-color: var(--color-accent);
    box-shadow: var(--shadow-1);
  }

  .icon-wrap {
    position: relative;
    width: 18px;
    height: 18px;
  }
  .icon-wrap .icon {
    position: absolute;
    inset: 0;
    transition:
      opacity 160ms ease,
      transform 160ms ease;
  }

  /* Crossfade between sun and moon depending on current theme */
  :global(html[data-theme="light"]) .theme-toggle .sun {
    opacity: 1;
    transform: rotate(0deg) scale(1);
  }
  :global(html[data-theme="light"]) .theme-toggle .moon {
    opacity: 0;
    transform: rotate(-15deg) scale(0.9);
  }
  :global(html[data-theme="dark"]) .theme-toggle .sun {
    opacity: 0;
    transform: rotate(15deg) scale(0.9);
  }
  :global(html[data-theme="dark"]) .theme-toggle .moon {
    opacity: 1;
    transform: rotate(0deg) scale(1);
  }

  .label {
    font-size: var(--text-size-sm);
    color: var(--color-muted);
  }
</style>
