// Text layout for the viewer, a page at a time: split bytes into segments
// (lines, with very long lines cut every MAX_LINE bytes) and segments into
// screen rows. Nothing needs the whole file: going down reads on from the
// top row, going up looks back for the previous line feed.
import { decode, unitOf } from "./encoding";
import type { ByteSource } from "./source";
import type { TextEncoding } from "./viewer-api";

/** Longest segment; a longer line goes on in the next one. */
export const MAX_LINE = 4096;
/** How far back to look for the start of a line. */
const LOOK_BACK = 64 * 1024;
/** Bytes read at a time while laying out. */
const CHUNK = 64 * 1024;

export interface TextOptions {
  encoding: TextEncoding;
  /** Bytes of the byte order mark the text starts after. */
  bom: number;
  /** Characters per row when wrapping; null for no wrapping. */
  cols: number | null;
}

/** One line of the file, or a MAX_LINE part of a longer one. */
export interface Segment {
  start: number;
  /** Offset after the segment, after its line feed if it has one. */
  end: number;
  /** The bytes shown, without the line break. */
  bytes: Uint8Array;
  /** Decoded text, control characters shown as "·". */
  text: string;
  /** Goes on from the previous segment (no line feed between them). */
  cont: boolean;
  /** Ends with a line feed. */
  brk: boolean;
}

/** A place in the layout: a segment and the wrapped row inside it. */
export interface Pos {
  seg: number;
  sub: number;
  cont: boolean;
  /** Line number (from 1) of the segment, when known. */
  line: number | null;
}

export interface Row {
  pos: Pos;
  segment: Segment;
  /** First character of this row in the segment's text. */
  from: number;
  text: string;
}

// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u0008\u000b-\u001f\u007f]/g;

export const textOf = (bytes: Uint8Array, enc: TextEncoding) => decode(bytes, enc).replace(CONTROL, "·");

function isNewline(b: Uint8Array, i: number, enc: TextEncoding): boolean {
  if (enc === "utf-16le") return b[i] === 10 && b[i + 1] === 0;
  if (enc === "utf-16be") return b[i] === 0 && b[i + 1] === 10;
  return b[i] === 10;
}

function isCr(b: Uint8Array, i: number, enc: TextEncoding): boolean {
  if (enc === "utf-16le") return b[i] === 13 && b[i + 1] === 0;
  if (enc === "utf-16be") return b[i] === 0 && b[i + 1] === 13;
  return b[i] === 13;
}

/** Offset `o` moved onto a code unit, at or after the text start. */
function align(o: number, opts: TextOptions): number {
  if (o <= opts.bom) return opts.bom;
  return unitOf(opts.encoding) === 2 ? o - ((o - opts.bom) % 2) : o;
}

/**
 * Segments from `start` (a segment start), at most `count` of them and none
 * past `stopAt`, which cuts the one that crosses it.
 */
