<script lang="ts">
  import { onDestroy } from 'svelte';
  import { follow, type Connection } from './lib/live';
  import { empty, reduce } from './lib/screen';
  import { Logo, Panel, StatusSquare } from './lib/design';

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
  <header>
    <Logo />
    <h1>POP</h1>
  </header>
  <Panel>
    <div class="body">
      <span data-testid="connection">
        <StatusSquare
          color={connection === 'live' ? 'var(--pop-moss)' : 'var(--pop-butter)'}
          label={label[connection]}
        />
      </span>
      {#if status}
        <dl>
          <dt>Version</dt>
          <dd data-testid="version">{status.version}</dd>
          <dt>Database schema</dt>
          <dd data-testid="schema">{status.schema_version}</dd>
        </dl>
      {/if}
    </div>
  </Panel>
</main>

<style>
  main {
    max-width: 40rem;
    margin: 0 auto;
    padding: 2rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  header {
    display: flex;
    align-items: center;
    gap: 16px;
  }
  h1 {
    font-size: 32px;
    font-weight: 600;
  }
  .body {
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.25rem 1rem;
    margin: 0;
  }
  dt {
    color: var(--pop-muted);
  }
  dd {
    margin: 0;
  }
</style>
