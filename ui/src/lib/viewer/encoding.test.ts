import { describe, expect, it } from "vitest";
import { decode, encode, ENCODINGS, parseHex } from "./encoding";

describe("UI-UNIT-030 viewer encodings", () => {
  it("decodes each encoding", () => {
    expect(decode(new Uint8Array([0x63, 0x61, 0x66, 0xe9, 0x20, 0x80]), "windows-1252")).toBe("café €");
    expect(decode(new Uint8Array([0x63, 0x61, 0x66, 0xe9, 0x20, 0x80]), "iso-8859-1")).toBe("café \u0080");
    expect(decode(new TextEncoder().encode("ok ✓"), "utf-8")).toBe("ok ✓");
    expect(decode(new Uint8Array([0x61, 0xff, 0x62]), "utf-8")).toBe("a�b");
    expect(decode(new Uint8Array([0x68, 0, 0x69, 0]), "utf-16le")).toBe("hi");
    expect(decode(new Uint8Array([0, 0x68, 0, 0x69, 0]), "utf-16be")).toBe("hi");
  });

  it("encodes and decodes back in every encoding", () => {
    for (const { id } of ENCODINGS) {
      const bytes = encode("Grüße, café", id);
      expect(bytes, id).not.toBeNull();
      expect(decode(bytes!, id), id).toBe("Grüße, café");
    }
    expect([...encode("€", "windows-1252")!]).toEqual([0x80]);
    expect(encode("€", "iso-8859-1")).toBeNull();
    expect(encode("✓", "windows-1252")).toBeNull();
    expect(decode(encode("😀", "utf-16be")!, "utf-16be")).toBe("😀");
  });

  it("reads hex bytes", () => {
    expect([...parseHex("DE AD be ef")!]).toEqual([0xde, 0xad, 0xbe, 0xef]);
    expect([...parseHex("0x4f4b")!]).toEqual([0x4f, 0x4b]);
    expect(parseHex("abc")).toBeNull();
    expect(parseHex("hello")).toBeNull();
    expect(parseHex("")).toBeNull();
  });
});
