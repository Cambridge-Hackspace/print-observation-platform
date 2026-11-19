<script lang="ts">
  // Your slicer keys (#13): make one for each slicer you send prints from,
  // see when each was last used, and revoke one you no longer use. A key is
  // shown once, when it is made; POP keeps only its hash.
  import { onMount } from 'svelte';
  import { call } from '../api';
  import { ago } from '../admin/kiosks';
  import { sensitive } from '../feedback/sensitive';
  import type { ApiKey } from '../generated/ApiKey';
  import type { CreatedApiKey } from '../generated/CreatedApiKey';

  let {
    /** Where a slicer reaches POP: its public address, or this page's. */
    address,
  }: { address: string } = $props();

  let keys = $state<ApiKey[] | null>(null);
  let name = $state('');
  let made = $state<CreatedApiKey | null>(null);
  let copied = $state(false);
  let problem = $state<string | null>(null);
  let busy = $state(false);
  /** The key asked to be revoked, waiting to be confirmed. */
  let revoking = $state<number | null>(null);
  let now = $state(Date.now());

  const live = $derived((keys ?? []).filter((k) => k.revoked_at === null));
  const revoked = $derived((keys ?? []).filter((k) => k.revoked_at !== null));

  async function load() {
    const a = await call<ApiKey[]>('GET', '/api/keys');
    now = Date.now();
    if (a.ok) keys = a.data;
    else problem = a.error;
  }

  onMount(load);

  async function make(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    problem = null;
    const a = await call<CreatedApiKey>('POST', '/api/keys', { name: name.trim() });
    busy = false;
    if (!a.ok) {
      problem = a.error;
      return;
    }
    made = a.data;
    copied = false;
    name = '';
    await load();
  }

  async function copy() {
    if (!made) return;
    try {
      await navigator.clipboard.writeText(made.secret);
      copied = true;
    } catch {
      problem = 'Couldn’t copy it: select the key and copy it by hand.';
    }
  }

  async function revoke(id: number) {
    busy = true;
    problem = null;
    const a = await call<unknown>('DELETE', `/api/keys/${id}`);
    busy = false;
    revoking = null;
    if (!a.ok) problem = a.error;
    if (made?.key.id === id) made = null;
    await load();
  }
</script>

<section class="keys" aria-labelledby="keys" data-testid="keys">
  <h2 id="keys">Your slicer keys</h2>
  <p class="muted">
    A key lets your slicer send prints to POP as you. It can only send files and see your own prints; it can never start or
    stop a printer. Make one for each slicer you use.
  </p>

  {#if made}
    <div class="made" role="status" data-testid="made">
      <p><strong>Your key for “{made.key.name}”.</strong> Copy it now: POP won’t show it again.</p>
      <code data-testid="secret" use:sensitive={'secret'}>{made.secret}</code>
      <div class="row">
        <button type="button" class="action" onclick={copy}>{copied ? 'Copied' : 'Copy'}</button>
        <button type="button" class="action" onclick={() => (made = null)}>Done</button>
      </div>
      <p class="muted">
        In your slicer, add a physical printer of the type <strong>OctoPrint</strong>, at <strong>{address}</strong>, and paste
        this as its API key.
      </p>
    </div>
  {/if}

  <form class="row" onsubmit={make}>
    <label for="key-name" class="visually-hidden">What the key is for</label>
    <input id="key-name" bind:value={name} placeholder="PrusaSlicer on my laptop" maxlength="64" required />
    <button type="submit" class="action" disabled={busy || name.trim() === ''}>Make a key</button>
  </form>
  {#if problem}<p class="problem" role="alert">{problem}</p>{/if}

  {#if keys === null}
    <p class="muted" role="status">Loading…</p>
  {:else if live.length === 0}
    <p class="muted">You have no keys yet.</p>
  {:else}
    <ul class="list">
      {#each live as k (k.id)}
        <li data-testid="key">
          <span class="kname">{k.name}</span>
          <span class="muted"
            ><code>{k.prefix}…</code> · {k.last_used_at === null ? 'never used' : `used ${ago(k.last_used_at, now)}`}</span
          >
          {#if revoking === k.id}
            <span class="row">
              <button type="button" class="action danger" disabled={busy} onclick={() => revoke(k.id)}>Revoke it</button>
              <button type="button" class="action" onclick={() => (revoking = null)}>Keep it</button>
            </span>
          {:else}
            <button type="button" class="link" aria-label="Revoke the key {k.name}" onclick={() => (revoking = k.id)}>Revoke</button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if revoked.length > 0}
    <details>
      <summary>Revoked keys ({revoked.length})</summary>
      <ul class="list">
        {#each revoked as k (k.id)}
          <li class="muted"><span>{k.name}</span> <code>{k.prefix}…</code> · revoked {ago(k.revoked_at!, now)}</li>
        {/each}
      </ul>
    </details>
  {/if}
</section>

<style>
  .keys {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 12px;
  }
  .keys h2 {
    margin: 0;
    font-size: 19px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  input {
    flex: 1 1 14rem;
    min-width: 0;
    font: inherit;
    font-size: 17px;
    padding: 10px;
    color: var(--pop-text);
    background: var(--pop-ground);
    border: 2px solid var(--pop-line);
    border-radius: var(--pop-radius);
  }
  .action {
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
  .action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .danger {
    color: var(--pop-ink);
    background: var(--pop-coral);
    border-color: var(--pop-ink);
  }
  .link {
    font: inherit;
    color: var(--pop-butter);
    background: none;
    border: 0;
    padding: 0;
    text-decoration: underline;
    cursor: pointer;
    justify-self: start;
  }
  .made {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px;
    background: var(--pop-panel);
    border: 3px solid var(--pop-butter);
    border-radius: var(--pop-radius);
  }
  .made p {
    margin: 0;
  }
  code {
    font-family: ui-monospace, 'SFMono-Regular', Menlo, monospace;
    overflow-wrap: anywhere;
  }
  .made code {
    padding: 8px;
    background: var(--pop-ground);
    border-radius: 4px;
    user-select: all;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .list li {
    display: grid;
    gap: 4px;
    padding: 10px 12px;
    background: var(--pop-panel);
    border-radius: var(--pop-radius);
  }
  .kname {
    font-weight: 500;
  }
  .problem {
    margin: 0;
    color: var(--pop-coral);
  }
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
