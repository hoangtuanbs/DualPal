// Text encodings of the viewer: decode pages of bytes, encode search terms.
// Mirrors `decode` / `encode` in crates/dualpal-core/src/viewer.rs.
import type { TextEncoding } from "./viewer-api";

export const ENCODINGS: { id: TextEncoding; label: string }[] = [
  { id: "utf-8", label: "UTF-8" },
  { id: "utf-16le", label: "UTF-16 LE" },
  { id: "utf-16be", label: "UTF-16 BE" },
  { id: "windows-1252", label: "Windows-1252" },
  { id: "iso-8859-1", label: "Latin-1 (ISO-8859-1)" },
];

/** Bytes per code unit. */
export const unitOf = (enc: TextEncoding) => (enc === "utf-16le" || enc === "utf-16be" ? 2 : 1);

/** Windows-1252 characters for bytes 0x80..0x9F; the rest equal Latin-1. */
const CP1252_HIGH = [
  0x20ac, 0x81, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039, 0x152, 0x8d, 0x17d, 0x8f,
  0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x2dc, 0x2122, 0x161, 0x203a, 0x153, 0x9d, 0x17e, 0x178,
];

const decoders = new Map<string, TextDecoder>();
function decoder(label: string): TextDecoder {
  let d = decoders.get(label);
  if (!d) decoders.set(label, (d = new TextDecoder(label)));
  return d;
}

function singleByte(bytes: Uint8Array, cp1252: boolean): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 4096) {
    const part = bytes.subarray(i, i + 4096);
    const codes = Array.from(part, (b) => (cp1252 && b >= 0x80 && b <= 0x9f ? CP1252_HIGH[b - 0x80] : b));
    out += String.fromCharCode(...codes);
  }
  return out;
}

/** Decode bytes; what cannot be decoded becomes U+FFFD. */
export function decode(bytes: Uint8Array, enc: TextEncoding): string {
  switch (enc) {
    case "windows-1252":
      return singleByte(bytes, true);
    case "iso-8859-1":
      return singleByte(bytes, false);
    case "utf-16be": {
      // Swap to little-endian: not every TextDecoder knows utf-16be.
      const n = bytes.length & ~1;
      const le = new Uint8Array(n);
      for (let i = 0; i < n; i += 2) {
        le[i] = bytes[i + 1];
        le[i + 1] = bytes[i];
      }
      return decoder("utf-16le").decode(le);
    }
    case "utf-16le":
      return decoder("utf-16le").decode(bytes.subarray(0, bytes.length & ~1));
    default:
      return decoder("utf-8").decode(bytes);
  }
}

/** Encode text, e.g. a search term. Null when a character has no byte in a single-byte encoding. */
export function encode(text: string, enc: TextEncoding): Uint8Array | null {
  switch (enc) {
    case "utf-8":
      return new TextEncoder().encode(text);
    case "utf-16le":
    case "utf-16be": {
      const out = new Uint8Array(text.length * 2);
      for (let i = 0; i < text.length; i++) {
        const u = text.charCodeAt(i);
        const [a, b] = [u & 0xff, u >> 8];
        out[2 * i] = enc === "utf-16le" ? a : b;
        out[2 * i + 1] = enc === "utf-16le" ? b : a;
      }
      return out;
    }
    default: {
      const out: number[] = [];
      for (const ch of text) {
        const cp = ch.codePointAt(0)!;
        const high = enc === "windows-1252" ? CP1252_HIGH.indexOf(cp) : -1;
        if (high >= 0) out.push(0x80 + high);
        else if (cp <= 0x7f || (cp >= 0xa0 && cp <= 0xff) || (enc === "iso-8859-1" && cp <= 0xff)) out.push(cp);
        else return null;
      }
      return Uint8Array.from(out);
    }
  }
}

/** "DE AD be ef" or "deadbeef" → bytes; null if it is not whole hex bytes. */
export function parseHex(text: string): Uint8Array | null {
  const clean = text.replace(/[\s,]+/g, "").replace(/^0x/i, "");
  if (!clean || clean.length % 2 || !/^[0-9a-f]+$/i.test(clean)) return null;
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.slice(2 * i, 2 * i + 2), 16);
  return out;
}
