<!-- A pine of the forest scene (#48), for inside an <svg>. -->
<svelte:options namespace="svg" />

<script lang="ts">
  import { pinePath } from './geometry';

  let { x, b, h, fill, ink = false }: { x: number; b: number; h: number; fill: string; ink?: boolean } =
    $props();
</script>

{#if ink}
  <path d={pinePath(x, b, h)} {fill} stroke="#121a14" stroke-width="3"></path>
{:else}
  <path d={pinePath(x, b, h)} {fill}></path>
{/if}
