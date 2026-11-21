// Archives (zip, tar, tar.gz, tar.bz2, tar.xz, 7z) opened as folders.
//
// The backend registers each open archive as a read-only file system with
// an id like "archive:…". This module remembers where each one came from,
// so that leaving its top returns to the folder holding it, and closes the
// archives no pane shows any more (which removes their temporary files).

import { SvelteMap } from "svelte/reactivity";
import { api, type Location, type OpenedArchive } from "./api";
import { fsNames } from "./fs-names.svelte";

/** Open archives, by file system id. */
export const openArchives = new SvelteMap<string, OpenedArchive>();

const ARCHIVE = /.+\.(zip|tar|tgz|tar\.gz|tbz|tbz2|tar\.bz2|txz|tar\.xz|7z)$/i;

/** Whether a file with this name opens as a folder. */
export function isArchiveName(name: string): boolean {
  return ARCHIVE.test(name);
}

/** The name without its archive extension: the folder "Extract Here" creates. */
export function archiveStem(name: string): string {
  const m = /^(.+?)\.(tar\.gz|tar\.bz2|tar\.xz|tgz|tbz2|tbz|txz|zip|tar|7z)$/i.exec(name);
  return m ? m[1] : name;
}

/** Whether `fs` is the inside of an archive. */
export function isArchiveFs(fs: string | undefined | null): boolean {
  return !!fs && fs.startsWith("archive:");
}

/** The archive being opened, with download or unpack progress. */
export const opening = $state<{
  current: { request: number; name: string; phase: string | null; done: number; total: number } | null;
}>({ current: null });

let nextRequest = 1;

/** Open the archive file at `source` and remember it. */
export async function openArchive(source: Location, name: string): Promise<OpenedArchive> {
  const request = nextRequest++;
  opening.current = { request, name, phase: null, done: 0, total: 0 };
  const unlisten = await api
    .onArchiveProgress((p) => {
      if (opening.current?.request === p.request) opening.current = { ...opening.current, ...p };
    })
    .catch(() => undefined);
  try {
    const a = await api.openArchive(source, request);
    openArchives.set(a.fsId, a);
    fsNames[a.fsId] = a.name;
    return a;
  } finally {
    unlisten?.();
    if (opening.current?.request === request) opening.current = null;
  }
}

/** Stop opening the archive (a download or unpack in progress). */
export function cancelOpening() {
  const r = opening.current?.request;
  if (r != null) api.cancelOpenArchive(r).catch(() => {});
}

/** Forget an archive and let the backend close it once nothing uses it. */
export function closeArchive(fsId: string) {
  if (!openArchives.delete(fsId)) return;
  delete fsNames[fsId];
  for (const f of forgetters) f(fsId);
  api.closeArchive(fsId).catch(() => {});
}

/** Where every pane is (all tabs of both sides); set by the app. */
let shown: () => (Location | null)[] = () => [];
/** Called with each closed archive, so panes can drop it from their history. */
const forgetters = new Set<(fsId: string) => void>();

export function setShownLocations(f: () => (Location | null)[]) {
  shown = f;
}

export function onArchiveClosed(f: (fsId: string) => void): () => void {
  forgetters.add(f);
  return () => forgetters.delete(f);
}

/** Close the archives that no pane shows. An archive holding a shown
 *  archive (a zip inside a zip) stays open, since leaving goes back into it. */
export function releaseArchives() {
  const used = new Set<string>();
  for (const loc of shown()) {
    let fs = loc?.fs;
    while (fs && openArchives.has(fs) && !used.has(fs)) {
      used.add(fs);
      fs = openArchives.get(fs)!.source.fs;
    }
  }
  for (const id of [...openArchives.keys()]) if (!used.has(id)) closeArchive(id);
}

/** Path crumbs for a location inside an archive: the folders holding the
 *  archive (from `outer`, the crumbs of its source), then the archive
 *  itself, then the folders inside it. */
export function archiveCrumbs(
  loc: Location,
  outer: (source: Location) => { name: string; loc: Location }[],
): { name: string; loc: Location }[] | null {
  const a = openArchives.get(loc.fs);
  if (!a) return null;
  // The source's crumbs end with the archive file; that crumb opens its top.
  const out = outer(a.source).slice(0, -1);
  out.push({ name: a.name, loc: { fs: loc.fs, path: "/" } });
  let acc = "";
  for (const part of loc.path.split("/").filter(Boolean)) {
    acc = `${acc}/${part}`;
    out.push({ name: part, loc: { fs: loc.fs, path: acc } });
  }
  return out;
}
