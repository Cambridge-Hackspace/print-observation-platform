// @vitest-environment jsdom
// Redaction before capture (#96), checked on the clone rather than by
// reading an image back: nothing marked survives, the backstop catches what
// was not marked, and the page on screen is untouched.
import { describe, expect, it } from 'vitest';
import { ATTRIBUTE, backstop, redact } from './redact';

function screen(html: string): HTMLElement {
  const root = document.createElement('div');
  root.innerHTML = html;
  document.body.replaceChildren(root);
  return root;
}

const NAME = 'Ada Lovelace';
const EMAIL = 'ada@example.org';

describe('redaction before capture', () => {
  it('leaves nothing marked in the clone, and the page as it was', () => {
    const page = screen(`
      <header><span ${ATTRIBUTE}="name">${NAME}</span></header>
      <p>Owner: <b ${ATTRIBUTE}="name">${NAME}</b>, reach her at <i ${ATTRIBUTE}="email">${EMAIL}</i></p>
      <span ${ATTRIBUTE}="file">benchy-for-ada.gcode</span>
      <span ${ATTRIBUTE}="card">04A2B3C4D5E6F7</span>`);
    const before = page.innerHTML;
    const clone = page.cloneNode(true) as HTMLElement;
    redact(clone);
    for (const secret of [NAME, 'Ada', 'Lovelace', EMAIL, 'benchy-for-ada', '04A2B3C4D5E6F7']) {
      expect(clone.textContent).not.toContain(secret);
    }
    expect(clone.textContent).toContain('[member]');
    expect(clone.textContent).toContain('[email]');
    expect(clone.textContent).toContain('[file]');
    expect(clone.textContent).toContain('[card]');
    // The text around it stays: only the marked part goes.
    expect(clone.textContent).toContain('Owner:');
    expect(page.innerHTML).toBe(before);
  });

  it('keeps a redacted name about as wide as the name was', () => {
    const page = screen(`<span ${ATTRIBUTE}="name">Bartholomew Featherstonehaugh</span>`);
    const clone = page.cloneNode(true) as HTMLElement;
    redact(clone);
    const block = clone.querySelector('[data-redacted]') as HTMLElement;
    expect(block.style.width).toBe('29ch');
  });

  it('catches what was not marked: an email, a card number, a token', () => {
    const page = screen(`
      <p>Mail kim@example.net about it</p>
      <p>card 4111 1111 1111 1111 and 0412345678901234</p>
      <p>key ghp_abcdefghijklmnopqrstuvwxyz0123456789</p>
      <p>id 123e4567-e89b-12d3-a456-426614174000</p>`);
    const clone = page.cloneNode(true) as HTMLElement;
    redact(clone);
    const t = clone.textContent!;
    for (const secret of ['kim@example.net', '4111 1111 1111 1111', '0412345678901234', 'ghp_', '123e4567']) {
      expect(t).not.toContain(secret);
    }
    expect(t).toContain('Mail [email] about it');
  });

  // Each pattern on its own: one alone must catch its kind, since they overlap.
  it('catches each kind of secret by itself', () => {
    expect(backstop('a ghs_x1 b')).toBe('a [token] b');
    expect(backstop('a github_pat_x b')).toBe('a [token] b');
    // A POP API key (#13), even one with no digit in it.
    expect(backstop('a pop_AbcdefGhijkLmnopQrstuVwxyzAbcdefGhijkLmno b')).toBe('a [token] b');
    expect(backstop('a eyJhbGc.eyJzdWI.c2ln b')).toBe('a [token] b');
    expect(backstop('a Abcdefghij0123456789abcdefghij0123 b')).toBe('a [token] b');
    expect(backstop('a 123e4567-e89b-12d3-a456-426614174000 b')).toBe('a [id] b');
    expect(backstop('a 1234-5678-9012-3456 b')).toBe('a [number] b');
    expect(backstop('a x@y.io b')).toBe('a [email] b');
  });

  it('leaves times, layers and percentages alone', () => {
    expect(backstop('Layer 37 of 120, 45%, done at 3:42 PM, 12 minutes left')).toBe(
      'Layer 37 of 120, 45%, done at 3:42 PM, 12 minutes left',
    );
  });

  it('empties form fields, and makes camera pictures grey boxes', () => {
    const page = screen(`
      <input id="u" value="ada">
      <textarea>my password is hunter2</textarea>
      <video src="/api/printers/p1/camera/mjpeg"></video>
      <img src="/api/printers/p1/camera/mjpeg" width="320" height="240">
      <div ${ATTRIBUTE}="camera">the live drawing</div>
      <img src="/logo.svg">`);
    (page.querySelector('#u') as HTMLInputElement).value = 'ada typed this';
    const clone = page.cloneNode(true) as HTMLElement;
    (clone.querySelector('#u') as HTMLInputElement).value = 'ada typed this';
    redact(clone);
    expect((clone.querySelector('#u') as HTMLInputElement).value).toBe('');
    expect(clone.querySelector('#u')!.getAttribute('value')).toBeNull();
    expect(clone.querySelector('textarea')!.textContent).toBe('');
    expect(clone.querySelector('video')).toBeNull();
    expect(clone.querySelectorAll('img').length).toBe(1);
    expect(clone.querySelector('img')!.getAttribute('src')).toBe('/logo.svg');
    expect(clone.textContent).not.toContain('the live drawing');
    expect(clone.textContent!.match(/\[camera\]/g)?.length).toBe(3);
    // The page keeps what was typed.
    expect((page.querySelector('#u') as HTMLInputElement).value).toBe('ada typed this');
  });
});
