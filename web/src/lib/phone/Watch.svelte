<script lang="ts">
  // Watching a print (#61): the camera, progress, time left and finish
  // time, live. There is deliberately no pause or stop: watching is remote,
  // and control is local (#7). Watch.test.ts holds it to that.
  import { ProgressBar, StatusSquare } from '../design';
  import { sensitive } from '../feedback/sensitive';
  import type { TowerPrinter } from '../generated/TowerPrinter';
  import { card, need, type Need } from '../tower/need';
  import CameraView from './CameraView.svelte';

  let {
    printer,
    now,
    signedIn,
    onsignin = () => {},
  }: {
    /** The printer, as the live stream has it; null when the tower has no such printer. */
    printer: TowerPrinter | null;
    now: Date;
    signedIn: boolean;
    onsignin?: () => void;
  } = $props();

  const COLOR: Record<Need, string> = {
    needs_hand: 'var(--pop-coral)',
    printing: 'var(--pop-sage)',
    done: 'var(--pop-butter)',
    free: 'var(--pop-text)',
    offline: 'var(--pop-dim)',
  };
</script>

<section class="watch" data-testid="watch">
  <a class="back" href="#printers">‹ Printers</a>
  {#if !printer}
    <h1>Not on the tower</h1>
    <p class="muted">That printer isn’t on the tower now.</p>
  {:else}
    {@const n = need(printer)}
    {@const w = card(printer, now)}
    <h1>{printer.name}</h1>
    <p class="state" style:color={COLOR[n]}><StatusSquare color={COLOR[n]} label={w.state} show /></p>
    {#if printer.camera}
      <CameraView printer={printer.id} {signedIn} {onsignin} />
    {/if}
    {#if w.job}<p class="job" use:sensitive={'file'}>{w.job}</p>{/if}
    {#if printer.state.progress !== null && (n === 'printing' || n === 'needs_hand' || n === 'done')}
      <ProgressBar
        value={n === 'done' ? 1 : printer.state.progress}
        label="{printer.name}: {w.left}"
        height={20}
        color={n === 'needs_hand' ? 'var(--pop-coral)' : 'var(--pop-sage)'}
        layer={n === 'needs_hand' ? '#e3957a' : 'var(--pop-sage-layer)'}
      />
    {/if}
    <dl class="facts">
      {#if w.left}<div><dt>Now</dt><dd data-testid="watch-left">{w.left}</dd></div>{/if}
      {#if w.right}<div><dt>Then</dt><dd data-testid="watch-right">{w.right}</dd></div>{/if}
    </dl>
    <p class="muted small">Watching only: a print is paused or stopped at the printer.</p>
  {/if}
</section>

<style>
  .watch {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .back {
    color: inherit;
    font-size: 18px;
  }
  h1 {
    margin: 0;
  }
  .state {
    margin: 0;
    font-size: 18px;
  }
  .job {
    margin: 0;
    font-size: 18px;
  }
  .facts {
    display: grid;
    gap: 6px;
    margin: 0;
  }
  .facts div {
    display: flex;
    gap: 8px;
  }
  dt {
    min-width: 3em;
    opacity: 0.7;
  }
  dd {
    margin: 0;
  }
  .small {
    font-size: 14px;
  }
</style>
