<script lang="ts">
  // The phone app's shell (#60): the space's printers, live; the guides; and
  // who you are signed in as. Installable as a web app (index.html's
  // manifest). Watching a print (#61), being told when it is done (#62) and
  // reporting a problem (#63) arrive as screens inside this shell.
  import { sensitive } from '../feedback/sensitive';
  import { onDestroy, onMount } from 'svelte';
  import { Logo, ProgressBar, StatusSquare, Version } from '../design';
  import { call } from '../api';
  import { follow, type Connection } from '../live';
  import { empty, reduce } from '../screen';
  import { byNeed, card, need, type Need } from '../tower/need';
  import type { User } from '../generated/User';
  import SignIn from '../admin/SignIn.svelte';
  import { tabFrom, watchHash, watching, type Tab } from './tabs';
  import Watch from './Watch.svelte';

  let connection: Connection = $state('connecting');
  let screen = $state(empty());
  let now = $state(new Date());
  let me: User | null = $state(null);
  let signingIn = $state(false);
  let tab: Tab = $state(tabFrom(location.hash));
  let watched: string | null = $state(watching(location.hash));

  const printers = $derived(byNeed(screen.printers));
  const COLOR: Record<Need, string> = {
    needs_hand: 'var(--pop-coral)',
    printing: 'var(--pop-sage)',
    done: 'var(--pop-butter)',
    free: 'var(--pop-text)',
    offline: 'var(--pop-dim)',
  };
  const LIVE: Record<Connection, string> = { connecting: 'Connecting', live: 'Live', reconnecting: 'Reconnecting' };

  const stop = follow('/api/events', {
    connection: (c) => (connection = c),
    event: (e) => (screen = reduce(screen, e)),
  });
  const tick = setInterval(() => (now = new Date()), 30_000);
  const onHash = () => {
    tab = tabFrom(location.hash);
    watched = watching(location.hash);
  };
  onMount(async () => {
    window.addEventListener('hashchange', onHash);
    const a = await call<User>('GET', '/api/me');
    if (a.ok) me = a.data;
  });
  onDestroy(() => {
    stop();
    clearInterval(tick);
    window.removeEventListener('hashchange', onHash);
  });

  async function signOut() {
    await call('DELETE', '/api/session');
    me = null;
  }
</script>

