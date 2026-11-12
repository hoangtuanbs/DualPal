import type { Connection, ImportedConnection, ImportReport } from "./api";

/** Every importable server ticked; the selection holds indexes into `report.connections`. */
export function allSelected(report: ImportReport): Set<number> {
  return new Set(report.connections.map((_, i) => i));
}

/** The selection with entry `index` ticked or unticked. The input is not changed. */
export function toggled(selection: ReadonlySet<number>, index: number): Set<number> {
  const next = new Set(selection);
  if (next.has(index)) next.delete(index);
  else next.add(index);
  return next;
}

/** The ticked servers, in the order the export listed them. */
export function chosen(report: ImportReport, selection: ReadonlySet<number>): ImportedConnection[] {
  return report.connections.filter((_, i) => selection.has(i));
}

export function importButtonLabel(count: number): string {
  if (count === 0) return "Import";
  return count === 1 ? "Import 1 server" : `Import ${count} servers`;
}

/** One line for the result, e.g. "2 servers can be imported, 1 skipped." */
export function summary(report: ImportReport): string {
  const found = report.connections.length;
  const skipped = report.skipped.length;
  const servers =
    found === 0 ? "No servers can be imported" : found === 1 ? "1 server can be imported" : `${found} servers can be imported`;
  return skipped ? `${servers}, ${skipped} skipped.` : `${servers}.`;
}

/** Shown under a server that came without a password; null when none is missing. */
export function passwordNote(item: ImportedConnection): string | null {
  return item.passwordMissing ? "No password in the export: you will be asked for it when you connect." : null;
}

/** Protocol and address, e.g. "SFTP · alice@example.com:22". */
export function describeEndpoint(c: Connection): string {
  const e = c.endpoint;
  switch (e.type) {
    case "sftp":
      return `SFTP · ${e.user}@${e.host}:${e.port}`;
    case "ftp":
      return `${e.security === "none" ? "FTP" : "FTPS"} · ${e.user || "anonymous"}@${e.host}:${e.port}`;
    case "webDav":
      return `WebDAV · ${e.url}`;
    case "s3":
      return `S3 · bucket ${e.bucket}`;
  }
}
