import { SvelteMap, SvelteSet } from "svelte/reactivity";
import { api, errorKind, errorMessage, type Entry, type FolderSize, type Location } from "./api";
import { fsNames } from "./fs-names.svelte";
import { nameFilter } from "./quick-filter";

export type SortKey = "name" | "size" | "modified";

/** Search results shown in a pane in place of a folder listing. The pane's
 *  `location` stays the folder that was searched; every entry is a real item
 *  on that same file system, so its Location is `{ fs: base.fs, path }`. */
export interface PaneResults {
  /** The folder searched. */
  base: Location;
  /** What was searched for, for the title and the results bar. */
  label: string;
  /** The search that produced them, which may still be adding matches. */
  search: number;
}
export type ViewMode = "list" | "icons";

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
/** Ids of folder size calculations, unique across all panes. */
let nextSizeRequest = 1;

/** State of one file pane: where it is, what it shows, what is selected. */
export class PaneState {
  /** Called after any pane opens a folder (used for synchronized browsing). */
  static navigated: ((pane: PaneState, from: Location | null, to: Location) => void) | null = null;

  location = $state<Location | null>(null);
  entries = $state<Entry[]>([]);
  /** Paths of the marked entries. */
  selected = new SvelteSet<string>();
  /** Index into `visible` of the keyboard cursor. */
  cursor = $state(0);
  loading = $state(false);
  error = $state<string | null>(null);
  showHidden = $state(false);
  sortKey = $state<SortKey>("name");
  sortAsc = $state(true);
  view = $state<ViewMode>("list");
  /** Columns in the icon grid, reported by the view; used for ↑/↓. */
  columns = $state(1);
  /** Free bytes on this pane's volume, when known. */
  freeBytes = $state<number | null>(null);
  /** Folder sizes calculated for the current listing, by entry path. */
  sizes = new SvelteMap<string, FolderSize>();
  /** Folders whose size is being calculated, by entry path. */
  sizing = new SvelteSet<string>();
  /** Calculations in flight, by entry path. */
  private sizeRequests = new Map<string, number>();
  /** Changes whenever the listing does, so that late results are dropped. */
  private sizeEpoch = 0;
  private history = $state<Location[]>([]);
  private future = $state<Location[]>([]);
  /** Quick filter text; empty when no filter is set. */
  filter = $state("");
  /** Whether the filter bar is shown. */
  filterOpen = $state(false);
  /** Bumped to move focus into the filter bar, even when it is already open. */
  filterFocus = $state(0);
  /** Set while the pane shows search results instead of a folder. */
  results = $state<PaneResults | null>(null);

  /** Entries shown before the quick filter: hidden files left out, sorted. */
  listed = $derived.by(() => {
    // Search results hold what the search was asked to find, hidden or not.
    const list = this.showHidden || this.results ? this.entries : this.entries.filter((e) => !e.hidden);
    const dir = this.sortAsc ? 1 : -1;
    const name = (e: Entry) => (this.results ? this.displayName(e) : e.name);
    return [...list].sort((a, b) => {
      // Folders always first.
      if ((a.kind === "dir") !== (b.kind === "dir")) return a.kind === "dir" ? -1 : 1;
      let c = 0;
      if (this.sortKey === "size") c = this.sizeOf(a) - this.sizeOf(b);
      else if (this.sortKey === "modified") c = (a.modifiedMs ?? 0) - (b.modifiedMs ?? 0);
      return (c || collator.compare(name(a), name(b))) * dir;
    });
  });

  /** The entries shown: `listed` narrowed by the quick filter. */
  visible = $derived.by(() => {
    const keep = nameFilter(this.filter);
    return keep ? this.listed.filter((e) => keep(e.name)) : this.listed;
  });

  get current(): Entry | undefined {
    return this.visible[this.cursor];
  }

  /** Marked entries, or the entry under the cursor when nothing is marked. */
  get targets(): Entry[] {
    if (this.selected.size > 0) return this.visible.filter((e) => this.selected.has(e.path));
    return this.current ? [this.current] : [];
  }

