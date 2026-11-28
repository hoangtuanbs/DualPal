import { api, errorKind, errorMessage, type Location, type SearchCriteria, type SearchHit } from "./api";

/** Server files larger than this are not searched for text (the core's REMOTE_CONTENT_LIMIT). */
export const REMOTE_CONTENT_LIMIT = 10_000_000;

export type DatePreset = "any" | "today" | "7d" | "30d" | "newer" | "older";
export type KindChoice = "any" | "files" | "folders";

/** What the Find dialog's fields hold. */
export interface FindForm {
  name: string;
  content: string;
  nameRegex: boolean;
  contentRegex: boolean;
  ignoreCase: boolean;
  includeBinary: boolean;
  includeHidden: boolean;
  kind: KindChoice;
  /** Sizes as typed, e.g. "10 MB"; empty for no limit. */
  minSize: string;
  maxSize: string;
  date: DatePreset;
  /** `yyyy-mm-dd` for the "newer" and "older" presets. */
  day: string;
}

export function defaultForm(includeHidden = false): FindForm {
  return {
    name: "",
    content: "",
    nameRegex: false,
    contentRegex: false,
    ignoreCase: true,
    includeBinary: false,
    includeHidden,
    kind: "any",
    minSize: "",
    maxSize: "",
    date: "any",
    day: "",
  };
}

const UNITS: Record<string, number> = { "": 1, b: 1, k: 1e3, kb: 1e3, m: 1e6, mb: 1e6, g: 1e9, gb: 1e9, t: 1e12, tb: 1e12 };

/** "10 MB" → 10 000 000 (units as in the size column: 1 KB = 1000 bytes).
 *  Empty → null; anything else that is not a size → undefined. */
export function parseSize(text: string): number | null | undefined {
  const t = text.trim().toLowerCase().replace(/,/g, ".");
  if (!t) return null;
  const m = /^(\d+(?:\.\d+)?)\s*([a-z]*)$/.exec(t);
  if (!m || !(m[2] in UNITS)) return undefined;
  return Math.round(Number(m[1]) * UNITS[m[2]]);
}

/** The modified-date limits of a preset, in ms since the epoch. */
export function dateRange(preset: DatePreset, day: string, now = new Date()): { newerThanMs?: number; olderThanMs?: number } | undefined {
  const midnight = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const DAY = 86_400_000;
  switch (preset) {
    case "any":
      return {};
    case "today":
      return { newerThanMs: midnight };
    case "7d":
      return { newerThanMs: midnight - 6 * DAY };
    case "30d":
      return { newerThanMs: midnight - 29 * DAY };
    case "newer":
    case "older": {
      const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
      if (!m) return undefined;
      const start = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).getTime();
      // "Newer than" a day includes it; "older than" a day is before it starts.
      return preset === "newer" ? { newerThanMs: start } : { olderThanMs: start };
    }
  }
}

/** The form as `find_files` arguments, or a message saying what is wrong. */
export function toCriteria(form: FindForm, now = new Date()): { query: string; criteria: SearchCriteria } | { problem: string } {
  const minSize = parseSize(form.minSize);
  const maxSize = parseSize(form.maxSize);
  if (minSize === undefined || maxSize === undefined) return { problem: "Sizes look like 500 KB or 2 MB." };
  const dates = dateRange(form.date, form.day, now);
  if (!dates) return { problem: "Choose a day." };
  return {
    query: form.name.trim(),
    criteria: {
      nameRegex: form.nameRegex,
      content: form.content,
      contentRegex: form.contentRegex,
      ignoreCase: form.ignoreCase,
      includeBinary: form.includeBinary,
      includeHidden: form.includeHidden,
      kind: form.kind,
      minSize,
      maxSize,
      newerThanMs: dates.newerThanMs ?? null,
      olderThanMs: dates.olderThanMs ?? null,
    },
  };
}

/** Whether the form asks for anything (kind and hidden alone do not). */
export function hasCriteria(form: FindForm): boolean {
  return !!(form.name.trim() || form.content || form.minSize.trim() || form.maxSize.trim() || form.date !== "any");
}

/** Ids of searches, unique across all sessions. */
let nextRequest = 1;

/** One search and its results. It runs in the background: the dialog that
 *  started it can close, and panes showing its results keep receiving
 *  matches until it ends. */
export class FindSession {
  form = $state<FindForm>(defaultForm());
  /** The folder searched. */
  base = $state<Location | null>(null);
  hits = $state<SearchHit[]>([]);
  folders = $state(0);
  files = $state(0);
  searching = $state(false);
  truncated = $state(false);
  skipped = $state(0);
  filesSkipped = $state(0);
  tooLarge = $state(0);
  binary = $state(0);
  error = $state("");
  /** A search has run (or is running) for the current form. */
  ran = $state(false);
  /** Request id of the latest search; events and answers for older ones are ignored. */
  id = $state(0);
  /** Matches as they arrive, for panes showing this search's results. */
  onhits: ((id: number, hits: SearchHit[]) => void) | null = null;
  private running = 0;
  private listening: Promise<() => void> | null = null;

  private listen() {
    this.listening ??= api.onFindProgress((p) => {
      if (p.request !== this.id || !this.searching) return;
      if (p.hits.length) {
        this.hits = [...this.hits, ...p.hits];
        this.onhits?.(p.request, p.hits);
      }
      this.folders = p.folders;
      this.files = p.files ?? this.files;
    });
    return this.listening;
  }

  private reset() {
    this.error = "";
    this.hits = [];
    this.folders = 0;
    this.files = 0;
    this.truncated = false;
    this.skipped = 0;
    this.filesSkipped = 0;
    this.tooLarge = 0;
    this.binary = 0;
  }

  /** Clear the results and stop any search. */
  clear() {
    this.stop();
    this.id = nextRequest++;
    this.reset();
    this.searching = false;
    this.ran = false;
  }

  /** Search `location` for what the form asks, replacing any earlier search. */
  async start(location: Location) {
    this.stop();
    const id = (this.id = nextRequest++);
    this.base = location;
    this.reset();
    if (!hasCriteria(this.form)) {
      this.searching = false;
      this.ran = false;
      return;
    }
    const args = toCriteria(this.form);
    this.ran = true;
    if ("problem" in args) {
      this.searching = false;
      this.error = args.problem;
      return;
    }
    this.searching = true;
    this.running = id;
    try {
      await this.listen().catch(() => {});
      if (id !== this.id) return;
      const found = await api.findFiles(location, args.query, id, args.criteria);
      if (id !== this.id) return;
      // Matches the events missed (the final answer has them all).
      const fresh = found.hits.slice(this.hits.length);
      this.hits = found.hits;
      if (fresh.length) this.onhits?.(id, fresh);
      this.truncated = found.truncated;
      this.skipped = found.skipped;
      this.files = found.filesRead ?? this.files;
      this.filesSkipped = found.filesSkipped ?? 0;
      this.tooLarge = found.tooLarge ?? 0;
      this.binary = found.binary ?? 0;
    } catch (e) {
      if (id !== this.id || errorKind(e) === "cancelled") return;
      this.error = errorMessage(e);
    } finally {
      if (id === this.id) {
        this.searching = false;
        this.running = 0;
      }
    }
  }

  stop() {
    if (this.running) void api.cancelFindFiles(this.running).catch(() => {});
    this.running = 0;
  }

  /** Stop listening for progress (the session is no longer used). */
  dispose() {
    this.stop();
    this.listening?.then((off) => off()).catch(() => {});
    this.listening = null;
  }
}
