<script lang="ts">
  // "Sign in with CS:S" and the like (#14): a link to each configured
  // provider, and what happened when one sent the browser back without a
  // sign-in. Not at the kiosk, which has no keyboard for a provider's own
  // page; there, members tap their card (#18).
  import { onMount } from 'svelte';
  import { call } from '../api';
  import type { SignInProvider } from '../generated/SignInProvider';

  let {
    /** Where the provider sends the browser back to: this screen. */
    then,
  }: { then: '/' | '/admin' } = $props();

  let providers = $state<SignInProvider[]>([]);

  /** What `?sign_in=` says about the last attempt, in plain words. */
  const SAID: Record<string, string> = {
    failed: 'Signing in didn’t work. Try again, or use your POP username and password.',
    cancelled: 'Signing in was cancelled.',
    expired: 'That sign-in took too long. Try again.',
    deactivated: 'That account is switched off. Ask a printer admin.',
    unreachable: 'POP couldn’t reach the sign-in service. Try again in a moment.',
  };
  const said = new URLSearchParams(location.search).get('sign_in');
  const problem = said ? (SAID[said] ?? SAID.failed) : null;

  onMount(async () => {
    const a = await call<SignInProvider[]>('GET', '/api/sign-in/providers');
    if (a.ok) providers = a.data;
  });
</script>

{#if problem}<p class="problem" role="alert" data-testid="sign-in-problem">{problem}</p>{/if}
{#if providers.length > 0}
  <div class="providers" data-testid="providers">
    {#each providers as p (p.id)}
      <a class="provider" href="/api/sign-in/{encodeURIComponent(p.id)}?then={encodeURIComponent(then)}">Sign in with {p.label}</a>
    {/each}
    <span class="or muted">or with your POP username and password</span>
  </div>
{/if}

<style>
  .providers {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .provider {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 56px;
    padding: 0 20px;
    font-size: 18px;
    font-weight: 600;
    color: var(--pop-ink);
    background: var(--pop-butter);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
    text-decoration: none;
  }
  .provider:focus-visible {
    outline: 3px solid var(--pop-cream);
    outline-offset: 3px;
  }
  .or {
    text-align: center;
    font-size: 14px;
  }
  .problem {
    margin: 0;
    padding: 10px 14px;
    color: var(--pop-ink);
    background: var(--pop-coral);
    border-radius: 4px;
  }
</style>
