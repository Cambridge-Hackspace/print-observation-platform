<!--
  A status square (#48): a printer's state at a glance. Color is never the
  only signal: the label is always there, shown or for a screen reader.
-->
<script lang="ts">
  let {
    color,
    label,
    show = true,
    size = 10,
  }: {
    color: string;
    label: string;
    /** Show the label beside the square, or only to a screen reader. */
    show?: boolean;
    size?: number;
  } = $props();
</script>

<span class="status">
  <span class="sq" style:width="{size}px" style:height="{size}px" style:background={color} aria-hidden="true"
  ></span>
  <span class:visually-hidden={!show}>{label}</span>
</span>

<style>
  .status {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .sq {
    display: inline-block;
    flex: none;
  }
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
