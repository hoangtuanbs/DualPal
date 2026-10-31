import { SvelteSet } from "svelte/reactivity";
import { api, errorMessage, type Entry, type Location } from "./api";

export type SortKey = "name" | "size" | "modified";

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** State of one file pane: where it is, what it shows, what is selected. */
export class PaneState {
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
  private history = $state<Location[]>([]);
  private future = $state<Location[]>([]);

  visible = $derived.by(() => {
    const list = this.showHidden ? this.entries : this.entries.filter((e) => !e.hidden);
    const dir = this.sortAsc ? 1 : -1;
    return [...list].sort((a, b) => {
      // Folders always first.
      if ((a.kind === "dir") !== (b.kind === "dir")) return a.kind === "dir" ? -1 : 1;
      let c = 0;
      if (this.sortKey === "size") c = a.size - b.size;
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

  async open(loc: Location, opts: { record?: boolean; focusName?: string } = {}) {
    this.loading = true;
    try {
      const entries = await api.listDir(loc);
      if (opts.record !== false && this.location) {
        this.history.push(this.location);
        this.future = [];
      }
      this.location = loc;
      this.entries = entries;
      this.selected.clear();
      this.error = null;
      const idx = opts.focusName ? this.visible.findIndex((e) => e.name === opts.focusName) : 0;
      this.cursor = Math.max(0, idx);
    } catch (e) {
      this.error = errorMessage(e);
    } finally {
      this.loading = false;
    }
  }

  /** Reload, keeping the cursor on the same item where possible. */
  async refresh() {
    if (!this.location) return;
    const keep = this.current?.name;
    const marked = new Set(this.selected);
    try {
      this.entries = await api.listDir(this.location);
      this.error = null;
    } catch (e) {
      this.error = errorMessage(e);
      return;
    }
    this.selected.clear();
    for (const e of this.entries) if (marked.has(e.path)) this.selected.add(e.path);
    const idx = keep ? this.visible.findIndex((e) => e.name === keep) : -1;
    this.cursor = idx >= 0 ? idx : Math.min(this.cursor, Math.max(0, this.visible.length - 1));
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

  async activate(entry: Entry) {
    if (entry.kind === "dir" && this.location) {
      await this.open({ fs: this.location.fs, path: entry.path });
    }
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
