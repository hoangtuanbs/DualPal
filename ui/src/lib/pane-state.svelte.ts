import { SvelteMap, SvelteSet } from "svelte/reactivity";
import { api, errorKind, errorMessage, type Entry, type FolderSize, type Location } from "./api";
import { fsNames } from "./fs-names.svelte";

export type SortKey = "name" | "size" | "modified";
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

  visible = $derived.by(() => {
    const list = this.showHidden ? this.entries : this.entries.filter((e) => !e.hidden);
    const dir = this.sortAsc ? 1 : -1;
    return [...list].sort((a, b) => {
      // Folders always first.
      if ((a.kind === "dir") !== (b.kind === "dir")) return a.kind === "dir" ? -1 : 1;
      let c = 0;
      if (this.sortKey === "size") c = this.sizeOf(a) - this.sizeOf(b);
      else if (this.sortKey === "modified") c = (a.modifiedMs ?? 0) - (b.modifiedMs ?? 0);
      return (c || collator.compare(a.name, b.name)) * dir;
    });
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
      this.entries = entries;
      this.forgetSizes();
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

  /** Size used for sorting: a folder's calculated size when it is known. */
  private sizeOf(e: Entry): number {
    return e.kind === "dir" ? (this.sizes.get(e.path)?.bytes ?? e.size) : e.size;
  }

  /** Work out the size of these folders (default: every folder in the listing).
   *  Entries that are not folders, or already known, are skipped. */
  calculateSizes(folders: Entry[] = this.entries.filter((e) => e.kind === "dir")) {
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
        this.sizes.set(entry.path, size);
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
    const idx = keep ? this.visible.findIndex((e) => e.name === keep) : -1;
    this.cursor = idx >= 0 ? idx : Math.min(this.cursor, Math.max(0, this.visible.length - 1));
    this.updateSpace();
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
