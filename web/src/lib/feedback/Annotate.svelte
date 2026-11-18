<script lang="ts">
  // Drawing on a feedback screenshot (#96), full screen and sized for a
  // finger: pen, arrow, box, text, and black-out for anything redaction
  // missed. What is drawn is marks.ts's; Konva draws it.
  import Konva from 'konva';
  import { onDestroy, onMount } from 'svelte';
  import { Button } from '../design';
  import { add, none, rect, redo, undo, type Mark, type Marks, type Tool } from './marks';

  let {
    picture,
    ondone = () => {},
    oncancel = () => {},
  }: {
    /** The redacted screenshot. */
    picture: HTMLCanvasElement;
    /** The annotated picture, as a PNG in base64. */
    ondone?: (png: string) => void;
    oncancel?: () => void;
  } = $props();

  const TOOLS: { tool: Tool; label: string }[] = [
    { tool: 'pen', label: 'Pen' },
    { tool: 'arrow', label: 'Arrow' },
    { tool: 'box', label: 'Box' },
    { tool: 'text', label: 'Text' },
    { tool: 'blackout', label: 'Black out' },
  ];
  const INK = '#e8553e';

  let tool: Tool = $state('pen');
  let marks: Marks = $state(none());
  let typing: { at: [number, number]; text: string } | null = $state(null);
  let host: HTMLDivElement;
  let stage: Konva.Stage | null = null;
  let layer: Konva.Layer | null = null;
  let scale = 1;
  let drawing: Mark | null = null;

  function shape(m: Mark): Konva.Shape {
    switch (m.tool) {
      case 'pen':
        return new Konva.Line({ points: m.points, stroke: INK, strokeWidth: 5, lineCap: 'round', lineJoin: 'round' });
      case 'arrow':
        return new Konva.Arrow({
          points: [...m.from, ...m.to],
          stroke: INK,
          fill: INK,
          strokeWidth: 5,
          pointerLength: 18,
          pointerWidth: 18,
        });
      case 'box':
        return new Konva.Rect({ ...rect(m.from, m.to), stroke: INK, strokeWidth: 5, cornerRadius: 6 });
      case 'blackout':
        return new Konva.Rect({ ...rect(m.from, m.to), fill: '#000' });
      case 'text':
        return new Konva.Text({ x: m.at[0], y: m.at[1], text: m.text, fontSize: 28, fill: INK, fontStyle: 'bold' });
    }
  }

  function draw() {
    if (!layer) return;
    layer.find('.mark').forEach((n) => n.destroy());
    for (const m of [...marks.done, ...(drawing ? [drawing] : [])]) {
      const s = shape(m);
      s.name('mark');
      layer.add(s);
    }
    layer.batchDraw();
  }

  function at(): [number, number] {
    const p = stage!.getPointerPosition() ?? { x: 0, y: 0 };
    return [p.x, p.y];
  }

  onMount(() => {
    const room = { width: host.clientWidth, height: host.clientHeight };
    scale = Math.min(room.width / picture.width, room.height / picture.height, 1);
    stage = new Konva.Stage({
      container: host,
      width: Math.round(picture.width * scale),
      height: Math.round(picture.height * scale),
    });
    layer = new Konva.Layer();
    layer.add(new Konva.Image({ image: picture, width: stage.width(), height: stage.height() }));
    stage.add(layer);
    stage.on('pointerdown', () => {
      const p = at();
      if (tool === 'text') {
        typing = { at: p, text: '' };
        return;
      }
      drawing = tool === 'pen' ? { tool, points: [...p] } : { tool, from: p, to: p };
    });
    stage.on('pointermove', () => {
      if (!drawing) return;
      const p = at();
      if (drawing.tool === 'pen') drawing.points.push(...p);
      else if ('to' in drawing) drawing.to = p;
      draw();
    });
    const finish = () => {
      if (!drawing) return;
      marks = add(marks, drawing);
      drawing = null;
      draw();
    };
    stage.on('pointerup pointerleave', finish);
  });

  onDestroy(() => stage?.destroy());

  $effect(() => {
    void marks;
    draw();
  });

  function placeText() {
    if (typing) marks = add(marks, { tool: 'text', at: typing.at, text: typing.text });
    typing = null;
  }

  function done() {
    if (!stage) return;
    // At the screenshot's own size, whatever size it was drawn at.
    const url = stage.toDataURL({ mimeType: 'image/png', pixelRatio: 1 / scale });
    ondone(url.replace(/^data:image\/png;base64,/, ''));
  }
</script>

<div class="annotate" data-testid="feedback-annotate">
  <div class="tools" role="toolbar" aria-label="Drawing tools">
    {#each TOOLS as t (t.tool)}
      <Button
        size="sm"
        variant={tool === t.tool ? 'primary' : 'plain'}
        aria-pressed={tool === t.tool}
        data-testid="tool-{t.tool}"
        onclick={() => (tool = t.tool)}>{t.label}</Button
      >
    {/each}
    <span class="gap"></span>
    <Button size="sm" disabled={marks.done.length === 0} onclick={() => (marks = undo(marks))}>Undo</Button>
    <Button size="sm" disabled={marks.undone.length === 0} onclick={() => (marks = redo(marks))}>Redo</Button>
    <span class="gap"></span>
    <Button size="sm" onclick={oncancel}>Cancel</Button>
    <Button size="sm" variant="primary" data-testid="annotate-done" onclick={done}>Next</Button>
  </div>
  <div class="canvas" bind:this={host}></div>
  {#if typing}
    <form class="typing" style="left:{typing.at[0]}px; top:{typing.at[1] + 56}px" onsubmit={(e) => (e.preventDefault(), placeText())}>
      <label for="feedback-mark-text" class="visually-hidden">Text to add</label>
      <!-- svelte-ignore a11y_autofocus -->
      <input id="feedback-mark-text" bind:value={typing.text} autofocus />
      <Button size="sm" type="submit">Add</Button>
    </form>
  {/if}
</div>

<style>
  .annotate {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    flex-direction: column;
    background: var(--pop-ink, #1d2620);
  }
  .tools {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px;
    background: var(--pop-cream, #f6f1e6);
  }
  .gap {
    flex: 1;
  }
  .canvas {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    touch-action: none;
  }
  .typing {
    position: absolute;
    display: flex;
    gap: 6px;
  }
  .typing input {
    font-size: 20px;
    padding: 8px;
  }
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
