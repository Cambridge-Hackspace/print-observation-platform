// @vitest-environment jsdom
// The design system (#48), held to the approved round 6 kit.
import { afterEach, describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { cleanup, render } from '@testing-library/svelte';
import golden from './golden.json';
import { fmt1 } from './geometry';
import { media } from '../../test/setup';
import PrinterDrawing from './PrinterDrawing.svelte';
import Spool from './Spool.svelte';
import Pine from './Pine.svelte';
import Fern from './Fern.svelte';
import Shroom from './Shroom.svelte';
import ProgressBar from './ProgressBar.svelte';
import StatusSquare from './StatusSquare.svelte';

afterEach(cleanup);

// ---- structural comparison with the kit's output -----------------------------------

interface Node {
  tag: string;
  attrs: Record<string, string>;
  children: Node[];
}

/** An element as tag, attributes and child elements, normalized only where
 * the kit and Svelte write the same thing differently: the case of a color,
 * the spacing of a style, and a drawing's pattern id. */
function tree(el: Element, id?: string): Node {
  const attrs: Record<string, string> = {};
  for (const a of Array.from(el.attributes)) {
    let v = a.value.toLowerCase();
    if (a.name === 'style') v = v.replace(/\s+/g, '').replace(/;$/, '');
    if (id) v = v.split(id.toLowerCase()).join('g');
    attrs[a.name] = v;
  }
  return { tag: el.tagName.toLowerCase(), attrs, children: Array.from(el.children).map((c) => tree(c, id)) };
}

function goldenSvg(html: string): Element {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  return doc.querySelector('svg')!;
}

function svgTarget(): SVGSVGElement {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  document.body.appendChild(svg);
  return svg;
}

const kind = (k: string) => (k === 'e' ? 'enclosed' : 'open') as 'enclosed' | 'open';

describe('the drawings match the round 6 kit', () => {
  for (const g of golden.printer) {
    it(`a printer: ${kind(g.args.kind)}, body ${g.args.body}, part ${g.args.part}`, () => {
      const { container } = render(PrinterDrawing, {
        kind: kind(g.args.kind),
        body: g.args.body,
        nozzle: g.args.nozzle,
        part: g.args.part,
        size: 120,
        label: 'X1',
        // The kit's screen and glass, as it passes them.
        screen: golden.palette.butter,
        id: 'g',
      });
      expect(tree(container.querySelector('svg')!)).toEqual(tree(goldenSvg(g.svg)));
    });
  }

  for (const g of golden.spool) {
    it(`a spool: ${g.args.color} at ${g.args.frac}`, () => {
      const { container } = render(Spool, { color: g.args.color, frac: g.args.frac, size: 56, label: 'PLA' });
      expect(tree(container.querySelector('svg')!)).toEqual(tree(goldenSvg(g.svg)));
    });
  }

  for (const g of golden.pine) {
    it(`a pine at ${g.args.x},${g.args.b}, ${g.args.h} high${g.args.ink ? ', inked' : ''}`, () => {
      const target = svgTarget();
      render(Pine, { target, props: { x: g.args.x, b: g.args.b, h: g.args.h, fill: g.args.fill, ink: g.args.ink } });
      expect(tree(target).children).toEqual(tree(goldenSvg(g.svg)).children);
    });
  }

  for (const g of golden.fern) {
    it(`a fern ${g.args.h} high`, () => {
      const target = svgTarget();
      render(Fern, { target, props: { x: g.args.x, b: g.args.b, h: g.args.h, color: g.args.color } });
      expect(tree(target).children).toEqual(tree(goldenSvg(g.svg)).children);
    });
  }

  for (const g of golden.shroom) {
    it(`a mushroom of size ${g.args.s}`, () => {
      const target = svgTarget();
      render(Shroom, { target, props: { x: g.args.x, b: g.args.b, s: g.args.s, cap: g.args.cap } });
      expect(tree(target).children).toEqual(tree(goldenSvg(g.svg)).children);
    });
  }

  it('gives each drawing its own pattern, so two on one page do not share one', () => {
    const a = render(PrinterDrawing, { kind: 'enclosed', body: '#fff', nozzle: '#000', label: 'A' });
    const b = render(PrinterDrawing, { kind: 'open', body: '#fff', nozzle: '#000', label: 'B' });
    const ida = a.container.querySelector('pattern')!.id;
    const idb = b.container.querySelector('pattern')!.id;
    expect(ida).not.toBe(idb);
    expect(a.container.querySelector(`[fill="url(#${ida})"]`)).not.toBeNull();
  });
});

describe('the tokens are the kit\'s palette', () => {
  // Vitest runs from web/; under it, CSS imports are handled, not read.
  const css = readFileSync(resolve('src/lib/design/tokens.css'), 'utf8');
  const token = (name: string) => css.match(new RegExp(`--pop-${name}:\\s*(#[0-9a-fA-F]{6})`))?.[1]?.toLowerCase();
  // Duplicated deliberately: the kit's names to the tokens' names.
  const names: Record<string, string> = {
    ink: 'ink', ground: 'ground', panel: 'panel', panel2: 'panel-2', text: 'text', muted: 'muted',
    sage: 'sage', butter: 'butter', coral: 'coral', cream: 'cream', moss: 'moss', dusk: 'dusk',
    far: 'far', mid: 'mid', hill: 'hill', soil: 'soil', line: 'line', dim: 'dim',
  };
  for (const [kit, tok] of Object.entries(names)) {
    it(`--pop-${tok} is the kit's ${kit}`, () => {
      expect(token(tok)).toBe((golden.palette as Record<string, string>)[kit]!.toLowerCase());
    });
  }
  it('names every color the kit has', () => {
    expect(Object.keys(golden.palette).sort()).toEqual(Object.keys(names).sort());
  });
});

describe('numbers are written as the kit writes them', () => {
  it("breaks an exact tie to even, as Python's :.1f does", () => {
    expect(fmt1(0.25)).toBe('0.2');
    expect(fmt1(0.75)).toBe('0.8');
    expect(fmt1(2.5)).toBe('2.5');
    expect(fmt1(-0.25)).toBe('-0.2');
    expect(fmt1(1.05)).toBe('1.1'); // 1.05 is just above 1.05 in binary
    expect(fmt1(7)).toBe('7.0');
    expect(fmt1(0.35)).toBe('0.3'); // stored just below 0.35
    expect(fmt1(12.65)).toBe('12.7'); // stored just above
  });
});

describe('progress', () => {
  it('says what is progressing, and how far, to a screen reader', () => {
    const { getByRole } = render(ProgressBar, { value: 0.42, label: 'Benchy on the X1' });
    const bar = getByRole('progressbar', { name: 'Benchy on the X1' });
    expect(bar.getAttribute('aria-valuenow')).toBe('42');
    expect(bar.getAttribute('aria-valuemin')).toBe('0');
    expect(bar.getAttribute('aria-valuemax')).toBe('100');
  });

  it('keeps a value outside 0 to 1, or not a number, inside it', () => {
    for (const [v, want] of [[1.7, '100'], [-0.2, '0'], [Number.NaN, '0']] as const) {
      const { getByRole, unmount } = render(ProgressBar, { value: v, label: 'x' });
      expect(getByRole('progressbar').getAttribute('aria-valuenow')).toBe(want);
      unmount();
    }
  });

  it('jumps at once for anyone who asked for reduced motion, and eases otherwise', async () => {
    media.reducedMotion = true;
    const still = render(ProgressBar, { value: 0.6, label: 'x' });
    await new Promise((r) => setTimeout(r, 20));
    expect(parseFloat((still.container.querySelector('i') as HTMLElement).style.width)).toBe(60);
    cleanup();
    media.reducedMotion = false;
    const moving = render(ProgressBar, { value: 0.6, label: 'x' });
    await new Promise((r) => setTimeout(r, 20));
    const w = parseFloat((moving.container.querySelector('i') as HTMLElement).style.width);
    expect(w).toBeLessThan(60);
  });
});

describe('status', () => {
  it('always carries its meaning in words, not only color', () => {
    const shown = render(StatusSquare, { color: '#9dbb8c', label: 'Printing' });
    expect(shown.getByText('Printing')).toBeTruthy();
    cleanup();
    const quiet = render(StatusSquare, { color: '#9dbb8c', label: 'Printing', show: false });
    expect(quiet.getByText('Printing')).toBeTruthy();
  });
});
