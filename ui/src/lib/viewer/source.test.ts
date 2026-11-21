import { describe, expect, it } from "vitest";
import { memorySource, PagedSource } from "./source";
import { toBytes } from "./viewer-api";

describe("UI-UNIT-032 viewer page cache", () => {
  const data = Uint8Array.from({ length: 1000 }, (_, i) => i % 251);
  const fetcher = () => {
    const asked: number[] = [];
    let fail = false;
    const f = async (offset: number, length: number) => {
      asked.push(offset);
      if (fail) {
        fail = false;
        throw new Error("dropped");
      }
      return data.slice(offset, offset + length);
    };
    return { asked, f, failNext: () => (fail = true) };
  };

  it("joins pages, cuts at the end and reads each page once", async () => {
    const { asked, f } = fetcher();
    const src = new PagedSource(1000, f, 100, 4);
    expect([...(await src.read(150, 100))]).toEqual([...data.slice(150, 250)]);
    expect(asked).toEqual([100, 200]);
    expect([...(await src.read(180, 30))]).toEqual([...data.slice(180, 210)]);
    expect(asked).toEqual([100, 200]);
    expect([...(await src.read(950, 100))]).toEqual([...data.slice(950)]);
    expect((await src.read(1000, 10)).length).toBe(0);
    expect((await src.read(5, 0)).length).toBe(0);
  });

  it("shares a fetch that is on its way, forgets old pages and retries failed ones", async () => {
    const { asked, f, failNext } = fetcher();
    const src = new PagedSource(1000, f, 100, 2);
    await Promise.all([src.read(0, 10), src.read(20, 10)]);
    expect(asked).toEqual([0]);
    await src.read(100, 1);
    await src.read(200, 1);
    await src.read(0, 1); // evicted: fetched again
    expect(asked).toEqual([0, 100, 200, 0]);
    failNext();
    await expect(src.read(500, 1)).rejects.toThrow("dropped");
    expect([...(await src.read(500, 1))]).toEqual([data[500]]);
  });

  it("memory sources count lines in UTF-8 and UTF-16", async () => {
    expect(await memorySource(new TextEncoder().encode("a\nb\nc")).countLines!(4, "utf-8")).toBe(2);
    expect(await memorySource(new Uint8Array([97, 0, 10, 0, 98, 0])).countLines!(6, "utf-16le")).toBe(1);
  });

  it("takes bytes from an ArrayBuffer, a typed array or a plain array", () => {
    expect([...toBytes(new Uint8Array([1, 2]).buffer)]).toEqual([1, 2]);
    expect([...toBytes([3, 4])]).toEqual([3, 4]);
    expect([...toBytes(new Uint16Array([0x0201]))]).toEqual([1, 2]);
    expect(toBytes(null).length).toBe(0);
  });
});
