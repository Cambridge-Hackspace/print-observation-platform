<script lang="ts">
  // Signing in with a local account (#11), for the admin desk.
  import { Button, Logo } from '../design';
  import { call } from '../api';
  import type { User } from '../generated/User';

  let { onsignedin }: { onsignedin: (u: User) => void } = $props();

  let username = $state('');
  let password = $state('');
  let problem: string | null = $state(null);
  let busy = $state(false);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    problem = null;
    const a = await call<User>('POST', '/api/session', { username, password });
    busy = false;
    if (a.ok) onsignedin(a.data);
    else problem = a.status === 401 ? 'That username and password don’t match an account.' : a.error;
  }
</script>

<main class="signin">
  <div class="brand"><Logo size={40} /><span class="name">POP</span><span class="muted">Printer admin</span></div>
  <form onsubmit={submit} data-testid="signin">
    <h1>Sign in</h1>
    <label for="signin-username">Username</label>
    <input id="signin-username" autocomplete="username" bind:value={username} required />
    <label for="signin-password">Password</label>
    <input id="signin-password" type="password" autocomplete="current-password" bind:value={password} required />
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    <Button variant="primary" type="submit" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</Button>
  </form>
</main>

<style>
  .signin {
    box-sizing: border-box;
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 28px;
    padding: 48px 16px;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .name {
    font-size: 22px;
    font-weight: 600;
  }
  .muted {
    color: var(--pop-muted);
  }
  form {
    width: 100%;
    max-width: 24rem;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 24px;
    background: var(--pop-panel);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
    box-shadow: var(--pop-shadow);
  }
  h1 {
    margin: 0 0 8px;
    font-size: 26px;
    font-weight: 600;
  }
  label {
    font-size: 15px;
    color: var(--pop-muted);
  }
  input {
    font: inherit;
    font-size: 18px;
    padding: 10px 12px;
    color: var(--pop-text);
    background: var(--pop-panel-2);
    border: 2px solid var(--pop-line);
    border-radius: 4px;
  }
  input:focus-visible {
    outline: 3px solid var(--pop-cream);
    outline-offset: 2px;
  }
  .problem {
    margin: 4px 0;
    padding: 8px 12px;
    background: var(--pop-coral);
    color: var(--pop-ink);
    border-radius: 4px;
  }
</style>
