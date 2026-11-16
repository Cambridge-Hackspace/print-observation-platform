<script lang="ts">
  import { onDestroy } from 'svelte';
  import { follow, type Connection } from './lib/live';
  import type { Status } from './lib/generated/Status';

  // The connection, and the newest status the server has sent over it.
  let connection: Connection = $state('connecting');
  let status: Status | null = $state(null);

  const label: Record<Connection, string> = {
    connecting: 'Connecting',
    live: 'Live',
    reconnecting: 'Reconnecting',
  };

  const stop = follow('/api/events', {
    connection: (c) => (connection = c),
    event: (e) => {
      if (e.type === 'status') status = e.data;
    },
  });
  onDestroy(stop);
</script>

<main>
  <h1>POP</h1>
  <p class="connection" class:live={connection === 'live'} data-testid="connection">
    {label[connection]}
  </p>
  {#if status}
    <dl>
      <dt>Version</dt>
      <dd data-testid="version">{status.version}</dd>
      <dt>Database schema</dt>
      <dd data-testid="schema">{status.schema_version}</dd>
    </dl>
  {/if}
</main>

<style>
  main {
    max-width: 40rem;
    margin: 0 auto;
    padding: 2rem 1rem;
  }
  .connection {
    display: inline-block;
    padding: 0.25rem 0.75rem;
    border-radius: 999px;
    color: white;
    background: var(--waiting);
  }
  .connection.live {
    background: var(--live);
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.25rem 1rem;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
  }
</style>
