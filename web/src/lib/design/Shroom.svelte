<!-- A mushroom of the forest scene (#48), for inside an <svg>. -->
<svelte:options namespace="svg" />

<script lang="ts">
  import { shroom } from './geometry';

  let { x, b, s, cap }: { x: number; b: number; s: number; cap: string } = $props();
  const g = $derived(shroom(x, b, s));
</script>

<rect
  x={g.stem.x}
  y={g.stem.y}
  width={g.stem.width}
  height={g.stem.height}
  fill="#fef7e4"
  stroke="#121a14"
  stroke-width="3"
></rect>
<path d={g.cap} fill={cap} stroke="#121a14" stroke-width="3" stroke-linejoin="round"></path>
{#each g.spots as spot, i (i)}
  <rect x={spot.x} y={spot.y} width={spot.size} height={spot.size} fill="#fef7e4"></rect>
{/each}