  async open(loc: Location, opts: { record?: boolean; focusName?: string; entries?: Entry[] } = {}) {
    this.loading = true;
    try {
      const entries = opts.entries ?? (await api.listDir(loc));
      const from = this.location;
      if (opts.record !== false && this.location) {
        this.history.push(this.location);
        this.future = [];
      }
      this.location = loc;
      this.results = null;
      this.entries = entries;
      this.forgetSizes();
      this.filter = "";
      this.filterOpen = false;
      this.selected.clear();
      this.error = null;
      const idx = opts.focusName ? this.visible.findIndex((e) => e.name === opts.focusName) : 0;
      this.cursor = Math.max(0, idx);
      this.updateSpace();
      PaneState.navigated?.(this, from, loc);
    } catch (e) {
      this.error = errorMessage(e);
    } finally {
      this.loading = false;
    }
  }

  /** Show search results in this pane: `entries` found below `base` by
   *  search `search`. Back, Backspace or opening a folder leaves them. */
  showResults(base: Location, label: string, entries: Entry[], search: number) {
    if (this.location && !this.results) {
      this.history.push(this.location);
      this.future = [];
    }
    // A copy: `base` may belong to another reactive object (the search).
    base = { fs: base.fs, path: base.path };
    this.location = base;
    this.results = { base, label, search };
    this.entries = dedupe(entries);
    this.forgetSizes();
    this.filter = "";
    this.filterOpen = false;
    this.selected.clear();
    this.error = null;
    this.cursor = 0;
    this.loading = false;
    this.updateSpace();
  }

  /** Matches that search `search` found later, when this pane shows its results. */
  addResults(search: number, entries: Entry[]) {
    if (this.results?.search !== search || !entries.length) return;
    const keep = this.current?.path;
    this.entries = dedupe([...this.entries, ...entries]);
    const idx = keep ? this.visible.findIndex((e) => e.path === keep) : -1;
    if (idx >= 0) this.cursor = idx;
  }

  /** Go back to browsing the folder that was searched. */
  async leaveResults() {
    const r = this.results;
    if (!r) return;
    await this.open(r.base, { record: false });
  }

  /** The name to show for `e`: in search results, its path below the folder searched. */
  displayName(e: Entry): string {
    const base = this.results?.base.path;
    if (!base || !e.path.startsWith(base)) return e.name;
    const rel = e.path.slice(base.length).replace(/^[\\/]+/, "");
    return rel || e.name;
  }

  /** Open the folder holding `entry` with it under the cursor (from search results). */
  async showInFolder(entry: Entry) {
    if (!this.location) return;
    const parent = await api.parentLocation({ fs: this.location.fs, path: entry.path }).catch(() => null);
    if (parent) await this.open(parent, { focusName: entry.name });
  }

  /** Size used for sorting: a folder's calculated size when it is known. */
  private sizeOf(e: Entry): number {
    return e.kind === "dir" ? (this.sizes.get(e.path)?.bytes ?? e.size) : e.size;
  }

  /** Work out the size of these folders. The default is every folder in the
   *  listing except links to folders, whose trees live elsewhere; a link is
   *  only sized when asked for by name. Entries that are not folders, or
   *  already known, are skipped. */
  calculateSizes(folders: Entry[] = this.entries.filter((e) => e.kind === "dir" && !e.symlink)) {
    for (const e of folders) this.startSize(e);
  }

  private startSize(entry: Entry) {
    const loc = this.location;
    if (!loc || entry.kind !== "dir" || this.sizes.has(entry.path) || this.sizing.has(entry.path)) return;
    const epoch = this.sizeEpoch;
    const request = nextSizeRequest++;
    this.sizeRequests.set(entry.path, request);
    this.sizing.add(entry.path);
    api.folderSize({ fs: loc.fs, path: entry.path }, request).then(
      (size) => {
        if (epoch !== this.sizeEpoch) return;
        this.sizeRequests.delete(entry.path);
        this.sizing.delete(entry.path);
        // Sorted by size, the new size can move rows: stay on the same entry.
        const keep = this.current?.path;
        this.sizes.set(entry.path, size);
        const idx = keep ? this.visible.findIndex((e) => e.path === keep) : -1;
        if (idx >= 0) this.cursor = idx;
      },
      () => {
        // Cancelled, or the folder cannot be read: there is no size to show.
        if (epoch !== this.sizeEpoch) return;
        this.sizeRequests.delete(entry.path);
        this.sizing.delete(entry.path);
      },
    );
  }

