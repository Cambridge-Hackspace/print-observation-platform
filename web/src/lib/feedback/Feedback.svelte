<script lang="ts">
  // Feedback from any screen (#96): a button for whoever holds
  // feedback.file. It captures the screen, redacted before it becomes an
  // image (redact.ts), lets the person draw on it (Annotate.svelte), asks
  // what it is, shows what goes with it, and sends it. "Sent" only once the
  // issue exists.
  import { onMount } from 'svelte';
  import { call } from '../api';
  import { Button } from '../design';
  import type { FeedbackKind } from '../generated/FeedbackKind';
  import type { FeedbackSettings } from '../generated/FeedbackSettings';
  import { context, type Context } from './context';
  import { redact } from './redact';
  import { send, type Outcome } from './send';

  type Step =
    | { at: 'idle' }
    | { at: 'capturing' }
    | { at: 'drawing'; picture: HTMLCanvasElement; context: Context }
    | { at: 'asking'; png: string; context: Context }
    | { at: 'sending' }
    | { at: 'said'; outcome: Outcome };

  let allowed = $state(false);
  let step: Step = $state({ at: 'idle' });
  let kind: FeedbackKind = $state('bug');
  let title = $state('');
  let description = $state('');
  let problem: string | null = $state(null);

  onMount(async () => {
    const s = await call<FeedbackSettings>('GET', '/api/feedback');
    allowed = s.ok && s.data.enabled && s.data.allowed;
  });

  async function capture() {
    problem = null;
    // What the screen knew when the button was pressed, not after.
    const known = context();
    step = { at: 'capturing' };
    try {
      // Loaded only when someone files feedback: no screen carries them otherwise.
      const { domToCanvas } = await import('modern-screenshot');
      const picture = await domToCanvas(document.body, {
        // The feedback controls are not part of what is reported.
        filter: (n) => !(n instanceof Element && n.hasAttribute('data-feedback-ui')),
        onCloneNode: redact,
        backgroundColor: getComputedStyle(document.body).backgroundColor || '#ffffff',
        scale: 1,
      });
      step = { at: 'drawing', picture, context: known };
    } catch (e) {
      problem = `The screen couldn’t be captured: ${e instanceof Error ? e.message : String(e)}`;
      step = { at: 'idle' };
    }
  }

  async function submit(png: string, known: Context) {
    if (!title.trim()) {
      problem = 'Give it a title.';
      return;
    }
    step = { at: 'sending' };
    const outcome = await send(
      { kind, title: title.trim(), description, device: known.device, screenshot: png, context: { ...known } },
      call,
    );
    step = { at: 'said', outcome };
    if (outcome.state === 'sent' || outcome.state === 'kept') {
      title = '';
      description = '';
    }
  }
</script>

{#if allowed}
  <div class="feedback" data-feedback-ui data-testid="feedback">
    {#if step.at === 'idle' || step.at === 'capturing'}
      <button
        class="open"
        type="button"
        data-testid="feedback-open"
        disabled={step.at === 'capturing'}
        onclick={capture}>{step.at === 'capturing' ? 'Capturing…' : 'Feedback'}</button
      >
      {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
    {:else if step.at === 'drawing'}
      {@const known = step.context}
      {@const picture = step.picture}
      {#await import('./Annotate.svelte') then { default: Annotate }}
        <Annotate
          {picture}
          oncancel={() => (step = { at: 'idle' })}
          ondone={(png) => (step = { at: 'asking', png, context: known })}
        />
      {/await}
    {:else if step.at === 'asking'}
      {@const s = step}
      <form class="sheet" data-testid="feedback-form" onsubmit={(e) => (e.preventDefault(), submit(s.png, s.context))}>
        <h2>Send feedback</h2>
        <fieldset>
          <legend>What is it?</legend>
          {#each [['bug', 'Something’s wrong'], ['suggestion', 'An idea'], ['question', 'A question']] as [k, label] (k)}
            <label class="choice"><input type="radio" name="feedback-kind" value={k} bind:group={kind} /> {label}</label>
          {/each}
        </fieldset>
        <label for="feedback-title">Title</label>
        <input id="feedback-title" data-testid="feedback-title" bind:value={title} maxlength="120" required />
        <label for="feedback-description">What happened, or what you’d like</label>
        <textarea id="feedback-description" data-testid="feedback-description" bind:value={description} maxlength="5000" rows="4"></textarea>
        <img class="preview" alt="The screenshot, as it will be sent" src="data:image/png;base64,{s.png}" />
        <details>
          <summary>What goes with it</summary>
          <pre data-testid="feedback-context">{JSON.stringify(s.context, null, 2)}</pre>
        </details>
        <p class="public">
          This goes to a <strong>public</strong> GitHub issue: anyone can read it, its picture and what goes with it, and it says who sent it.
        </p>
        {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
        <div class="actions">
          <Button size="sm" onclick={() => (step = { at: 'idle' })}>Cancel</Button>
          <Button size="sm" variant="primary" type="submit" data-testid="feedback-send">Send</Button>
        </div>
      </form>
    {:else if step.at === 'sending'}
      <p class="sheet" role="status">Sending…</p>
    {:else}
      {@const o = step.outcome}
      <div class="sheet" role="status" data-testid="feedback-outcome">
        {#if o.state === 'sent'}
          <p>Sent. <a href={o.url} target="_blank" rel="noopener">See the issue</a></p>
        {:else if o.state === 'kept'}
          <p>POP has it, and will send it as soon as GitHub can be reached.</p>
        {:else if o.state === 'failed'}
          <p>GitHub refused it. An admin should look at POP’s log.</p>
        {:else}
          <p>{o.error}</p>
        {/if}
        <Button size="sm" onclick={() => (step = { at: 'idle' })}>Close</Button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .feedback {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 900;
    font-family: inherit;
  }
  .open {
    min-height: 56px;
    min-width: 120px;
    padding: 0 20px;
    border: 0;
    border-radius: 28px;
    background: var(--pop-ink, #1d2620);
    color: var(--pop-cream, #f6f1e6);
    font: inherit;
    font-size: 18px;
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.25);
  }
  .sheet {
    position: fixed;
    inset: auto 16px 16px auto;
    width: min(560px, calc(100vw - 32px));
    max-height: calc(100vh - 32px);
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 20px;
    border-radius: 12px;
    /* POP's own panel, which its buttons are drawn for: on a light sheet the
       plain buttons' light text (Cancel, Close) could not be read. */
    background: var(--pop-panel);
    color: var(--pop-text);
    border: 3px solid var(--pop-ink);
    box-shadow: var(--pop-shadow);
  }
  .sheet a {
    color: var(--pop-butter);
  }
  .sheet input:not([type='radio']),
  .sheet textarea {
    font: inherit;
    font-size: 18px;
    padding: 10px;
  }
  fieldset {
    border: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }
  .choice {
    font-size: 18px;
  }
  .preview {
    max-width: 100%;
    border: 2px solid var(--pop-line);
  }
  pre {
    font-size: 12px;
    white-space: pre-wrap;
  }
  .public {
    font-size: 15px;
  }
  .problem {
    color: var(--pop-coral);
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
</style>
