<script lang="ts">
  // The built-in file compare view: two files from any places side by side
  // (or inline), changes tinted, changed words marked, and each change can
  // be copied to the other side and saved. Only the rows on screen are
  // fetched from the backend, so long files scroll at once.
  import { onDestroy, tick, untrack } from "svelte";
  import { errorMessage, type Location } from "../api";
  import { formatBytes } from "../format";
  import { ENCODINGS } from "../viewer/encoding";
  import Scrollbar from "../viewer/Scrollbar.svelte";
  import { newRequest } from "../viewer/source";
  import type { TextEncoding } from "../viewer/viewer-api";
  import { diffApi, NO_OPTIONS, type DiffLine, type DiffOptions, type DiffOutcome, type DiffState, type Side } from "./diff-api";
  import { buildBlocks, hunkRow, rowAt, totalRows, type Mode, type RowRef } from "./rows";

  let {
    left,
    right,
    nameOf = (fs: string) => fs,
    onclose,
    onexternal,
  }: {
    left: Location;
    right: Location;
    /** Display name of a file system id. */
    nameOf?: (fs: string) => string;
    onclose: () => void;
    /** Open the two files in the external compare app. */
    onexternal?: (left: Location, right: Location) => Promise<void>;
  } = $props();

  const lineHeight = 18;
  /** Lines fetched at a time. */
  const CHUNK = 200;
  const SIDES: Side[] = ["left", "right"];
  const fileName = (p: string) => p.split(/[\\/]/).filter(Boolean).at(-1) ?? p;

  function load<T>(key: string, fallback: T): T {
    try {
      const v = localStorage.getItem(key);
      return v === null ? fallback : { ...fallback, ...JSON.parse(v) };
    } catch {
      return fallback;
    }
  }
  function save<T>(key: string, value: T) {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Private window or blocked storage: the choice lasts this session.
    }
  }

  let mode = $state<Mode>(load("dualpal.diff.view", { mode: "split" as Mode }).mode);
  let options = $state<DiffOptions>(load("dualpal.diff.options", NO_OPTIONS));
  let outcome = $state<DiffOutcome | null>(null);
  let diff = $state<DiffState | null>(null);
  let error = $state<string | null>(null);
  let message = $state("");
  let busy = $state(false);
  let current = $state(-1);
  let top = $state(0);
  let confirming = $state(false);

  const request = newRequest();
  // svelte-ignore state_referenced_locally
  const opening = diffApi.open(request, left, right, $state.snapshot(options)).then(
    (o) => {
      outcome = o;
      if (o.kind === "text") {
        diff = o.state;
        void tick().then(() => body?.focus());
      }
      return o;
    },
    (e) => {
      error = errorMessage(e);
      return null;
    },
  );
  onDestroy(() => {
    diffApi.cancel(request).catch(() => {});
    void opening
      .then((o) => {
        if (o?.kind === "text") return diffApi.close(o.state.session);
      })
      .catch(() => {});
  });

  // ---- layout -------------------------------------------------------------------
  let body: HTMLDivElement | undefined = $state();
  let height = $state(0);
  $effect(() => {
    const el = body;
    if (!el) return;
    height = el.clientHeight;
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => (height = el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  });
  const visible = $derived(height > 0 ? Math.max(1, Math.floor(height / lineHeight)) : 30);
  const blocks = $derived(diff ? buildBlocks(diff.hunks, diff.sides[0].lines, diff.sides[1].lines, mode) : []);
  const total = $derived(totalRows(blocks));
  const maxTop = $derived(Math.max(0, total - visible));
  $effect(() => {
    if (top > maxTop) top = maxTop;
  });

  function scrollTo(row: number) {
    top = Math.max(0, Math.min(maxTop, Math.round(row)));
  }

  // ---- lines ----------------------------------------------------------------------
  let lines = new Map<string, DiffLine>();
  let linesOf = -1;
  const asked = new Set<string>();
  let linesTick = $state(0);

  async function fetchChunk(s: DiffState, side: Side, chunk: number) {
    const key = `${s.revision}:${side}:${chunk}`;
    if (asked.has(key)) return;
    asked.add(key);
    try {
      const got = await diffApi.lines(s.session, side, chunk * CHUNK, CHUNK);
      if (diff?.revision !== s.revision) return;
      got.forEach((l, i) => lines.set(`${side}:${chunk * CHUNK + i}`, l));
      linesTick++;
    } catch (e) {
      asked.delete(key);
      message = errorMessage(e);
    }
  }

  // Fetch the lines of the rows on screen.
  $effect(() => {
    const s = diff;
    const from = top;
    const n = visible;
    const b = blocks;
    const m = mode;
    if (!s) return;
    untrack(() => {
      if (linesOf !== s.revision) {
        lines = new Map();
        linesOf = s.revision;
        linesTick++;
      }
      const want = new Set<string>();
      for (let r = from; r < from + n; r++) {
        const ref = rowAt(b, r, m);
        if (!ref) break;
        if (ref.left !== null) want.add(`left:${Math.floor(ref.left / CHUNK)}`);
        if (ref.right !== null && !(m === "inline" && ref.hunk < 0)) want.add(`right:${Math.floor(ref.right / CHUNK)}`);
      }
      for (const w of want) {
        const [side, chunk] = w.split(":");
        void fetchChunk(s, side as Side, Number(chunk));
      }
    });
  });

  interface Shown {
    row: number;
    ref: RowRef;
    l: DiffLine | undefined;
    r: DiffLine | undefined;
  }
  const shown = $derived.by<Shown[]>(() => {
    void linesTick;
    const out: Shown[] = [];
    for (let row = top; row < Math.min(total, top + visible); row++) {
      const ref = rowAt(blocks, row, mode);
      if (!ref) break;
      out.push({
        row,
        ref,
        l: ref.left !== null ? lines.get(`left:${ref.left}`) : undefined,
        r: ref.right !== null ? lines.get(`right:${ref.right}`) : undefined,
      });
    }
    return out;
  });

  // eslint-disable-next-line no-control-regex
  const CONTROL = /[\u0000-\u0008\u000b-\u001f\u007f]/g;
  /** A line's text split around its marked parts. */
  function pieces(line: DiffLine | undefined, marked: boolean): { text: string; mark: boolean }[] {
    if (!line) return [];
    const text = line.text.replace(CONTROL, "·");
    if (!marked || !line.marks.length) return [{ text, mark: false }];
    const out: { text: string; mark: boolean }[] = [];
    let at = 0;
    for (const [a, b] of line.marks) {
      if (a > at) out.push({ text: text.slice(at, a), mark: false });
      out.push({ text: text.slice(a, b), mark: true });
      at = b;
    }
    if (at < text.length) out.push({ text: text.slice(at), mark: false });
    return out;
  }

  /** How a row's cell on `side` is tinted. */
  function kind(ref: RowRef, side: Side): "same" | "removed" | "added" | "filler" {
    if (ref.hunk < 0) return "same";
    const line = side === "left" ? ref.left : ref.right;
    if (line === null) return "filler";
    return side === "left" ? "removed" : "added";
  }

  // ---- changes ----------------------------------------------------------------------
  const count = $derived(diff?.hunks.length ?? 0);
  const status = $derived.by(() => {
    if (!diff) return "";
    if (count === 0) return diff.identical ? "The files are identical" : "No differences with these options";
    const n = `${count} ${count === 1 ? "change" : "changes"}`;
    return current >= 0 ? `Change ${current + 1} of ${count}` : n;
  });

  function go(i: number) {
    if (!count) return;
    current = Math.max(0, Math.min(count - 1, i));
    scrollTo(hunkRow(blocks, current) - 3);
  }
  function next() {
    if (current < 0) {
      // The first change below the top of the screen.
      const i = diff!.hunks.findIndex((_, k) => hunkRow(blocks, k) >= top);
      go(i < 0 ? count - 1 : i);
    } else go(current + 1);
  }
  function previous() {
    if (current < 0) {
      let i = -1;
      diff!.hunks.forEach((_, k) => {
        if (hunkRow(blocks, k) < top) i = k;
      });
      go(i < 0 ? 0 : i);
    } else go(current - 1);
  }

  async function run(f: () => Promise<DiffState>, done = "") {
    if (busy) return false;
    busy = true;
    try {
      diff = await f();
      message = done;
      return true;
    } catch (e) {
      message = errorMessage(e);
      return false;
    } finally {
      busy = false;
    }
  }

  /** Copy change `i` to side `to`. */
  async function copy(i: number, to: Side) {
    const s = diff;
    if (!s || i < 0 || i >= s.hunks.length) return;
    if (await run(() => diffApi.apply(s.session, s.revision, i, to))) {
      current = diff!.hunks.length ? Math.min(i, diff!.hunks.length - 1) : -1;
    }
  }

  async function saveSide(side: Side) {
    const s = diff;
    if (!s) return false;
    const i = side === "left" ? 0 : 1;
    return run(() => diffApi.save(s.session, side), `Saved ${fileName((i ? right : left).path)}.`);
  }

  async function setOption(key: keyof DiffOptions, value: boolean) {
    options = { ...options, [key]: value };
    save("dualpal.diff.options", options);
    const s = diff;
    current = -1;
    if (s) await run(() => diffApi.setOptions(s.session, $state.snapshot(options)));
  }

  async function setEncoding(side: Side, encoding: TextEncoding) {
    const s = diff;
    if (s) await run(() => diffApi.setEncoding(s.session, side, encoding));
  }

  function setMode(m: Mode) {
    const ref = rowAt(blocks, top, mode);
    mode = m;
    save("dualpal.diff.view", { mode });
    // Keep the line at the top.
    void tick().then(() => {
      if (!ref) return;
      const b = blocks.find((x) => ref.left !== null && ref.left >= x.left && ref.left < x.left + Math.max(1, x.leftLen));
      if (b) scrollTo(b.row + (b.hunk < 0 ? ref.left! - b.left : 0));
    });
  }

  const dirty = $derived(!!diff && (diff.sides[0].dirty || diff.sides[1].dirty));

  function requestClose() {
    if (dirty) confirming = true;
    else onclose();
  }

  async function saveAndClose() {
    for (const [i, side] of SIDES.entries()) {
      if (diff?.sides[i].dirty && !(await saveSide(side))) {
        confirming = false;
        return;
      }
    }
    onclose();
  }

  function external() {
    onexternal?.(left, right).catch((e) => (message = errorMessage(e)));
  }

  // ---- keys and wheel -----------------------------------------------------------------
  function onkeydown(e: KeyboardEvent) {
    const handled = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    if (confirming) {
      if (e.key === "Escape") {
        confirming = false;
        handled();
      }
      return;
    }
    if (e.key === "Escape") {
      requestClose();
      return handled();
    }
    if ((e.altKey && e.key === "ArrowDown") || (e.key === "F7" && !e.shiftKey)) {
      next();
      return handled();
    }
    if ((e.altKey && e.key === "ArrowUp") || (e.key === "F7" && e.shiftKey)) {
      previous();
      return handled();
    }
    if (e.altKey && (e.key === "ArrowRight" || e.key === "ArrowLeft")) {
      if (current >= 0) void copy(current, e.key === "ArrowRight" ? "right" : "left");
      return handled();
    }
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "SELECT" || t.tagName === "TEXTAREA")) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const page = Math.max(1, visible - 1);
    const moves: Record<string, () => void> = {
      ArrowDown: () => scrollTo(top + 1),
      ArrowUp: () => scrollTo(top - 1),
      PageDown: () => scrollTo(top + page),
      PageUp: () => scrollTo(top - page),
      " ": () => scrollTo(top + (e.shiftKey ? -page : page)),
      Home: () => scrollTo(0),
      End: () => scrollTo(maxTop),
    };
    if (moves[e.key] && diff) {
      moves[e.key]();
      handled();
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
      scrollTo(top + rows);
    }
  }

  // Both sides scroll sideways together.
  let panes: (HTMLDivElement | undefined)[] = $state([undefined, undefined]);
  function syncScroll(i: number) {
    const from = panes[i];
    const to = panes[1 - i];
    if (from && to && to.scrollLeft !== from.scrollLeft) to.scrollLeft = from.scrollLeft;
  }

  /** Change markers for the overview bar, at most one per percent of height step. */
  const markers = $derived.by(() => {
    if (!total) return [];
    const out: { top: number; height: number; kind: string; hunk: number }[] = [];
    for (const b of blocks) {
      if (b.hunk < 0) continue;
      const k = b.leftLen && b.rightLen ? "changed" : b.leftLen ? "removed" : "added";
      const t = (b.row / total) * 100;
      const last = out.at(-1);
      if (last && t - (last.top + last.height) < 0.25) {
        last.height = Math.max(last.height, t + (b.rows / total) * 100 - last.top);
        if (last.kind !== k) last.kind = "changed";
        continue;
      }
      out.push({ top: t, height: (b.rows / total) * 100, kind: k, hunk: b.hunk });
    }
    return out;
  });

  const lineNo = (n: number | null) => (n === null ? "" : String(n + 1));
  const encodingLabel = (e: TextEncoding) => ENCODINGS.find((x) => x.id === e)?.label ?? e;
  const ENDINGS: Record<string, string> = { none: "No line breaks", lf: "LF", crLf: "CRLF", mixed: "Mixed line endings" };
  const locs = $derived([left, right]);