  /** Discard the folder sizes of this listing and stop calculating them. */
  forgetSizes() {
    this.sizeEpoch++;
    for (const request of this.sizeRequests.values()) api.cancelFolderSize(request).catch(() => {});
    this.sizeRequests.clear();
    this.sizing.clear();
    this.sizes.clear();
  }

  private updateSpace() {
    const loc = this.location;
    if (!loc) return;
    api.availableSpace(loc).then((n) => {
      if (this.location === loc) this.freeBytes = n;
    }, () => (this.freeBytes = null));
  }

  /** A short name for tab titles. */
  get title(): string {
    if (this.results) return `Search: ${this.results.label}`;
    const path = this.location?.path ?? "";
    const last = path.split(/[\\/]/).filter(Boolean).pop();
    return last ?? (this.location && fsNames[this.location.fs]) ?? path;
  }

  /** Mark (or unmark) every visible entry whose name matches. */
  selectWhere(match: (name: string) => boolean, select: boolean) {
    for (const e of this.visible) {
      if (!match(e.name)) continue;
      if (select) this.selected.add(e.path);
      else this.selected.delete(e.path);
    }
  }

  /** Reload, keeping the cursor on the same item where possible. */
  async refresh() {
    if (this.results) return this.refreshResults();
    const here = this.location;
    if (!here) return;
    const keep = this.current?.name;
    const marked = new Set(this.selected);
    try {
      const entries = await api.listDir(here);
      if (this.location !== here) return; // the user went elsewhere meanwhile
      this.entries = entries;
      this.forgetSizes();
      this.error = null;
    } catch (e) {
      if (this.location !== here) return;
      // The folder was deleted or renamed elsewhere: show the nearest
      // folder above it that still exists instead of an error.
      if (errorKind(e) === "notFound" && (await this.openNearestParent(here))) return;
      if (this.location === here) this.error = errorMessage(e);
      return;
    }
    this.selected.clear();
    for (const e of this.entries) if (marked.has(e.path)) this.selected.add(e.path);
    this.unmarkFiltered();
    const idx = keep ? this.visible.findIndex((e) => e.name === keep) : -1;
    this.cursor = idx >= 0 ? idx : Math.min(this.cursor, Math.max(0, this.visible.length - 1));
    this.updateSpace();
  }

  /** Check the results again: items deleted or moved away drop out, the rest
   *  show their new size and date. */
  private async refreshResults() {
    const r = this.results;
    if (!r) return;
    const keep = this.current?.path;
    const checked = await Promise.all(
      this.entries.map((e) =>
        api.stat({ fs: r.base.fs, path: e.path }).then(
          (fresh) => fresh,
          (err) => (errorKind(err) === "notFound" ? null : e),
        ),
      ),
    );
    if (this.results?.search !== r.search || this.results.base.path !== r.base.path) return; // the user went elsewhere meanwhile
    this.entries = checked.filter((e): e is Entry => e !== null);
    const paths = new Set(this.entries.map((e) => e.path));
    for (const p of [...this.selected]) if (!paths.has(p)) this.selected.delete(p);
    const idx = keep ? this.visible.findIndex((e) => e.path === keep) : -1;
    this.cursor = idx >= 0 ? idx : Math.min(this.cursor, Math.max(0, this.visible.length - 1));
    this.updateSpace();
  }

  /** Narrow the list to the names matching `text` (see nameFilter). */
  setFilter(text: string) {
    const keep = this.current?.path;
    this.filter = text;
    this.unmarkFiltered();
    // Stay on the same entry when it is still shown, else go to the first.
    const idx = keep ? this.visible.findIndex((e) => e.path === keep) : -1;
    this.cursor = Math.max(0, idx);
  }

  /** Clear the filter and hide its bar. */
  closeFilter() {
    this.setFilter("");
    this.filterOpen = false;
  }

  openFilter() {
    this.filterOpen = true;
    this.filterFocus++;
  }

