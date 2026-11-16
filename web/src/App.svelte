<script lang="ts">
  import { onDestroy } from 'svelte';
  import { follow, type Connection } from './lib/live';
  import { empty, reduce } from './lib/screen';

  // The connection, and what the server has said over it.
  let connection: Connection = $state('connecting');
  let screen = $state(empty());
  const status = $derived(screen.status);

  const label: Record<Connection, string> = {
    connecting: 'Connecting',
    live: 'Live',
    reconnecting: 'Reconnecting',
  };

  const stop = follow('/api/events', {
    connection: (c) => (connection = c),
    event: (e) => (screen = reduce(screen, e)),
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
