// Test setup. jsdom has no matchMedia, which Svelte's prefersReducedMotion
// calls when it is imported; this gives it one whose reduced-motion answer a
// test can set through `media.reducedMotion`.
export const media = { reducedMotion: false };

if (typeof window !== 'undefined' && !window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    get matches() {
      return query.includes('prefers-reduced-motion') && media.reducedMotion;
    },
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}
