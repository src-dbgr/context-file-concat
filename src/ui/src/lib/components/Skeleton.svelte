<script lang="ts">
  export let width: string = "100%";
  export let height: string = "1rem";
  export let radius: string = "var(--radius-md)";
  export let animated: boolean = true;
  export let ariaLabel: string = "Loading content";
</script>

<div
  class="cfc-skeleton"
  class:no-animate={!animated}
  style={`--_w:${width};--_h:${height};--_r:${radius}`}
  aria-hidden="true"
  role="presentation"
  title={ariaLabel}
></div>

<style>
  .cfc-skeleton {
    width: var(--_w);
    height: var(--_h);
    border-radius: var(--_r);
    background:
      linear-gradient(90deg,
        color-mix(in srgb, var(--surface-2) 70%, transparent) 25%,
        color-mix(in srgb, var(--surface-1) 95%, transparent) 37%,
        color-mix(in srgb, var(--surface-2) 70%, transparent) 63%
      );
    background-size: 400% 100%;
    animation: cfc-skeleton 1.25s ease-in-out infinite;
  }
  :global(.no-animate) .cfc-skeleton,
  .cfc-skeleton:not(:where(:has(+ *))) { animation: none; background: var(--surface-2); }
  @keyframes cfc-skeleton { 0% {background-position: 100% 50%} 100%{background-position: 0% 50%} }
</style>
