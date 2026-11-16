<script lang="ts">
  // The tower's home screen (#49), following the live stream.
  import { onDestroy } from 'svelte';
  import { follow, type Connection } from '../live';
  import { empty, reduce } from '../screen';
  import TowerView from './TowerView.svelte';
  import HandView from './HandView.svelte';
  import { toldAbout } from './hand';

  let connection: Connection = $state('connecting');
  let screen = $state(empty());
  let now = $state(new Date());
  /** The printer whose "needs a hand" screen is open (#55), by id. */
  let open: string | null = $state(null);

  const stop = follow('/api/events', {
    connection: (c) => (connection = c),
    event: (e) => (screen = reduce(screen, e)),
  });
  const tick = setInterval(() => (now = new Date()), 15_000);
  onDestroy(() => {
    stop();
    clearInterval(tick);
  });
</script>

{#if open === null}
  <TowerView {screen} {connection} {now} onopen={(id) => (open = id)} />
{:else}
  <HandView
    printer={screen.printers.find((p) => p.id === open)}
    told={toldAbout(screen.told, open)}
    onback={() => (open = null)}
  />
{/if}
