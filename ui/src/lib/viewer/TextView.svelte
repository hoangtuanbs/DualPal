<script lang="ts" module>
  let nextId = 1;
</script>

<script lang="ts">
  // Shows text from a ByteSource a screen at a time, so files of any size
  // open at once: only the rows on screen are read and decoded. Reusable by
  // the file compare view (one per side).
  import { untrack } from "svelte";
  import Scrollbar from "./Scrollbar.svelte";
  import { charRange, posAt, rowsFrom, rowsUp, segmentBefore, startPos, rowCount, type Pos, type Row, type TextOptions } from "./layout";
  import type { ByteSource } from "./source";
  import type { TextEncoding } from "./viewer-api";

  let {
    source,
    encoding,
    bom = 0,
    wrap = false,
    lineNumbers = true,
    highlight = null,
    rows: fixedRows = null,
    cols: fixedCols = null,
    label = "Text",
    onmove,
  }: {
    source: ByteSource;
    encoding: TextEncoding;
    /** Bytes of the byte order mark to skip. */
    bom?: number;
    wrap?: boolean;
    lineNumbers?: boolean;
    /** Bytes to mark, e.g. a search match. */
    highlight?: { offset: number; length: number } | null;
    /** Rows on screen; measured from the element when not given. */
    rows?: number | null;
    /** Characters per row when wrapping; measured when not given. */
    cols?: number | null;
    label?: string;
    /** The byte offset of the top row changed. */
    onmove?: (offset: number) => void;
  } = $props();

  const id = `textview-${nextId++}`;
  let box: HTMLDivElement | undefined = $state();
  let probe: HTMLSpanElement | undefined = $state();
  let height = $state(0);
  let width = $state(0);
  let lineHeight = $state(18);
  let charWidth = $state(8);

  const visible = $derived(fixedRows ?? (height > 0 ? Math.max(1, Math.floor(height / lineHeight)) : 30));
  const gutterChars = $derived(lineNumbers ? 7 : 0);
  const cols = $derived(
    wrap ? (fixedCols ?? (width > 0 ? Math.max(8, Math.floor(width / charWidth) - gutterChars - 2) : 100)) : null,
  );
  const opts = $derived<TextOptions>({ encoding, bom, cols });

  let top = $state<Pos>({ seg: 0, sub: 0, cont: false, line: 1 });
  let shown = $state<Row[]>([]);
  let serial = 0;

  // Measure the box and one character of the font.
  $effect(() => {
    const el = box;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      height = el.clientHeight;
      width = el.clientWidth;
      const r = probe?.getBoundingClientRect();
      if (r && r.height) {
        lineHeight = r.height;
        charWidth = r.width / 10;
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  // A new file starts at the top; a new encoding or width keeps the place.
  let lastSource: ByteSource | null = null;
  $effect(() => {
    const o = opts;
    const src = source;
    untrack(() => {
      if (src !== lastSource) {
        lastSource = src;
        top = startPos(o);
      } else {
        const keep = top;
        void posAt(src, o, keep.seg).then((p) => {
          if (source === src && top === keep) top = keep.line !== null && p.seg === keep.seg ? { ...p, line: keep.line } : p;
        });
      }
    });
  });

  // Lay out the screen.
  $effect(() => {
    const t = top;
    const n = visible;
    const o = opts;
    const src = source;
    const mine = ++serial;
    void rowsFrom(src, o, t, n).then((r) => {
      if (mine === serial) shown = r;
    });
  });

  // Line numbers after a jump: count the lines before the top row.
  $effect(() => {
    const t = top;
    const src = source;
    const enc = encoding;
    if (!lineNumbers || t.line !== null || !src.countLines) return;
    void src.countLines(t.seg, enc).then(
      (n) => {
        if (top.seg === t.seg && top.line === null && source === src) top = { ...top, line: n + 1 };
      },
      () => {},
    );
  });

  $effect(() => {
    onmove?.(top.seg);
  });

  /** Move the top row by `delta` rows. */
  export async function scrollBy(delta: number) {
    const src = source;
    const o = opts;
    const from = top;
    if (delta < 0) {
      const p = await rowsUp(src, o, from, -delta);
      if (top === from) top = p;
      return;
    }
    const n = visible;
    const r = await rowsFrom(src, o, from, delta + n);
    if (top !== from || !r.length) return;
    top = r[Math.min(delta, Math.max(0, r.length - n))].pos;
  }

  export function scrollToStart() {
    top = startPos(opts);
  }

  export async function scrollToEnd() {
    const src = source;
    const o = opts;
    const last = await segmentBefore(src, o, src.size);
    if (!last) return scrollToStart();
    const end: Pos = { seg: last.start, sub: rowCount(last, o.cols) - 1, cont: last.cont, line: null };
    top = await rowsUp(src, o, end, visible - 1);
  }

  /** Show byte `offset`, `context` rows below the top. */
  export async function scrollToOffset(offset: number, context = 3) {
    const src = source;
    const o = opts;
    const p = await posAt(src, o, offset);
    top = await rowsUp(src, o, p, Math.min(context, Math.max(0, visible - 1)));
  }

  /** Byte offset of the top row. */
  export function topOffset(): number {
    return top.seg;
  }

  function onkeydown(e: KeyboardEvent) {
    const handled = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    switch (e.key) {
      case "ArrowDown":
        void scrollBy(1);
        return handled();
      case "ArrowUp":
        void scrollBy(-1);
        return handled();
      case "PageDown":
      case " ":
        void scrollBy(e.shiftKey && e.key === " " ? -Math.max(1, visible - 1) : Math.max(1, visible - 1));
        return handled();
      case "PageUp":
        void scrollBy(-Math.max(1, visible - 1));
        return handled();
      case "Home":
        scrollToStart();
        return handled();
      case "End":
        void scrollToEnd();
        return handled();
    }
  }

  let wheelRest = 0;
  function onwheel(e: WheelEvent) {
    if (Math.abs(e.deltaY) < Math.abs(e.deltaX)) return; // sideways: native scroll
    e.preventDefault();
    const px = e.deltaMode === 1 ? e.deltaY * lineHeight : e.deltaMode === 2 ? e.deltaY * height : e.deltaY;
    wheelRest += px / lineHeight;
    const rows = Math.trunc(wheelRest);
    if (rows) {
      wheelRest -= rows;
      void scrollBy(rows);
    }
  }

  const span = $derived(Math.max(1, source.size - bom));
  const fraction = $derived(Math.min(1, Math.max(0, (top.seg - bom) / span)));
  const thumb = $derived.by(() => {
    const last = shown.at(-1);
    return last ? (last.segment.end - (shown[0]?.segment.start ?? 0)) / span : 1;
  });

  /** A row's text split around the highlight. */
  function pieces(row: Row): { text: string; mark: boolean }[] {
    const h = highlight;
    const range = h ? charRange(row.segment, encoding, h.offset, h.length) : null;
    if (!range) return [{ text: row.text, mark: false }];
    const a = Math.max(0, range[0] - row.from);
    const b = Math.min(row.text.length, range[1] - row.from);
    if (b <= 0 || a >= row.text.length || a >= b) return [{ text: row.text, mark: false }];
    return [
      { text: row.text.slice(0, a), mark: false },
      { text: row.text.slice(a, b), mark: true },
      { text: row.text.slice(b), mark: false },
    ].filter((p) => p.text);
  }

  const lineLabel = (row: Row) => (row.pos.sub === 0 && !row.pos.cont && row.pos.line !== null ? String(row.pos.line) : "");
</script>

<div class="view">
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="rows"
    class:wrap
    {id}
    bind:this={box}
    role="document"
    aria-label={label}
    tabindex="0"
    data-testid="text-view"
    data-top={top.seg}
    {onkeydown}
    {onwheel}
  >
    <span class="probe" bind:this={probe} aria-hidden="true">0123456789</span>
    {#each shown as row (`${row.pos.seg}:${row.pos.sub}`)}
      <div class="row" data-offset={row.pos.seg}>
        {#if lineNumbers}<span class="ln" aria-hidden="true">{lineLabel(row)}</span>{/if}
        <span class="tx">{#each pieces(row) as p, i (i)}{#if p.mark}<mark data-testid="text-match">{p.text}</mark>{:else}{p.text}{/if}{/each}</span>
      </div>
    {/each}
  </div>
  <Scrollbar value={fraction} {thumb} label="{label} position" controls={id} onjump={(f) => void scrollToOffset(bom + f * span, 0)} />
</div>

<style>
  .view {
    display: flex;
    min-height: 0;
    flex: 1;
    background: var(--pane-bg);
  }
  .rows {
    position: relative;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    overflow-y: hidden;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
    font-size: 12.5px;
    line-height: 18px;
    padding: 2px 0;
    outline: none;
    tab-size: 4;
  }
  .rows:focus-visible {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .probe {
    position: absolute;
    visibility: hidden;
    white-space: pre;
  }
  .row {
    display: flex;
    white-space: pre;
    height: 18px;
    width: max-content;
    min-width: 100%;
  }
  .wrap .row {
    width: auto;
    overflow: hidden;
  }
  .ln {
    position: sticky;
    left: 0;
    flex: none;
    width: 7ch;
    padding-right: 1ch;
    text-align: right;
    color: var(--muted);
    background: var(--stripe);
    user-select: none;
  }
  .tx {
    padding-left: 1ch;
  }
  mark {
    background: var(--accent);
    color: var(--selection-text);
    border-radius: 2px;
  }
</style>
