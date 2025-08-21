<!-- src/lib/components/LogoMark.svelte (Svelte 5 Runes) -->
<script lang="ts">
  import { onMount, onDestroy } from "svelte";

  type LogoEffect = "none" | "bolt";

  type Props = {
    size?: number;
    ariaLabel?: string;
    ariaHidden?: boolean;
    effect?: LogoEffect;
    color?: string;
    randomizeDelay?: boolean;
    strikeMin?: number;
    strikeMax?: number;
    intensity?: number;   // 0..1
    flashColor?: string;
    startWithStrike?: boolean;
  };

  let {
    size = 16,
    ariaLabel = "CFC",
    ariaHidden = false,
    effect = "bolt",
    color,
    randomizeDelay = false,
    strikeMin = 3.5,
    strikeMax = 9,
    intensity = 0.8,
    flashColor = "#ffe066",
    startWithStrike = true
  }: Props = $props();

  let delay = $state("0s");
  let striking = $state(false);
  let scheduleId: number | null = null;
  let clearStrikeId: number | null = null;

  // Clamp intensity -> CSS var
  const amp = $derived(Math.max(0, Math.min(1, intensity)));

  function scheduleNext() {
    const min = Math.max(0.8, strikeMin);
    const max = Math.max(min + 0.4, strikeMax);
    const ms = (Math.random() * (max - min) + min) * 1000;

    scheduleId = window.setTimeout(() => {
      triggerStrike();
      scheduleNext();
    }, ms);
  }

  function triggerStrike() {
    // toggle to restart CSS animations
    striking = false;
    requestAnimationFrame(() => {
      striking = true;
      clearStrikeId = window.setTimeout(() => (striking = false), 900);
    });
  }

  onMount(() => {
    if (randomizeDelay) {
      const n = (Math.random() * 2.0 + 0.2).toFixed(2);
      delay = `-${n}s`;
    }

    const m = window.matchMedia("(prefers-reduced-motion: reduce)");
    if (!m.matches && effect === "bolt") {
      if (startWithStrike) triggerStrike();
      scheduleNext();
    }
  });

  onDestroy(() => {
    if (scheduleId) clearTimeout(scheduleId);
    if (clearStrikeId) clearTimeout(clearStrikeId);
  });

  // Original bolt path
  const D =
    "M 0.973 23.982 L 12.582 13.522 L 16.103 13.434 L 18.889 8.027 L 11.321 8.07 L 12.625 5.577 L 20.237 5.496 L 23.027 0.018 L 9.144 0.02 L 2.241 13.408 L 6.333 13.561 L 0.973 23.982 Z";
</script>

<span
  class="cfc-logo"
  class:bolt={effect === "bolt"}
  class:is-striking={striking}
  role={ariaHidden ? undefined : "img"}
  aria-label={ariaHidden ? undefined : ariaLabel}
  aria-hidden={ariaHidden}
  style={`--_s:${size}px;${color ? `--c:${color};` : ""}--delay:${delay};--flash:${flashColor};--amp:${amp};`}
>
  <svg viewBox="0 0 24 24" class="svg" aria-hidden="true" focusable="false">
    <defs>
      <filter id="boltJagged" x="-50%" y="-50%" width="200%" height="200%">
        <feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="1" seed="7" result="n"/>
        <feDisplacementMap in="SourceGraphic" in2="n" scale="1.6" xChannelSelector="R" yChannelSelector="G"/>
      </filter>
      <filter id="boltBloom" x="-50%" y="-50%" width="200%" height="200%">
        <feGaussianBlur stdDeviation="0.6" result="b1"/>
        <feGaussianBlur stdDeviation="1.8" result="b2"/>
        <feMerge><feMergeNode in="b2"/><feMergeNode in="b1"/><feMergeNode in="SourceGraphic"/></feMerge>
      </filter>
    </defs>

    <path class="bolt-fill" d={D}></path>
    <path class="bolt-stroke" d={D}></path>
    <path class="bolt-flash" d={D}></path>

    <g class="strike-flash">
      <circle class="flash-ring" cx="12" cy="12" r="12" />
    </g>
  </svg>
</span>

