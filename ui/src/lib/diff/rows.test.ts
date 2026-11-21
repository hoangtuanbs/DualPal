import { describe, expect, it } from "vitest";
import { buildBlocks, hunkRow, rowAt, totalRows } from "./rows";

// left:  a b c d e          right: a B c d X e
const hunks = [
  { leftStart: 1, leftLen: 1, rightStart: 1, rightLen: 1 },
  { leftStart: 4, leftLen: 0, rightStart: 4, rightLen: 1 },
];

describe("UI-UNIT-040 compare view rows", () => {
  it("lays out side by side: a changed run takes its longer side, the other side gets fillers", () => {
    const b = buildBlocks(hunks, 5, 6, "split");
    expect(totalRows(b)).toBe(6);
    expect([0, 1, 2, 3, 4, 5].map((r) => rowAt(b, r, "split"))).toEqual([
      { hunk: -1, left: 0, right: 0, first: true },
      { hunk: 0, left: 1, right: 1, first: true },
      { hunk: -1, left: 2, right: 2, first: true },
      { hunk: -1, left: 3, right: 3, first: false },
      { hunk: 1, left: null, right: 4, first: true },
      { hunk: -1, left: 4, right: 5, first: true },
    ]);
    expect(rowAt(b, 6, "split")).toBeNull();
    expect(hunkRow(b, 1)).toBe(4);
  });

  it("lays out inline: removed lines, then added ones", () => {
    const b = buildBlocks(hunks, 5, 6, "inline");
    expect(totalRows(b)).toBe(7);
    expect(rowAt(b, 1, "inline")).toEqual({ hunk: 0, left: 1, right: null, first: true });
    expect(rowAt(b, 2, "inline")).toEqual({ hunk: 0, left: null, right: 1, first: false });
    expect(rowAt(b, 5, "inline")).toEqual({ hunk: 1, left: null, right: 4, first: true });
    expect(hunkRow(b, 1)).toBe(5);
  });

  it("handles empty files and a million lines without a row each", () => {
    expect(totalRows(buildBlocks([], 0, 0, "split"))).toBe(0);
    const big = buildBlocks([{ leftStart: 999_990, leftLen: 2, rightStart: 999_990, rightLen: 5 }], 1_000_000, 1_000_003, "split");
    expect(big).toHaveLength(3);
    expect(totalRows(big)).toBe(1_000_003);
    expect(rowAt(big, 999_994, "split")).toEqual({ hunk: 0, left: null, right: 999_994, first: false });
    expect(rowAt(big, 1_000_002, "split")).toEqual({ hunk: -1, left: 999_999, right: 1_000_002, first: false });
  });
});
