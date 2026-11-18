<script lang="ts">
  // The signed-in bar at the kiosk (#12, round 6): who, "Signs you out in
  // 0:45 if the screen is left alone", and Tag out. A touch anywhere is
  // passed on to POP (at most every ten seconds), and the countdown runs from
  // the last time POP heard from this screen (./session).
  import { onMount } from 'svelte';
  import { call } from '../api';
  import { Button } from '../design';
  import { sensitive } from '../feedback/sensitive';
  import type { SessionInfo } from '../generated/SessionInfo';
  import { clockOf, secondsLeft, shouldPing } from './session';

  let { session, onsignedout = () => {} }: { session: SessionInfo; onsignedout?: () => void } = $props();

  let heardAt = $state(Date.now());
  let lastPing = Date.now();
  let now = $state(Date.now());
  const idle = $derived(session.idle_secs ?? 0);
  const left = $derived(secondsLeft(heardAt, now, idle));
  let ending = false;

  async function tagOut() {
    if (ending) return;
    ending = true;
    await call('DELETE', '/api/session');
    onsignedout();
  }

  async function touched() {
    const t = Date.now();
    if (!shouldPing(lastPing, t)) return;
    lastPing = t;
    const a = await call('GET', '/api/session');
    if (a.ok) heardAt = t;
    else if (a.status === 401) onsignedout();
  }

  onMount(() => {
    const tick = setInterval(() => {
      now = Date.now();
      if (session.idle_secs !== null && secondsLeft(heardAt, now, idle) === 0) void tagOut();
    }, 1000);
    window.addEventListener('pointerdown', touched, true);
    return () => {
      clearInterval(tick);
      window.removeEventListener('pointerdown', touched, true);
    };
  });
</script>

<div class="bar" data-testid="kiosk-session">
  <span class="who">Hi, <span use:sensitive={'name'}>{session.user.name}</span></span>
  <span class="right">
    {#if session.idle_secs !== null}
      <span class="timer" data-testid="countdown">Signs you out in {clockOf(left)}<br />if the screen is left alone</span>
    {/if}
    <Button size="md" onclick={tagOut}>Tag out</Button>
  </span>
</div>

<style>
  .bar {
    position: fixed;
    top: env(safe-area-inset-top, 0px);
    left: 0;
    right: 0;
    z-index: 10;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 24px;
    padding: 12px 24px;
    background: var(--pop-panel);
    color: var(--pop-text);
    font-family: var(--pop-font);
    border-bottom: 3px solid var(--pop-ink);
  }
  .who {
    font-size: 28px;
    font-weight: 600;
  }
  .right {
    display: flex;
    align-items: center;
    gap: 18px;
  }
  .timer {
    font-size: 16px;
    text-align: right;
    color: var(--pop-muted);
  }
</style>