<style>
  .cfc-logo {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--_s);
    height: var(--_s);
    color: var(--c, currentColor);
  }
  .svg { width: 100%; height: 100%; }

  .bolt-fill   { fill: currentColor; transition: fill 120ms ease; }
  .bolt-stroke { fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; stroke-linejoin: round; opacity: 0.85; }
  .bolt-flash  { fill: var(--flash, #ffe066); opacity: 0; }

  .cfc-logo.bolt .bolt-stroke {
    stroke-dasharray: 3 7;
    animation: cfc-dash 2.4s linear infinite, cfc-glow 2.2s ease-in-out infinite;
    animation-delay: var(--delay);
    filter: drop-shadow(0 0 1px currentColor) drop-shadow(0 0 4px currentColor);
  }
  @keyframes cfc-dash { from { stroke-dashoffset: 0; } to { stroke-dashoffset: -22; } }
  @keyframes cfc-glow { 0%,100% { opacity:.85; } 50% { opacity:1; } }

  .cfc-logo.is-striking .svg {
    animation: cfc-strike-strobe 0.9s cubic-bezier(.2,.75,.2,1) both, cfc-strike-shake 0.24s steps(2,end) 3;
    filter: url(#boltBloom);
    transform-origin: 50% 50%;
  }
  .cfc-logo.is-striking .bolt-fill,
  .cfc-logo.is-striking .bolt-stroke,
  .cfc-logo.is-striking .bolt-flash { filter: url(#boltJagged); }

  .cfc-logo.is-striking .bolt-stroke {
    stroke-dasharray: none;
    animation: none;
    stroke-width: calc(1.5px + .7px * var(--amp));
    opacity: 1;
    filter: drop-shadow(0 0 calc(1px + 1px * var(--amp)) currentColor) drop-shadow(0 0 calc(6px + 10px * var(--amp)) currentColor);
  }

  .cfc-logo.is-striking .bolt-fill { fill: #fff; }
  .cfc-logo.is-striking .bolt-flash { animation: cfc-flash-pulses 0.9s linear both; }

  .flash-ring {
    fill: none; stroke: var(--flash); stroke-width: calc(.6px + .6px * var(--amp));
    opacity: 0; transform-origin: 12px 12px; transform: scale(0.1);
  }
  .cfc-logo.is-striking .flash-ring { animation: cfc-ring 0.6s ease-out both; }

  @keyframes cfc-strike-strobe { 0%{filter:brightness(1.1)} 3%{filter:brightness(1.7)} 6%{filter:brightness(1.0)} 14%{filter:brightness(1.9)} 20%{filter:brightness(1.1)} 32%{filter:brightness(2.0)} 38%{filter:brightness(1.1)} 52%{filter:brightness(2.0)} 68%{filter:brightness(1.05)} 100%{filter:brightness(1.0)} }
  @keyframes cfc-flash-pulses { 0%,100%{opacity:0} 3%{opacity:1} 7%{opacity:.2} 14%{opacity:1} 20%{opacity:.15} 32%{opacity:1} 38%{opacity:.12} 60%{opacity:0} }
  @keyframes cfc-ring { 0%{transform:scale(0.15);opacity:0} 8%{opacity:1} 55%{transform:scale(1.18);opacity:.25} 100%{transform:scale(1.6);opacity:0} }

  .cfc-logo::after {
    content: ""; position: absolute; inset: -20%; pointer-events: none; opacity: 0;
    background:
      radial-gradient(closest-side, rgba(255,255,255,.95), rgba(255,255,255,0) 70%),
      radial-gradient(closest-side, color-mix(in oklab, var(--flash) 60%, transparent), transparent 75%);
    mix-blend-mode: screen; filter: blur(calc(.3px + .5px * var(--amp))); transition: opacity 120ms ease;
  }
  .cfc-logo.is-striking::after { animation: cfc-whiteout 0.38s ease-out both; }
  @keyframes cfc-whiteout { 0%{opacity:0} 6%{opacity:.95} 24%{opacity:.35} 100%{opacity:0} }

  @media (prefers-reduced-motion: reduce) {
    .cfc-logo.bolt .bolt-stroke,
    .cfc-logo .svg,
    .cfc-logo .bolt-flash,
    .cfc-logo::after {
      animation: none !important;
      filter: none !important;
      opacity: 1 !important;
      transform: none !important;
    }
  }
</style>