export async function readSegments(
  src: ByteSource,
  opts: TextOptions,
  start: number,
  cont: boolean,
  count: number,
  stopAt = Infinity,
): Promise<Segment[]> {
  const enc = opts.encoding;
  const unit = unitOf(enc);
  const stop = Math.min(src.size, stopAt);
  const out: Segment[] = [];
  let pos = start;
  let buf: Uint8Array = new Uint8Array();
  let bufStart = pos;
  while (out.length < count && pos < stop) {
    if (bufStart + buf.length - pos < MAX_LINE + 2 * unit && bufStart + buf.length < src.size) {
      buf = await src.read(pos, CHUNK);
      bufStart = pos;
      if (!buf.length) break;
    }
    const rel = pos - bufStart;
    const limit = Math.min(buf.length, rel + MAX_LINE, stop - bufStart);
    // A line feed right after MAX_LINE bytes still ends this segment.
    const look = Math.min(buf.length, rel + MAX_LINE + unit, stop - bufStart);
    let nl = -1;
    for (let i = rel; i + unit <= look; i += unit) {
      if (isNewline(buf, i, enc)) {
        nl = i;
        break;
      }
    }
    let end: number;
    let contentEnd: number;
    if (nl >= 0) {
      contentEnd = nl;
      end = nl + unit;
      if (contentEnd - unit >= rel && isCr(buf, contentEnd - unit, enc)) contentEnd -= unit;
    } else {
      end = limit;
      // A lone byte at the end of UTF-16 text stays with the segment.
      if (unit === 2 && (end - rel) % 2 && end - 1 > rel) end -= 1;
      // A cut inside a long line must not split a UTF-8 character.
      if (enc === "utf-8" && end === rel + MAX_LINE) {
        let back = end;
        while (back > rel + 1 && back > end - 4 && (buf[back] & 0xc0) === 0x80) back--;
        end = back;
      }
      // Nor a UTF-16 surrogate pair: each segment is decoded on its own.
      if (unit === 2 && end === rel + MAX_LINE && end - 2 > rel) {
        const u = enc === "utf-16le" ? buf[end - 2] | (buf[end - 1] << 8) : (buf[end - 2] << 8) | buf[end - 1];
        if (u >= 0xd800 && u <= 0xdbff) end -= 2;
      }
      contentEnd = end;
    }
    if (end <= rel) break;
    const bytes = buf.subarray(rel, contentEnd);
    out.push({ start: pos, end: bufStart + end, bytes, text: textOf(bytes, enc), cont, brk: nl >= 0 });
    cont = nl < 0;
    pos = bufStart + end;
  }
  return out;
}

/**
 * The start of the line that holds `pos`. With `strict`, a line feed right
 * before `pos` does not count, so this is the line before a line start.
 * `cont` is true when no line feed was found close enough, so the line
 * really starts further back.
 */
async function lineStart(src: ByteSource, opts: TextOptions, pos: number, strict: boolean): Promise<{ start: number; cont: boolean }> {
  const unit = unitOf(opts.encoding);
  const from = align(Math.max(opts.bom, pos - LOOK_BACK), opts);
  const bytes = await src.read(from, pos - from);
  const last = bytes.length - (strict ? unit : 0);
  for (let i = last - unit; i >= 0; i -= unit) {
    if (isNewline(bytes, i, opts.encoding)) return { start: from + i + unit, cont: false };
  }
  if (from <= opts.bom) return { start: opts.bom, cont: false };
  let start = from;
  if (opts.encoding === "utf-8") while (start < pos && (bytes[start - from] & 0xc0) === 0x80) start++;
  return { start, cont: true };
}

/** The segment that ends at `pos` (a segment start after the text start). */
export async function segmentBefore(src: ByteSource, opts: TextOptions, pos: number): Promise<Segment | null> {
  if (pos <= opts.bom) return null;
  const line = await lineStart(src, opts, pos, true);
  const segs = await readSegments(src, opts, line.start, line.cont, Infinity, pos);
  return segs.at(-1) ?? null;
}

/** The segment that holds byte `offset` (the last one at or past the end). */
export async function segmentAt(src: ByteSource, opts: TextOptions, offset: number): Promise<Segment | null> {
  const o = align(Math.min(offset, src.size), opts);
  if (o >= src.size) return segmentBefore(src, opts, src.size);
  const line = await lineStart(src, opts, o + unitOf(opts.encoding), true);
  const segs = await readSegments(src, opts, line.start, line.cont, Infinity, o + MAX_LINE + 8);
  return segs.find((s) => s.end > o) ?? segs.at(-1) ?? null;
}

/**
 * Where each wrapped row of a text starts, in UTF-16 code units: every
 * `cols`, but a row ends a unit early rather than split a surrogate pair.
 */
export function wrapStarts(text: string, cols: number | null): number[] {
  const starts = [0];
  if (!cols) return starts;
  for (let at = 0; ; ) {
    let next = at + cols;
    if (next >= text.length) break;
    const c = text.charCodeAt(next - 1);
    if (c >= 0xd800 && c <= 0xdbff && next - 1 > at) next--;
    starts.push(next);
    at = next;
  }
  return starts;
}

/** How many screen rows a segment takes. */
export const rowCount = (seg: Segment, cols: number | null) => wrapStarts(seg.text, cols).length;

/** The first position of the text. */
export const startPos = (opts: TextOptions): Pos => ({ seg: opts.bom, sub: 0, cont: false, line: 1 });

