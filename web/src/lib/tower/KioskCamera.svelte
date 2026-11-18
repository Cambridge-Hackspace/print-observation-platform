<script lang="ts">
  // A printer's camera, full screen at the kiosk (#58), through POP and
  // go2rtc (#67). The kiosk has no session of its own until kiosk sessions
  // (#12), and the space's own network may watch without one, so it watches
  // as a member would. It goes back to the tower by itself after a while,
  // so the screen is never left on one camera.
  import { onMount } from 'svelte';
  import { Button, Icon } from '../design';
  import type { TowerPrinter } from '../generated/TowerPrinter';
  import CameraView from '../phone/CameraView.svelte';

  let {
    printer,
    onback = () => {},
    returnAfterMs = 5 * 60_000,
  }: {
    /** The printer, as the live stream has it; undefined once it leaves. */
    printer: TowerPrinter | undefined;
    onback?: () => void;
    /** How long before the tower comes back on its own. */
    returnAfterMs?: number;
  } = $props();

  onMount(() => {
    const back = setTimeout(() => onback(), returnAfterMs);
    return () => clearTimeout(back);
  });
</script>

<section class="camera-screen" data-testid="kiosk-camera">
  <header>
    <Button size="md" onclick={onback}><Icon name="back" size={28} /> Back</Button>
    <h1>{printer?.name ?? 'That printer'}</h1>
  </header>
  {#if printer?.camera}
    <div class="view">
      <CameraView printer={printer.id} signedIn={true} />
    </div>
  {:else}
    <p class="gone">This printer has no camera on the tower now.</p>
  {/if}
</section>

<style>
  .camera-screen {
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 24px;
    background: var(--pop-bg, #1d2620);
    color: var(--pop-text, #f6f1e6);
  }
  header {
    display: flex;
    align-items: center;
    gap: 24px;
  }
  h1 {
    margin: 0;
    font-size: 40px;
  }
  .view {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .view :global(.camera) {
    max-height: 100%;
    height: 100%;
    width: auto;
  }
  .gone {
    font-size: 28px;
  }
</style>
