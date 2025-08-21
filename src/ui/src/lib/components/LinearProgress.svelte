<script lang="ts">
  /**
   * Accessible linear progress bar (Svelte 5 Runes).
   * - Determinate:   <LinearProgress value={42} ariaLabel="Scanning" />
   * - Indeterminate: <LinearProgress indeterminate ariaLabel="Working..." />
   * - Legacy IPC control: keep idForFill so external width updates still work.
   */

  type Props = {
    ariaLabel?: string;
    value?: number;           // 0..100 (determinate) – leave undefined for indeterminate
    indeterminate?: boolean;  // true => animated bar
    idForFill?: string;       // optional id on inner fill for legacy updates
  };

  let {
    ariaLabel = "Progress",
    value,
    indeterminate = false,
    idForFill
  }: Props = $props();

  const clamp100 = (n: number) => Math.max(0, Math.min(100, n));

  // Derived aria-valuenow (only in determinate mode)
  const valueNow = $derived(!indeterminate && typeof value === "number" ? clamp100(value) : undefined);

  // Inline width style for determinate mode
  const widthStyle = $derived(valueNow !== undefined ? `width:${valueNow}%` : undefined);
</script>

<div
  class="scan-progress-bar"
  class:cfc-indeterminate={indeterminate}
  role="progressbar"
  aria-label={ariaLabel}
  aria-busy={indeterminate}
  aria-valuemin="0"
  aria-valuemax="100"
  aria-valuenow={valueNow}
>
  <!-- NOTE: non-void element must not be self-closing -->
  <div class="scan-progress-fill" id={idForFill} style={widthStyle}></div>
</div>

<style>
  /* Minimal fallback visuals for indeterminate mode. */
  .scan-progress-bar {
    position: relative;
    height: 8px;
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  /* Only active in indeterminate mode */
  .scan-progress-bar.cfc-indeterminate::before {
    content: "";
    position: absolute;
    left: -40%;
    top: 0;
    height: 100%;
    width: 40%;
    background: var(--color-accent);
    opacity: 0.85;
    animation: cfc-indeterminate-slide 1.2s ease-in-out infinite;
  }

  @keyframes cfc-indeterminate-slide {
    0%   { left: -40%; }
    50%  { left: 60%; }
    100% { left: 100%; }
  }
</style>
