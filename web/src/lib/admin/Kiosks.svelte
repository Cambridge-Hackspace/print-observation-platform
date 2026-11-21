<script lang="ts">
  // Kiosks (#12): the print stations. An admin makes THIS screen a kiosk, on
  // the kiosk itself; the kiosk then checks in every half minute, and the
  // space's network counts as the space while it does. Revoking one stops its
  // word at once.
  import { onMount } from 'svelte';
  import { sensitive } from '../feedback/sensitive';
  import { Button } from '../design';
  import { call } from '../api';
  import type { Kiosk } from '../generated/Kiosk';
  import { ago } from './kiosks';

  let kiosks: Kiosk[] | null = $state(null);
  let problem: string | null = $state(null);
  let said: string | null = $state(null);
  let name = $state('The tower');
  let now = $state(Date.now());

  async function load() {
    const a = await call<Kiosk[]>('GET', '/api/admin/kiosks');
    if (a.ok) kiosks = a.data;
    else problem = a.error;
    now = Date.now();
  }
  onMount(load);

  async function enroll(e: SubmitEvent) {
    e.preventDefault();
    problem = null;
    said = null;
    const a = await call('POST', '/api/admin/kiosks', { name: name.trim() });
    if (a.ok) said = `This screen is now the kiosk “${name.trim()}”. Open /kiosk on it.`;
    else problem = a.error;
    await load();
  }

  /** What each kiosk's card reader field says, by kiosk, while edited (#19). */
  let readers: Record<number, string> = $state({});

  /** Which CS:S resource this kiosk's card reader is (#19): a tap there signs in here. */
  async function setReader(k: Kiosk) {
    problem = null;
    said = null;
    const resource = (readers[k.id] ?? k.card_resource ?? '').trim();
    const a = await call('PUT', `/api/admin/kiosks/${k.id}/card-reader`, { resource });
    if (a.ok) said = resource ? `A card tap at ${resource} now signs in at ${k.name}.` : `${k.name} has no card reader now.`;
    else problem = a.error;
    delete readers[k.id];
    await load();
  }

  async function revoke(k: Kiosk) {
    problem = null;
    said = null;
    const a = await call('DELETE', `/api/admin/kiosks/${k.id}`);
    if (a.ok) said = `${k.name} is no longer a kiosk.`;
    else problem = a.error;
    await load();
  }
</script>

<section class="kiosks" data-testid="kiosks">
  <div class="head"><h2>Kiosks</h2><span class="muted">The print stations. The space’s network counts as the space while one checks in from it.</span></div>
  {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
  {#if said}<p class="said" role="status" use:sensitive={'name'}>{said}</p>{/if}
  {#if kiosks === null}
    <p class="muted" role="status">Loading…</p>
  {:else if kiosks.length === 0}
    <p class="muted">No kiosk yet. Until one checks in, nothing counts as the space: signed out, nobody sees names or cameras.</p>
  {:else}
    <ul>
      {#each kiosks as k (k.id)}
        <li data-testid="kiosk" class:revoked={k.revoked}>
          <span class="kname">{k.name}</span>
          <span class="muted">
            {#if k.revoked}Revoked{:else if k.last_seen_at === null}Never checked in{:else}Checked in {ago(k.last_seen_at, now)} from {k.last_address}{/if}
          </span>
          {#if !k.revoked}
            <span class="reader">
              <label for="reader-{k.id}" class="muted">Card reader</label>
              <input
                id="reader-{k.id}"
                data-testid="kiosk-reader"
                placeholder="none"
                maxlength="128"
                value={readers[k.id] ?? k.card_resource ?? ''}
                oninput={(e) => (readers[k.id] = e.currentTarget.value)}
              />
              <Button size="sm" onclick={() => setReader(k)} aria-label="Save the card reader of {k.name}">Save</Button>
            </span>
            <Button size="sm" onclick={() => revoke(k)} aria-label="Revoke the kiosk {k.name}">Revoke</Button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  <form class="enroll" onsubmit={enroll}>
    <label for="kiosk-name">Make this screen a kiosk</label>
    <div class="row">
      <input id="kiosk-name" bind:value={name} maxlength="64" required />
      <Button variant="primary" size="sm" type="submit">Make this screen a kiosk</Button>
    </div>
    <span class="muted">Do this on the kiosk itself, signed in as an admin: it is this browser that becomes the kiosk.</span>
    <span class="muted">A kiosk’s card reader is the CS:S resource its reader is set to, by its id or its external id: a card tap there signs the member in at that kiosk (docs/integrations/card-sign-in.md).</span>
  </form>
</section>

<style>
  .kiosks {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 20px 22px;
    background: var(--pop-panel);
    border-radius: var(--pop-radius);
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 8px;
    font-size: 14px;
  }
  h2 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
  }
  .muted {
    color: var(--pop-muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  li {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 14px;
  }
  .kname {
    font-weight: 500;
  }
  .revoked .kname {
    text-decoration: line-through;
  }
  .reader {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-left: auto;
  }
  .reader input {
    width: 12rem;
  }
  .enroll {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .row {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }
  label {
    font-size: 14px;
    color: var(--pop-muted);
  }
  input {
    font: inherit;
    font-size: 16px;
    padding: 8px 10px;
    color: var(--pop-text);
    background: var(--pop-panel);
    border: 2px solid var(--pop-line);
    border-radius: 4px;
  }
  input:focus-visible {
    outline: 3px solid var(--pop-cream);
    outline-offset: 2px;
  }
  .problem {
    margin: 0;
    padding: 10px 14px;
    background: var(--pop-coral);
    color: var(--pop-ink);
    border-radius: 4px;
  }
  .said {
    margin: 0;
  }
</style>
