<script lang="ts">
  // Managing local users (#90): every account, adding one, and editing one --
  // its name, role, training, password, and whether it can sign in.
  import { onMount } from 'svelte';
  import { Button } from '../design';
  import { call } from '../api';
  import type { Account } from '../generated/Account';
  import type { AdminPrinter } from '../generated/AdminPrinter';
  import type { User } from '../generated/User';
  import { holds, summary, tick, trainingChoices } from './accounts';

  let { printers, me }: { printers: AdminPrinter[]; me: User } = $props();

  let accounts: Account[] | null = $state(null);
  let problem: string | null = $state(null);
  let said: string | null = $state(null);
  let open: number | null = $state(null);

  // The account being edited.
  let name = $state('');
  let role: 'member' | 'admin' = $state('member');
  let training: string[] = $state([]);
  let extra = $state('');
  let password = $state('');

  // A new account.
  let adding = $state(false);
  let nUsername = $state('');
  let nName = $state('');
  let nPassword = $state('');
  let nRole: 'member' | 'admin' = $state('member');

  const choices = $derived(trainingChoices(printers, accounts ?? []));

  async function load() {
    const a = await call<Account[]>('GET', '/api/admin/users');
    if (a.ok) accounts = a.data;
    else problem = a.error;
  }
  onMount(load);

  function edit(a: Account) {
    open = a.id;
    name = a.name;
    role = a.role;
    training = [...a.training];
    extra = '';
    password = '';
    problem = null;
    said = null;
  }

  /** Each change that differs, in turn; stops at the first refusal. */
  async function save(a: Account) {
    problem = null;
    said = null;
    const wanted = extra.trim() ? tick(training, extra.trim(), true) : training;
    const steps: [string, unknown, boolean][] = [
      ['name', { name }, name.trim() !== a.name],
      ['role', { role }, role !== a.role],
      [
        'training',
        { training: wanted },
        wanted.length !== a.training.length || wanted.some((t) => !holds(a.training, t)),
      ],
      ['password', { password }, password !== ''],
    ];
    for (const [what, body, changed] of steps) {
      if (!changed) continue;
      const r = await call('PUT', `/api/admin/users/${a.id}/${what}`, body);
      if (!r.ok) {
        problem = r.error;
        await load();
        return;
      }
    }
    open = null;
    await load();
    said = `Saved ${name.trim()}.`;
  }

  async function setActive(a: Account, active: boolean) {
    problem = null;
    said = null;
    const r = await call('PUT', `/api/admin/users/${a.id}/active`, { active });
    await load();
    if (!r.ok) problem = r.error;
    else said = active ? `${a.name} can sign in again.` : `${a.name} is signed out and can’t sign in.`;
  }

  async function create(e: SubmitEvent) {
    e.preventDefault();
    problem = null;
    said = null;
    const r = await call('POST', '/api/admin/users', {
      username: nUsername,
      name: nName,
      password: nPassword,
      role: nRole,
    });
    if (!r.ok) {
      problem = r.error;
      return;
    }
    const added = `Added ${nName.trim()}. They sign in as ${nUsername.trim()}.`;
    adding = false;
    nUsername = nName = nPassword = '';
    nRole = 'member';
    await load();
    said = added;
  }
</script>

