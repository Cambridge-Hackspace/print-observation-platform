<script lang="ts" module>
  /** What a secret form sends. */
  export type Secret = { access_code: string } | { username: string; password: string } | { api_key: string };
</script>

<script lang="ts">
  // Entering a secret POP keeps for a printer, never BAM!: a Bambu printer's
  // LAN access code (#21), its camera's login (#150), or the login for its
  // own host, OctoPrint's or PrusaLink's (#29, #30). Sealed by POP as it
  // arrives; nothing here keeps it once it is sent.
  import { untrack } from 'svelte';
  import { Button } from '../design';

  let {
    name,
    kind,
    host = 'prusalink',
    onsubmit = async () => false,
    oncancel = () => {},
  }: {
    /** The printer's name. */
    name: string;
    kind: 'access_code' | 'camera_login' | 'printer_login';
    /** For a printer's own login: what it connects through. */
    host?: 'octoprint' | 'prusalink';
    /** Send it; true once POP has it. */
    onsubmit?: (secret: Secret) => Promise<boolean>;
    oncancel?: () => void;
  } = $props();

  let code = $state('');
  // The form is made again for each kind ({#key} in DeskView), so the kind
  // it starts with is the one it has.
  let username = $state(untrack(() => kind) === 'printer_login' ? 'maker' : '');
  let password = $state('');
  let apiKey = $state('');
  /** PrusaLink on the printer asks a user and password; on a Pi, a key. */
  let onPi = $state(false);
  const keyed = $derived(kind === 'printer_login' && (host === 'octoprint' || onPi));
  let sending = $state(false);

  function secret(): Secret {
    if (kind === 'access_code') return { access_code: code.trim() };
    if (keyed) return { api_key: apiKey.trim() };
    return { username, password };
  }

  async function send(e: SubmitEvent) {
    e.preventDefault();
    sending = true;
    const ok = await onsubmit(secret());
    sending = false;
    if (ok) {
      code = '';
      password = '';
      apiKey = '';
    }
  }
</script>

<form class="secret" onsubmit={send} data-testid="secret-form" data-kind={kind}>
  {#if kind === 'access_code'}
    <h3>{name}’s LAN access code</h3>
    <p class="muted">On the printer’s own screen, under its network settings. POP keeps it sealed; BAM! never has it.</p>
    <label for="secret-code">Access code</label>
    <input id="secret-code" type="password" autocomplete="off" bind:value={code} required minlength="4" maxlength="32" />
  {:else if kind === 'printer_login'}
    {#if host === 'octoprint'}
      <h3>{name}’s OctoPrint API key</h3>
      <p class="muted">In OctoPrint, under Settings → Application Keys (or Settings → API). POP keeps it sealed, and gives it only to the address BAM! has now.</p>
    {:else}
      <h3>The login for {name}’s PrusaLink</h3>
      <p class="muted">POP keeps it sealed, and signs in only at the address BAM! has now. If that address changes, enter it again.</p>
      <fieldset class="where">
        <legend>Where PrusaLink runs</legend>
        <label><input type="radio" name="secret-where" checked={!onPi} onchange={() => (onPi = false)} /> On the printer</label>
        <label><input type="radio" name="secret-where" checked={onPi} onchange={() => (onPi = true)} /> On a Raspberry Pi</label>
      </fieldset>
    {/if}
    {#if keyed}
      <label for="secret-key">API key</label>
      <input id="secret-key" type="password" autocomplete="off" bind:value={apiKey} required maxlength="128" />
    {:else}
      <p class="muted">The user and password the printer shows in its network settings: PrusaLink’s user is usually “maker”.</p>
      <label for="secret-username">Username</label>
      <input id="secret-username" autocomplete="off" bind:value={username} required maxlength="64" />
      <label for="secret-password">Password</label>
      <input id="secret-password" type="password" autocomplete="off" bind:value={password} required maxlength="128" />
    {/if}
  {:else}
    <h3>The login for {name}’s camera</h3>
    <p class="muted">
      POP keeps it sealed and gives it only to the camera at the address BAM! has now. If that address changes, enter it again.
    </p>
    <label for="secret-username">Username <span class="muted">(leave empty if the camera asks for a password only)</span></label>
    <input id="secret-username" autocomplete="off" bind:value={username} maxlength="64" />
    <label for="secret-password">Password</label>
    <input id="secret-password" type="password" autocomplete="off" bind:value={password} required maxlength="128" />
  {/if}
  <div class="actions">
    <Button variant="primary" size="sm" type="submit" disabled={sending}>Save</Button>
    <Button size="sm" onclick={oncancel}>Cancel</Button>
  </div>
</form>

<style>
  .secret {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 30rem;
    padding: 16px 18px;
    background: var(--pop-panel-2);
    border-radius: var(--pop-radius);
  }
  h3 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }
  p {
    margin: 0 0 6px;
    font-size: 14px;
  }
  .muted {
    color: var(--pop-muted);
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
  .actions {
    display: flex;
    gap: 10px;
    margin-top: 8px;
  }
</style>
