// Typed wrappers around the viewer commands in src-tauri/src/viewer.rs.
// Types mirror crates/dualpal-core/src/viewer.rs.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Location } from "../api";

/** WHATWG labels, as `TextDecoder` takes them. */
export type TextEncoding = "utf-8" | "utf-16le" | "utf-16be" | "windows-1252" | "iso-8859-1";

/** What the start of a file says about its text. */
export interface Detected {
  encoding: TextEncoding;
  /** Length of the byte order mark (0 without one). */
  bom: number;
  /** The bytes do not look like text. */
  binary: boolean;
}

/** A file open in the viewer, from `viewer_open`. */
export interface ViewerFile {
  session: number;
  size: number;
  modifiedMs: number | null;
  detected: Detected;
  /** Reads can start anywhere without reading what is before. */
  seekable: boolean;
}

/** Raw bytes from the backend: an ArrayBuffer from Tauri, an array from mocks. */
export function toBytes(data: unknown): Uint8Array {
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (Array.isArray(data)) return Uint8Array.from(data as number[]);
  return new Uint8Array();
}

export const viewerApi = {
  open: (location: Location) => invoke<ViewerFile>("viewer_open", { location }),
  read: async (session: number, offset: number, length: number) =>
    toBytes(await invoke<unknown>("viewer_read", { session, offset, length })),
  /**
   * Offset of the first match at or after `from` (or the last one before it, `backward`), or null.
   * `encoding` is the text encoding of a text search (UTF-16 matches start on a code unit and fold
   * whole ones); null compares raw bytes.
   */
  search: (
    session: number,
    request: number,
    needle: Uint8Array,
    from: number,
    backward: boolean,
    ignoreCase: boolean,
    encoding: TextEncoding | null,
  ) => invoke<number | null>("viewer_search", { session, request, needle: [...needle], from, backward, ignoreCase, encoding }),
  /** Line feeds in the first `upto` bytes. */
  countLines: (session: number, request: number, upto: number, encoding: TextEncoding) =>
    invoke<number>("viewer_count_lines", { session, request, upto, encoding }),
  cancel: (request: number) => invoke<void>("viewer_cancel", { request }),
  close: (session: number) => invoke<void>("viewer_close", { session }),
  onSearchProgress: (cb: (p: { request: number; scanned: number }) => void): Promise<UnlistenFn> =>
    listen<{ request: number; scanned: number }>("viewer-search-progress", (e) => cb(e.payload)),
};
