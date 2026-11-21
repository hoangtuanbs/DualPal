// The rows of the compare view, worked out from the hunks without holding a
// row per line: equal runs and changes become blocks, and a row is found
// by a binary search over them.
import type { Hunk } from "./diff-api";

/** Side by side (a changed run takes as many rows as its longer side) or inline (removed lines, then added ones). */
export type Mode = "split" | "inline";

export interface Block {
  /** Index of the hunk, or -1 for lines that are the same. */
  hunk: number;
  /** First row. */
  row: number;
  rows: number;
  left: number;
  leftLen: number;
  right: number;
  rightLen: number;
}

export interface RowRef {
  hunk: number;
  /** Line of each side shown on the row (from 0), or null for none. */
  left: number | null;
  right: number | null;
  /** First row of its block. */
  first: boolean;
}

/** Blocks for files of `leftLines` and `rightLines` lines with these changes. */
export function buildBlocks(hunks: Hunk[], leftLines: number, rightLines: number, mode: Mode): Block[] {
  const out: Block[] = [];
  let row = 0;
  let l = 0;
  let r = 0;
  const equal = (n: number) => {
    if (n <= 0) return;
    out.push({ hunk: -1, row, rows: n, left: l, leftLen: n, right: r, rightLen: n });
    row += n;
    l += n;
    r += n;
  };
  hunks.forEach((h, i) => {
    equal(h.leftStart - l);
    const rows = mode === "split" ? Math.max(h.leftLen, h.rightLen) : h.leftLen + h.rightLen;
    out.push({ hunk: i, row, rows, left: h.leftStart, leftLen: h.leftLen, right: h.rightStart, rightLen: h.rightLen });
    row += rows;
    l = h.leftStart + h.leftLen;
    r = h.rightStart + h.rightLen;
  });
  equal(Math.max(leftLines - l, rightLines - r));
  return out;
}

export const totalRows = (blocks: Block[]) => {
  const last = blocks.at(-1);
  return last ? last.row + last.rows : 0;
};

/** What row `row` shows, or null past the end. */
export function rowAt(blocks: Block[], row: number, mode: Mode): RowRef | null {
  let lo = 0;
  let hi = blocks.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const b = blocks[mid];
    if (row < b.row) hi = mid - 1;
    else if (row >= b.row + b.rows) lo = mid + 1;
    else {
      const k = row - b.row;
      const first = k === 0;
      if (b.hunk < 0) return { hunk: -1, left: b.left + k, right: b.right + k, first };
      if (mode === "inline") {
        return k < b.leftLen
          ? { hunk: b.hunk, left: b.left + k, right: null, first }
          : { hunk: b.hunk, left: null, right: b.right + k - b.leftLen, first };
      }
      return {
        hunk: b.hunk,
        left: k < b.leftLen ? b.left + k : null,
        right: k < b.rightLen ? b.right + k : null,
        first,
      };
    }
  }
  return null;
}

/** First row of hunk `i`. */
export function hunkRow(blocks: Block[], i: number): number {
  return blocks.find((b) => b.hunk === i)?.row ?? 0;
}
