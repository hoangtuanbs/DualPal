import type { FailedItem } from "./api";

export const KB = 1024;
export const MB = 1024 * 1024;

/** Options for new copies and moves, chosen in the Operations panel. */
export interface TransferDefaults {
  verify: boolean;
  keepGoing: boolean;
}

/** Failed items as text, one `path: reason` per line, to copy or save. */
export function failureText(items: FailedItem[]) {
  return items.map((f) => `${f.source.path}: ${f.error.message}\n`).join("");
}

/** A bandwidth limit for display, e.g. "512 KB/s" or "1.5 MB/s". */
export function formatRate(bytesPerSec: number) {
  if (bytesPerSec >= MB) return `${Math.round((bytesPerSec / MB) * 10) / 10} MB/s`;
  return `${Math.round((bytesPerSec / KB) * 10) / 10} KB/s`;
}
