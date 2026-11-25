/** The last session: open tabs with their folders and view settings, and the
 *  window layout, saved while the app runs and restored at the next launch. */

import { api, type Entry, type Location } from "./api";
import { PaneGroup, PaneState, type SortKey, type ViewMode } from "./pane-state.svelte";
import { clampSplit } from "./split";

export const SESSION_VERSION = 1;
/** More tabs than this on one side are not restored. */
export const MAX_TABS = 50;
/** Changes are written this long after the last one, so a burst of steps is one save. */
export const SESSION_SAVE_DELAY_MS = 400;

export interface SessionTab {
  location: Location;
  view: ViewMode;
  showHidden: boolean;
  sortKey: SortKey;
  sortAsc: boolean;
}

export interface SessionSide {
  tabs: SessionTab[];
  /** The tab shown on this side. */
  index: number;
}

export interface Session {
  version: typeof SESSION_VERSION;
  /** The left pane's share of the width. */
  split: number | null;
  showInspector: boolean | null;
  /** The pane that had the focus: 0 left, 1 right. */
  activeSide: 0 | 1;
  sides: [SessionSide, SessionSide];
}

/** The layout around the panes that a session keeps. */
export interface SessionLayout {
  split: number;
  showInspector: boolean;
  activeSide: number;
}

const VIEWS: readonly ViewMode[] = ["list", "icons"];
const SORT_KEYS: readonly SortKey[] = ["name", "size", "modified"];

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function parseLocation(v: unknown): Location | null {
  if (!isObject(v) || typeof v.fs !== "string" || typeof v.path !== "string") return null;
  if (!v.fs || !v.path) return null;
  return { fs: v.fs, path: v.path };
}

function parseTab(v: unknown): SessionTab | null {
  if (!isObject(v)) return null;
  const location = parseLocation(v.location);
  if (!location) return null;
  return {
    location,
    view: VIEWS.includes(v.view as ViewMode) ? (v.view as ViewMode) : "list",
    showHidden: v.showHidden === true,
    sortKey: SORT_KEYS.includes(v.sortKey as SortKey) ? (v.sortKey as SortKey) : "name",
    sortAsc: v.sortAsc !== false,
  };
}

function parseSide(v: unknown): SessionSide {
  if (!isObject(v) || !Array.isArray(v.tabs)) return { tabs: [], index: 0 };
  const tabs: SessionTab[] = [];
  let index = 0;
  const saved = Number.isInteger(v.index) ? (v.index as number) : 0;
  v.tabs.forEach((t, i) => {
    const tab = parseTab(t);
    if (!tab || tabs.length >= MAX_TABS) return;
    // The shown tab, or the nearest kept one before it.
    if (i <= saved) index = tabs.length;
    tabs.push(tab);
  });
  return { tabs, index };
}

/** A saved session checked field by field. Anything unusable falls back to
 *  its default; a value that is not a session at all gives null. */
export function parseSession(raw: unknown): Session | null {
  if (!isObject(raw) || raw.version !== SESSION_VERSION) return null;
  const sides = Array.isArray(raw.sides) ? raw.sides : [];
  const split = typeof raw.split === "number" && Number.isFinite(raw.split) ? clampSplit(raw.split) : null;
  return {
    version: SESSION_VERSION,
    split,
    showInspector: typeof raw.showInspector === "boolean" ? raw.showInspector : null,
    activeSide: raw.activeSide === 1 ? 1 : 0,
    sides: [parseSide(sides[0]), parseSide(sides[1])],
  };
}

function captureSide(group: PaneGroup): SessionSide {
  const tabs: SessionTab[] = [];
  let index = 0;
  group.tabs.forEach((p, i) => {
    if (!p.location || tabs.length >= MAX_TABS) return;
    if (i <= group.index) index = tabs.length;
    tabs.push({
      location: { fs: p.location.fs, path: p.location.path },
      view: p.view,
      showHidden: p.showHidden,
      sortKey: p.sortKey,
      sortAsc: p.sortAsc,
    });
  });
  return { tabs, index };
}

