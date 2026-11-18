<script lang="ts">
  // Entering a secret POP keeps for a printer, never BAM!: a Bambu printer's
  // LAN access code (#21), or its camera's login (#150). Sealed by POP as it
  // arrives; nothing here keeps it once it is sent.
  import { Button } from '../design';

  let {
    name,
    kind,
    onsubmit = async () => false,
    oncancel = () => {},
  }: {
    /** The printer's name. */
    name: string;
    kind: 'access_code' | 'camera_login';
    /** Send it; true once POP has it. */
    onsubmit?: (secret: { access_code: string } | { username: string; password: string }) => Promise<boolean>;
    oncancel?: () => void;
  } = $props();

  let code = $state('');
  let username = $state('');
  let password = $state('');
  let sending = $state(false);

  async function send(e: SubmitEvent) {
    e.preventDefault();
    sending = true;
    const ok = await onsubmit(kind === 'access_code' ? { access_code: code.trim() } : { username, password });
    sending = false;
    if (ok) {
      code = '';
      password = '';
    }
  }
</script>

<form class="secret" onsubmit={send} data-testid="secret-form" data-kind={kind}>
  {#if kind === 'access_code'}
    <h3>{name}’s LAN access code</h3>
    <p class="muted">On the printer’s own screen, under its network settings. POP keeps it sealed; BAM! never has it.</p>
    <label for="secret-code">Access code</label>
    <input id="secret-code" type="password" autocomplete="off" bind:value={code} required minlength="4" maxlength="32" />
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
