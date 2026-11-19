<script lang="ts">
  // The running POP's version, at the foot of every screen (#189): what an
  // admin reads off when asking whether an upgrade took, or reporting a
  // fault. Asked of the server, not built in, so it is the POP that answered
  // this page; when it cannot be asked, nothing is shown rather than a guess.
  import { onMount } from 'svelte';
  import { call } from '../api';
  import type { Status } from '../generated/Status';

  /** Without its own space around it, for a corner of a screen's frame. */
  let { compact = false }: { compact?: boolean } = $props();

  let version = $state<string | null>(null);

  onMount(async () => {
    const a = await call<Status>('GET', '/api/status');
    if (a.ok && typeof a.data.version === 'string') version = a.data.version;
  });
</script>

{#if version}
  <p class="version" class:compact>POP v{version}</p>
{/if}

<style>
  .version {
    margin: 0;
    padding-block: 12px;
    font-family: var(--pop-font);
    font-size: 12px;
    letter-spacing: 0.02em;
    color: var(--pop-dim);
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
  .compact {
    padding-block: 0;
    text-align: left;
  }
</style>
