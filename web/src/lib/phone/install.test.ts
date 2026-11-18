// Installable as a web app (#60): the manifest the browser reads, the icons it
// names, and index.html's links to them, checked against the files themselves
// so a renamed or resized icon fails here rather than on someone's phone.
import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const WEB = join(__dirname, '..', '..', '..');
const PUBLIC = join(WEB, 'public');

/** A PNG's width and height, from its IHDR chunk. */
function pngSize(file: string): [number, number] {
  const b = readFileSync(file);
  expect(b.subarray(1, 4).toString('ascii'), `${file} is a PNG`).toBe('PNG');
  return [b.readUInt32BE(16), b.readUInt32BE(20)];
}

const manifest = JSON.parse(readFileSync(join(PUBLIC, 'manifest.webmanifest'), 'utf8')) as {
  name: string;
  short_name: string;
  start_url: string;
  scope: string;
  display: string;
  icons: { src: string; sizes: string; type: string; purpose: string }[];
};
const html = readFileSync(join(WEB, 'index.html'), 'utf8');

describe('the web app manifest', () => {
  it('opens POP full-screen at the phone app', () => {
    expect(manifest.name).toBe('POP');
    expect(manifest.short_name).toBe('POP');
    expect(manifest.start_url).toBe('/');
    expect(manifest.scope).toBe('/');
    expect(manifest.display).toBe('standalone');
  });

  it('names icons that exist at the sizes it says, including 192 and 512 and a maskable one', () => {
    for (const icon of manifest.icons) {
      const file = join(PUBLIC, icon.src.replace(/^\//, ''));
      expect(existsSync(file), icon.src).toBe(true);
      if (icon.type === 'image/png') {
        const [w, h] = pngSize(file);
        expect(`${w}x${h}`, icon.src).toBe(icon.sizes);
      }
    }
    const pngs = manifest.icons.filter((i) => i.type === 'image/png');
    expect(pngs.map((i) => i.sizes)).toEqual(expect.arrayContaining(['192x192', '512x512']));
    expect(manifest.icons.some((i) => i.purpose === 'maskable')).toBe(true);
  });
});

describe('index.html', () => {
  const href = (rel: string) => new RegExp(`<link rel="${rel}" href="([^"]+)"`).exec(html)?.[1];

  it('links the manifest, and the icon iOS puts on the home screen, which exist', () => {
    expect(href('manifest')).toBe('/manifest.webmanifest');
    const touch = href('apple-touch-icon');
    expect(touch).toBeDefined();
    expect(pngSize(join(PUBLIC, touch!.replace(/^\//, '')))).toEqual([180, 180]);
  });

  it('asks iOS to open it as an app', () => {
    expect(html).toContain('<meta name="apple-mobile-web-app-capable" content="yes" />');
  });
});
