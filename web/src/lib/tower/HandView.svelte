<script lang="ts">
  // "A printer needs a hand" (#55), at the kiosk: the cause in plain words,
  // numbered steps, a link to the guide, and what the group chat saw. The
  // kiosk never resumes a printer; the last step says to, at the printer.
  import { onDestroy, onMount } from 'svelte';
  import { Button, Icon, Logo, PrinterDrawing } from '../design';
  import type { GroupNotice } from '../generated/GroupNotice';
  import type { PublicGuide } from '../generated/PublicGuide';
  import type { TowerPrinter } from '../generated/TowerPrinter';
  import { clock } from './need';
  import { cause, fixSteps, whose } from './hand';

  let {
    printer,
    told,
    onback,
    idleMs = 120_000,
  }: {
    /** As the live stream has it now; undefined once the tower drops it. */
    printer: TowerPrinter | undefined;
    /** What the group chat was told about it, if anything. */
    told: GroupNotice | null;
    /** Back to the printers. */
    onback: () => void;
    /** Left alone this long, the screen goes back to the printers. */
    idleMs?: number;
  } = $props();

  const key = $derived(printer?.state.error?.key ?? null);
  let guide: PublicGuide | null = $state.raw(null);
  let slug: string | null = $state(null);

  // The guide linked to this error (#73), whenever the error changes.
  $effect(() => {
    const k = key;
    guide = null;
    slug = null;
    if (k === null) return;
    let gone = false;
    (async () => {
      try {
        const links = await fetch('/api/public/error-guides');
        if (!links.ok) return;
        const s = ((await links.json()) as Record<string, string>)[k];
        if (gone || s === undefined) return;
        slug = s;
        const res = await fetch(`/api/public/guides/${encodeURIComponent(s)}`);
        if (!res.ok) return;
        const g = (await res.json()) as PublicGuide;
        if (!gone) guide = g;
      } catch {
        // The steps that need no guide still show.
      }
    })();
    return () => {
      gone = true;
    };
  });

  const stillNeeds = $derived(printer?.state.status === 'needs_hand');
  const fix = $derived(fixSteps(guide, printer?.state.held ?? false));

  let idle: ReturnType<typeof setTimeout> | undefined;
  const touched = () => {
    clearTimeout(idle);
    idle = setTimeout(onback, idleMs);
  };
  onMount(() => {
    touched();
    for (const e of ['pointerdown', 'keydown', 'wheel', 'touchstart'] as const) {
      window.addEventListener(e, touched, { passive: true });
    }
  });
  onDestroy(() => {
    clearTimeout(idle);
    for (const e of ['pointerdown', 'keydown', 'wheel', 'touchstart'] as const) {
      window.removeEventListener(e, touched);
    }
  });
</script>

<div class="hand" data-testid="hand">
  <header>
    <Button variant="secondary" aria-label="Back to the printers" onclick={onback}>
      <Icon name="back" size={30} />
    </Button>
    <h1>
      {#if !printer}
        That printer has left the tower
      {:else if stillNeeds}
        {printer.name} needs a hand
      {:else}
        {printer.name} is {printer.state.status === 'printing' ? 'printing again' : 'fixed'}
      {/if}
    </h1>
  </header>

  {#if printer && stillNeeds}
    <div class="body">
      <section class="panel">
        <div class="drawing">
          <PrinterDrawing kind="open" body="var(--pop-sage)" nozzle="var(--pop-coral)" size={200} label="{printer.name}, stopped" />
        </div>
        <div class="what">
          <div class="cause">
            <p class="words" data-testid="cause">{cause(printer)}</p>
            {#if whose(printer)}<p class="muted">{whose(printer)}</p>{/if}
            {#if printer.state.error && !printer.state.error.known}
              <p class="muted">POP doesn’t know this one yet, so this is what the printer said.</p>
            {/if}
          </div>
          <ol class="steps" data-testid="steps">
            {#each fix.steps as s, i (i)}
              <li>
                <span class="n" aria-hidden="true">{i + 1}</span>
                <div><span class="title">{s.title}</span>{#if s.body}<span class="muted">{s.body}</span>{/if}</div>
              </li>
            {/each}
          </ol>
          {#if slug}
            <div class="actions">
              <a class="show" href="/kiosk/guides/{encodeURIComponent(slug)}" data-testid="guide">
                {fix.more ? 'The rest of the guide' : 'Show me how'}
              </a>
            </div>
          {:else}
            <p class="muted">No guide for this one yet. If you’re not sure, ask someone who knows the {printer.name}.</p>
          {/if}
        </div>
      </section>
      <aside>
        {#if told}
          <span class="muted">What the group chat saw</span>
          <div class="notice" data-testid="told">
            <div class="from"><Logo size={30} /><span class="pop">POP</span><span class="muted">{clock(new Date(told.at * 1000))}</span></div>
            <p>{told.text}</p>
          </div>
        {:else}
          <p class="muted" data-testid="not-told">POP hasn’t posted this one to the group chat.</p>
        {/if}
      </aside>
    </div>
  {:else}
    <div class="body done">
      <p class="muted">Nothing to do here now.</p>
      <Button variant="primary" onclick={onback}>Back to the printers</Button>
    </div>
  {/if}
</div>

<style>
  .hand {
    box-sizing: border-box;
    height: 100vh;
    display: flex;
    flex-direction: column;
    gap: 24px;
    padding: 28px 56px 32px;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    gap: 18px;
  }
  h1 {
    margin: 0;
    font-size: 32px;
    font-weight: 600;
  }
  .muted {
    color: var(--pop-muted);
  }
  p {
    margin: 0;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
    gap: 24px;
    align-items: stretch;
  }
  .body.done {
    flex-direction: column;
    align-items: flex-start;
    font-size: 20px;
  }
  .panel {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 32px;
    padding: 30px;
    background: var(--pop-panel);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
    box-shadow: var(--pop-shadow);
    overflow-y: auto;
  }
  .drawing {
    flex: none;
  }
  .what {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }
  .cause {
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: 18px;
  }
  .words {
    font-size: 26px;
    font-weight: 600;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .steps li {
    display: flex;
    gap: 16px;
    align-items: flex-start;
  }
  .n {
    width: 40px;
    height: 40px;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    box-sizing: border-box;
    background: var(--pop-butter);
    color: var(--pop-ink);
    border: 3px solid var(--pop-ink);
    border-radius: 4px;
    font-weight: 600;
    font-size: 19px;
  }
  .steps div {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 16px;
  }
  .title {
    font-size: 21px;
    font-weight: 500;
  }
  .actions {
    display: flex;
    gap: 16px;
  }
  .show {
    display: inline-flex;
    align-items: center;
    height: 64px;
    padding: 0 24px;
    box-sizing: border-box;
    font-size: 20px;
    font-weight: 600;
    color: var(--pop-ink);
    background: var(--pop-butter);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
    box-shadow: 5px 5px 0 var(--pop-coral);
    text-decoration: none;
  }
  .show:focus-visible {
    outline: 3px solid var(--pop-cream);
    outline-offset: 3px;
  }
  aside {
    width: 330px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 16px;
    font-size: 16px;
  }
  .notice {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 18px;
    background: var(--pop-panel-2);
    border-radius: var(--pop-radius);
    line-height: 1.45;
  }
  .from {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .pop {
    font-weight: 600;
  }
</style>
