const UNITS = ["bytes", "KB", "MB", "GB", "TB"];

/** Decimal units, like Finder and Explorer's "size on disk" labels. */
export function formatBytes(n: number): string {
  if (n < 1000) return `${n} ${n === 1 ? "byte" : "bytes"}`;
  let i = 0;
  let v = n;
  while (v >= 1000 && i < UNITS.length - 1) {
    v /= 1000;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${UNITS[i]}`;
}

const dateFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

export function formatDate(ms: number | null): string {
  return ms == null ? "" : dateFmt.format(new Date(ms));
}

const shortFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" });

/** Compact date for list columns; the full date is in the tooltip. */
export function formatShortDate(ms: number | null): string {
  return ms == null ? "" : shortFmt.format(new Date(ms));
}

export function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString();
}
