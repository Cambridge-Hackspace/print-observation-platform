<script lang="ts">
  // The tower's home screen (#49), as given: Tower.svelte connects it to the
  // live stream. One card per printer, most needed first.
  import { onDestroy, onMount } from 'svelte';
  import { prefersReducedMotion } from 'svelte/motion';
  import type { Connection } from '../live';
  import type { ScreenState } from '../screen';
  import { Icon, Logo, Pine, Fern, Shroom, PrinterDrawing, ProgressBar, StatusSquare } from '../design';
  import { byNeed, card, clock, countLabels, need, type Need } from './need';
  import { AutoScroll, DEFAULTS } from './autoscroll';
  import Qr from './Qr.svelte';
  import type { TowerPrinter } from '../generated/TowerPrinter';

  let {
    screen,
    connection,
    now,
  }: {
    screen: ScreenState;
    connection: Connection;
    /** The time shown, and that "done about" counts from. */
    now: Date;
  } = $props();

  const printers = $derived(byNeed(screen.printers));
  const tally = $derived(countLabels(screen.printers));
  const phone = $derived(screen.status?.public_url ?? null);

  const COLOR: Record<Need, string> = {
    needs_hand: 'var(--pop-coral)',
    printing: 'var(--pop-sage)',
    done: 'var(--pop-butter)',
    free: 'var(--pop-text)',
    offline: 'var(--pop-dim)',
  };
  // Each printer drawn in its own colors, kept as it moves up and down.
  const BODIES = [
    ['var(--pop-sage)', 'var(--pop-coral)'],
    ['var(--pop-coral)', 'var(--pop-butter)'],
    ['var(--pop-butter)', 'var(--pop-coral)'],
    ['var(--pop-butter)', 'var(--pop-sage)'],
  ] as const;
  const colors = (p: TowerPrinter) => {
    if (need(p) === 'offline') return ['#6f7d72', 'var(--pop-dim)'] as const;
    let h = 0;
    for (const c of p.id) h = (h * 31 + c.charCodeAt(0)) >>> 0;
    return BODIES[h % BODIES.length]!;
  };

  let list: HTMLDivElement | undefined = $state();
  let frame = 0;
  onMount(() => {
    if (!list) return;
    const scroller = new AutoScroll(list, performance.now(), {
      ...DEFAULTS,
      reducedMotion: prefersReducedMotion.current,
    });
    const touched = () => scroller.touch(performance.now());
    for (const e of ['pointerdown', 'wheel', 'keydown', 'touchstart'] as const) {
      window.addEventListener(e, touched, { passive: true });
    }
    let last = performance.now();
    const step = (t: number) => {
      scroller.tick(t, t - last);
      last = t;
      frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => {
      for (const e of ['pointerdown', 'wheel', 'keydown', 'touchstart'] as const) {
        window.removeEventListener(e, touched);
      }
    };
  });
  onDestroy(() => cancelAnimationFrame(frame));
</script>

<div class="tower" data-testid="tower" data-connection={connection}>
  <header>
    <div class="brand">
      <Logo />
      <span class="name">POP</span>
      <span class="muted">Print tower</span>
    </div>
    <ul class="tally" aria-label="Printers by state">
      {#each tally as t (t.need)}
        <li class="tag"><StatusSquare color={COLOR[t.need]} label={t.text} /></li>
      {/each}
    </ul>
    <span class="clock muted" data-testid="clock">{clock(now)}</span>
  </header>

  {#if connection !== 'live'}
    <p class="stale" role="status" data-testid="connection">
      {connection === 'connecting' ? 'Connecting' : 'Reconnecting: what you see may be out of date'}
    </p>
  {/if}

  <div class="list" bind:this={list} data-testid="printers">
    {#each printers as p (p.id)}
      {@const n = need(p)}
      {@const w = card(p, now)}
      {@const [body, nozzle] = colors(p)}
      <article class="card" class:hi={n === 'needs_hand'} data-testid="printer" data-need={n}>
        <div class="tile">
          <PrinterDrawing
            kind="open"
            {body}
            {nozzle}
            size={84}
            part={n === 'needs_hand' || n === 'printing' || n === 'done'}
            label="{p.name}, {w.state}"
          />
        </div>
        <div class="who">
          <h2>{p.name}</h2>
          <span style:color={COLOR[n]}><StatusSquare color={COLOR[n]} label={w.state} /></span>
        </div>
        <div class="what">
          <span class="job">{w.job}</span>
          {#if n === 'free' || n === 'offline' || p.state.progress === null}
            <div class="empty" aria-hidden="true"></div>
          {:else}
            <ProgressBar
              value={n === 'done' ? 1 : p.state.progress}
              label="{p.name}: {w.left}"
              height={24}
              color={n === 'needs_hand' ? 'var(--pop-coral)' : n === 'done' ? 'var(--pop-butter)' : 'var(--pop-sage)'}
              layer={n === 'needs_hand' ? '#e3957a' : n === 'done' ? 'var(--pop-butter)' : 'var(--pop-sage-layer)'}
            />
          {/if}
          <div class="under"><span>{w.left}</span><span class="muted">{w.right}</span></div>
        </div>
      </article>
    {:else}
      <p class="none muted">No printers yet: an admin adds them from BAM!.</p>
    {/each}
  </div>

  <footer>
    <Icon name="down" size={40} color="var(--pop-butter)" weight={2.6} />
    <div class="prompt">
      <span class="tap">Tap your card to start printing</span>
      <span class="muted">The card reader is just below this screen</span>
    </div>
    {#if phone}
      <div class="phone">
        <Qr text={phone} size={64} label="A QR code for {phone}" />
        <span>Follow a print from your phone</span>
      </div>
    {/if}
    <svg class="scene" width="230" height="96" viewBox="0 0 230 96" aria-hidden="true">
      <Pine x={40} b={84} h={70} fill="var(--pop-far)" />
      <Pine x={200} b={84} h={84} fill="var(--pop-far)" />
      <Pine x={160} b={84} h={56} fill="var(--pop-mid)" />
      <rect x="-4" y="84" width="238" height="14" fill="var(--pop-moss)" stroke="var(--pop-ink)" stroke-width="3" />
      <Fern x={90} b={84} h={40} color="var(--pop-sage)" />
      <Shroom x={124} b={84} s={15} cap="var(--pop-coral)" />
      <Shroom x={148} b={84} s={9} cap="var(--pop-butter)" />
    </svg>
  </footer>
</div>

<style>
  .tower {
    box-sizing: border-box;
    height: 100vh;
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding: 28px 56px 32px;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 24px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .name {
    font-size: 24px;
    font-weight: 600;
  }
  .brand .muted {
    font-size: 18px;
  }
  .muted {
    color: var(--pop-muted);
  }
  .tally {
    display: flex;
    gap: 10px;
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .tag {
    font-size: 16px;
    padding: 7px 12px;
    border: 2px solid var(--pop-line);
    border-radius: 4px;
  }
  .clock {
    font-size: 22px;
    font-variant-numeric: tabular-nums;
  }
  .stale {
    margin: 0;
    padding: 8px 14px;
    background: var(--pop-butter);
    color: var(--pop-ink);
    border-radius: 4px;
    font-size: 17px;
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    scrollbar-width: none;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .card {
    display: flex;
    align-items: center;
    gap: 24px;
    padding: 12px 24px 12px 12px;
    flex: none;
    background: var(--pop-panel);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
  }
  .card.hi {
    box-shadow: var(--pop-shadow);
  }
  .tile {
    width: 104px;
    height: 84px;
    flex: none;
    display: flex;
    justify-content: center;
    background: var(--pop-panel-2);
    border-radius: 4px;
  }
  .who {
    width: 230px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 17px;
  }
  h2 {
    margin: 0;
    font-size: 24px;
    font-weight: 600;
  }
  .what {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 9px;
    font-size: 17px;
  }
  .job {
    min-height: 1.3em;
  }
  .empty {
    height: 24px;
    box-sizing: border-box;
    border: 2px dashed var(--pop-line);
    border-radius: 3px;
  }
  .under {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  .none {
    font-size: 20px;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 22px;
    padding-left: 26px;
    height: 104px;
    flex: none;
    overflow: hidden;
    background: var(--pop-panel);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
    box-shadow: var(--pop-shadow);
  }
  .prompt {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    font-size: 17px;
  }
  .tap {
    font-size: 30px;
    font-weight: 600;
  }
  .phone {
    display: flex;
    align-items: center;
    gap: 14px;
    padding-right: 8px;
  }
  .phone span {
    font-size: 15px;
    line-height: 1.35;
    width: 130px;
  }
  .scene {
    flex: none;
    align-self: flex-end;
  }
</style>