{#if signingIn}
  <SignIn
    subtitle="Sign in"
    onsignedin={(u) => {
      me = u;
      signingIn = false;
    }}
    oncancel={() => (signingIn = false)}
  />
{:else}
  <div class="phone" data-testid="phone">
    <header>
      <div class="brand"><Logo size={34} /><span class="name">POP</span></div>
      {#if me}
        <a class="who" href="#you" use:sensitive={'name'}>{me.name}</a>
      {:else}
        <button type="button" class="link" onclick={() => (signingIn = true)}>Sign in</button>
      {/if}
    </header>

    {#if connection !== 'live'}
      <p class="stale" role="status">{connection === 'connecting' ? 'Connecting…' : 'Reconnecting: this may be out of date'}</p>
    {/if}

    <main>
      {#if watched !== null}
        <Watch
          printer={screen.printers.find((p) => p.id === watched) ?? null}
          {now}
          signedIn={me !== null}
          onsignin={() => (signingIn = true)}
        />
      {:else if tab === 'printers'}
        <h1>Printers</h1>
        <ul class="list" data-testid="phone-printers">
          {#each printers as p (p.id)}
            {@const n = need(p)}
            {@const w = card(p, now)}
            <li class="printer" data-need={n}>
              <div class="row">
                <a class="pname" href={watchHash(p.id)}>{p.name}</a>
                <span style:color={COLOR[n]}><StatusSquare color={COLOR[n]} label={w.state} /></span>
              </div>
              {#if w.job}<span class="job" use:sensitive={'file'}>{w.job}</span>{/if}
              {#if (n === 'printing' || n === 'needs_hand') && p.state.progress !== null}
                <ProgressBar
                  value={p.state.progress}
                  label="{p.name}: {w.left}"
                  height={16}
                  color={n === 'needs_hand' ? 'var(--pop-coral)' : 'var(--pop-sage)'}
                  layer={n === 'needs_hand' ? '#e3957a' : 'var(--pop-sage-layer)'}
                />
              {/if}
              <span class="muted small">{[w.left, w.right].filter(Boolean).join(' · ')}</span>
            </li>
          {:else}
            <li class="muted">No printers yet: an admin adds them from BAM!.</li>
          {/each}
        </ul>
      {:else}
        <h1>You</h1>
        {#if me}
          <p>Signed in as <strong use:sensitive={'name'}>{me.name}</strong> (<span use:sensitive={'name'}>{me.username}</span>).</p>
          {#if me.role === 'admin'}<p><a href="/admin">Open the admin desk</a></p>{/if}
          <button type="button" class="action" onclick={signOut}>Sign out</button>
        {:else}
          <p class="muted">Sign in to send prints and hear when they’re done.</p>
          <button type="button" class="action" onclick={() => (signingIn = true)}>Sign in</button>
        {/if}
        <section class="about" aria-labelledby="about">
          <h2 id="about">About this POP</h2>
          <dl>
            <dt>Connection</dt>
            <dd data-testid="connection">{LIVE[connection]}</dd>
            {#if screen.status}
              <dt>Version</dt>
              <dd data-testid="version">{screen.status.version}</dd>
              <dt>Database schema</dt>
              <dd data-testid="schema">{screen.status.schema_version}</dd>
            {/if}
          </dl>
        </section>
      {/if}
      <Version />
    </main>

    <nav aria-label="Sections">
      <a href="#printers" aria-current={tab === 'printers' ? 'page' : undefined}>Printers</a>
      <a href="/guides">Guides</a>
      <a href="#you" aria-current={tab === 'you' ? 'page' : undefined}>You</a>
    </nav>
  </div>
{/if}

<style>
  a.pname {
    color: inherit;
    text-decoration: none;
  }
  .phone {
    box-sizing: border-box;
    min-height: 100vh;
    max-width: 40rem;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: calc(16px + env(safe-area-inset-top, 0px)) 16px calc(88px + env(safe-area-inset-bottom, 0px));
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .name {
    font-size: 19px;
    font-weight: 600;
  }
  a {
    color: var(--pop-butter);
  }
  .who,
  .link {
    font: inherit;
    font-size: 15px;
    color: var(--pop-muted);
    background: none;
    border: 0;
    padding: 8px 0;
    cursor: pointer;
  }
  .stale {
    margin: 0;
    padding: 8px 12px;
    background: var(--pop-butter);
    color: var(--pop-ink);
    border-radius: 4px;
    font-size: 15px;
  }
  main {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 600;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 18px;
    font-weight: 600;
  }
  .muted {
    color: var(--pop-muted);
  }
  .small {
    font-size: 14px;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .printer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px 16px;
    background: var(--pop-panel);
    border-radius: var(--pop-radius);
  }
  .printer[data-need='needs_hand'] {
    border: 3px solid var(--pop-ink);
    box-shadow: var(--pop-shadow);
  }
  .row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .pname {
    font-size: 18px;
    font-weight: 600;
  }
  .job {
    font-size: 15px;
  }
  .action {
    align-self: flex-start;
    font: inherit;
    font-size: 17px;
    font-weight: 500;
    height: 48px;
    padding: 0 20px;
    color: var(--pop-text);
    background: var(--pop-panel-2);
    border: 2px solid var(--pop-line);
    border-radius: var(--pop-radius);
    cursor: pointer;
  }
  .about {
    margin-top: 12px;
    padding: 14px 16px;
    background: var(--pop-panel);
    border-radius: var(--pop-radius);
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px 16px;
    margin: 0;
  }
  dd {
    margin: 0;
  }
  nav {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    justify-content: center;
    gap: 4px;
    padding: 8px 16px calc(8px + env(safe-area-inset-bottom, 0px));
    background: var(--pop-panel);
    border-top: 3px solid var(--pop-ink);
  }
  nav a {
    flex: 1;
    max-width: 12rem;
    text-align: center;
    padding: 12px 8px;
    color: var(--pop-muted);
    text-decoration: none;
    font-size: 16px;
    border-radius: 4px;
  }
  nav a[aria-current='page'] {
    color: var(--pop-ink);
    background: var(--pop-butter);
    font-weight: 600;
  }
  a:focus-visible,
  button:focus-visible {
    outline: 3px solid var(--pop-cream);
    outline-offset: 2px;
  }
</style>
