<!--
  A spool face-on (#48): a dark flange, a ring of filament whose thickness
  shows what is left, and the hub. Mirrors kit.py's spool_svg().
-->
<script lang="ts">
  import { spoolRadius } from './geometry';

  let {
    color,
    frac,
    size = 56,
    label,
  }: {
    color: string;
    /** How much is left, 0 to 1. */
    frac: number;
    size?: number;
    label: string;
  } = $props();

  const ink = '#121a14';
  const flange = '#3c4a3e';
</script>

<svg width={size} height={size} viewBox="0 0 60 60" role="img" aria-label={label} style="flex:none">
  <circle cx="30" cy="30" r="27" fill={flange} stroke={ink} stroke-width="2.5"></circle>
  <circle cx="30" cy="30" r={spoolRadius(frac)} fill={color} stroke={ink} stroke-width="2"></circle>
  <circle cx="30" cy="30" r="11" fill={flange} stroke={ink} stroke-width="2"></circle>
  <circle cx="30" cy="30" r="5" fill="#1c2620" stroke={ink} stroke-width="2"></circle>
</svg>
