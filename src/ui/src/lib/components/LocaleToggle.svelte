<script lang="ts">
  // Minimal locale switcher (Svelte 5 Runes)
  // - Cycles through available locales (en ⇄ de)
  // - A11y: role="switch", keyboard Space/Enter
  import {
    availableLocales,
    locale,
    setLocale,
    t,
    type Locale,
  } from "$lib/i18n";

  const locales = $derived(availableLocales());
  const current = $derived($locale);

  function nextLocale(): Locale {
    const list = locales.length ? locales : ["en", "de"];
    const idx = Math.max(0, list.indexOf(current));
    const next = list[(idx + 1) % list.length] as Locale;
    return next;
  }

  function toggle() {
    setLocale(nextLocale());
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      toggle();
    }
  }

  const label = $derived($t("locale.current"));
</script>

<button
  type="button"
  class="locale-toggle"
  role="switch"
  aria-checked={current === "de"}
  aria-label={label}
  title={label}
  onclick={toggle}
  onkeydown={onKey}
>
  <span>{current.toUpperCase()}</span>
</button>

<style>
  .locale-toggle {
    min-height: var(--size-input-min-height);
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
  .locale-toggle:hover {
    border-color: var(--color-accent);
    box-shadow: var(--shadow-1);
  }
</style>
