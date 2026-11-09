// A self-contained in-browser fake of the Tauri backend, injected by Playwright
// via `page.addInitScript`. It sets up `window.__TAURI_INTERNALS__` so the
// bundled `@tauri-apps/api` talks to this in-memory implementation.
//
// `addInitScript` serialises the function, so everything must be self-contained
// (no imports, no references to module scope).

export interface BrowserScenario {
  home: string;
  files: Record<string, string>; // path -> content ("d/" = folder)
}

export function installBrowserMock(scenario: BrowserScenario) {
  type Location = { fs: string; path: string };
  type Entry = {
    name: string; path: string; kind: "file" | "dir" | "other"; symlink: boolean;
    size: number; modifiedMs: number | null; createdMs: number | null;
    hidden: boolean; readonly: boolean; mode: number | null;
  };

  const files: Record<string, string> = scenario.files;
  const paths = Object.keys(files).sort();
  const isDir = (p: string) => p.endsWith("/");
  const clean = (p: string) => (isDir(p) ? p.slice(0, -1) : p);
  const parent = (p: string) => (p === "/" ? null : p.slice(0, p.lastIndexOf("/")) || "/");
  const base = (p: string) => p.slice(p.lastIndexOf("/") + 1);
  const join = (d: string, n: string) => (d === "/" ? `/${n}` : `${d}/${n}`);

  // Ensure parent folders exist.
  const tree = new Map<string, string>([["/", ""]]);
  for (const p of paths) {
    const parts = clean(p).split("/").filter(Boolean);
    let acc = "/";
    for (let i = 0; i < parts.length - 1; i++) {
      acc = join(acc, parts[i]);
      if (!tree.has(acc)) tree.set(acc, "");
    }
    tree.set(clean(p), files[p]);
  }

  const entry = (p: string): Entry => {
    const content = tree.get(p);
    const dir = content === "" && isDir(p) || (content !== undefined && (p === "/" || [...tree.keys()].some((k) => parent(k) === p)));
    return {
      name: p === "/" ? "/" : base(p),
      path: p,
      kind: dir ? "dir" : "file",
      symlink: false,
      size: content?.length ?? 0,
      modifiedMs: 1_700_000_000_000,
      createdMs: 1_700_000_000_000,
      hidden: p !== "/" && base(p).startsWith("."),
      readonly: false,
      mode: dir ? 0o755 : 0o644,
    };
  };

  const listDir = (loc: Location): Entry[] => {
    const here = tree.get(loc.path);
    if (here === undefined) throw { kind: "notFound", message: `not found: ${loc.path}` };
    return [...tree.keys()].filter((k) => k !== "/" && parent(k) === loc.path).map(entry);
  };

  let operations: unknown[] = [];
  const callbacks = new Map<number, (payload: unknown) => void>();
  let nextCallback = 1;
  const listeners = new Map<string, number[]>();

  const handlers: Record<string, (args: any) => unknown> = {
    file_systems: () => [{ id: "local", name: "Local" }],
    home_location: () => ({ fs: "local", path: scenario.home }),
    list_dir: (a) => listDir(a.location),
    parent_location: (a) => (parent(a.location.path) ? { fs: "local", path: parent(a.location.path)! } : null),
    stat: (a) => {
      const e = entry(a.location.path);
      if (tree.get(a.location.path) === undefined) throw { kind: "notFound", message: `not found: ${a.location.path}` };
      return e;
    },
    read_text: (a) => ({ text: tree.get(a.location.path) ?? "", truncated: false, binary: false }),
    write_text: (a) => {
      const p = a.location.path;
      tree.set(p, a.text);
      return null;
    },
    available_space: () => 1_000_000_000,
    volumes: () => [{ name: "System", location: { fs: "local", path: "/" }, totalBytes: 1e9, availableBytes: 5e8, removable: false, wsl: false }],
    favorites: () => [],
    add_favorite: (a) => [a.favorite],
    remove_favorite: () => [],
    reorder_favorites: () => [],
    settings: () => ({ editor: null, diffTool: null }),
    save_settings: () => null,
    connections: () => [],
    ssh_config_hosts: () => [],
    operations: () => operations,
    activity: () => [],
    activity_insights: () => ({ days: [], operations: 0, completed: 0, failed: 0, cancelled: 0, files: 0, bytes: 0, busyMs: 0, throughput: null, byKind: [], byPlace: [], largest: [], failures: [] }),
    about_info: () => ({ version: "0.1.0", os: "test", arch: "test" }),
    edit_sessions: () => [],
    sync_favorites: () => [],
    rename_presets: () => [],
    rename_preview: () => [],
    diff_tools: () => [],
    sync_plan: (a) => ({ id: "plan-1", mode: a.mode, comparison: { left: a.left, right: a.right, options: a.options, items: [], summary: { same: 0, leftOnly: 0, rightOnly: 0, leftNewer: 0, rightNewer: 0, different: 0, kindMismatch: 0 } }, steps: [], hasHistory: false }),
    live_sync_status: () => null,
    live_sync_stop: () => null,
    live_sync_start: (a) => ({ from: a.from, to: a.to, direction: a.direction, state: "watching", fromWatched: true, toWatched: true, lastSyncMs: null, itemsSynced: 0, syncs: 0, error: null, conflicts: 0 }),
    enqueue_operation: (a) => {
      const id = "op-1";
      operations = [{
        id, kind: a.operation.type, summary: `${a.operation.type} test`, status: "completed",
        progress: { filesTotal: 0, filesDone: 0, bytesTotal: 0, bytesDone: 0, skipped: 0, current: null },
        error: null, queuedMs: Date.now(), startedMs: Date.now(), finishedMs: Date.now(), places: [],
      }];
      return id;
    },
    cancel_operation: () => false,
    clear_finished_operations: () => null,
    clear_activity: () => null,
    open_url: () => null,
    open_path: () => null,
    open_file: () => null,
    preview_path: (a) => a.location.path,
    cancel_compare: () => null,
    run_sync: () => "op-1",
    forget_sync_history: () => null,
    compare_files: () => null,
    "plugin:event|listen": (a) => {
      const id = nextCallback++;
      const list = listeners.get(a.event) ?? [];
      list.push(id);
      listeners.set(a.event, list);
      return id;
    },
    "plugin:event|unlisten": (a) => {
      const list = listeners.get(a.event) ?? [];
      listeners.set(a.event, list.filter((i) => i !== a.eventId));
      return null;
    },
  };

  const emit = (event: string, payload: unknown) => {
    for (const id of listeners.get(event) ?? []) {
      callbacks.get(id)?.(payload);
    }
  };

  const invoke = (cmd: string, args: any) => {
    const handler = handlers[cmd];
    if (!handler) return Promise.reject({ kind: "unsupported", message: `unsupported command ${cmd}` });
    try {
      return Promise.resolve(handler(args ?? {}));
    } catch (e) {
      return Promise.reject(e);
    }
  };

  // The `@tauri-apps/api` runtime looks for this global.
  (window as any).__TAURI_INTERNALS__ = {
    invoke,
    transformCallback: (cb?: (payload: unknown) => void) => {
      const id = nextCallback++;
      if (cb) callbacks.set(id, cb);
      return id;
    },
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    // Minimal window/webview APIs the app touches at startup (drag-drop).
    convertFileSrc: (p: string) => `asset://${p}`,
  };
}
