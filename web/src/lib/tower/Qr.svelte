<script lang="ts">
  // A QR code, drawn crisp in the kit's ink on cream: one path, one unit per
  // module, with the quiet zone the standard asks for.
  import { encode } from 'uqr';

  let { text, size = 72, label }: { text: string; size?: number; label: string } = $props();

  const QUIET = 4;
  const code = $derived(encode(text, { ecc: 'M', border: 0 }));
  const path = $derived(
    code.data
      .flatMap((row, y) => row.map((on, x) => (on ? `M${x + QUIET} ${y + QUIET}h1v1h-1z` : '')))
      .join(''),
  );
  const span = $derived(code.size + QUIET * 2);
</script>

<svg width={size} height={size} viewBox="0 0 {span} {span}" role="img" aria-label={label} shape-rendering="crispEdges">
  <rect width={span} height={span} fill="var(--pop-cream)" />
  <path d={path} fill="var(--pop-ink)" />
</svg>
