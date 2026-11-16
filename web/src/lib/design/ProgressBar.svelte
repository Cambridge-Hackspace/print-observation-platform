<!--
  Progress, in layer lines (#48): the fill is striped like the layers of a
  print. It eases forward when the value changes, and jumps there at once
  for anyone who asked for reduced motion.
-->
<script lang="ts">
  import { Tween, prefersReducedMotion } from 'svelte/motion';
  import { cubicOut } from 'svelte/easing';
  import { clamp01 } from './geometry';

  let {
    value,
    label,
    color = 'var(--pop-sage)',
    layer = 'var(--pop-sage-layer)',
    height = 16,
  }: {
    /** How far along, 0 to 1. */
    value: number;
    /** What is progressing, for a screen reader: "Benchy on the X1". */
    label: string;
    color?: string;
    layer?: string;
    height?: number;
  } = $props();

  const target = $derived(clamp01(value));
  const shown = new Tween(0, { easing: cubicOut });
  $effect(() => {
    shown.set(target, { duration: prefersReducedMotion.current ? 0 : 600 });
  });
  const percent = $derived(Math.round(target * 100));
</script>

<div
  class="bar"
  style:height="{height}px"
  role="progressbar"
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={percent}
>
  <i
    style:width="{(shown.current * 100).toFixed(2)}%"
    style:background="repeating-linear-gradient(to bottom, {color} 0 3px, {layer} 3px 4px)"
  ></i>
</div>

<style>
  .bar {
    box-sizing: border-box;
    background: var(--pop-ground);
    border: 3px solid var(--pop-ink);
    border-radius: 3px;
    overflow: hidden;
  }
  i {
    display: block;
    height: 100%;
  }
</style>