  /** Marks never stay on entries the filter hides, so operations only act on what is shown. */
  private unmarkFiltered() {
    const keep = nameFilter(this.filter);
    if (!keep) return;
    for (const e of this.entries) if (!keep(e.name)) this.selected.delete(e.path);
  }

  /** Open the closest existing folder above `from`, unless the user moves
   *  elsewhere in the meantime. */
  private async openNearestParent(from: Location): Promise<boolean> {
    let loc = from;
    while (true) {
      const parent = await api.parentLocation(loc).catch(() => null);
      if (!parent || parent.path === loc.path) return false;
      let entries: Entry[];
      try {
        entries = await api.listDir(parent);
      } catch (e) {
        if (errorKind(e) !== "notFound") return false;
        loc = parent;
        continue;
      }
      if (this.location !== from) return true;
      await this.open(parent, { record: false, entries });
      return this.error === null;
    }
  }

  async up() {
    if (this.results) return this.leaveResults();
    if (!this.location) return;
    const parent = await api.parentLocation(this.location);
    if (!parent || parent.path === this.location.path) return;
    const name = this.location.path.split(/[\\/]/).filter(Boolean).pop();
    await this.open(parent, { focusName: name });
  }

  async back() {
    const prev = this.history.pop();
    if (!prev || !this.location) return;
    this.future.push(this.location);
    await this.open(prev, { record: false });
  }

  async forward() {
    const next = this.future.pop();
    if (!next || !this.location) return;
    this.history.push(this.location);
    await this.open(next, { record: false });
  }

  get canBack() {
    return this.history.length > 0;
  }

  get canForward() {
    return this.future.length > 0;
  }

  /** Open a folder in place, or a file with its default application.
   *  Remote files are downloaded and uploaded again when the app saves them. */
  async activate(entry: Entry) {
    if (!this.location) return;
    const loc = { fs: this.location.fs, path: entry.path };
    if (entry.kind === "dir" && !entry.name.toLowerCase().endsWith(".app")) await this.open(loc);
    else await api.openFile(loc).catch((e) => (this.error = errorMessage(e)));
  }

  /** A new tab state at the same place with the same view settings. */
  /** A new pane with the same view settings, showing `at` (default: this folder). */
  clone(at: Location | null = this.location): PaneState {
    const p = new PaneState();
    p.view = this.view;
    p.showHidden = this.showHidden;
    p.sortKey = this.sortKey;
    p.sortAsc = this.sortAsc;
    if (at) p.open(at);
    return p;
  }

  moveCursor(delta: number) {
    const n = this.visible.length;
    if (n === 0) return;
    this.cursor = Math.min(n - 1, Math.max(0, this.cursor + delta));
  }

  toggle(entry: Entry) {
    if (this.selected.has(entry.path)) this.selected.delete(entry.path);
    else this.selected.add(entry.path);
  }

  setSort(key: SortKey) {
    if (this.sortKey === key) this.sortAsc = !this.sortAsc;
    else {
      this.sortKey = key;
      this.sortAsc = true;
    }
  }
}

/** `entries` without repeated paths, keeping the first of each. */
function dedupe(entries: Entry[]): Entry[] {
  const seen = new Set<string>();
  return entries.filter((e) => !seen.has(e.path) && (seen.add(e.path), true));
}

/** The tabs of one side of the window. */
export class PaneGroup {
  tabs = $state<PaneState[]>([]);
  index = $state(0);

  constructor(first: PaneState) {
    this.tabs = [first];
  }

  get current(): PaneState {
    return this.tabs[Math.min(this.index, this.tabs.length - 1)];
  }

  newTab(state?: PaneState) {
    this.tabs.push(state ?? this.current.clone());
    this.index = this.tabs.length - 1;
  }

  closeTab(i = this.index) {
    if (this.tabs.length <= 1) return;
    this.tabs[i].forgetSizes();
    this.tabs.splice(i, 1);
    if (this.index >= this.tabs.length || this.index > i) this.index = Math.max(0, this.index - 1);
  }

  cycle(delta: number) {
    const n = this.tabs.length;
    this.index = (this.index + delta + n) % n;
  }
}