/** The session as it stands now. Reads every value it saves, so inside an
 *  `$effect` it reruns whenever one of them changes. */
export function captureSession(groups: readonly PaneGroup[], layout: SessionLayout): Session {
  return {
    version: SESSION_VERSION,
    split: Math.round(layout.split * 1000) / 1000,
    showInspector: layout.showInspector,
    activeSide: layout.activeSide === 1 ? 1 : 0,
    sides: [captureSide(groups[0]), captureSide(groups[1])],
  };
}

/** Open `loc` in `pane`. A local folder that is gone opens the nearest
 *  folder above it that still exists. False when nothing could be opened. */
async function openSaved(pane: PaneState, loc: Location): Promise<boolean> {
  await pane.open(loc, { record: false });
  if (pane.location) return true;
  if (loc.fs !== "local") return false; // a server that is not connected yet
  let at = loc;
  for (let depth = 0; depth < 64; depth++) {
    const parent = await api.parentLocation(at).catch(() => null);
    if (!parent || parent.path === at.path) return false;
    let entries: Entry[] | null = null;
    try {
      entries = await api.listDir(parent);
    } catch {
      at = parent;
      continue;
    }
    await pane.open(parent, { record: false, entries });
    return pane.location !== null;
  }
  return false;
}

/** Put the saved tabs back on one side. Tabs that cannot be opened (a server
 *  that is not connected, a drive that is gone) are left out; a side left
 *  without tabs shows `fallback`. */
async function restoreSide(group: PaneGroup, side: SessionSide | undefined, fallback: Location): Promise<void> {
  const saved = side?.tabs ?? [];
  const panes = saved.map((t) => {
    const p = new PaneState();
    p.view = t.view;
    p.showHidden = t.showHidden;
    p.sortKey = t.sortKey;
    p.sortAsc = t.sortAsc;
    return p;
  });
  const opened = await Promise.all(panes.map((p, i) => openSaved(p, saved[i].location)));
  const kept: PaneState[] = [];
  let index = 0;
  panes.forEach((p, i) => {
    if (!opened[i]) return p.forgetSizes();
    if (i <= (side?.index ?? 0)) index = kept.length;
    kept.push(p);
  });
  if (kept.length === 0) {
    const p = new PaneState();
    if (saved[0]) {
      p.view = saved[0].view;
      p.showHidden = saved[0].showHidden;
      p.sortKey = saved[0].sortKey;
      p.sortAsc = saved[0].sortAsc;
    }
    await p.open(fallback, { record: false });
    kept.push(p);
    index = 0;
  }
  for (const old of group.tabs) old.forgetSizes();
  group.tabs = kept;
  group.index = index;
}

/** Restore both sides from `session`, or open `fallback` on both without one. */
export async function restoreSides(groups: readonly PaneGroup[], session: Session | null, fallback: Location): Promise<void> {
  await Promise.all(groups.map((g, i) => restoreSide(g, session?.sides[i], fallback)));
}

/** Saves sessions a short while after the last change, skipping ones that
 *  equal what was saved last. A failed save is retried on the next change. */
export function sessionSaver(save: (s: Session) => Promise<unknown>, delayMs = SESSION_SAVE_DELAY_MS) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let pending: Session | null = null;
  let last = "";

  function flush(): Promise<void> {
    clearTimeout(timer);
    timer = undefined;
    const s = pending;
    pending = null;
    if (!s) return Promise.resolve();
    const json = JSON.stringify(s);
    if (json === last) return Promise.resolve();
    last = json;
    return save(s).then(
      () => {},
      () => {
        if (last === json) last = "";
      },
    );
  }

  return {
    schedule(s: Session) {
      pending = s;
      clearTimeout(timer);
      timer = setTimeout(flush, delayMs);
    },
    flush,
    cancel() {
      clearTimeout(timer);
      timer = undefined;
      pending = null;
    },
  };
}
