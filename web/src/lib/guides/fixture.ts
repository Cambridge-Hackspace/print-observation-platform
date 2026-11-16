// The round 6 canvas's stringing flowchart, for tests.
import type { PublicGuide } from '../generated/PublicGuide';

export const STRINGING: PublicGuide = {
  slug: 'stringing-on-petg',
  title: 'Stringing on PETG',
  summary: 'Fine hairs between the parts of a print.',
  start: 'hairs',
  license: 'CC BY-SA 4.0',
  steps: [
    { id: 'hairs', title: 'Are there fine hairs between parts?', body: '', image: null, then: { kind: 'question', yes: 'dry-box', no: 'blobs' } },
    { id: 'blobs', title: 'Those are blobs, not strings: see the blobs guide.', body: '', image: null, then: { kind: 'done' } },
    {
      id: 'dry-box',
      title: 'Has this spool been out of the dry box for more than a day?',
      body: 'PETG soaks up water from the air. Wet filament strings, pops and looks fuzzy.',
      image: null,
      then: { kind: 'question', yes: 'dry-it', no: 'cooler' },
    },
    { id: 'dry-it', title: 'Dry it for 4 hours at 65 °C.', body: '', image: 'abc123', then: { kind: 'next', step: 'test-tower' } },
    { id: 'cooler', title: 'Print with the nozzle 5 °C cooler.', body: '', image: null, then: { kind: 'next', step: 'test-tower' } },
    { id: 'test-tower', title: 'Print the test tower.', body: '', image: null, then: { kind: 'done' } },
  ],
};
