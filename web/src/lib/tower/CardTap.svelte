<script lang="ts">
  // A card tap at this kiosk's reader (#18): the member POP's CS:S edge says
  // tapped, welcomed by name and signed in at once. If POP cannot finish it,
  // it says so, and OK is all that is left.
  import { onMount } from 'svelte';
  import { call } from '../api';
  import { Button } from '../design';
  import type { SessionInfo } from '../generated/SessionInfo';
  import type { TapView } from '../generated/TapView';
  import type { User } from '../generated/User';

  let {
    tap,
    onsignedin = () => {},
    ondone = () => {},
  }: { tap: TapView; onsignedin?: (s: SessionInfo) => void; ondone?: () => void } = $props();

  let problem: string | null = $state(null);

  async function signIn() {
    const a = await call<User>('POST', '/api/kiosk/tap');
    if (!a.ok) {
      problem = a.error;
      return;
    }
    const s = await call<SessionInfo>('GET', '/api/session');
    if (s.ok) onsignedin(s.data);
    else problem = s.error;
  }

  // Each tap mounts this afresh, so it signs in once, here.
  onMount(() => void signIn());
</script>

<section class="tap" data-testid="card-tap" aria-live="polite">
  {#if problem}
    <h1>Your card didn’t sign you in</h1>
    <p class="problem" role="alert">{problem}</p>
    <Button variant="primary" size="md" onclick={ondone}>OK</Button>
  {:else}
    <h1 data-testid="tap-welcome">{tap.name ? `Welcome, ${tap.name}!` : 'Welcome!'}</h1>
    <p class="said">Signing you in…</p>
  {/if}
</section>

<style>
  .tap {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 24px;
    padding: 24px;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
    text-align: center;
  }
  h1 {
    margin: 0;
    font-size: 36px;
  }
  .said {
    margin: 0;
    font-size: 28px;
    max-width: 36ch;
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
