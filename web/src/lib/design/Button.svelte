<!--
  A button (#48). Primary is the one action a screen leads with: butter, with
  a coral paper shadow. Secondary is a quiet panel; plain is an outline.
  Kiosk-sized by default, 64px high, for a finger at a tower.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { HTMLButtonAttributes } from 'svelte/elements';

  let {
    variant = 'plain',
    size = 'md',
    type = 'button',
    children,
    ...rest
  }: {
    variant?: 'primary' | 'secondary' | 'plain';
    size?: 'md' | 'sm';
    children: Snippet;
  } & HTMLButtonAttributes = $props();
</script>

<button {type} class="btn {variant} {size}" {...rest}>{@render children()}</button>

<style>
  .btn {
    font: inherit;
    font-size: 20px;
    font-weight: 500;
    color: var(--pop-text);
    background: transparent;
    border: 2px solid var(--pop-line);
    border-radius: var(--pop-radius);
    height: 64px;
    padding: 0 24px;
    white-space: nowrap;
    cursor: pointer;
    box-sizing: border-box;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
  }
  .secondary {
    background: var(--pop-panel-2);
    border-color: var(--pop-panel-2);
  }
  .primary {
    font-weight: 600;
    color: var(--pop-ink);
    background: var(--pop-butter);
    border: 3px solid var(--pop-ink);
    box-shadow: 5px 5px 0 var(--pop-coral);
  }
  .primary:active:not(:disabled) {
    transform: translate(3px, 3px);
    box-shadow: 2px 2px 0 var(--pop-coral);
  }
  .sm {
    font-size: 17px;
    height: 52px;
    padding: 0 18px;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
