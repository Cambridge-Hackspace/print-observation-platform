<!-- An icon (#48). Decorative: what it means is said by the text beside it. -->
<script lang="ts">
  import { icons, type IconName } from './icons';

  let {
    name,
    size = 26,
    color = 'currentColor',
    weight = 2.4,
  }: { name: IconName; size?: number; color?: string; weight?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke={color}
  stroke-width={weight}
  stroke-linecap="square"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={icons[name]}></path>
</svg>
