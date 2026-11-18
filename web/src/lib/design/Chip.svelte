<!-- A chip (#48): a small label, "PLA", "Needs a hand", "Yours". -->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let { children }: { children: Snippet } = $props();
</script>

<span class="chip">{@render children()}</span>

<style>
  .chip {
    font-size: 14px;
    padding: 4px 9px;
    border-radius: 3px;
    background: var(--pop-panel-2);
    white-space: nowrap;
  }
</style>
