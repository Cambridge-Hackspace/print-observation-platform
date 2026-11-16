<!-- A fern of the forest scene (#48), for inside an <svg>. -->
<svelte:options namespace="svg" />

<script lang="ts">
  import { fernPath } from './geometry';

  let { x, b, h, color }: { x: number; b: number; h: number; color: string } = $props();
</script>

<path d={fernPath(x, b, h)} fill="none" stroke={color} stroke-width="3" stroke-linecap="square"></path>
