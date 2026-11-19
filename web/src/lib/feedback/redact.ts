// Redaction before capture (#96). A feedback screenshot is built from a
// clone of the page, and the clone is redacted before it becomes an image:
// sensitive data never enters the picture, so nothing has to be blurred or
// covered afterwards. The page on screen is never touched.
//
// - What a screen marks with `use:sensitive` (sensitive.ts) becomes a block
//   the width of what it hid, labelled by its kind: `[member]`, `[email]`...
// - Form fields are emptied.
// - Camera pictures, and anything marked as a camera, become a grey
//   `[camera]` box.
// - Then a backstop over every text node left: email addresses, runs of 12
//   or more digits, UUIDs and token-shaped strings are redacted the same way,
//   for whatever a screen forgot to mark.

/** What a marked element holds. */
export type SensitiveKind = 'name' | 'email' | 'card' | 'file' | 'camera' | 'secret';

/** The attribute `use:sensitive` sets, and redaction reads. */
export const ATTRIBUTE = 'data-sensitive';

const LABEL: Record<SensitiveKind, string> = {
  name: '[member]',
  email: '[email]',
  card: '[card]',
  file: '[file]',
  camera: '[camera]',
  secret: '[secret]',
};

/** What the backstop looks for, in text nothing marked. */
export const PATTERNS: [RegExp, string][] = [
  [/[^\s@<>()]+@[^\s@<>()]+\.[a-z]{2,}/gi, '[email]'],
  [/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi, '[id]'],
  // Twelve or more digits, as a card number is written: with or without
  // spaces or dashes between groups.
  [/\b\d(?:[ -]?\d){11,}\b/g, '[number]'],
  [/\b(?:gh[pousr]_|github_pat_)[A-Za-z0-9_]+/g, '[token]'],
  // POP's own API keys (#13), digit or none.
  [/\bpop_[A-Za-z0-9_-]{16,}/g, '[token]'],
  [/\beyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g, '[token]'],
  // A long run of key-like characters with a digit in it.
  [/\b(?=[A-Za-z0-9_+/=-]*\d)[A-Za-z0-9_+/=-]{32,}/g, '[token]'],
];

/** `text` with everything the backstop catches replaced. */
export function backstop(text: string): string {
  let out = text;
  for (const [re, label] of PATTERNS) out = out.replace(re, label);
  return out;
}

function block(doc: Document, label: string, width: string, height?: string): HTMLElement {
  const b = doc.createElement('span');
  b.textContent = label;
  b.setAttribute('data-redacted', '');
  b.style.cssText = [
    'display:inline-block',
    `width:${width}`,
    height ? `height:${height}` : '',
    'max-width:100%',
    'overflow:hidden',
    'white-space:nowrap',
    'vertical-align:middle',
    'background:#c9c9c9',
    'color:#333',
    'border-radius:4px',
    'font:inherit',
    'text-align:center',
  ]
    .filter(Boolean)
    .join(';');
  return b;
}

function cameraBox(doc: Document, el: Element): HTMLElement {
  const style = (el as HTMLElement).style;
  const w = style?.width || (el.getAttribute('width') ? `${el.getAttribute('width')}px` : '100%');
  const h = style?.height || (el.getAttribute('height') ? `${el.getAttribute('height')}px` : '12em');
  const b = block(doc, '[camera]', w, h);
  b.style.display = 'flex';
  b.style.alignItems = 'center';
  b.style.justifyContent = 'center';
  return b;
}

/** Whether `el` shows a camera's picture. */
function isCamera(el: Element): boolean {
  const tag = el.tagName.toLowerCase();
  if (tag === 'video') return true;
  if (tag === 'img' || tag === 'iframe') {
    const src = el.getAttribute('src') ?? '';
    return src.includes('/camera');
  }
  return false;
}

/**
 * Redact `root`, a clone of the page, in place. Call it on the clone only:
 * it changes what it is given.
 */
export function redact(root: Node): void {
  const el = root as Element;
  if (typeof el.querySelectorAll !== 'function') return;
  const doc = el.ownerDocument;
  const all = (sel: string): Element[] => {
    const found = Array.from(el.querySelectorAll?.(sel) ?? []);
    if (el.matches?.(sel)) found.unshift(el);
    return found;
  };

  // Marked first: what a screen said is sensitive, whatever it holds.
  for (const m of all(`[${ATTRIBUTE}]`)) {
    // Inside a marked element already redacted: gone with it.
    if (m !== el && !el.contains(m)) continue;
    const kind = (m.getAttribute(ATTRIBUTE) ?? 'name') as SensitiveKind;
    if (kind === 'camera' || isCamera(m)) {
      m.replaceWith(cameraBox(doc, m));
      continue;
    }
    const hidden = m.textContent ?? '';
    const width = `${Math.max(LABEL[kind].length, hidden.trim().length)}ch`;
    m.replaceChildren(block(doc, LABEL[kind] ?? '[member]', width));
  }
  // Camera pictures, marked or not.
  for (const c of all('video, img, iframe')) {
    if (isCamera(c)) c.replaceWith(cameraBox(doc, c));
  }
  // Form fields hold what someone typed.
  for (const f of all('input, textarea')) {
    const field = f as HTMLInputElement;
    if (field.type === 'checkbox' || field.type === 'radio') continue;
    field.value = '';
    field.removeAttribute('value');
    if (f.tagName.toLowerCase() === 'textarea') f.textContent = '';
  }
  for (const s of all('select')) {
    for (const o of Array.from((s as HTMLSelectElement).options)) {
      o.selected = false;
      o.removeAttribute('selected');
    }
  }
  // The backstop, over every text node left.
  const walker = doc.createTreeWalker(el, 4 /* NodeFilter.SHOW_TEXT */);
  const texts: Text[] = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) texts.push(n as Text);
  for (const t of texts) {
    if (t.parentElement?.hasAttribute('data-redacted')) continue;
    const before = t.data;
    const after = backstop(before);
    if (after !== before) t.data = after;
  }
}
