<script lang="ts">
  // Watching a print (#61): the camera, progress, time left and finish
  // time, live. There is deliberately no pause or stop: watching is remote,
  // and control is local (#7). Watch.test.ts holds it to that. What a
  // watcher can do is tell the space something looks wrong (#63): POP posts
  // it to the group chat, and never touches the printer.
  import { call } from '../api';
  import { Button, ProgressBar, StatusSquare } from '../design';
  import type { ReportTaken } from '../generated/ReportTaken';
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

  let reporting = $state(false);
  let note = $state('');
  let reported: string | null = $state(null);
  let problem: string | null = $state(null);

  async function report(id: string) {
    problem = null;
    const r = await call<ReportTaken>('POST', `/api/printers/${encodeURIComponent(id)}/report`, { note });
    if (!r.ok) {
      problem = r.error;
      return;
    }
    reported = r.data.told
      ? 'Told the group chat. Someone at the space will take a look.'
      : 'Someone flagged it a moment ago: the group chat already knows.';
    reporting = false;
    note = '';
  }

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
    <section class="report" aria-label="Something looks wrong?">
      {#if reported}
        <p role="status" data-testid="reported">{reported}</p>
      {:else if !signedIn}
        <p class="muted small">
          Something look wrong? <button type="button" class="link" onclick={onsignin}>Sign in</button> to tell the space.
        </p>
      {:else if reporting}
        {@const id = printer.id}
        <form onsubmit={(e) => (e.preventDefault(), report(id))}>
          <label for="report-note">What looks wrong? (if you can say)</label>
          <textarea id="report-note" bind:value={note} maxlength="200" rows="2"></textarea>
          {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
          <div class="actions">
            <Button size="sm" onclick={() => (reporting = false)}>Never mind</Button>
            <Button size="sm" variant="primary" type="submit">Tell the space</Button>
          </div>
        </form>
      {:else}
        <Button size="sm" onclick={() => (reporting = true)}>Something looks wrong?</Button>
      {/if}
    </section>
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
  .report form {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .report textarea {
    font: inherit;
    font-size: 18px;
    padding: 8px;
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .problem {
    color: var(--pop-coral);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: inherit;
    text-decoration: underline;
    font: inherit;
  }
</style>
