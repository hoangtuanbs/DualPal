<script lang="ts">
  /** A vertical scroll bar for views that are laid out a screen at a time. */
  let {
    value,
    thumb,
    label,
    controls,
    onjump,
  }: {
    /** Where the view is, 0 (start) to 1 (end). */
    value: number;
    /** How much of the whole the screen shows, 0 to 1. */
    thumb: number;
    label: string;
    /** Id of the view it scrolls. */
    controls: string;
    /** Go to this place, 0 to 1. */
    onjump: (fraction: number) => void;
  } = $props();

  let track: HTMLDivElement | undefined = $state();
  let dragging = false;
  const size = $derived(Math.min(1, Math.max(0.03, thumb)));

  function at(e: PointerEvent): number {
    const r = track!.getBoundingClientRect();
    if (!r.height) return 0;
    return Math.min(1, Math.max(0, (e.clientY - r.top) / r.height - size / 2) / Math.max(0.0001, 1 - size));
  }

  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    dragging = true;
    track?.setPointerCapture?.(e.pointerId);
    onjump(at(e));
    e.preventDefault();
  }
  function move(e: PointerEvent) {
    if (dragging) onjump(at(e));
  }
  function up() {
    dragging = false;
  }
</script>

<div
  class="track"
  bind:this={track}
  role="scrollbar"
  aria-label={label}
  aria-controls={controls}
  aria-orientation="vertical"
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={Math.round(value * 100)}
  tabindex="-1"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
>
  <div class="thumb" style:top="{value * (1 - size) * 100}%" style:height="{size * 100}%"></div>
</div>

<style>
  .track {
    position: relative;
    width: 12px;
    flex: none;
    background: var(--pane-bg);
    border-left: 1px solid var(--border);
    cursor: default;
    touch-action: none;
  }
  .thumb {
    position: absolute;
    left: 2px;
    right: 2px;
    min-height: 18px;
    border-radius: 4px;
    background: var(--muted);
    opacity: 0.5;
  }
  .track:hover .thumb {
    opacity: 0.8;
  }
</style>
