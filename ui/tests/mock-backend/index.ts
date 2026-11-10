// An in-browser fake of the Tauri backend, installed with `mockIPC`.
// It keeps a file tree in memory, records every invoke, and can emit the
// events the UI listens to. Tests assert both the UI result and the exact
// command + arguments that produced it.

import { mockIPC } from "@tauri-apps/api/mocks";
import type {
  ActivityRecord,
  ConnectionView,
  Entry,
  Favorite,
  Insights,
  Location,
  OperationInfo,
  SyncPreview,
  Volume,
} from "../../src/lib/api";

// ---- the in-memory file system ----------------------------------------------

/** A file or folder entry with optional text content. */
export interface MemEntry extends Entry {
  content?: string;
}

/** One file system: path → entry. POSIX paths, case-sensitive. */
export type Tree = Map<string, MemEntry>;

export function file(name: string, content = "", extra: Partial<Entry> = {}): MemEntry {
  return {
    name,
    path: name,
    kind: "file",
    symlink: false,
    size: content.length,
    modifiedMs: 1_700_000_000_000,
    createdMs: 1_700_000_000_000,
    hidden: name.startsWith("."),
    readonly: false,
    mode: 0o644,
    content,
    ...extra,
  };
}

export function dir(name: string): MemEntry {
  return {
    name,
    path: name,
    kind: "dir",
    symlink: false,
    size: 0,
    modifiedMs: 1_700_000_000_000,
    createdMs: 1_700_000_000_000,
    hidden: false,
    readonly: false,
    mode: 0o755,
  };
}

const join = (dir: string, name: string) => (dir === "/" ? `/${name}` : `${dir}/${name}`);
const parentOf = (p: string) => (p === "/" ? null : p.slice(0, p.lastIndexOf("/")) || "/");
const baseName = (p: string) => p.slice(p.lastIndexOf("/") + 1);

/** Build a tree from a list of `path → content` strings ("d/" = a folder). */
export function tree(entries: Record<string, string>): Tree {
  const t: Tree = new Map([["/", dir("/")]]);
  for (const raw of Object.keys(entries)) {
    const isDir = raw.endsWith("/");
    const clean = isDir ? raw.slice(0, -1) : raw;
    const parts = clean.split("/").filter(Boolean);
    // Ensure every parent folder exists.
    let acc = "/";
    for (let i = 0; i < parts.length - 1; i++) {
      acc = join(acc, parts[i]);
      if (!t.has(acc)) t.set(acc, dir(parts[i]));
    }
    // The final entry.
    const name = parts[parts.length - 1];
    const full = join(acc, name);
    if (isDir) {
      if (!t.has(full)) t.set(full, dir(name));
    } else {
      t.set(full, file(name, entries[raw]));
    }
  }
  // Fix paths and names to match the map keys.
  for (const [path, e] of t) {
    e.path = path;
    e.name = baseName(path) || "/";
  }
  return t;
}

// ---- scenarios ---------------------------------------------------------------

export interface Scenario {
  /** Local tree (fs id "local"). */
  home: Location;
  /** Additional trees, by file system id. */
  trees: Record<string, Tree>;
  favorites: Favorite[];
  connections: ConnectionView[];
  volumes: Volume[];
  settings: { editor: string | null; diffTool: string | null };
  activity: ActivityRecord[];
}

export const defaultScenario = (): Scenario => ({
  home: { fs: "local", path: "/home/user" },
  trees: {
    local: tree({
      "/home/user/": "",
      "/home/user/readme.md": "# Readme\n",
      "/home/user/docs/": "",
      "/home/user/docs/notes.txt": "alpha\nbeta\n",
      "/home/user/docs/report.pdf": "pdf-bytes",
      "/home/user/empty.txt": "",
      "/home/user/.hidden": "h",
    }),
  },
  favorites: [],
  connections: [],
  volumes: [
    { name: "System", location: { fs: "local", path: "/" }, totalBytes: 1_000_000_000, availableBytes: 500_000_000, removable: false, wsl: false },
  ],
  settings: { editor: null, diffTool: null },
  activity: [],
});

// ---- the recorder ------------------------------------------------------------

export interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

export class Recorder {
  private log: Call[] = [];
  record(cmd: string, args: unknown) {
    this.log.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
  }
  calls(name: string): Record<string, unknown>[] {
    return this.log.filter((c) => c.cmd === name).map((c) => c.args);
  }
  lastCall(name: string): Record<string, unknown> | undefined {
    return this.calls(name).at(-1);
  }
}

// ---- the handle --------------------------------------------------------------