</script>

<svelte:window {onkeydown} />

<div class="backdrop">
  <div class="diff" role="dialog" aria-modal="true" aria-label="Compare {fileName(left.path)} and {fileName(right.path)}" data-testid="diff-view">
    <header>
      <h2>Compare Files</h2>
      {#if diff}
        <div class="modes" role="group" aria-label="Layout">
          <button aria-pressed={mode === "split"} onclick={() => setMode("split")}>Side by side</button>
          <button aria-pressed={mode === "inline"} onclick={() => setMode("inline")}>Inline</button>
        </div>
        <label><input type="checkbox" checked={options.ignoreWhitespace} onchange={(e) => setOption("ignoreWhitespace", e.currentTarget.checked)} /> Ignore whitespace</label>
        <label><input type="checkbox" checked={options.ignoreCase} onchange={(e) => setOption("ignoreCase", e.currentTarget.checked)} /> Ignore case</label>
        <label
          ><input type="checkbox" checked={options.ignoreLineEndings} onchange={(e) => setOption("ignoreLineEndings", e.currentTarget.checked)} /> Ignore line endings</label
        >
        <button onclick={previous} disabled={!count} title="Previous change (Alt ↑ or Shift F7)">Previous</button>
        <button onclick={next} disabled={!count} title="Next change (Alt ↓ or F7)">Next</button>
        <span class="status" data-testid="diff-status">{status}</span>
      {/if}
      <div class="spacer"></div>
      {#if onexternal}
        <button onclick={external} title="Open both files in your compare app">Open in external app…</button>
      {/if}
      <button class="close" onclick={requestClose} aria-label="Close" title="Close (Esc)">×</button>
    </header>

    {#if diff}
      <div class="sides" class:inline={mode === "inline"}>
        {#each SIDES as side, i (side)}
          {@const info = diff.sides[i]}
          <div class="side-head" data-testid="diff-{side}-head">
            <span class="name" title="{nameOf(locs[i].fs)} · {locs[i].path}">{fileName(locs[i].path)}</span>
            {#if info.dirty}<span class="dirty" title="Changed, not saved">●</span>{/if}
            <select
              aria-label="Encoding of the {side} file"
              value={info.encoding}
              disabled={info.dirty || busy}
              onchange={(e) => setEncoding(side, e.currentTarget.value as TextEncoding)}
            >
              {#each ENCODINGS as enc (enc.id)}
                <option value={enc.id}>{enc.label}</option>
              {/each}
            </select>
            <button
              onclick={() => saveSide(side)}
              disabled={!info.dirty || busy}
              aria-label="Save {side}"
              title={info.saveable ? `Save ${fileName(locs[i].path)}` : "This file does not read back the same in this encoding"}>Save</button
            >
          </div>
        {/each}
      </div>
    {/if}

    <div class="body">
      {#if error}
        <p class="message" role="alert">Could not compare the files: {error}</p>
      {:else if !outcome}
        <p class="message">Comparing…</p>
      {:else if outcome.kind === "binary"}
        <p class="message" data-testid="diff-binary">
          {outcome.identical ? "The files are identical (compared byte by byte)." : "Binary files differ."}
          <span class="muted">{formatBytes(outcome.sizes[0])} and {formatBytes(outcome.sizes[1])}</span>
        </p>
      {:else if outcome.kind === "tooLarge"}
        <p class="message" data-testid="diff-too-large">
          These files are too big to compare here (more than {formatBytes(outcome.maxBytes)} or {outcome.maxLines.toLocaleString()} lines). Open them in your compare app instead.
        </p>
      {:else if diff}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
        <div class="rows" id="diff-rows" bind:this={body} role="document" aria-label="Differences" tabindex="0" data-testid="diff-rows" data-top={top} {onwheel}>
          {#if mode === "split"}
            {#each SIDES as side, i (side)}
              {#if i === 1}
                <div class="gutter" aria-label="Copy changes">
                  {#each shown as s (s.row)}
                    <div class="grow">
                      {#if s.ref.hunk >= 0 && s.ref.first}
                        <button class="copy" onclick={() => copy(s.ref.hunk, "right")} aria-label="Copy change {s.ref.hunk + 1} to the right" title="Copy to the right">→</button>
                        <button class="copy" onclick={() => copy(s.ref.hunk, "left")} aria-label="Copy change {s.ref.hunk + 1} to the left" title="Copy to the left">←</button>
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}
              <div class="pane" bind:this={panes[i]} onscroll={() => syncScroll(i)} data-testid="diff-{side}">
                {#each shown as s (s.row)}
                  {@const k = kind(s.ref, side)}
                  {@const line = side === "left" ? s.l : s.r}
                  <div class="row {k}" class:current={s.ref.hunk >= 0 && s.ref.hunk === current} data-row={s.row}>
                    <span class="ln" aria-hidden="true">{lineNo(side === "left" ? s.ref.left : s.ref.right)}</span>
                    <span class="tx"
                      >{#each pieces(line, k !== "same") as p, j (j)}{#if p.mark}<mark>{p.text}</mark>{:else}{p.text}{/if}{/each}</span
                    >
                  </div>
                {/each}
              </div>
            {/each}
          {:else}
            <div class="gutter narrow" aria-label="Copy changes">
              {#each shown as s (s.row)}
                <div class="grow">
                  {#if s.ref.hunk >= 0 && s.ref.first}
                    <button class="copy" onclick={() => copy(s.ref.hunk, "right")} aria-label="Copy change {s.ref.hunk + 1} to the right" title="Copy to the right">→</button>
                    <button class="copy" onclick={() => copy(s.ref.hunk, "left")} aria-label="Copy change {s.ref.hunk + 1} to the left" title="Copy to the left">←</button>
                  {/if}
                </div>
              {/each}
            </div>
            <div class="pane" data-testid="diff-inline">
              {#each shown as s (s.row)}
                {@const k = s.ref.hunk < 0 ? "same" : s.ref.left !== null ? "removed" : "added"}
                <div class="row {k}" class:current={s.ref.hunk >= 0 && s.ref.hunk === current} data-row={s.row}>
                  <span class="lns" aria-hidden="true"><span>{lineNo(s.ref.left)}</span><span>{lineNo(s.ref.right)}</span></span>
                  <span class="sign" aria-hidden="true">{k === "removed" ? "−" : k === "added" ? "+" : ""}</span>
                  <span class="tx"
                    >{#each pieces(k === "added" ? s.r : s.l, k !== "same") as p, j (j)}{#if p.mark}<mark>{p.text}</mark>{:else}{p.text}{/if}{/each}</span
                  >
                </div>
              {/each}
            </div>
          {/if}
        </div>
        <div class="overview" data-testid="diff-overview" aria-hidden="true">
          {#each markers as m (m.hunk)}
            <button
              class="marker {m.kind}"
              tabindex="-1"
              style:top="{m.top}%"
              style:height="max(2px, {m.height}%)"
              aria-label="Change {m.hunk + 1}"
              onclick={() => go(m.hunk)}
            ></button>
          {/each}
        </div>
        <Scrollbar
          value={maxTop ? top / maxTop : 0}
          thumb={total ? visible / total : 1}
          label="Compare position"
          controls="diff-rows"
          onjump={(f) => scrollTo(f * maxTop)}
        />
      {/if}
    </div>

    <footer>
      {#if diff}
        <span>{encodingLabel(diff.sides[0].encoding)} · {ENDINGS[diff.sides[0].endings]} · {diff.sides[0].lines.toLocaleString()} lines</span>
        <span class="vs">|</span>
        <span>{encodingLabel(diff.sides[1].encoding)} · {ENDINGS[diff.sides[1].endings]} · {diff.sides[1].lines.toLocaleString()} lines</span>
      {/if}
      <span class="status" role="status" data-testid="diff-message">{message}</span>
      <div class="spacer"></div>
      <span class="muted">Alt ↓ / ↑ next / previous change · Alt → / ← copy it · Esc closes</span>
    </footer>

    {#if confirming}
      <div class="confirm-backdrop">
        <div class="confirm" role="alertdialog" aria-modal="true" aria-labelledby="diff-confirm-title" data-testid="diff-unsaved">
          <h3 id="diff-confirm-title">Save changes before closing?</h3>
          <p>
            {#each SIDES.filter((_, i) => diff?.sides[i].dirty) as side, i (side)}{i ? " and " : ""}“{fileName(
                (side === "left" ? left : right).path,
              )}”{/each} changed.
          </p>
          <div class="buttons">
            <button onclick={() => (confirming = false)}>Cancel</button>
            <button onclick={onclose}>Don’t Save</button>
            <!-- svelte-ignore a11y_autofocus -->
            <button class="primary" onclick={saveAndClose} autofocus>Save</button>
          </div>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: grid;
    place-items: stretch;
    padding: 16px;
    z-index: 9;
  }
  .diff {
    --removed: color-mix(in srgb, var(--danger) 13%, var(--pane-bg));
    --removed-strong: color-mix(in srgb, var(--danger) 38%, var(--pane-bg));
    --added: color-mix(in srgb, var(--success) 13%, var(--pane-bg));
    --added-strong: color-mix(in srgb, var(--success) 38%, var(--pane-bg));
    position: relative;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  header,
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  footer {
    border-bottom: none;
    border-top: 1px solid var(--border);
    padding: 4px 12px;
    font-size: 12px;
  }
  h2 {
    margin: 0;
    font-size: 14px;
  }
  label {
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .muted,
  .status,
  .vs {
    color: var(--muted);
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .modes {
    display: flex;
    gap: 2px;
  }
  button[aria-pressed="true"] {
    background: var(--accent);
    color: var(--selection-text);
  }
  .close {
    font-size: 16px;
    line-height: 1;
    padding: 2px 8px;
  }
  .sides {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 44px;
    padding: 4px 24px 4px 8px;
    border-bottom: 1px solid var(--border);
    background: var(--stripe);
  }
  .sides.inline {
    gap: 12px;
  }
  .side-head {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: 12px;
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }
  .dirty {
    color: var(--accent);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .rows {
    flex: 1;
    min-width: 0;
    display: flex;
    overflow: hidden;
    outline: none;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
    font-size: 12.5px;
    line-height: 18px;
    tab-size: 4;
  }
  .rows:focus-visible {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .pane {
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .row,
  .grow {
    height: 18px;
    white-space: pre;
  }
  .row {
    display: flex;
    width: max-content;
    min-width: 100%;
  }
  .row.removed {
    background: var(--removed);
  }
  .row.added {
    background: var(--added);
  }
  .row.filler {
    background: repeating-linear-gradient(135deg, var(--stripe) 0 4px, var(--pane-bg) 4px 8px);
  }
  .row.current .ln,
  .row.current .lns {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .removed mark {
    background: var(--removed-strong);
    color: inherit;
    border-radius: 2px;
  }
  .added mark {
    background: var(--added-strong);
    color: inherit;
    border-radius: 2px;
  }
  .ln {
    position: sticky;
    left: 0;
    flex: none;
    width: 6ch;
    padding-right: 1ch;
    text-align: right;
    color: var(--muted);
    background: var(--stripe);
    user-select: none;
  }
  .lns {
    position: sticky;
    left: 0;
    flex: none;
    display: flex;
    color: var(--muted);
    background: var(--stripe);
    user-select: none;
  }
  .lns span {
    width: 6ch;
    padding-right: 1ch;
    text-align: right;
  }
  .sign {
    flex: none;
    width: 2ch;
    text-align: center;
    color: var(--muted);
  }
  .tx {
    padding-left: 1ch;
  }
  .gutter {
    flex: none;
    width: 44px;
    border-left: 1px solid var(--border);
    border-right: 1px solid var(--border);
    background: var(--chrome);
  }
  .gutter.narrow {
    border-left: none;
  }
  .grow {
    display: flex;
    justify-content: center;
    gap: 2px;
  }
  .copy {
    height: 16px;
    margin-top: 1px;
    padding: 0 4px;
    font-size: 11px;
    line-height: 1;
  }
  .overview {
    position: relative;
    flex: none;
    width: 10px;
    border-left: 1px solid var(--border);
    background: var(--pane-bg);
  }
  .marker {
    position: absolute;
    left: 1px;
    right: 1px;
    padding: 0;
    border: none;
    border-radius: 1px;
    min-height: 2px;
    cursor: pointer;
  }
  .marker.removed {
    background: var(--danger);
  }
  .marker.added {
    background: var(--success);
  }
  .marker.changed {
    background: var(--accent);
  }
  .message {
    margin: auto;
    color: var(--muted);
    text-align: center;
    max-width: 60ch;
  }
  .message .muted {
    display: block;
    margin-top: 4px;
  }
  .confirm-backdrop {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.25);
  }
  .confirm {
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 16px 20px;
    max-width: 420px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.3);
  }
  .confirm h3 {
    margin: 0 0 8px;
    font-size: 14px;
  }
  .confirm p {
    margin: 0 0 14px;
    font-size: 13px;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .primary {
    background: var(--accent);
    color: var(--selection-text);
  }
</style>
