<script lang="ts">
  // One step or yes/no question at a time, with large buttons, and the whole
  // flowchart beside it showing the path taken so far (#56). The same walk on
  // a phone and on the public pages (#72, #94), laid out for each.
  import { Button } from '../design';
  import GuideMap from './GuideMap.svelte';
  import { answers, back, begin, choose, current, lastSaid, progress, type Answer, type Walk, type Walkable } from './walk';

  let {
    guide,
    title,
    imageUrl,
    layout = 'page',
  }: {
    guide: Walkable;
    title: string;
    /** Where one of the guide's images is. */
    imageUrl: (id: string) => string;
    /** The kiosk puts the map beside the step; a page puts it below. */
    layout?: 'kiosk' | 'page';
  } = $props();

  // The walk, with the guide it is of: a different guide starts afresh.
  // Raw, not deep: a deep $state would hold a proxy of the guide, never ===
  // the guide itself, and every walk would look like another guide's.
  let walked = $state.raw<{ of: Walkable; walk: Walk } | null>(null);
  const walk = $derived(walked !== null && walked.of === guide ? walked.walk : begin(guide));
  const go = (w: Walk) => (walked = { of: guide, walk: w });
  const step = $derived(current(guide, walk));
  const where = $derived(progress(guide, walk));
  const said = $derived(lastSaid(guide, walk));
  const label: Record<Answer, string> = { yes: 'Yes', no: 'No', next: 'Next' };
</script>

<div class="guide {layout}" data-testid="guide">
  <section class="step" aria-live="polite">
    <p class="muted where" data-testid="where">
      {step.then.kind === 'done' ? `Step ${where.at}: the last one` : `Step ${where.at} of about ${where.about}`}
    </p>
    {#if said}
      <p class="muted said" data-testid="said">You said {said.answer} to: {said.question}</p>
    {/if}
    <h2 data-testid="step-title">{step.title}</h2>
    {#if step.body}
      <p class="body">{step.body}</p>
    {/if}
    {#if step.image}
      <img src={imageUrl(step.image)} alt="" />
    {/if}
    <div class="buttons">
      {#each answers(step) as a (a)}
        <Button variant={a === 'no' ? 'secondary' : 'primary'} onclick={() => go(choose(guide, walk, a))}>
          {label[a]}
        </Button>
      {/each}
      {#if step.then.kind === 'done'}
        <Button variant="secondary" onclick={() => go(begin(guide))}>Start over</Button>
      {/if}
      {#if walk.trail.length > 1}
        <Button onclick={() => go(back(walk))}>Back</Button>
      {/if}
    </div>
  </section>
  {#if layout === 'kiosk'}
    <aside class="whole">
      <span class="muted">The whole guide</span>
      <GuideMap {guide} {walk} label="The {title} flowchart, with your path so far marked" />
    </aside>
  {:else}
    <details class="whole">
      <summary>The whole guide</summary>
      <!-- At its own size, scrolled sideways if it is wider than the screen:
           shrunk to fit a phone, its words are too small to read (#72). -->
      <!-- Focusable, so a keyboard can scroll it too: a scrollable region must
           be (WCAG 2.1.1; axe scrollable-region-focusable, which the phone e2e
           runs). Svelte's rule judges the div alone and cannot see that it
           scrolls, so it is excused here, on this element only. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        class="scroll"
        data-testid="map-scroll"
        role="region"
        aria-label="The whole flowchart: scroll sideways to see all of it"
        tabindex="0"
      >
        <GuideMap {guide} {walk} fit={false} label="The {title} flowchart, with your path so far marked" />
      </div>
    </details>
  {/if}
</div>

<style>
  .guide {
    display: flex;
    gap: 32px;
  }
  .guide.page {
    flex-direction: column;
  }
  .step {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .where,
  .said {
    margin: 0;
    font-size: 17px;
  }
  .muted {
    color: var(--pop-muted);
  }
  h2 {
    margin: 4px 0 0;
    font-size: 28px;
    font-weight: 600;
    line-height: 1.25;
    text-wrap: balance;
  }
  .body {
    margin: 0;
    font-size: 19px;
    line-height: 1.45;
    max-width: 38em;
  }
  img {
    max-width: 100%;
    max-height: 40vh;
    object-fit: contain;
    align-self: flex-start;
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
  }
  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
    margin-top: 8px;
  }
  .kiosk .whole {
    width: 380px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 12px;
    font-size: 16px;
  }
  .scroll {
    overflow-x: auto;
    max-width: 100%;
  }
  .scroll:focus-visible {
    outline: 3px solid var(--pop-cream);
    outline-offset: 3px;
  }
  details.whole summary {
    cursor: pointer;
    font-size: 18px;
    padding: 8px 0;
  }
</style>
