// Where the viewer's bytes come from. `TextView` and `HexView` only need a
// `ByteSource`, so the file compare view can feed them its own sides.
import { viewerApi, type TextEncoding, type ViewerFile } from "./viewer-api";

export interface ByteSource {
  /** File size in bytes. */
  readonly size: number;
  /** Up to `length` bytes from `offset`: shorter at the end, empty past it. */
  read(offset: number, length: number): Promise<Uint8Array>;
  /** Line feeds before `upto`, when the source can count them cheaply. */
  countLines?(upto: number, encoding: TextEncoding): Promise<number>;
}

/** Bytes per page fetched from the backend. */
export const PAGE = 64 * 1024;

/**
 * A source that fetches whole pages and keeps the most recent ones, so the
 * views can ask for small overlapping ranges while scrolling. Requests for a
 * page already on its way share one fetch.
 */
export class PagedSource implements ByteSource {
  readonly size: number;
  /** Pages fetched so far (for tests). */
  fetches = 0;
  private pages = new Map<number, Promise<Uint8Array>>();
  constructor(
    size: number,
    private fetchPage: (offset: number, length: number) => Promise<Uint8Array>,
    private pageSize = PAGE,
    private keep = 64,
  ) {
    this.size = size;
  }

  private page(index: number): Promise<Uint8Array> {
    const hit = this.pages.get(index);
    if (hit) {
      // Most recently used last.
      this.pages.delete(index);
      this.pages.set(index, hit);
      return hit;
    }
    this.fetches++;
    const p = this.fetchPage(index * this.pageSize, this.pageSize);
    this.pages.set(index, p);
    p.catch(() => this.pages.delete(index));
    while (this.pages.size > this.keep) this.pages.delete(this.pages.keys().next().value!);
    return p;
  }

  async read(offset: number, length: number): Promise<Uint8Array> {
    const end = Math.min(this.size, offset + length);
    if (length <= 0 || offset >= end) return new Uint8Array();
    const first = Math.floor(offset / this.pageSize);
    const last = Math.floor((end - 1) / this.pageSize);
    const parts = await Promise.all(Array.from({ length: last - first + 1 }, (_, i) => this.page(first + i)));
    const out = new Uint8Array(end - offset);
    let at = 0;
    parts.forEach((bytes, i) => {
      const start = (first + i) * this.pageSize;
      const from = Math.max(0, offset - start);
      const to = Math.min(bytes.length, end - start);
      if (to > from) {
        out.set(bytes.subarray(from, to), at);
        at += to - from;
      }
    });
    return at === out.length ? out : out.subarray(0, at);
  }
}

/** Bytes already in memory. */
export function memorySource(bytes: Uint8Array): ByteSource {
  return {
    size: bytes.length,
    read: async (offset, length) => bytes.subarray(Math.min(offset, bytes.length), Math.min(bytes.length, offset + length)),
    countLines: async (upto, encoding) => {
      let n = 0;
      if (encoding === "utf-16le" || encoding === "utf-16be") {
        const [a, b] = encoding === "utf-16le" ? [10, 0] : [0, 10];
        for (let i = 0; i + 1 < Math.min(upto, bytes.length); i += 2) if (bytes[i] === a && bytes[i + 1] === b) n++;
      } else {
        for (let i = 0; i < Math.min(upto, bytes.length); i++) if (bytes[i] === 10) n++;
      }
      return n;
    },
  };
}

let nextRequest = 1;
/** A fresh id for a search or line count, to cancel it by. */
export const newRequest = () => nextRequest++;

/** A file opened with `viewer_open`. Line counts stream the file, so they are offered for local files only. */
export function backendSource(file: ViewerFile, local: boolean): PagedSource {
  const source = new PagedSource(file.size, (offset, length) => viewerApi.read(file.session, offset, length));
  if (local) {
    let running: number | null = null;
    (source as ByteSource).countLines = (upto, encoding) => {
      if (running !== null) viewerApi.cancel(running).catch(() => {});
      const request = (running = newRequest());
      return viewerApi.countLines(file.session, request, upto, encoding).finally(() => {
        if (running === request) running = null;
      });
    };
  }
  return source;
}
