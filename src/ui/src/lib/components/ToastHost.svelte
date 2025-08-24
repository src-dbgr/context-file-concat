<script lang="ts">
  /**
   * ToastHost – accessible toast stack (Svelte 5 Runes)
   * - No {@html} (avoids XSS / satisfies svelte/no-at-html-tags)
   * - Only imports `toasts` from the store (fixes svelte-check errors)
   */
  import { toasts, toast } from "$lib/stores/toast";

  type Variant = "info" | "success" | "warning" | "error" | "default";
  type Item = {
    id: number; // Fixed: should be number to match toast.ts
    variant?: Variant;
    icon?: string | null;
    title?: string;
    message: string;
    dismissible?: boolean;
  };

  // Snapshot for markup
  const items = $derived($toasts as unknown as Item[]);

  /** Very small allowlist for keyword icons */
  const KEYWORD_ICONS = new Set(["info", "check", "warning", "error", "bolt"]);

  /** True if `s` looks like markup; we refuse to render it to avoid XSS. */
  function looksLikeHtml(s: string): boolean {
    return /<[^>]+>/.test(s);
  }

  /**
   * Choose a safe icon rendering strategy:
   * - If `icon` is an allowed keyword → render matching inline SVG
   * - Else if `icon` is a short emoji/glyph → render as text
   * - Else → fallback to variant's default SVG
   */
  function pickIconKind(variant: Variant = "default", icon?: string | null): string | { emoji: string } {
    const v = variant ?? "default";
    if (icon && !looksLikeHtml(icon)) {
      const trimmed = icon.trim();
      if (KEYWORD_ICONS.has(trimmed)) return trimmed;
      if (trimmed.length > 0 && trimmed.length <= 3) return { emoji: trimmed };
    }
    if (v === "success") return "check";
    if (v === "warning") return "warning";
    if (v === "error") return "error";
    return "info";
  }

  /** Dismiss a toast using the imported toast store */
  function dismissToast(id: number) {
    toast.dismiss(id);
  }
</script>

<!-- Landmark region for notifications -->
<aside class="toast-host" role="region" aria-label="Notifications">
  <ol class="toast-list" role="list" aria-live="polite" aria-relevant="additions text">
    {#each items as t (t.id)}
      <li class="toast {t.variant}" role="status" aria-atomic="true" data-id={t.id}>
        <div class="content">
          <span class="icon" aria-hidden="true">
            {#if typeof pickIconKind(t.variant ?? "default", t.icon) === "string"}
              {#if pickIconKind(t.variant ?? "default", t.icon) === "check"}
                <svg class="svg" viewBox="0 0 24 24" focusable="false" aria-hidden="true">
                  <polyline points="20 6 9 17 4 12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
              {:else if pickIconKind(t.variant ?? "default", t.icon) === "warning"}
                <svg class="svg" viewBox="0 0 24 24" focusable="false" aria-hidden="true">
                  <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" fill="none" stroke="currentColor" stroke-width="2"/>
                  <line x1="12" y1="9" x2="12" y2="13" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                  <line x1="12" y1="17" x2="12" y2="17" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                </svg>
              {:else if pickIconKind(t.variant ?? "default", t.icon) === "error"}
                <svg class="svg" viewBox="0 0 24 24" focusable="false" aria-hidden="true">
                  <circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2"/>
                  <line x1="15" y1="9" x2="9" y2="15" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                  <line x1="9" y1="9" x2="15" y2="15" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                </svg>
              {:else}
                <svg class="svg" viewBox="0 0 24 24" focusable="false" aria-hidden="true">
                  <circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2"/>
                  <line x1="12" y1="8" x2="12" y2="12" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                  <line x1="12" y1="16" x2="12.01" y2="16" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                </svg>
              {/if}
            {:else}
              <!-- Emoji/glyph path -->
              <span class="emoji">{(pickIconKind(t.variant ?? "default", t.icon) as {emoji:string}).emoji}</span>
            {/if}
          </span>

          <div class="text">
            {#if t.title}<strong class="title">{t.title}</strong>{/if}
            <div class="message">{t.message}</div>
          </div>
        </div>

        {#if t.dismissible !== false}
          <button
            type="button"
            class="close"
            aria-label="Dismiss notification"
            onclick={() => dismissToast(t.id)}
          >
            <svg viewBox="0 0 12 12" fill="currentColor">
              <path d="M6.707 6l3.647-3.646a.5.5 0 0 0-.708-.708L6 5.293 2.354 1.646a.5.5 0 1 0-.708.708L5.293 6 1.646 9.646a.5.5 0 0 0 .708.708L6 6.707l3.646 3.647a.5.5 0 0 0 .708-.708L6.707 6z"/>
            </svg>
          </button>
        {/if}
      </li>
    {/each}
  </ol>
</aside>

<style>
  .toast-host {
    position: fixed;
    inset: auto var(--space-8) var(--space-8) auto;
    z-index: var(--z-toast);
    pointer-events: none;
  }
  .toast-list {
    display: grid;
    gap: var(--space-4);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .toast {
    pointer-events: auto;
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: flex-start;
    gap: var(--space-6);
    min-width: 280px;
    max-width: 520px;
    padding: var(--space-6) var(--space-6);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-lg);
    background: var(--surface-1);
    color: var(--color-text);
    box-shadow: var(--shadow-2);
  }
  .toast .content {
    display: grid;
    grid-template-columns: auto 1fr;
    align-items: start;
    gap: var(--space-5);
  }
  .icon { 
    width: 18px; 
    height: 18px; 
    display: inline-flex; 
    align-items: center; 
    justify-content: center; 
  }
  .icon .svg { 
    width: 18px; 
    height: 18px; 
  }
  .icon .emoji { 
    font-size: 16px; 
    line-height: 1; 
  }
  .title { 
    display: block; 
    margin: 0 0 var(--space-2) 0; 
    font-size: var(--text-size-md); 
  }
  .message { 
    font-size: var(--text-size-sm); 
  }

  .close {
    appearance: none;
    background: transparent;
    border: 1px solid var(--color-border, #444);
    border-radius: var(--radius-sm, 4px);
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    color: currentColor;
    opacity: 0.7;
    transition: opacity 0.2s, border-color 0.2s;
  }
  .close:hover { 
    opacity: 1;
    border-color: var(--color-accent, #007acc); 
  }
  .close svg {
    width: 12px;
    height: 12px;
    flex-shrink: 0;
  }

  .toast.success { border-color: color-mix(in srgb, var(--color-success) 40%, var(--color-border)); }
  .toast.warning { border-color: color-mix(in srgb, var(--color-warning) 50%, var(--color-border)); }
  .toast.error   { border-color: color-mix(in srgb, var(--color-error) 50%, var(--color-border)); }
  .toast.info    { border-color: color-mix(in srgb, var(--color-accent) 50%, var(--color-border)); }
</style>