<script lang="ts">
  // The tower's home screen (#49), following the live stream.
  import { onDestroy, onMount } from 'svelte';
  import { call } from '../api';
  import { Button } from '../design';
  import type { SessionInfo } from '../generated/SessionInfo';
  import { follow, type Connection } from '../live';
  import { empty, reduce } from '../screen';
  import TowerView from './TowerView.svelte';
  import HandView from './HandView.svelte';
  import { toldAbout } from './hand';
  import KioskCamera from './KioskCamera.svelte';
  import { startCheckins } from './checkin';
  import KioskSignIn from './KioskSignIn.svelte';
  import KioskSession from './KioskSession.svelte';

  let connection: Connection = $state('connecting');
  let screen = $state(empty());
  let now = $state(new Date());
  /** The printer whose "needs a hand" screen is open (#55), by id. */
  let open: string | null = $state(null);
  /** The printer whose camera is open, full screen (#58), by id. */
  let watching: string | null = $state(null);
  /** Who is signed in here (#12), and whether the sign-in screen is up. */
  let session: SessionInfo | null = $state(null);
  let signingIn = $state(false);

  // A reload keeps a session that is still going.
  onMount(async () => {
    const a = await call<SessionInfo>('GET', '/api/session');
    if (a.ok) session = a.data;
  });

  const stop = follow('/api/events', {
    connection: (c) => (connection = c),
    event: (e) => (screen = reduce(screen, e)),
  });
  const tick = setInterval(() => (now = new Date()), 15_000);
  // This screen vouches for the space's network, if it is an enrolled kiosk (#12).
  const stopCheckins = startCheckins();
  onDestroy(() => {
    stop();
    clearInterval(tick);
    stopCheckins();
  });
</script>

{#if signingIn}
  <KioskSignIn
    onsignedin={(s) => {
      session = s;
      signingIn = false;
    }}
    oncancel={() => (signingIn = false)}
  />
{:else if watching !== null}
  <KioskCamera printer={screen.printers.find((p) => p.id === watching)} onback={() => (watching = null)} />
{:else if open === null}
  <TowerView {screen} {connection} {now} onopen={(id) => (open = id)} oncamera={(id) => (watching = id)} />
{:else}
  <HandView
    printer={screen.printers.find((p) => p.id === open)}
    told={toldAbout(screen.told, open)}
    onback={() => (open = null)}
  />
{/if}

{#if session !== null && !signingIn}
  <KioskSession {session} onsignedout={() => (session = null)} />
{:else if !signingIn}
  <div class="signin"><Button size="md" onclick={() => (signingIn = true)}>Sign in</Button></div>
{/if}

<style>
  .signin {
    position: fixed;
    top: calc(env(safe-area-inset-top, 0px) + 16px);
    right: 16px;
    z-index: 10;
  }
</style>
