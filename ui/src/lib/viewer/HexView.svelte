<script lang="ts" module>
  let nextId = 1;
  /** Bytes per row. */
  export const BYTES_PER_ROW = 16;
</script>

<script lang="ts">
  // Offset, 16 bytes in hex and the same bytes as ASCII, a screen at a time.
  import Scrollbar from "./Scrollbar.svelte";
  import type { ByteSource } from "./source";

  let {
    source,
    highlight = null,
    rows: fixedRows = null,
    label = "Hex",
    onmove,
  }: {
    source: ByteSource;
    highlight?: { offset: number; length: number } | null;
    /** Rows on screen; measured when not given. */
    rows?: number | null;
    label?: string;
    onmove?: (offset: number) => void;
  } = $props();

  const id = `hexview-${nextId++}`;
  let box: HTMLDivElement | undefined = $state();
  let height = $state(0);
  const LINE = 18;
  const visible = $derived(fixedRows ?? (height > 0 ? Math.max(1, Math.floor((height - 4) / LINE)) : 30));
  const totalRows = $derived(Math.max(1, Math.ceil(source.size / BYTES_PER_ROW)));
  const maxTop = $derived(Math.max(0, totalRows - visible));
  const digits = $derived(Math.max(8, Math.ceil(Math.log2(Math.max(2, source.size)) / 4)));

  let top = $state(0);
  let bytes = $state<Uint8Array>(new Uint8Array());
  let shownTop = $state(0);
  let serial = 0;

  $effect(() => {
    const el = box;
    if (!el) return;
    // Measure now too: the first observer callback can come late (WebKit).
    height = el.clientHeight;
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => (height = el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  });

  let lastSource: ByteSource | null = null;
  $effect(() => {
    if (source !== lastSource) {
      lastSource = source;
      top = 0;
    }
  });

  // A taller screen moves the top up rather than show empty rows after the end.
  $effect(() => {
    if (top > maxTop) top = maxTop;
  });

  $effect(() => {
    const t = top;
    const n = visible;
    const src = source;
    const mine = ++serial;
    void src.read(t * BYTES_PER_ROW, n * BYTES_PER_ROW).then((b) => {
      if (mine !== serial) return;
      bytes = b;
      shownTop = t;
    });
  });

  $effect(() => {
    onmove?.(top * BYTES_PER_ROW);
  });

  export function scrollBy(delta: number) {
    top = Math.min(maxTop, Math.max(0, top + delta));
  }

  /** Show byte `offset`, `context` rows below the top. */
  export function scrollToOffset(offset: number, context = 3) {
    top = Math.min(maxTop, Math.max(0, Math.floor(offset / BYTES_PER_ROW) - context));
  }

  export function topOffset(): number {
    return top * BYTES_PER_ROW;
  }

  function onkeydown(e: KeyboardEvent) {
    const page = Math.max(1, visible - 1);
    const map: Record<string, () => void> = {
      ArrowDown: () => scrollBy(1),
      ArrowUp: () => scrollBy(-1),
      PageDown: () => scrollBy(page),
      PageUp: () => scrollBy(-page),
      " ": () => scrollBy(e.shiftKey ? -page : page),
      Home: () => (top = 0),
      End: () => (top = maxTop),
    };
    const f = map[e.key];
    if (!f) return;
    f();
    e.preventDefault();
    e.stopPropagation();
  }

  let wheelRest = 0;
  function onwheel(e: WheelEvent) {
    e.preventDefault();
    const px = e.deltaMode === 1 ? e.deltaY * LINE : e.deltaMode === 2 ? e.deltaY * height : e.deltaY;
    wheelRest += px / LINE;
    const n = Math.trunc(wheelRest);
    if (n) {
      wheelRest -= n;
      scrollBy(n);
    }
  }

  const hex = (b: number) => b.toString(16).padStart(2, "0").toUpperCase();
  const ascii = (b: number) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".");
  const marked = (o: number) => !!highlight && o >= highlight.offset && o < highlight.offset + highlight.length;

  const lines = $derived.by(() => {
    const out: { offset: number; cells: { o: number; b: number }[] }[] = [];
    for (let i = 0; i < bytes.length; i += BYTES_PER_ROW) {
      const offset = (shownTop + i / BYTES_PER_ROW) * BYTES_PER_ROW;
      out.push({ offset, cells: Array.from(bytes.subarray(i, i + BYTES_PER_ROW), (b, j) => ({ o: offset + j, b })) });
    }
    return out;
  });
</script>

<div class="view">
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div class="rows" {id} bind:this={box} role="document" aria-label={label} tabindex="0" data-testid="hex-view" data-top={top * BYTES_PER_ROW} {onkeydown} {onwheel}>
    {#each lines as line (line.offset)}
      <div class="row" data-offset={line.offset}>
        <span class="off">{line.offset.toString(16).padStart(digits, "0").toUpperCase()}</span>
        <span class="hex">{#each line.cells as c, j (c.o)}{j === 8 ? "  " : j ? " " : ""}<span class:m={marked(c.o)}>{hex(c.b)}</span>{/each}</span>
        <span class="asc">{#each line.cells as c (c.o)}<span class:m={marked(c.o)}>{ascii(c.b)}</span>{/each}</span>
      </div>
    {/each}
  </div>
  <Scrollbar
    value={maxTop ? top / maxTop : 0}
    thumb={visible / totalRows}
    label="{label} position"
    controls={id}
    onjump={(f) => (top = Math.round(f * maxTop))}
  />
</div>

<style>
  .view {
    display: flex;
    min-height: 0;
    flex: 1;
    background: var(--pane-bg);
  }
  .rows {
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    overflow-y: hidden;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
    font-size: 12.5px;
    line-height: 18px;
    padding: 2px 0;
    outline: none;
  }
  .rows:focus-visible {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .row {
    display: flex;
    gap: 2ch;
    height: 18px;
    white-space: pre;
    padding: 0 1ch;
    width: max-content;
  }
  .off {
    color: var(--muted);
  }
  .hex {
    min-width: 48ch;
  }
  .m {
    background: var(--accent);
    color: var(--selection-text);
  }
</style>
