<!--
  A printer, as round 6 draws it (#48): enclosed (a box with a window) or
  open frame, its body and nozzle colored per printer, with the part on the
  bed in layer lines while one is printing. Mirrors kit.py's enclosed() and
  openframe(); design.test.ts compares the two.
-->
<script lang="ts">
  export type PrinterKind = 'enclosed' | 'open';

  let {
    kind,
    body,
    nozzle,
    part = true,
    size = 120,
    label,
    screen = 'var(--pop-butter)',
    glass = '#26332a',
    id,
  }: {
    kind: PrinterKind;
    body: string;
    nozzle: string;
    /** A part on the bed, shown in layer lines. */
    part?: boolean;
    size?: number;
    /** What a screen reader says: the printer's name and state. */
    label: string;
    screen?: string;
    glass?: string;
    /** The layer pattern's id; unique per drawing unless a test sets it. */
    id?: string;
  } = $props();

  const uid = $props.id();
  const pid = $derived(id ?? `layers-${uid}`);
  const ink = '#121a14';
  const cream = '#fef7e4';
</script>

<svg width={size} height={size} viewBox="0 0 230 230" role="img" aria-label={label} style="flex:none">
  <defs>
    <pattern id={pid} width="4" height="4" patternUnits="userSpaceOnUse">
      <rect width="4" height="4" fill="#C3D7BB"></rect>
      <rect y="3" width="4" height="1" fill="#a3bd99"></rect>
    </pattern>
  </defs>
  {#if kind === 'enclosed'}
    <rect x="27" y="23" width="180" height="191" rx="4" fill={ink} opacity="0.6"></rect>
    <rect x="20" y="16" width="180" height="198" rx="4" fill={body} stroke={ink} stroke-width="4"></rect>
    <rect x="38" y="34" width="144" height="122" rx="2" fill={glass} stroke={ink} stroke-width="4"></rect>
    <rect x="46" y="50" width="128" height="9" fill={cream} stroke={ink} stroke-width="3"></rect>
    <rect x="109" y="59" width="2" height="12" fill={ink}></rect>
    <rect x="96" y="70" width="28" height="22" fill={nozzle} stroke={ink} stroke-width="3"></rect>
    <path d="M104 92h12l-6 9z" fill={cream} stroke={ink} stroke-width="2.5" stroke-linejoin="round"></path>
    {#if part}
      <rect x="86" y="112" width="48" height="22" fill="url(#{pid})" stroke={ink} stroke-width="3"></rect>
    {/if}
    <rect x="56" y="134" width="108" height="8" fill={cream} stroke={ink} stroke-width="3"></rect>
    <rect x="38" y="170" width="84" height="28" rx="2" fill={ink}></rect>
    <rect x="46" y="180" width="44" height="6" fill={screen}></rect>
    <circle cx="170" cy="184" r="9" fill={cream} stroke={ink} stroke-width="3"></circle>
  {:else}
    <rect x="30" y="20" width="16" height="174" fill={body} stroke={ink} stroke-width="3"></rect>
    <rect x="174" y="20" width="16" height="174" fill={body} stroke={ink} stroke-width="3"></rect>
    <rect x="26" y="10" width="168" height="16" fill={body} stroke={ink} stroke-width="3"></rect>
    <rect x="16" y="190" width="188" height="24" rx="3" fill={body} stroke={ink} stroke-width="3"></rect>
    {#if part}
      <rect x="88" y="150" width="44" height="26" fill="url(#{pid})" stroke={ink} stroke-width="3"></rect>
    {/if}
    <rect x="52" y="176" width="116" height="8" fill={cream} stroke={ink} stroke-width="3"></rect>
    <rect x="44" y="90" width="132" height="9" fill={cream} stroke={ink} stroke-width="3"></rect>
    <rect x="96" y="99" width="28" height="22" fill={nozzle} stroke={ink} stroke-width="3"></rect>
    <path d="M104 121h12l-6 8z" fill={cream} stroke={ink} stroke-width="2.5" stroke-linejoin="round"></path>
  {/if}
</svg>
