<script lang="ts">
  // The kiosk's on-screen keyboard (#12): large keys for a finger, no
  // keyboard needed to sign in by password.
  import { apply, rows, type Key } from './keys';

  let { value = $bindable(''), label = 'Keyboard' }: { value?: string; label?: string } = $props();

  let shift = $state(false);
  let symbols = $state(false);
  const layout = $derived(rows(shift, symbols));

  function press(k: Key) {
    if (k.kind === 'shift') shift = !shift;
    else if (k.kind === 'symbols') symbols = !symbols;
    else {
      value = apply(value, k);
      // One capital, then back to small, as a phone does.
      if (k.kind === 'char') shift = false;
    }
  }

  function text(k: Key): string {
    switch (k.kind) {
      case 'char':
        return k.char;
      case 'shift':
        return '⇧';
      case 'symbols':
        return symbols ? 'abc' : '#+=';
      case 'back':
        return '⌫';
      case 'space':
        return 'space';
    }
  }

  function name(k: Key): string {
    switch (k.kind) {
      case 'char':
        return k.char;
      case 'shift':
        return 'Shift';
      case 'symbols':
        return symbols ? 'Letters' : 'Symbols';
      case 'back':
        return 'Delete';
      case 'space':
        return 'Space';
    }
  }
</script>

<div class="keyboard" role="group" aria-label={label}>
  {#each layout as row, r (r)}
    <div class="row">
      {#each row as k, i (`${r}-${i}`)}
        <button
          type="button"
          class="key {k.kind}"
          class:on={(k.kind === 'shift' && shift) || (k.kind === 'symbols' && symbols)}
          aria-label={name(k)}
          onpointerdown={(e) => e.preventDefault()}
          onclick={() => press(k)}>{text(k)}</button
        >
      {/each}
    </div>
  {/each}
</div>

<style>
  .keyboard {
    display: flex;
    flex-direction: column;
    gap: 8px;
    user-select: none;
  }
  .row {
    display: flex;
    justify-content: center;
    gap: 8px;
  }
  .key {
    min-width: 64px;
    height: 64px;
    padding: 0 12px;
    font: inherit;
    font-size: 26px;
    color: var(--pop-ink);
    background: var(--pop-cream);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
    cursor: pointer;
  }
  .key.space {
    flex: 0 1 420px;
  }
  .key.on {
    background: var(--pop-butter);
  }
  .key:focus-visible {
    outline: 3px solid var(--pop-cream);
    outline-offset: 3px;
  }
</style>