<section class="people" aria-labelledby="people">
  <div class="head">
    <h2 id="people">People</h2>
    <Button size="sm" onclick={() => (adding = !adding)}>{adding ? 'Cancel' : 'Add a person'}</Button>
  </div>
  {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
  {#if said}<p class="said" role="status">{said}</p>{/if}

  {#if adding}
    <form class="card" onsubmit={create} data-testid="add-person">
      <label for="new-name">Name</label>
      <input id="new-name" bind:value={nName} required />
      <label for="new-username">Username, to sign in with</label>
      <input id="new-username" bind:value={nUsername} autocomplete="off" required />
      <label for="new-password">Password</label>
      <input id="new-password" type="password" bind:value={nPassword} autocomplete="new-password" required />
      <label for="new-role">Role</label>
      <select id="new-role" bind:value={nRole}>
        <option value="member">Member</option>
        <option value="admin">Admin</option>
      </select>
      <Button variant="primary" size="sm" type="submit">Add</Button>
    </form>
  {/if}

  {#if accounts === null}
    <p class="muted" role="status">Loading…</p>
  {:else}
    <ul class="list">
      {#each accounts as a (a.id)}
        <li class="card" class:off={!a.active} data-testid="person">
          <div class="row">
            <div class="who">
              <span class="pname">{a.name}</span>
              <span class="muted">{a.username} · {summary(a)}{a.active ? '' : ' · deactivated'}</span>
            </div>
            {#if open !== a.id}
              <Button size="sm" onclick={() => edit(a)}>Edit</Button>
            {/if}
            {#if a.active}
              <Button size="sm" disabled={a.id === me.id} onclick={() => setActive(a, false)}>Deactivate</Button>
            {:else}
              <Button size="sm" onclick={() => setActive(a, true)}>Reactivate</Button>
            {/if}
          </div>
          {#if open === a.id}
            <form
              class="edit"
              data-testid="edit-person"
              onsubmit={(e) => {
                e.preventDefault();
                save(a);
              }}
            >
              <label for="edit-name-{a.id}">Name</label>
              <input id="edit-name-{a.id}" bind:value={name} required />
              <label for="edit-role-{a.id}">Role</label>
              <select id="edit-role-{a.id}" bind:value={role}>
                <option value="member">Member</option>
                <option value="admin">Admin</option>
              </select>
              <fieldset>
                <legend>Training held here</legend>
                {#each choices as t (t)}
                  <label class="tick">
                    <input
                      type="checkbox"
                      checked={holds(training, t)}
                      onchange={(e) => (training = tick(training, t, e.currentTarget.checked))}
                    />
                    {t}
                  </label>
                {/each}
                <label for="edit-extra-{a.id}">Another, as a printer’s BAM! training field names it</label>
                <input id="edit-extra-{a.id}" bind:value={extra} />
              </fieldset>
              <label for="edit-password-{a.id}">New password (leave empty to keep it)</label>
              <input id="edit-password-{a.id}" type="password" bind:value={password} autocomplete="new-password" />
              <div class="row">
                <Button variant="primary" size="sm" type="submit">Save</Button>
                <Button size="sm" onclick={() => (open = null)}>Cancel</Button>
              </div>
            </form>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .people {
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
    align-items: center;
    gap: 12px;
  }
  h2 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
  }
  .muted {
    color: var(--pop-muted);
  }
  .problem,
  .said {
    margin: 0;
    padding: 8px 12px;
    border-radius: 4px;
    color: var(--pop-ink);
  }
  .problem {
    background: var(--pop-coral);
  }
  .said {
    background: var(--pop-sage);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px;
    background: var(--pop-panel-2);
    border-radius: var(--pop-radius);
  }
  .card.off .pname {
    color: var(--pop-dim);
  }
  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
  }
  .who {
    flex: 1;
    min-width: 12rem;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .pname {
    font-weight: 500;
    font-size: 17px;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 28rem;
  }
  label {
    font-size: 14px;
    color: var(--pop-muted);
  }
  input,
  select {
    font: inherit;
    font-size: 16px;
    padding: 8px 10px;
    color: var(--pop-text);
    background: var(--pop-panel);
    border: 2px solid var(--pop-line);
    border-radius: 4px;
  }
  input:focus-visible,
  select:focus-visible {
    outline: 3px solid var(--pop-cream);
    outline-offset: 2px;
  }
  fieldset {
    display: flex;
    flex-direction: column;
    gap: 6px;
    border: 2px solid var(--pop-line);
    border-radius: 4px;
    padding: 10px 12px;
  }
  legend {
    font-size: 14px;
    color: var(--pop-muted);
  }
  .tick {
    display: flex;
    gap: 8px;
    align-items: center;
    color: var(--pop-text);
    font-size: 16px;
  }
</style>
