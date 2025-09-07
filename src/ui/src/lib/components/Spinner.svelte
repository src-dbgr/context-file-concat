<script lang="ts">
  // Runes props
  type Props = {
    size?: number;
    ariaLabel?: string;
    ariaHidden?: boolean;
  };
  let {
    size = 16,
    ariaLabel = "Loading",
    ariaHidden = false,
  }: Props = $props();
</script>

<span
  class="cfc-spinner"
  role={ariaHidden ? undefined : "status"}
  aria-live={ariaHidden ? undefined : "polite"}
  aria-label={ariaHidden ? undefined : ariaLabel}
  style={`--_s:${size}px`}
>
  <svg
    viewBox="0 0 50 50"
    class="cfc-spinner-svg"
    aria-hidden="true"
    focusable="false"
  >
    <circle class="track" cx="25" cy="25" r="20" fill="none" stroke-width="5" />
    <circle
      class="arc"
      cx="25"
      cy="25"
      r="20"
      fill="none"
      stroke-width="5"
      stroke-linecap="round"
    />
  </svg>
</span>

<style>
  .cfc-spinner {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--_s);
    height: var(--_s);
  }
  .cfc-spinner-svg {
    width: 100%;
    height: 100%;
  }
  .track {
    stroke: color-mix(in srgb, var(--color-border) 60%, transparent);
    opacity: 0.6;
  }
  .arc {
    stroke: var(--color-accent);
    transform-origin: 50% 50%;
    animation:
      cfc-spin 1s linear infinite,
      cfc-sweep 1.25s ease-in-out infinite;
    stroke-dasharray: 1, 150;
    stroke-dashoffset: 0;
  }

  @keyframes cfc-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @keyframes cfc-sweep {
    0% {
      stroke-dasharray: 1, 150;
      stroke-dashoffset: 0;
    }
    50% {
      stroke-dasharray: 90, 150;
      stroke-dashoffset: -35;
    }
    100% {
      stroke-dasharray: 90, 150;
      stroke-dashoffset: -124;
    }
  }
</style>
