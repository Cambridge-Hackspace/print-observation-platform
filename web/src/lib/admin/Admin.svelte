<script lang="ts">
  // The admin desk (#86): who is signed in, the printers from BAM!, and the
  // tower's live state beside them. A member who is not an admin is told so;
  // nobody signed in is asked to.
  import { sensitive } from '../feedback/sensitive';
  import { onDestroy, onMount } from 'svelte';
  import { call } from '../api';
  import { follow } from '../live';
  import { empty, reduce } from '../screen';
  import type { AdminPrinter } from '../generated/AdminPrinter';
  import type { User } from '../generated/User';
  import DeskView from './DeskView.svelte';
  import SignIn from './SignIn.svelte';

  let me: User | null = $state(null);
  let checked = $state(false);
  let printers: AdminPrinter[] | null = $state(null);
  let problem: string | null = $state(null);
  let busy: string | null = $state(null);
  let screen = $state(empty());

  async function load() {
    const a = await call<AdminPrinter[]>('GET', '/api/admin/printers');
    if (a.ok) {
      printers = a.data;
    } else {
      printers = printers ?? [];
      problem = a.error;
    }
  }

  async function signedIn(u: User) {
    me = u;
    if (u.role === 'admin') await load();
  }

  async function act(id: string, what: 'add' | 'decline') {
    busy = id;
    problem = null;
    const a = await call('POST', `/api/admin/printers/${encodeURIComponent(id)}/${what}`);
    busy = null;
    if (!a.ok) problem = a.error;
    await load();
  }

  /** Send a secret for a printer; true once POP has it. */
  async function secret(method: 'PUT' | 'DELETE', id: string, what: string, body?: unknown): Promise<boolean> {
    problem = null;
    const a = await call(method, `/api/admin/printers/${encodeURIComponent(id)}/${what}`, body);
    if (!a.ok) problem = a.error;
    await load();
    return a.ok;
  }

  async function signOut() {
    await call('DELETE', '/api/session');
    me = null;
    printers = null;
  }

  onMount(async () => {
    const a = await call<User>('GET', '/api/me');
    checked = true;
    if (a.ok) await signedIn(a.data);
  });
  const stop = follow('/api/events', { connection: () => {}, event: (e) => (screen = reduce(screen, e)) });
  onDestroy(stop);
</script>

{#if !checked}
  <p class="wait" role="status">Loading…</p>
{:else if me === null}
  <SignIn onsignedin={signedIn} />
{:else if me.role !== 'admin'}
  <main class="wait">
    <p>This is for printer admins, and <span use:sensitive={'name'}>{me.name}</span> isn’t one. An admin can change that.</p>
    <button type="button" onclick={signOut}>Sign out</button>
  </main>
{:else}
  <DeskView
    {me}
    {printers}
    live={screen.printers}
    told={screen.told}
    {problem}
    {busy}
    onadd={(id) => act(id, 'add')}
    ondecline={(id) => act(id, 'decline')}
    onaccesscode={(id, code) => secret('PUT', id, 'access-code', { access_code: code })}
    oncameralogin={(id, username, password) => secret('PUT', id, 'camera-login', { username, password })}
    onclearlogin={(id) => secret('DELETE', id, 'camera-login')}
    onsignout={signOut}
  />
{/if}

<style>
  .wait {
    min-height: 100vh;
    margin: 0;
    padding: 48px 16px;
    box-sizing: border-box;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
  }
</style>
