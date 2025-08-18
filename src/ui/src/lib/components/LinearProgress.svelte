<script lang="ts">
  /**
   * Accessible linear progress bar.
   *
   * Examples:
   *  - Determinate:    <LinearProgress value={42} ariaLabel="Scanning" />
   *  - Indeterminate:  <LinearProgress indeterminate ariaLabel="Working..." />
   *  - Legacy external control (IPC): keep idForFill to let external code set width.
   *      <LinearProgress idForFill="scan-progress-fill" indeterminate />
   */

  /** Screen-reader label */
  export let ariaLabel: string = "Progress";

  /** 0..100 for determinate mode; leave undefined for indeterminate */
  export let value: number | undefined = undefined;

  /** Show animated indeterminate bar */
  export let indeterminate: boolean = false;

  /**
   * Optional id on the inner fill element (for legacy external width updates).
   * E.g. document.getElementById(id)?.style.width = "37%".
   */
  export let idForFill: string | undefined = undefined;

  const clamp100 = (n: number) => Math.max(0, Math.min(100, n));

  // Derived aria-valuenow (only in determinate mode)
  let valueNow: number | undefined;
  $: valueNow = !indeterminate && typeof value === "number" ? clamp100(value) : undefined;

  // Inline width style for determinate mode
  let widthStyle: string | undefined;
  $: widthStyle = valueNow !== undefined ? `width:${valueNow}%` : undefined;
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
  <div
    class="scan-progress-fill"
    id={idForFill}
    style={widthStyle}
  ></div>
</div>

<style>
  /* Minimal fallback visuals for indeterminate mode.
     Your global CSS already styles .scan-progress-bar/.scan-progress-fill.
     This ensures a nice animation even if no external width is applied. */
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
