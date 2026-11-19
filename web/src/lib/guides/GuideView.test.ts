// @vitest-environment jsdom
// The guide viewer (#56, #72, #94): one step at a time, large buttons, and
// the whole flowchart with the path taken so far.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import GuideView from './GuideView.svelte';
import GuidePage from './GuidePage.svelte';
import GuidesIndex from './GuidesIndex.svelte';
import { STRINGING } from './fixture';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const view = (layout: 'kiosk' | 'page' = 'kiosk') =>
  render(GuideView, {
    guide: STRINGING,
    title: STRINGING.title,
    imageUrl: (id: string) => `/img/${id}`,
    layout,
  });

const marked = (c: HTMLElement, attr: 'been' | 'here') =>
  [...c.querySelectorAll(`[data-step][data-${attr}="true"]`)].map((g) => g.getAttribute('data-step'));

describe('the guide viewer', () => {
  it('asks one question at a time and goes where the answer leads', async () => {
    const v = view();
    expect(v.getByTestId('step-title').textContent).toBe('Are there fine hairs between parts?');
    expect(v.getByTestId('where').textContent).toBe('Step 1 of about 2');
    await fireEvent.click(v.getByRole('button', { name: 'Yes' }));
    expect(v.getByTestId('step-title').textContent).toBe('Has this spool been out of the dry box for more than a day?');
    expect(v.getByTestId('where').textContent).toBe('Step 2 of about 4');
    expect(v.getByTestId('said').textContent).toBe('You said yes to: Are there fine hairs between parts?');
    expect(v.getByText(/PETG soaks up water/)).toBeTruthy();
    await fireEvent.click(v.getByRole('button', { name: 'Yes' }));
    expect(v.getByTestId('step-title').textContent).toBe('Dry it for 4 hours at 65 °C.');
    expect((v.container.querySelector('img') as HTMLImageElement).getAttribute('src')).toBe('/img/abc123');
    await fireEvent.click(v.getByRole('button', { name: 'Next' }));
    expect(v.getByTestId('step-title').textContent).toBe('Print the test tower.');
    expect(v.getByTestId('where').textContent).toBe('Step 4: the last one');
    expect(v.queryByRole('button', { name: 'Next' })).toBeNull();
  });

  it('goes back, and starts over from the end', async () => {
    const v = view();
    await fireEvent.click(v.getByRole('button', { name: 'No' }));
    expect(v.getByTestId('step-title').textContent).toContain('blobs');
    await fireEvent.click(v.getByRole('button', { name: 'Back' }));
    expect(v.getByTestId('step-title').textContent).toBe('Are there fine hairs between parts?');
    expect(v.queryByRole('button', { name: 'Back' })).toBeNull();
    await fireEvent.click(v.getByRole('button', { name: 'No' }));
    await fireEvent.click(v.getByRole('button', { name: 'Start over' }));
    expect(v.getByTestId('step-title').textContent).toBe('Are there fine hairs between parts?');
  });

  it('marks the path taken so far on the whole flowchart', async () => {
    const v = view();
    expect(marked(v.container, 'here')).toEqual(['hairs']);
    await fireEvent.click(v.getByRole('button', { name: 'Yes' }));
    await fireEvent.click(v.getByRole('button', { name: 'No' }));
    expect(marked(v.container, 'here')).toEqual(['cooler']);
    expect(marked(v.container, 'been').sort()).toEqual(['cooler', 'dry-box', 'hairs']);
    const taken = [...v.container.querySelectorAll('path[data-taken="true"]')].length;
    expect(taken).toBe(2);
    expect(v.getByRole('img', { name: 'The Stringing on PETG flowchart, with your path so far marked' })).toBeTruthy();
  });

  it('starts another guide at its own beginning', async () => {
    const v = view();
    await fireEvent.click(v.getByRole('button', { name: 'Yes' }));
    const other = {
      ...STRINGING,
      slug: 'warping',
      start: 'corners',
      steps: [{ id: 'corners', title: 'Do the corners lift?', body: '', image: null, then: { kind: 'done' as const } }],
    };
    await v.rerender({ guide: other, title: 'Warping', imageUrl: (id: string) => id, layout: 'kiosk' });
    expect(v.getByTestId('step-title').textContent).toBe('Do the corners lift?');
    expect(v.getByTestId('where').textContent).toBe('Step 1: the last one');
  });

  it('puts the flowchart beside the step at the kiosk, and below it on a page', () => {
    const k = view('kiosk');
    expect(k.container.querySelector('aside [data-testid="guide-map"]')).toBeTruthy();
    cleanup();
    const p = view('page');
    expect(p.container.querySelector('details [data-testid="guide-map"]')).toBeTruthy();
  });

  it('keeps the flowchart at its own size on a page, to scroll sideways on a phone, and fits it at the kiosk (#72)', () => {
    const k = view('kiosk');
    expect(k.getByTestId('guide-map').getAttribute('width')).toBe('100%');
    cleanup();
    const p = view('page');
    const map = p.getByTestId('guide-map');
    expect(map.closest('[data-testid="map-scroll"]')).toBeTruthy();
    const [, , w, h] = map.getAttribute('viewBox')!.split(' ');
    expect(map.getAttribute('width')).toBe(w);
    expect(map.getAttribute('height')).toBe(h);
  });
});

describe('the public guide page', () => {
  it('shows a published guide with its license', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(STRINGING), { status: 200 })));
    const p = render(GuidePage, { slug: 'stringing-on-petg' });
    await waitFor(() => expect(p.getByRole('heading', { level: 1 }).textContent).toBe('Stringing on PETG'));
    expect(p.getByText('Shared under CC BY-SA 4.0.')).toBeTruthy();
    // It asks for that guide by its slug, and nothing else but the version
    // line's /api/status (#189).
    const asked = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.map((c) => c[0]);
    expect(asked.filter((u) => u !== '/api/status')).toEqual(['/api/public/guides/stringing-on-petg']);
    expect(p.getByRole('link', { name: /All guides/ }).getAttribute('href')).toBe('/guides');
    // And back to the phone app (#72).
    expect(p.getByRole('link', { name: 'Printers' }).getAttribute('href')).toBe('/');
  });

  it('says so when there is no such published guide', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response('{"error":"no"}', { status: 404 })));
    const p = render(GuidePage, { slug: 'warping' });
    await waitFor(() => expect(p.getByRole('heading', { level: 1 }).textContent).toBe('No such guide'));
  });

  it('leads back to the printers at the kiosk', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(STRINGING), { status: 200 })));
    const p = render(GuidePage, { slug: 'stringing-on-petg', kiosk: true });
    await waitFor(() => expect(p.getByTestId('guide')).toBeTruthy());
    expect(p.getByRole('link', { name: /Back to the printers/ }).getAttribute('href')).toBe('/kiosk');
    expect(p.queryByRole('link', { name: 'Printers' }), 'the kiosk has its own way back').toBeNull();
    expect(p.container.querySelector('aside [data-testid="guide-map"]')).toBeTruthy();
  });
});

describe('the guides index', () => {
  it('lists the published guides and leads back to the phone app (#72)', async () => {
    const list = { guides: [{ slug: 'stringing-on-petg', title: 'Stringing on PETG', summary: 'Hairs between parts.' }], license: null };
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(list), { status: 200 })));
    const p = render(GuidesIndex);
    await waitFor(() => expect(p.getByRole('link', { name: 'Stringing on PETG' }).getAttribute('href')).toBe('/guides/stringing-on-petg'));
    expect(p.getByRole('link', { name: 'Printers' }).getAttribute('href')).toBe('/');
  });
});
