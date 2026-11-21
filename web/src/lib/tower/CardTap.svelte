<script lang="ts">
  // A card tap at this kiosk's reader (#19), as POP says to show it:
  // - a sign-in, taken at once;
  // - CS:S asking for the member's authenticator code, on a pad;
  // - a refusal, in CS:S's words, gone by itself after a while.
  // The code or ticket CS:S gave never reaches this screen: POP holds it,
  // and redeems it when this screen says to.
  import { onDestroy, onMount } from 'svelte';
  import { call } from '../api';
  import { Button, Icon } from '../design';
  import type { SessionInfo } from '../generated/SessionInfo';
  import type { TapView } from '../generated/TapView';
  import type { User } from '../generated/User';
  import { REFUSAL_MS } from './taps';

  let {
    tap,
    onsignedin = () => {},
    ondone = () => {},
  }: { tap: TapView; onsignedin?: (s: SessionInfo) => void; ondone?: () => void } = $props();

  let digits = $state('');
  let busy = $state(false);
  let problem: string | null = $state(null);
  /** The tap is spent or gone: only OK is left. */
  let over = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function signIn(totp?: string) {
    busy = true;
    problem = null;
    const a = await call<User>('POST', '/api/kiosk/tap', totp === undefined ? {} : { totp });
    if (a.ok) {
      const s = await call<SessionInfo>('GET', '/api/session');
      busy = false;
      if (s.ok) onsignedin(s.data);
      else {
        problem = s.error;
        over = true;
      }
      return;
    }
    busy = false;
    digits = '';
    problem = a.error;
    // A wrong code may be tried again while POP still holds the tap; a code,
    // or a tap POP no longer has, is over.
    if (tap.kind !== 'second_factor' || a.status !== 400) over = true;
  }

  function press(d: string) {
    if (busy || over || digits.length >= 6) return;
    digits += d;
    if (digits.length === 6) void signIn(digits);
  }

  async function cancel() {
    await call('DELETE', '/api/kiosk/tap');
    ondone();
  }

  // Each tap mounts this afresh, so what it says is read once, here.
  onMount(() => {
    if (tap.kind === 'second_factor') problem = tap.problem ?? null;
    if (tap.kind === 'sign_in') void signIn();
    if (tap.kind === 'refused') timer = setTimeout(ondone, REFUSAL_MS);
  });
  onDestroy(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
</script>

<section class="tap" data-testid="card-tap" aria-live="polite">
  {#if tap.kind === 'refused'}
    <h1>Your card didn’t sign you in</h1>
    <p class="said" data-testid="tap-reason">{tap.reason}</p>
    <Button variant="primary" size="md" onclick={cancel}>OK</Button>
  {:else if tap.kind === 'second_factor' && !over}
    <h1>Enter the code from your authenticator app</h1>
    <p class="digits" data-testid="tap-digits" aria-label="{digits.length} of 6 digits entered">
      {#each Array.from({ length: 6 }, (_, i) => i) as i (i)}<span class:filled={i < digits.length}></span>{/each}
    </p>
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    <div class="pad">
      {#each ['1', '2', '3', '4', '5', '6', '7', '8', '9'] as d (d)}
        <Button size="md" disabled={busy} onclick={() => press(d)}>{d}</Button>
      {/each}
      <Button size="md" disabled={busy || digits.length === 0} onclick={() => (digits = digits.slice(0, -1))} aria-label="Delete a digit"
        ><Icon name="back" size={28} /></Button
      >
      <Button size="md" disabled={busy} onclick={() => press('0')}>0</Button>
      <Button size="md" disabled={busy} onclick={cancel}>Cancel</Button>
    </div>
  {:else if over}
    <h1>Your card didn’t sign you in</h1>
    {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    <Button variant="primary" size="md" onclick={ondone}>OK</Button>
  {:else}
    <h1>Signing you in…</h1>
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
  .digits {
    display: flex;
    gap: 14px;
    margin: 0;
  }
  .digits span {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    border: 3px solid var(--pop-line);
  }
  .digits span.filled {
    background: var(--pop-cream);
    border-color: var(--pop-cream);
  }
  .pad {
    display: grid;
    grid-template-columns: repeat(3, 7rem);
    gap: 12px;
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