export interface MockHandle {
  calls(name: string): Record<string, unknown>[];
  lastCall(name: string): Record<string, unknown> | undefined;
  failNext(name: string, error: { kind: string; message: string }): void;
  setTree(fs: string, entries: Tree): void;
  removeFolder(path: string): void;
  completeAll(): void;
  setSettings(settings: { editor: string | null; diffTool: string | null }): void;
}

type Handler = (args: any, state: State) => unknown;

class State {
  scenario: Scenario;
  operations = new Map<string, OperationInfo>();
  failures = new Map<string, { kind: string; message: string }>();
  listeners = new Map<string, Set<(payload: any) => void>>();
  constructor(scenario: Scenario) {
    this.scenario = scenario;
  }
}

// ---- command handlers --------------------------------------------------------

const handlers: Record<string, Handler> = {
  file_systems: (_a, s) => [
    { id: "local", name: "Local" },
    ...Object.keys(s.scenario.trees).filter((f) => f !== "local").map((id) => ({ id, name: id })),
  ],
  home_location: (_a, s) => s.scenario.home,
  list_dir: (a, s) => {
    const loc: Location = a.location;
    const t = s.scenario.trees[loc.fs] ?? s.scenario.trees.local;
    const here = t.get(loc.path);
    if (!here || here.kind !== "dir") throw { kind: "notFound", message: `not found: ${loc.path}` };
    const out: Entry[] = [];
    for (const [p, e] of t) {
      if (parentOf(p) === loc.path) out.push({ ...e });
    }
    return out;
  },
  parent_location: (a) => {
    const loc: Location = a.location;
    const parent = parentOf(loc.path);
    return parent ? { fs: loc.fs, path: parent } : null;
  },
  available_space: (_a, s) => s.scenario.volumes[0]?.availableBytes ?? null,
  stat: (a, s) => {
    const loc: Location = a.location;
    const t = s.scenario.trees[loc.fs] ?? s.scenario.trees.local;
    const e = t.get(loc.path);
    if (!e) throw { kind: "notFound", message: `not found: ${loc.path}` };
    return { ...e };
  },
  read_text: (a, s) => {
    const loc: Location = a.location;
    const t = s.scenario.trees[loc.fs] ?? s.scenario.trees.local;
    const e = t.get(loc.path);
    if (!e) throw { kind: "notFound", message: `not found: ${loc.path}` };
    return { text: e.content ?? "", truncated: false, binary: false };
  },
  write_text: (a, s) => {
    const loc: Location = a.location;
    const t = s.scenario.trees[loc.fs] ?? s.scenario.trees.local;
    const e = t.get(loc.path);
    if (e) {
      e.content = a.text;
      e.size = a.text.length;
    }
    return null;
  },
  favorites: (_a, s) => s.scenario.favorites,
  add_favorite: (a, s) => {
    s.scenario.favorites.push(a.favorite);
    return s.scenario.favorites;
  },
  remove_favorite: (a, s) => {
    s.scenario.favorites = s.scenario.favorites.filter((f) => f.location.path !== a.location.path);
    return s.scenario.favorites;
  },
  reorder_favorites: (_a, s) => s.scenario.favorites,
  volumes: (_a, s) => s.scenario.volumes,
  connections: (_a, s) => s.scenario.connections,
  ssh_config_hosts: () => [],
  settings: (_a, s) => s.scenario.settings,
  save_settings: (a, s) => {
    s.scenario.settings = a.settings;
    return null;
  },
  operations: (_a, s) => [...s.operations.values()],
  activity: (_a, s) => s.scenario.activity,
  activity_insights: (_a, s) =>
    ({
      days: [],
      operations: 0,
      completed: 0,
      failed: 0,
      cancelled: 0,
      files: 0,
      bytes: 0,
      busyMs: 0,
      throughput: null,
      byKind: [],
      byPlace: [],
      largest: [],
      failures: [],
    }) as Insights,
  about_info: () => ({ version: "0.1.0", os: "test", arch: "test" }),
  rename_preview: (a) => {
    const { rules, inputs } = a;
    // Only enough for the unit tests: apply `replace` rules.
    return (inputs as any[]).map((input: any) => {
      let name = input.name;
      for (const r of rules as any[]) {
        if (r.type === "replace" && !r.regex) {
          name = name.replace(r.find, r.replace);
        }
      }
      return { name: input.name, newName: name, issue: null };
    });
  },
  rename_presets: () => [],
  diff_tools: () => [],
  sync_plan: (a): SyncPreview => ({
    id: "plan-1",
    mode: a.mode,
    comparison: { left: a.left, right: a.right, options: a.options, items: [], summary: { same: 0, leftOnly: 0, rightOnly: 0, leftNewer: 0, rightNewer: 0, different: 0, kindMismatch: 0 } },
    steps: [],
    hasHistory: false,
  }),
  live_sync_status: () => null,
  edit_sessions: () => [],
  enqueue_operation: (a, s) => {
    const id = crypto.randomUUID();
    const op = a.operation;
    const info: OperationInfo = {
      id,
      kind: op.type,
      summary: `${op.type} test`,
      status: "completed",
      progress: { filesTotal: 0, filesDone: 0, bytesTotal: 0, bytesDone: 0, skipped: 0, current: null },
      error: null,
      queuedMs: Date.now(),
      startedMs: Date.now(),
      finishedMs: Date.now(),
      places: [],
    };
    s.operations.set(id, info);
    return id;
  },
  cancel_operation: () => false,
  clear_finished_operations: () => null,
  clear_activity: () => null,
  open_url: () => null,
  open_path: () => null,
  cancel_compare: () => null,
  folder_size: (a, s) => {
    const loc: Location = a.location;
    const t = s.scenario.trees[loc.fs] ?? s.scenario.trees.local;
    const here = t.get(loc.path);
    if (!here || here.kind !== "dir") throw { kind: "invalid", message: `not a folder: ${loc.path}` };
    const prefix = loc.path === "/" ? "/" : `${loc.path}/`;
    const total = { bytes: 0, files: 0, folders: 0, skipped: 0 };
    for (const [p, e] of t) {
      if (p === loc.path || !p.startsWith(prefix)) continue;
      if (e.kind === "dir") total.folders++;
      else {
        total.files++;
        total.bytes += e.size;
      }
    }
    return total;
  },
  cancel_folder_size: () => null,
  run_sync: () => "op-1",
  forget_sync_history: () => null,
  live_sync_start: (a) => ({
    from: a.from,
    to: a.to,
    direction: a.direction,
    state: "watching",
    fromWatched: true,
    toWatched: true,
    lastSyncMs: null,
    itemsSynced: 0,
    syncs: 0,
    error: null,
    conflicts: 0,
  }),
  live_sync_stop: () => null,
  sync_favorites: () => [],
  save_sync_favorite: (a) => a.favorite,
  remove_sync_favorite: () => null,
  save_connection: (a) => ({ ...a.connection, connected: false, fsId: `conn:${a.connection.id}` }),
  remove_connection: () => null,
  reorder_connections: () => null,
  connect: (a) => ({ location: { fs: "local", path: "/" }, name: "test" }),
  disconnect: () => null,
  add_connection_favorite: (a, s) => {
    const c = s.scenario.connections.find((x) => x.id === a.id);
    return c ? c : { id: a.id, name: "c", endpoint: { type: "ftp", host: "h", port: 21, user: "", security: "none" }, initialPath: null, favorites: [], connected: false, fsId: `conn:${a.id}` };
  },
  remove_connection_favorite: () => null,
  open_file: () => null,
  stop_edit_session: () => false,
  upload_edit_session: () => null,
  preview_path: (a) => a.location.path,
  save_rename_preset: () => [],
  remove_rename_preset: () => [],
  compare_files: () => null,
};

// ---- install ------------------------------------------------------------------

export function installMockBackend(scenario: Scenario = defaultScenario()): MockHandle {
  const recorder = new Recorder();
  const state = new State(structuredClone(scenario));

  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as any;
    recorder.record(cmd, args);
    if (state.failures.has(cmd)) {
      const err = state.failures.get(cmd)!;
      state.failures.delete(cmd);
      throw err;
    }
    const handler = handlers[cmd];
    if (handler) return handler(args, state);
    // Unknown command → the real backend would reject it.
    throw { kind: "unsupported", message: `unsupported command ${cmd}` };
  });

  return {
    calls: (name) => recorder.calls(name),
    lastCall: (name) => recorder.lastCall(name),
    failNext: (name, error) => state.failures.set(name, error),
    setTree: (fs, entries) => {
      state.scenario.trees[fs] = entries;
    },
    removeFolder: (path) => {
      const t = state.scenario.trees.local;
      for (const p of [...t.keys()]) if (p.startsWith(path)) t.delete(p);
    },
    completeAll: () => {},
    setSettings: (settings) => {
      state.scenario.settings = settings;
    },
  };
}

// Silence "unused" for a type we re-export for future component tests.
export type { Location, Entry };
