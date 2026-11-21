// Sources for the viewer tests.
import type { ByteSource } from "../../src/lib/viewer/source";

/** Width of one line of `bigSource`: "line 000000001\n". */
export const LINE = 15;

/**
 * A source of `lines` fixed-width lines, generated on demand, so a test can
 * use a 1 GB file without the memory. Records every read.
 */
export function bigSource(lines: number): ByteSource & { reads: [number, number][] } {
  const size = lines * LINE;
  const reads: [number, number][] = [];
  return {
    size,
    reads,
    read: async (offset, length) => {
      reads.push([offset, length]);
      const end = Math.min(size, offset + length);
      const out = new Uint8Array(Math.max(0, end - offset));
      for (let o = offset; o < end; ) {
        const n = Math.floor(o / LINE) + 1;
        const line = `line ${String(n).padStart(9, "0")}\n`;
        const from = o % LINE;
        for (let i = from; i < LINE && o < end; i++, o++) out[o - offset] = line.charCodeAt(i);
      }
      return out;
    },
    countLines: async (upto) => Math.floor(upto / LINE),
  };
}
