// @vitest-environment jsdom
// The text box on a feedback screenshot (#192): Escape puts it away, and
// choosing another tool, another place or Next is done with it, keeping what
// was typed. Konva needs a real canvas, so a stand-in records what is drawn
// and lets a test press on the picture.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';

const k = vi.hoisted(() => ({
  /** The stage's handlers, by event. */
  on: new Map<string, () => void>(),
  /** Where the pointer is, on the picture. */
  pointer: { x: 0, y: 0 },
  /** Every text drawn, as it was drawn. */
  texts: [] as string[],
}));

vi.mock('konva', () => {
  class Node {
    name() {}
    destroy() {}
  }
  class Stage {
    constructor({ container }: { container: HTMLElement }) {
      const content = document.createElement('div');
      content.className = 'konvajs-content';
      container.appendChild(content);
    }
    on(events: string, f: () => void) {
      for (const e of events.split(' ')) k.on.set(e, f);
    }
    getPointerPosition() {
      return k.pointer;
    }
    width() {
      return 400;
    }
    height() {
      return 300;
    }
    add() {}
    destroy() {}
    toDataURL() {
      return 'data:image/png;base64,UE5H';
    }
  }
  class Layer {
    add() {}
    find() {
      return [];
    }
    batchDraw() {}
  }
  class Text extends Node {
    constructor({ text }: { text: string }) {
      super();
      k.texts.push(text);
    }
  }
  return { default: { Stage, Layer, Image: Node, Line: Node, Arrow: Node, Rect: Node, Text } };
});

import Annotate from './Annotate.svelte';

afterEach(() => {
  cleanup();
  k.on.clear();
  k.texts.length = 0;
});

function picture(): HTMLCanvasElement {
  const c = document.createElement('canvas');
  c.width = 400;
  c.height = 300;
  return c;
}

/** What typeAt needs of a rendered screen. */
type Screen = { getByTestId: (id: string) => HTMLElement; getByLabelText: (text: string) => HTMLElement };

/** Choose the text tool, press the picture at (x, y), and type `text`. */
async function typeAt(p: Screen, x: number, y: number, text: string) {
  await fireEvent.click(p.getByTestId('tool-text'));
  k.pointer = { x, y };
  k.on.get('pointerdown')!();
  const input = await waitFor(() => p.getByLabelText('Text to add') as HTMLInputElement);
  await fireEvent.input(input, { target: { value: text } });
  return input;
}

describe('the text box on the screenshot', () => {
  it('becomes a mark when added', async () => {
    const p = render(Annotate, { picture: picture() });
    const input = await typeAt(p, 40, 30, 'the X1');
    await fireEvent.submit(input.form!);
    await waitFor(() => expect(p.queryByTestId('feedback-typing')).toBeNull());
    expect(k.texts).toContain('the X1');
  });

  it('goes away on Escape, and what was typed with it', async () => {
    const p = render(Annotate, { picture: picture() });
    const input = await typeAt(p, 40, 30, 'never mind');
    await fireEvent.keyDown(input, { key: 'Escape' });
    await waitFor(() => expect(p.queryByTestId('feedback-typing')).toBeNull());
    await fireEvent.click(p.getByTestId('annotate-done'));
    expect(k.texts).not.toContain('never mind');
  });

  it('is done with when another tool is chosen, keeping what was typed', async () => {
    const p = render(Annotate, { picture: picture() });
    await typeAt(p, 40, 30, 'here');
    await fireEvent.click(p.getByTestId('tool-box'));
    await waitFor(() => expect(p.queryByTestId('feedback-typing')).toBeNull());
    expect(k.texts).toContain('here');
  });

  it('keeps what was typed when another place is pressed', async () => {
    const p = render(Annotate, { picture: picture() });
    await typeAt(p, 40, 30, 'first');
    k.pointer = { x: 200, y: 150 };
    k.on.get('pointerdown')!();
    await waitFor(() => expect(k.texts).toContain('first'));
    expect((p.getByLabelText('Text to add') as HTMLInputElement).value).toBe('');
  });

  it('is kept when Next is pressed while it is open', async () => {
    const ondone = vi.fn();
    const p = render(Annotate, { picture: picture(), ondone });
    await typeAt(p, 40, 30, 'last word');
    await fireEvent.click(p.getByTestId('annotate-done'));
    expect(k.texts).toContain('last word');
    expect(ondone).toHaveBeenCalledOnce();
  });

  it('opens where the picture was pressed, though the picture is centered', async () => {
    const p = render(Annotate, { picture: picture() });
    const content = p.container.querySelector('.konvajs-content')!;
    // The picture sits 120px in and 70px down from the screen's corner.
    content.getBoundingClientRect = () => ({ left: 120, top: 70 }) as DOMRect;
    await typeAt(p, 40, 30, 'x');
    const box = p.getByTestId('feedback-typing');
    expect(box.style.left).toBe('160px');
    expect(box.style.top).toBe('100px');
  });
});
