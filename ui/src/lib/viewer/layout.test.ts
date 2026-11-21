import { describe, expect, it } from "vitest";
import { encode } from "./encoding";
import { charRange, MAX_LINE, posAt, readSegments, rowOffset, rowsFrom, rowsUp, segmentAt, startPos, type TextOptions } from "./layout";
import { memorySource } from "./source";
import { bigSource, LINE } from "../../../tests/viewer/fixtures";

const utf8 = (s: string) => new TextEncoder().encode(s);
const plain: TextOptions = { encoding: "utf-8", bom: 0, cols: null };

describe("UI-UNIT-031 viewer text layout", () => {
  it("splits lines, drops CR, keeps empty lines", async () => {
    const src = memorySource(utf8("one\r\ntwo\n\nfour"));
    const segs = await readSegments(src, plain, 0, false, 10);
    expect(segs.map((s) => s.text)).toEqual(["one", "two", "", "four"]);
    expect(segs.map((s) => [s.start, s.end])).toEqual([[0, 5], [5, 9], [9, 10], [10, 14]]);
    const rows = await rowsFrom(src, plain, startPos(plain), 10);
    expect(rows.map((r) => r.pos.line)).toEqual([1, 2, 3, 4]);
  });

  it("cuts very long lines without splitting a UTF-8 character, and numbers them once", async () => {
    const long = "é".repeat(MAX_LINE); // 2 bytes each
    const src = memorySource(utf8(`${long}\nend`));
    const segs = await readSegments(src, plain, 0, false, 10);
    expect(segs.length).toBe(3);
    expect(segs[0].bytes.length).toBe(MAX_LINE);
    expect(segs.slice(0, 2).map((s) => s.text).join("")).toBe(long);
    expect(segs.map((s) => s.cont)).toEqual([false, true, false]);
    const rows = await rowsFrom(src, plain, startPos(plain), 10);
    expect(rows.map((r) => r.pos.line)).toEqual([1, 1, 2]);
  });

  it("lays out UTF-16 after its byte order mark", async () => {
    const bytes = new Uint8Array([0xff, 0xfe, ...encode("ab\r\ncd\n", "utf-16le")!]);
    const opts: TextOptions = { encoding: "utf-16le", bom: 2, cols: null };
    const rows = await rowsFrom(memorySource(bytes), opts, startPos(opts), 10);
    expect(rows.map((r) => r.text)).toEqual(["ab", "cd"]);
    expect(rows[1].pos.seg).toBe(10);
  });

  it("wraps at the column width", async () => {
    const opts: TextOptions = { encoding: "utf-8", bom: 0, cols: 4 };
    const src = memorySource(utf8("abcdefghij\nxy"));
    const rows = await rowsFrom(src, opts, startPos(opts), 10);
    expect(rows.map((r) => r.text)).toEqual(["abcd", "efgh", "ij", "xy"]);
    expect(rows.map((r) => r.pos.sub)).toEqual([0, 1, 2, 0]);
    // Up from "xy" is the last wrapped row of the line before.
    const up = await rowsUp(src, opts, rows[3].pos, 1);
    expect(up).toEqual(rows[2].pos);
    expect((await posAt(src, opts, 6)).sub).toBe(1);
  });

  it("cuts a long UTF-16 line without splitting a surrogate pair", async () => {
    // 2047 characters put the emoji's high surrogate last in the first MAX_LINE bytes.
    const text = `${"a".repeat(MAX_LINE / 2 - 1)}😀b`;
    for (const encoding of ["utf-16le", "utf-16be"] as const) {
      const opts: TextOptions = { encoding, bom: 0, cols: null };
      const segs = await readSegments(memorySource(encode(text, encoding)!), opts, 0, false, 10);
      expect(segs.length).toBe(2);
      expect(segs.map((s) => s.text).join("")).toBe(text);
      expect(segs[1].text).toBe("😀b");
    }
  });

  it("wraps on whole characters and maps a wrapped row back to its byte", async () => {
    const opts: TextOptions = { encoding: "utf-8", bom: 0, cols: 3 };
    const src = memorySource(utf8("ab😀cd\nx")); // the emoji is 2 UTF-16 code units, 4 bytes
    const rows = await rowsFrom(src, opts, startPos(opts), 10);
    expect(rows.map((r) => r.text)).toEqual(["ab", "😀c", "d", "x"]);
    expect(rows.map((r) => rowOffset(r, "utf-8"))).toEqual([0, 2, 7, 9]);
    expect((await posAt(src, opts, 2)).sub).toBe(1);
    expect((await posAt(src, opts, 7)).sub).toBe(2);
    const up = await rowsUp(src, opts, rows[3].pos, 2);
    expect(up).toEqual(rows[1].pos);
  });

  it("goes up and down a big file the same way, reading only near the screen", async () => {
    const src = bigSource(70_000_000); // ~1 GB
    const mid = 500_000_000;
    const at = await posAt(src, plain, mid);
    expect(at.seg).toBe(Math.floor(mid / LINE) * LINE);
    expect(at.line).toBeNull();
    const pos = { ...at, line: Math.floor(mid / LINE) + 1 };
    const down = await rowsFrom(src, plain, pos, 40);
    expect(down[0].text).toBe(`line ${String(pos.line).padStart(9, "0")}`);
    expect(down[39].pos.line).toBe(pos.line + 39);
    const back = await rowsUp(src, plain, down[39].pos, 39);
    expect(back).toEqual(pos);
    // Every read stayed within a few pages of the place.
    for (const [o] of src.reads) expect(Math.abs(o - mid)).toBeLessThan(200_000);
    // The last line ends the file.
    const last = await segmentAt(src, plain, src.size);
    expect(last!.end).toBe(src.size);
  });

  it("maps a byte range to characters of a segment", async () => {
    const [seg] = await readSegments(memorySource(utf8("héllo wörld")), plain, 0, false, 1);
    // "wörld" starts at byte 7, character 6.
    expect(charRange(seg, "utf-8", 7, 6)).toEqual([6, 11]);
    expect(charRange(seg, "utf-8", 100, 2)).toBeNull();
  });

  it("shows control characters as dots and an empty file as one empty row", async () => {
    const [seg] = await readSegments(memorySource(utf8("a\u0001b\tc")), plain, 0, false, 1);
    expect(seg.text).toBe("a·b\tc");
    const rows = await rowsFrom(memorySource(new Uint8Array()), plain, startPos(plain), 5);
    expect(rows.map((r) => r.text)).toEqual([""]);
  });
});