/** `count` rows from `top` down (fewer at the end of the file). */
export async function rowsFrom(src: ByteSource, opts: TextOptions, top: Pos, count: number): Promise<Row[]> {
  const rows: Row[] = [];
  // Each segment gives at least one row; read a few more for wrapping.
  let segs = await readSegments(src, opts, top.seg, top.cont, count + 1);
  let line = top.line;
  let first = true;
  while (rows.length < count && segs.length) {
    for (const seg of segs) {
      if (!first) line = line === null ? null : seg.cont ? line : line + 1;
      const starts = wrapStarts(seg.text, opts.cols);
      for (let sub = first ? top.sub : 0; sub < starts.length && rows.length < count; sub++) {
        const from = starts[sub];
        const text = seg.text.slice(from, starts[sub + 1] ?? seg.text.length);
        rows.push({ pos: { seg: seg.start, sub, cont: seg.cont, line }, segment: seg, from, text });
      }
      first = false;
      if (rows.length >= count) break;
    }
    const last = segs.at(-1)!;
    if (rows.length >= count || last.end >= src.size) break;
    segs = await readSegments(src, opts, last.end, !last.brk, count - rows.length + 1);
  }
  // An empty file still shows one empty row.
  if (!rows.length && top.seg >= src.size && top.seg === opts.bom) {
    const seg: Segment = { start: opts.bom, end: opts.bom, bytes: new Uint8Array(), text: "", cont: false, brk: false };
    rows.push({ pos: startPos(opts), segment: seg, from: 0, text: "" });
  }
  return rows;
}

/** The position `count` rows above `pos` (stops at the start). */
export async function rowsUp(src: ByteSource, opts: TextOptions, pos: Pos, count: number): Promise<Pos> {
  let p = pos;
  for (let i = 0; i < count; i++) {
    if (p.sub > 0) {
      p = { ...p, sub: p.sub - 1 };
      continue;
    }
    const prev = await segmentBefore(src, opts, p.seg);
    if (!prev) break;
    const line = p.line === null ? null : p.cont ? p.line : p.line - 1;
    p = { seg: prev.start, sub: rowCount(prev, opts.cols) - 1, cont: prev.cont, line };
  }
  return p;
}

/** The position of the row that holds byte `offset`; its line number is not known. */
export async function posAt(src: ByteSource, opts: TextOptions, offset: number): Promise<Pos> {
  if (offset <= opts.bom) return startPos(opts);
  const seg = await segmentAt(src, opts, offset);
  if (!seg) return startPos(opts);
  let sub = 0;
  if (opts.cols && offset > seg.start) {
    const chars = textOf(seg.bytes.subarray(0, Math.min(seg.bytes.length, offset - seg.start)), opts.encoding).length;
    const starts = wrapStarts(seg.text, opts.cols);
    while (sub + 1 < starts.length && starts[sub + 1] <= chars) sub++;
  }
  return { seg: seg.start, sub, cont: seg.cont, line: seg.start <= opts.bom ? 1 : null };
}

/**
 * The byte offset a row starts at: the end of the longest start of its
 * segment that decodes to no more than the characters before the row.
 */
export function rowOffset(row: Row, enc: TextEncoding): number {
  const seg = row.segment;
  if (row.from <= 0) return seg.start;
  const unit = unitOf(enc);
  let lo = 0;
  let hi = Math.floor(seg.bytes.length / unit);
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (textOf(seg.bytes.subarray(0, mid * unit), enc).length <= row.from) lo = mid;
    else hi = mid - 1;
  }
  return seg.start + lo * unit;
}

/** Character range [from, to) of a byte range inside a segment, or null if they do not meet. */
export function charRange(seg: Segment, enc: TextEncoding, offset: number, length: number): [number, number] | null {
  const a = offset - seg.start;
  const b = a + length;
  if (b <= 0 || a >= seg.bytes.length || length <= 0) return null;
  const from = a <= 0 ? 0 : textOf(seg.bytes.subarray(0, a), enc).length;
  const to = textOf(seg.bytes.subarray(0, Math.min(b, seg.bytes.length)), enc).length;
  return [from, Math.max(to, from + 1)];
}
