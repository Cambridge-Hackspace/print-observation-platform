<!--
  A panel (#48): a surface on the forest ground. "paper" lifts it with the
  ink shadow round 6 gives what you act on; "flat" sits it down; "tile" is a
  smaller surface inside one.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    variant = 'paper',
    tag = 'section',
    children,
    ...rest
  }: {
    variant?: 'paper' | 'flat' | 'tile';
    tag?: 'section' | 'div' | 'article' | 'aside';
    children: Snippet;
    [k: string]: unknown;
  } = $props();
</script>

<svelte:element this={tag} class="panel {variant}" {...rest}>{@render children()}</svelte:element>

<style>
  .panel {
    background: var(--pop-panel);
    border-radius: var(--pop-radius);
  }
  .paper {
    box-shadow: var(--pop-shadow);
  }
  .tile {
    background: var(--pop-panel-2);
    border-radius: 4px;
  }
</style>
