<script lang="ts">
  // The whole guide as a flowchart, with the path taken so far marked (#56).
  import { layout, BOX_W, BOX_H } from './layout';
  import type { Walk, Walkable } from './walk';

  let { guide, walk, label }: { guide: Walkable; walk: Walk; label: string } = $props();

  const chart = $derived(layout(guide));
  const here = $derived(walk.trail[walk.trail.length - 1]);
  const been = $derived(new Set(walk.trail));
  /** Each arrow taken, as "from>to". */
  const taken = $derived(new Set(walk.trail.slice(1).map((to, i) => `${walk.trail[i]}>${to}`)));
  const title = (id: string) => guide.steps.find((s) => s.id === id)?.title ?? id;
  /** As much of a title as fits a box, on two lines. */
  function lines(t: string): string[] {
    const words = t.split(' ');
    const out: string[] = [''];
    for (const w of words) {
      const last = out[out.length - 1]!;
      if ((last + ' ' + w).trim().length <= 22) out[out.length - 1] = (last + ' ' + w).trim();
      else if (out.length < 2) out.push(w);
      else {
        out[1] = out[1]!.replace(/\s*\S*$/, '') + '…';
        break;
      }
    }
    return out;
  }
</script>

<svg
  class="map"
  viewBox="0 0 {chart.width} {chart.height}"
  width="100%"
  role="img"
  aria-label={label}
  data-testid="guide-map"
>
  {#each chart.arrows as a (a.from + '>' + a.to + a.label)}
    {@const on = taken.has(`${a.from}>${a.to}`)}
    <path
      d={a.d}
      fill="none"
      stroke={on ? 'var(--pop-butter)' : 'var(--pop-line)'}
      stroke-width="3"
      stroke-dasharray={a.back ? '6 5' : undefined}
      data-taken={on}
    />
    {#if a.label}
      {@const from = chart.boxes.find((b) => b.id === a.from)!}
      {@const to = chart.boxes.find((b) => b.id === a.to)!}
      <text
        x={a.back ? from.x + BOX_W + 4 : (from.x + to.x) / 2 + BOX_W / 2 + 6}
        y={a.back ? from.y + BOX_H / 2 - 6 : from.y + BOX_H + 16}
        class="lbl">{a.label}</text
      >
    {/if}
  {/each}
  {#each chart.boxes as b (b.id)}
    {@const t = lines(title(b.id))}
    <g data-step={b.id} data-been={been.has(b.id)} data-here={b.id === here}>
      <title>{title(b.id)}</title>
      <rect
        x={b.x}
        y={b.y}
        width={BOX_W}
        height={BOX_H}
        rx="4"
        fill={b.id === here ? 'var(--pop-butter)' : been.has(b.id) ? 'var(--pop-sage)' : 'var(--pop-panel-2)'}
        stroke={b.done ? 'var(--pop-line)' : 'var(--pop-ink)'}
        stroke-width={b.done ? 2 : 3}
        stroke-dasharray={b.done ? '6 5' : undefined}
      />
      {#each t as line, i (i)}
        <text
          x={b.x + BOX_W / 2}
          y={b.y + (t.length === 1 ? 27 : 19 + i * 16)}
          text-anchor="middle"
          class="box"
          class:dark={been.has(b.id)}>{line}</text
        >
      {/each}
    </g>
  {/each}
</svg>

<style>
  .map {
    display: block;
    max-width: 100%;
    height: auto;
  }
  .box {
    font: 13px var(--pop-font);
    fill: var(--pop-text);
  }
  .box.dark {
    fill: var(--pop-ink);
  }
  .lbl {
    font: 12px var(--pop-font);
    fill: var(--pop-muted);
  }
</style>
