<script lang="ts" module>
  let nextId = 1;
</script>

<script lang="ts">
  // Shows text from a ByteSource a screen at a time, so files of any size
  // open at once: only the rows on screen are read and decoded. Reusable by
  // the file compare view (one per side).
  import { untrack } from "svelte";
  import { errorMessage } from "../api";
  import Scrollbar from "./Scrollbar.svelte";
  import { charRange, posAt, rowOffset, rowsFrom, rowsUp, segmentBefore, startPos, rowCount, type Pos, type Row, type TextOptions } from "./layout";
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
  /** Row height, as in the CSS below. */
  const lineHeight = 18;
  let charWidth = $state(8);

  const visible = $derived(fixedRows ?? (height > 0 ? Math.max(1, Math.floor((height - 4) / lineHeight)) : 30));
  const gutterChars = $derived(lineNumbers ? 7 : 0);
  const cols = $derived(
    wrap ? (fixedCols ?? (width > 0 ? Math.max(8, Math.floor(width / charWidth) - gutterChars - 2) : 100)) : null,
  );
  const opts = $derived<TextOptions>({ encoding, bom, cols });

  let top = $state<Pos>({ seg: 0, sub: 0, cont: false, line: 1 });
  let shown = $state<Row[]>([]);
  /** The encoding `shown` was decoded with. */
  let shownEncoding: TextEncoding = untrack(() => encoding);
  let serial = 0;
  /** A read failed (e.g. a server connection dropped); bumping `attempt` tries again. */
  let failed = $state<string | null>(null);
  let attempt = $state(0);
  /** Bumped by every move; a slower, older move does not apply. */
  let nav = 0;

  // Measure the box and one character of the font.
  $effect(() => {
    const el = box;
    if (!el) return;
    const measure = () => {
      height = el.clientHeight;
      width = el.clientWidth;
      const r = probe?.getBoundingClientRect();
      if (r && r.width) charWidth = r.width / 10;
    };
    // Measure now too: the first observer callback can come after the
    // first keys (WebKit), and a screen laid out for the default row count
    // would be too short.
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
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
        // The byte at the top row, also when it is wrapped inside a line.
        const keep = top;
        void posAt(src, o, topOffset()).then(
          (p) => {
            if (source === src && top === keep) top = keep.line !== null && p.seg === keep.seg ? { ...p, line: keep.line } : p;
          },
          (e) => {
            if (source === src && top === keep) failed = errorMessage(e);
          },
        );
      }
    });
  });

  // Lay out the screen.
  $effect(() => {
    const t = top;
    const n = visible;
    const o = opts;
    const src = source;
    void attempt;
    const mine = ++serial;
    rowsFrom(src, o, t, n)
      .then(async (r) => {
        if (mine !== serial) return;
        shown = r;
        shownEncoding = o.encoding;
        failed = null;
        // Never leave empty rows below the end of the file while there is
        // text above: when the screen grows (or was laid out for fewer rows),
        // the end stays at the bottom.
        if (r.length < n && (t.seg > o.bom || t.sub > 0)) {
          const p = await rowsUp(src, o, t, n - r.length);
          if (mine === serial && top === t && (p.seg !== t.seg || p.sub !== t.sub)) top = p;
        }
      })
      .catch((e) => {
        // Rows of another place must not stay under the new position.
        if (mine !== serial) return;
        shown = [];
        failed = errorMessage(e);
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

  /** Rows asked for by scrolling that are not applied yet. */
  let queued = 0;
  let scrolling: Promise<void> | null = null;

  /**
   * Move the top row by `delta` rows. Moves that come while one is reading
   * add up, so fast keys or wheel turns on a slow source are not lost.
   */
  export function scrollBy(delta: number): Promise<void> {
    queued += delta;
    if (!scrolling && queued) scrolling = drainScroll();
    return scrolling ?? Promise.resolve();
  }

  async function drainScroll() {
    try {
      while (queued) {
        const delta = queued;
        queued = 0;
        const mine = ++nav;
        const src = source;
        const o = opts;
        const from = top;
        let p: Pos | null;
        if (delta < 0) {
          p = await rowsUp(src, o, from, -delta);
        } else {
          const n = visible;
          const r = await rowsFrom(src, o, from, delta + n);
          p = r.length ? r[Math.min(delta, Math.max(0, r.length - n))].pos : null;
        }
        if (mine !== nav) return; // a jump came after
        if (p && top === from) top = p;
      }
    } catch (e) {
      failed = errorMessage(e);
    } finally {
      queued = 0;
      scrolling = null;
    }
  }

  /** Start a jump: it replaces moves on their way. Returns its id. */
  function jump(): number {
    queued = 0;
    return ++nav;
  }

  export function scrollToStart() {
    jump();
    top = startPos(opts);
  }

  export async function scrollToEnd() {
    const mine = jump();
    const src = source;
    const o = opts;
    try {
      const last = await segmentBefore(src, o, src.size);
      if (mine !== nav) return;
      if (!last) return scrollToStart();
      const end: Pos = { seg: last.start, sub: rowCount(last, o.cols) - 1, cont: last.cont, line: null };
      const p = await rowsUp(src, o, end, visible - 1);
      if (mine === nav) top = p;
    } catch (e) {
      if (mine === nav) failed = errorMessage(e);
    }
  }

  /** Show byte `offset`, `context` rows below the top. */
  export async function scrollToOffset(offset: number, context = 3) {
    const mine = jump();
    const src = source;
    const o = opts;
    try {
      const p = await posAt(src, o, offset);
      const up = await rowsUp(src, o, p, Math.min(context, Math.max(0, visible - 1)));
      if (mine === nav) top = up;
    } catch (e) {
      if (mine === nav) failed = errorMessage(e);
    }
  }

  /** Byte offset of the top row; inside its line when the line is wrapped. */
  export function topOffset(): number {
    const row = shown[0];
    if (row && row.pos.seg === top.seg && row.pos.sub === top.sub) return rowOffset(row, shownEncoding);
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
  /** Bytes on screen. */
  const onScreen = $derived(shown.length ? shown.at(-1)!.segment.end - shown[0].segment.start : span);
  /** How far the top can go: at the end, the thumb is at the bottom. */
  const range = $derived(Math.max(1, span - onScreen));
  const fraction = $derived(Math.min(1, Math.max(0, (top.seg - bom) / range)));
  const thumb = $derived(onScreen / span);

  /** Go to a place on the scroll bar, 0 (start) to 1 (end), on a whole byte. */
  export function scrollToFraction(f: number) {
    return scrollToOffset(bom + Math.round(f * range), 0);
  }

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
    {#if failed}
      <p class="failed" role="alert">
        Could not read the file: {failed}
        <button onclick={() => ((failed = null), attempt++)}>Retry</button>
      </p>
    {/if}
    {#each shown as row (`${row.pos.seg}:${row.pos.sub}`)}
      <div class="row" data-offset={row.pos.seg}>
        {#if lineNumbers}<span class="ln" aria-hidden="true">{lineLabel(row)}</span>{/if}
        <span class="tx">{#each pieces(row) as p, i (i)}{#if p.mark}<mark data-testid="text-match">{p.text}</mark>{:else}{p.text}{/if}{/each}</span>
      </div>
    {/each}
  </div>
  <Scrollbar value={fraction} {thumb} label="{label} position" controls={id} onjump={(f) => void scrollToFraction(f)} />
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
  .failed {
    margin: 8px 12px;
    color: var(--muted);
    white-space: normal;
  }
  mark {
    background: var(--accent);
    color: var(--selection-text);
    border-radius: 2px;
  }
</style>
