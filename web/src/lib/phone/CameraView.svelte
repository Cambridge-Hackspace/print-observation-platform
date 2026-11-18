<script lang="ts">
  // A printer's camera, through POP (#61, #67): MSE over POP's WebSocket for
  // cameras that send H.264, MJPEG for those that send JPEG pictures, as
  // player.ts decides. For members: one not signed in is asked to sign in.
  // Marked as a camera, so a feedback screenshot never holds a frame (#96).
  import { onDestroy, onMount } from 'svelte';
  import { sensitive } from '../feedback/sensitive';
  import { addresses, catchUp, codecs, said } from './player';

  let {
    printer,
    signedIn,
    onsignin = () => {},
  }: {
    /** The printer's BAM! id. */
    printer: string;
    signedIn: boolean;
    onsignin?: () => void;
  } = $props();

  type Showing = 'connecting' | 'mse' | 'mjpeg' | 'no_picture' | 'lost';
  let showing: Showing = $state('connecting');
  let video: HTMLVideoElement | undefined = $state();
  let socket: WebSocket | null = null;
  let objectUrl: string | null = null;

  /** MediaSource, or iOS's ManagedMediaSource where that is all there is. */
  function mediaSource(): typeof MediaSource | null {
    const w = window as unknown as { MediaSource?: typeof MediaSource; ManagedMediaSource?: typeof MediaSource };
    return w.MediaSource ?? w.ManagedMediaSource ?? null;
  }

  function stop() {
    socket?.close();
    socket = null;
    if (objectUrl) URL.revokeObjectURL(objectUrl);
    objectUrl = null;
  }

  function start() {
    stop();
    const MS = mediaSource();
    const offer = MS ? codecs((m) => MS.isTypeSupported(m)) : '';
    if (!offer) {
      showing = 'mjpeg';
      return;
    }
    showing = 'connecting';
    const where = addresses(printer, location);
    const ws = new WebSocket(where.mse);
    ws.binaryType = 'arraybuffer';
    socket = ws;
    const queue: ArrayBuffer[] = [];
    let buffer: SourceBuffer | null = null;
    const feed = () => {
      if (!buffer || buffer.updating || queue.length === 0) return;
      buffer.appendBuffer(queue.shift()!);
    };
    ws.onopen = () => ws.send(JSON.stringify({ type: 'mse', value: offer }));
    ws.onmessage = (m) => {
      if (typeof m.data !== 'string') {
        queue.push(m.data as ArrayBuffer);
        feed();
        return;
      }
      const s = said(m.data);
      if (s.kind === 'mjpeg') {
        stop();
        showing = 'mjpeg';
      } else if (s.kind === 'playing') {
        const ms = new MS!();
        objectUrl = URL.createObjectURL(ms);
        showing = 'mse';
        ms.addEventListener('sourceopen', () => {
          buffer = ms.addSourceBuffer(s.mime);
          buffer.mode = 'segments';
          buffer.addEventListener('updateend', () => {
            // Stay near live: a camera is watched now, not replayed.
            if (video && buffer && buffer.buffered.length > 0) {
              const to = catchUp(video.currentTime, buffer.buffered.end(buffer.buffered.length - 1));
              if (to !== null) video.currentTime = to;
            }
            feed();
          });
          feed();
        });
      } else {
        stop();
        showing = 'no_picture';
      }
    };
    ws.onclose = () => {
      if (socket === ws && showing !== 'mjpeg' && showing !== 'no_picture') showing = 'lost';
    };
  }

  onMount(() => {
    if (signedIn) start();
  });
  onDestroy(stop);
  $effect(() => {
    if (video && objectUrl && video.src !== objectUrl) {
      video.src = objectUrl;
      void video.play().catch(() => {});
    }
  });
</script>

<div class="camera" data-testid="camera" data-showing={signedIn ? showing : 'sign_in'}>
  {#if !signedIn}
    <p class="note">The camera is for members. <button type="button" class="link" onclick={onsignin}>Sign in</button> to watch it.</p>
  {:else if showing === 'mse' || showing === 'connecting'}
    <!-- Live, so there is nothing to caption: no sound is played. -->
    <video bind:this={video} muted playsinline autoplay use:sensitive={'camera'} aria-label="The printer’s camera, live"></video>
    {#if showing === 'connecting'}<p class="note overlay">Connecting to the camera…</p>{/if}
  {:else if showing === 'mjpeg'}
    <img src={addresses(printer, location).mjpeg} alt="The printer’s camera, live" use:sensitive={'camera'} />
  {:else if showing === 'no_picture'}
    <p class="note">The camera isn’t giving a picture right now.</p>
  {:else}
    <p class="note">The camera stopped. <button type="button" class="link" onclick={start}>Try again</button></p>
  {/if}
</div>

<style>
  .camera {
    position: relative;
    width: 100%;
    aspect-ratio: 16 / 9;
    background: #111;
    border-radius: 10px;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  video,
  img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .note {
    color: #eee;
    padding: 16px;
    text-align: center;
  }
  .overlay {
    position: absolute;
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
