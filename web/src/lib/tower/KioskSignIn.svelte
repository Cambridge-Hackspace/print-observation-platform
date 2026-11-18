<script lang="ts">
  // Signing in at the kiosk by password (#12), with the on-screen keyboard;
  // a card tap does the same once the reader is wired (#19). At an enrolled
  // kiosk POP makes it a kiosk session, which ends left alone.
  import { call } from '../api';
  import { Button, Icon } from '../design';
  import type { SessionInfo } from '../generated/SessionInfo';
  import type { User } from '../generated/User';
  import Keyboard from './Keyboard.svelte';

  let { onsignedin = () => {}, oncancel = () => {} }: { onsignedin?: (s: SessionInfo) => void; oncancel?: () => void } = $props();

  let username = $state('');
  let password = $state('');
  /** The field the keyboard types into. */
  let field: 'username' | 'password' = $state('username');
  let problem: string | null = $state(null);
  let busy = $state(false);

  async function signIn() {
    problem = null;
    busy = true;
    const a = await call<User>('POST', '/api/session', { username: username.trim(), password });
    if (!a.ok) {
      busy = false;
      problem = a.error;
      password = '';
      return;
    }
    const s = await call<SessionInfo>('GET', '/api/session');
    busy = false;
    if (s.ok) onsignedin(s.data);
    else problem = s.error;
  }
</script>

<section class="signin" data-testid="kiosk-signin">
  <header>
    <Button size="md" onclick={oncancel}><Icon name="back" size={28} /> Back</Button>
    <h1>Sign in</h1>
  </header>
  <div class="fields">
    <button type="button" class="field" class:chosen={field === 'username'} onclick={() => (field = 'username')}>
      <span class="label">Username</span>
      <span class="value" data-testid="username">{username || ' '}</span>
    </button>
    <button type="button" class="field" class:chosen={field === 'password'} onclick={() => (field = 'password')}>
      <span class="label">Password</span>
      <span class="value" data-testid="password">{'•'.repeat([...password].length) || ' '}</span>
    </button>
    <Button variant="primary" size="md" disabled={busy || !username.trim() || !password} onclick={signIn}>Sign in</Button>
  </div>
  {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
  {#if field === 'username'}
    <Keyboard bind:value={username} label="Keyboard, typing the username" />
  {:else}
    <Keyboard bind:value={password} label="Keyboard, typing the password" />
  {/if}
</section>

<style>
  .signin {
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: 20px;
    padding: 24px;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
  }
  header {
    display: flex;
    align-items: center;
    gap: 24px;
  }
  h1 {
    margin: 0;
    font-size: 36px;
  }
  .fields {
    display: flex;
    gap: 16px;
    align-items: stretch;
    flex-wrap: wrap;
  }
  .field {
    flex: 1 1 18rem;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 12px 16px;
    text-align: left;
    font: inherit;
    color: var(--pop-text);
    background: var(--pop-panel);
    border: 3px solid var(--pop-line);
    border-radius: var(--pop-radius);
    cursor: pointer;
  }
  .field.chosen {
    border-color: var(--pop-cream);
  }
  .label {
    font-size: 16px;
    color: var(--pop-muted);
  }
  .value {
    font-size: 28px;
    min-height: 1.3em;
  }
  .problem {
    margin: 0;
    padding: 10px 14px;
    background: var(--pop-coral);
    color: var(--pop-ink);
    border-radius: 4px;
    font-size: 20px;
  }
</style>
