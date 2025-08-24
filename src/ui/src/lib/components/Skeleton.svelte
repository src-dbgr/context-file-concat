<script lang="ts">
  // Lightweight skeleton block with inline width/height
  type Props = {
    width?: string;
    height?: string;
    radius?: string;
    ariaLabel?: string;
  };
  let {
    width = "100%",
    height = "1rem",
    radius = "4px",
    ariaLabel = "Loading content",
  }: Props = $props();
</script>

<div
  class="cfc-skeleton"
  style={`width:${width}; height:${height}; border-radius:${radius};`}
  role="status"
  aria-label={ariaLabel}
  aria-live="polite"
></div>

<style>
  .cfc-skeleton {
    --shine: linear-gradient(
      90deg,
      rgba(0, 0, 0, 0) 0%,
      rgba(255, 255, 255, 0.25) 50%,
      rgba(0, 0, 0, 0) 100%
    );
    background:
      var(--shine), color-mix(in srgb, var(--color-border) 30%, transparent);
    background-size:
      200% 100%,
      100% 100%;
    animation: cfc-skeleton-shimmer 1.2s linear infinite;
  }
  @keyframes cfc-skeleton-shimmer {
    from {
      background-position:
        -100% 0,
        0 0;
    }
    to {
      background-position:
        100% 0,
        0 0;
    }
  }
</style>
