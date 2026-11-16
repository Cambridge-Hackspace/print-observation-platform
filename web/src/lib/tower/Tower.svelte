<script lang="ts">
  // The tower's home screen (#49), following the live stream.
  import { onDestroy } from 'svelte';
  import { follow, type Connection } from '../live';
  import { empty, reduce } from '../screen';
  import TowerView from './TowerView.svelte';

  let connection: Connection = $state('connecting');
  let screen = $state(empty());
  let now = $state(new Date());

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

<TowerView {screen} {connection} {now} />
