// A self-contained in-browser fake of the Tauri backend, injected by Playwright
// via `page.addInitScript`. It sets up `window.__TAURI_INTERNALS__` so the
// bundled `@tauri-apps/api` talks to this in-memory implementation.
//
// `addInitScript` serialises the function, so everything must be self-contained
// (no imports, no references to module scope).

/** A server the fake can connect to, keyed by connection id. */
export interface ServerScenario {
  /** The saved connection (as `Connection` in api.ts). */
  connection: { id: string; name: string; endpoint: any; initialPath: string | null; favorites: { name: string; path: string }[] };
  /** Its files, like `BrowserScenario.files` (POSIX paths). */
  files: Record<string, string>;
  /** Folder a connection opens in. Default "/". */
  start?: string;
  /** Sign-in needs this secret (unless remembered). */
  password?: string;
  /** Host key / certificate fingerprint to confirm on first contact. */
  fingerprint?: string;
  /** The server's key changed since it was trusted: refuse to connect. */
  hostKeyChanged?: boolean;
}

export interface BrowserScenario {
  home: string;
  files: Record<string, string>; // path -> content ("d/" or "d\\" = folder)
  /** Commands that fail with this error instead of running. */
  failures?: Record<string, { kind: string; message: string }>;
  /** Fixed results for commands, replacing the default handler. */
  results?: Record<string, unknown>;
  servers?: Record<string, ServerScenario>;
}

/** A backend call the app made, as recorded by the mock. */
export interface MockCall {
  cmd: string;
  args: any;
}

export function installBrowserMock(scenario: BrowserScenario) {
  type Location = { fs: string; path: string };
  type Entry = {
    name: string; path: string; kind: "file" | "dir" | "other"; symlink: boolean;
    size: number; modifiedMs: number | null; createdMs: number | null;
    hidden: boolean; readonly: boolean; mode: number | null;
  };

  // ---- one file system: a tree of paths, with / or \ separators ----------------

  /** Build a tree from `files`. Windows-style paths ("C:\\…") use "\\". */
  const makeTree = (files: Record<string, string>, sample: string) => {
    const sep = /^[A-Za-z]:\\/.test(sample) ? "\\" : "/";
    const isRoot = (p: string) => (sep === "/" ? p === "/" : /^[A-Za-z]:\\$/.test(p));
    const isDir = (p: string) => p.endsWith(sep) && !isRoot(p);
    const clean = (p: string) => (isDir(p) ? p.slice(0, -1) : p);
    const parent = (p: string): string | null => {
      if (isRoot(p)) return null;
      const i = p.lastIndexOf(sep);
      const up = p.slice(0, i);
      return sep === "/" ? up || "/" : /^[A-Za-z]:$/.test(up) ? up + "\\" : up;
    };
    const base = (p: string) => (isRoot(p) ? p : p.slice(p.lastIndexOf(sep) + 1));
    const join = (d: string, n: string) => (d.endsWith(sep) ? d + n : d + sep + n);

    const tree = new Map<string, string>();
    // Folders are tracked on their own: an empty folder and an empty file both
    // have "" as content and no children.
    const dirs = new Set<string>();
    const addDir = (p: string) => {
      tree.set(p, tree.get(p) ?? "");
      dirs.add(p);
    };
    for (const p of Object.keys(files).sort()) {
      // Create every folder above the item.
      const chain: string[] = [];
      for (let up = parent(clean(p)); up !== null; up = parent(up)) chain.unshift(up);
      chain.forEach(addDir);
      if (isDir(p) || isRoot(p)) addDir(clean(p));
      else tree.set(p, files[p]);
    }
    if (sep === "/") addDir("/");

    const entry = (p: string): Entry => {
      const dir = dirs.has(p);
      return {
        name: base(p),
        path: p,
        kind: dir ? "dir" : "file",
        symlink: false,
        size: tree.get(p)?.length ?? 0,
        modifiedMs: 1_700_000_000_000,
        createdMs: 1_700_000_000_000,
        hidden: !isRoot(p) && base(p).startsWith("."),
        readonly: false,
        mode: dir ? 0o755 : 0o644,
      };
    };
    const has = (p: string) => tree.has(p);
    const notFound = (p: string) => ({ kind: "notFound", message: `not found: ${p}` });
    const list = (p: string): Entry[] => {
      if (!dirs.has(p)) throw notFound(p);
      return [...tree.keys()].filter((k) => !isRoot(k) && parent(k) === p).map(entry);
    };
    const remove = (p: string) => {
      for (const k of [...tree.keys()]) {
        if (k === p || k.startsWith(join(p, ""))) {
          tree.delete(k);
          dirs.delete(k);
        }
      }
    };
    /** Every item at or below `p`, as [path relative to p, content, is folder]. */
    const subtree = (p: string): [string, string, boolean][] =>
      [...tree.keys()]
        .filter((k) => k === p || k.startsWith(join(p, "")))
        .sort()
        .map((k) => [k.slice(p.length), tree.get(k)!, dirs.has(k)]);
    const put = (p: string, content: string, dir: boolean) => {
      tree.set(p, content);
      if (dir) dirs.add(p);
    };
    return { sep, parent, base, join, tree, dirs, entry, has, list, remove, subtree, put, notFound };
  };
  type Tree = ReturnType<typeof makeTree>;

  const local = makeTree(scenario.files, scenario.home);
  const fsId = (connectionId: string) => `conn:${connectionId}`;

  // ---- servers ------------------------------------------------------------------

  type Server = ServerScenario & { connected: boolean; trusted: boolean; remembered: boolean; tree: Tree };
  const servers = new Map<string, Server>();
  const addServer = (s: ServerScenario) =>
    servers.set(s.connection.id, {
      ...JSON.parse(JSON.stringify(s)),
      connected: false,
      trusted: !s.fingerprint,
      remembered: false,
      tree: makeTree(s.files ?? {}, "/"),
    });
  for (const s of Object.values(scenario.servers ?? {})) addServer(s);
  let serverCounter = 0;

  const view = (s: Server) => ({ ...s.connection, connected: s.connected, fsId: fsId(s.connection.id) });
  const server = (id: string) => {
    const s = servers.get(id);
    if (!s) throw { kind: "notFound", message: `no such server: ${id}` };
    return s;
  };
  /** The tree of a file system id: "local" or a connected server. */
  const fsOf = (fs: string): Tree => {
    if (fs === "local") return local;
    for (const s of servers.values()) if (fsId(s.connection.id) === fs && s.connected) return s.tree;
    throw { kind: "unknownFileSystem", message: `unknown file system: ${fs}` };
  };

  // ---- state ----------------------------------------------------------------------

  let operations: any[] = [];
  /** Finished operations, newest first, as the activity log keeps them. */
  let activity: any[] = [];
  let presets: any[] = [];
  let favorites: any[] = [];
  let syncFavorites: any[] = [];
  let editSessions: any[] = [];
  /** Operations by id, so a held one can be finished later. */
  const requested = new Map<string, any>();
  let opCounter = 0;
  /** Every command the app invoked, in order (event plumbing left out). */
  const calls: { cmd: string; args: unknown }[] = [];
  /** Steering for tests: set through `window.__mock` with page.evaluate. */
  const control = {
    /** The next enqueued operation fails with this error. */
    nextOpError: null as null | { kind: string; message: string },
    /** Leave operations running; the test emits their updates itself. */
    holdOps: false,
    /** Extra settings for the next server added through save_connection. */
    newServer: null as null | Partial<ServerScenario>,
    /** Keep `connect` calls waiting until the test calls `__mock.releaseConnects()`. */
    holdConnect: false,
  };
  /** Held `connect` calls, run when released. */
  let heldConnects: (() => void)[] = [];
  const releaseConnects = () => {
    const held = heldConnects;
    heldConnects = [];
    held.forEach((run) => run());
  };

  /** A small stand-in for the Rust rename engine: Replace and Case rules. */
  const previewNames = (rules: any[], inputs: { name: string }[], others: string[]) => {
    const out = inputs.map(({ name }) => {
      let next = name;
      for (const r of rules) {
        if (r.type === "replace" && r.find) {
          if (r.regex) {
            try {
              next = next.replace(new RegExp(r.find, r.caseSensitive ? "g" : "gi"), r.replace);
            } catch {
              throw { kind: "invalid", message: `invalid regular expression: ${r.find}` };
            }
          } else {
            next = next.split(r.find).join(r.replace);
            if (!r.caseSensitive) {
              const lower = r.find.toLowerCase();
              let i = next.toLowerCase().indexOf(lower);
              while (i >= 0) {
                next = next.slice(0, i) + r.replace + next.slice(i + r.find.length);
                i = next.toLowerCase().indexOf(lower, i + r.replace.length);
              }
            }
          }
        } else if (r.type === "case") {
          next = r.case === "upper" ? next.toUpperCase() : r.case === "lower" ? next.toLowerCase() : next;
        }
      }
      return { name, newName: next, issue: null as string | null };
    });
    const taken = new Map<string, number>();
    for (const o of out) taken.set(o.newName.toLowerCase(), (taken.get(o.newName.toLowerCase()) ?? 0) + 1);
    const otherNames = new Set(others.map((n) => n.toLowerCase()));
    for (const o of out) {
      if (!o.newName.trim()) o.issue = "empty name";
      else if (o.newName.includes("/")) o.issue = "contains /";
      else if ((taken.get(o.newName.toLowerCase()) ?? 0) > 1 || otherNames.has(o.newName.toLowerCase())) o.issue = "name already used";
    }
    return out;
  };

  // ---- operations -------------------------------------------------------------------

  /** Copy (or move) the item at `from` to `to`, across file systems too. */
  const relocate = (from: Location, to: Location, move: boolean) => {
    const src = fsOf(from.fs);
    const dst = fsOf(to.fs);
    for (const [rel, content, dir] of src.subtree(from.path)) {
      const tail = rel.split(src.sep).join(dst.sep);
      dst.put(to.path + tail, content, dir);
    }
    if (move) src.remove(from.path);
  };
  /** Carry out an operation on the in-memory trees. */
  const apply = (op: any) => {
    switch (op.type) {
      case "createDir":
        fsOf(op.location.fs).put(op.location.path, "", true);
        break;
      case "rename": {
        const t = fsOf(op.location.fs);
        const to = { fs: op.location.fs, path: t.join(t.parent(op.location.path)!, op.newName) };
        relocate(op.location, to, true);
        break;
      }
      case "batchRename":
        for (const it of op.items) apply({ type: "rename", location: it.location, newName: it.newName });
        break;
      case "trash":
      case "delete":
        for (const t of op.targets) fsOf(t.fs).remove(t.path);
        break;
      case "copy":
      case "move": {
        const dst = fsOf(op.destination.fs);
        for (const s of op.sources) {
          const name = fsOf(s.fs).base(s.path);
          relocate(s, { fs: op.destination.fs, path: dst.join(op.destination.path, name) }, op.type === "move");
        }
        break;
      }
      case "compress":
        fsOf(op.destination.fs).put(op.destination.path, "zip", false);
        break;
    }
  };
  const opInfo = (id: string, op: any, status: string, error: unknown = null) => ({
    id, kind: op.type, summary: `${op.type} test`, status,
    progress: { filesTotal: 1, filesDone: status === "completed" ? 1 : 0, bytesTotal: 100, bytesDone: status === "completed" ? 100 : 0, skipped: 0, current: null },
    error, queuedMs: Date.now(), startedMs: Date.now(), finishedMs: status === "running" ? null : Date.now(), places: ["local"],
  });
  /** End operation `id`: apply it (unless it failed), record it in the
   *  activity log and broadcast the final state. */
  const finishOp = (id: string, error: { kind: string; message: string } | null = null) => {
    const op = requested.get(id);
    if (!error) apply(op);
    const done = opInfo(id, op, error ? "failed" : "completed", error);
    operations = operations.map((o) => (o.id === id ? done : o));
    activity = [
      {
        id, kind: op.type, summary: done.summary, status: done.status, startedMs: Date.now(), finishedMs: Date.now(),
        filesDone: done.progress.filesDone, bytesDone: done.progress.bytesDone, skipped: 0,
        error: error ? error.message : null, places: ["local"],
      },
      ...activity,
    ];
    emit("operation-updated", done);
  };

  // ---- remote edit -----------------------------------------------------------------

  let editCounter = 0;
  const updateSession = (id: string, change: Record<string, unknown>) => {
    editSessions = editSessions.map((s) => (s.id === id ? { ...s, ...change } : s));
    const s = editSessions.find((x) => x.id === id);
    if (s) emit("edit-session-updated", s);
    return s;
  };

  // ---- events -------------------------------------------------------------------------

  const callbacks = new Map<number, (payload: unknown) => void>();
  let nextCallback = 1;
  const listeners = new Map<string, number[]>();
  const emit = (event: string, payload: unknown) => {
    for (const id of listeners.get(event) ?? []) {
      callbacks.get(id)?.({ event, id, payload });
    }
  };

  // ---- commands -----------------------------------------------------------------------

  const handlers: Record<string, (args: any) => unknown> = {
    file_systems: () => [
      { id: "local", name: "Local" },
      ...[...servers.values()].filter((s) => s.connected).map((s) => ({ id: fsId(s.connection.id), name: s.connection.name })),
    ],
    home_location: () => ({ fs: "local", path: scenario.home }),
    list_dir: (a) => fsOf(a.location.fs).list(a.location.path),
    find_files: (a) => {
      const t = fsOf(a.location.fs);
      if (!t.dirs.has(a.location.path)) throw t.notFound(a.location.path);
      const q = String(a.query).trim().toLowerCase();
      const hits = q
        ? t.subtree(a.location.path)
            .filter(([rel]) => rel !== "" && t.base(rel).toLowerCase().includes(q))
            .map(([rel]) => t.entry(a.location.path + rel))
        : [];
      return { hits, truncated: false, skipped: 0 };
    },
    cancel_find_files: () => null,
    parent_location: (a) => {
      const up = fsOf(a.location.fs).parent(a.location.path);
      return up === null ? null : { fs: a.location.fs, path: up };
    },
    stat: (a) => {
      const t = fsOf(a.location.fs);
      if (!t.has(a.location.path)) throw t.notFound(a.location.path);
      return t.entry(a.location.path);
    },
    read_text: (a) => ({ text: fsOf(a.location.fs).tree.get(a.location.path) ?? "", truncated: false, binary: false }),
    write_text: (a) => {
      fsOf(a.location.fs).put(a.location.path, a.text, false);
      return null;
    },
    available_space: () => 1_000_000_000,
    volumes: () => [{
      name: "System",
      location: { fs: "local", path: local.sep === "/" ? "/" : scenario.home.slice(0, 3) },
      totalBytes: 1e9, availableBytes: 5e8, removable: false, wsl: false,
    }],
    favorites: () => favorites,
    add_favorite: (a) => (favorites = [...favorites, a.favorite]),
    remove_favorite: (a) =>
      (favorites = favorites.filter((f) => !(f.location.fs === a.location.fs && f.location.path === a.location.path))),
    reorder_favorites: () => favorites,
    settings: () => ({ editor: null, diffTool: null }),
    save_settings: () => null,

    // Servers.
    connections: () => [...servers.values()].map(view),
    ssh_config_hosts: () => [],
    save_connection: (a) => {
      const c = a.connection;
      const id = c.id || `server-${++serverCounter}`;
      const old = servers.get(id);
      if (old) old.connection = { ...c, id };
      else {
        addServer({ connection: { ...c, id }, files: { "/": "" }, password: a.secret ?? undefined, ...(control.newServer ?? {}) });
        control.newServer = null;
      }
      return view(servers.get(id)!);
    },
    remove_connection: (a) => {
      servers.delete(a.id);
      return null;
    },
    reorder_connections: () => null,
    connect: (a) => {
      if (control.holdConnect) {
        return new Promise((resolve, reject) =>
          heldConnects.push(() => {
            try {
              resolve(connectNow(a));
            } catch (e) {
              reject(e);
            }
          }),
        );
      }
      return connectNow(a);
    },
    disconnect: (a) => {
      const s = servers.get(a.id);
      if (s) s.connected = false;
      editSessions = editSessions.filter((e) => e.remote.fs !== fsId(a.id));
      return null;
    },
    add_connection_favorite: (a) => {
      const s = server(a.id);
      s.connection.favorites = [...s.connection.favorites.filter((f) => f.path !== a.favorite.path), a.favorite];
      return view(s);
    },
    remove_connection_favorite: (a) => {
      const s = server(a.id);
      s.connection.favorites = s.connection.favorites.filter((f) => f.path !== a.path);
      return view(s);
    },

    operations: () => operations,
    activity: () => activity,
    activity_insights: () => ({ days: [], operations: 0, completed: 0, failed: 0, cancelled: 0, files: 0, bytes: 0, busyMs: 0, throughput: null, byKind: [], byPlace: [], largest: [], failures: [] }),
    about_info: () => ({ version: "0.1.0", os: "test", arch: "test" }),

    // Remote edit: F4 on a server file starts a session.
    edit_sessions: () => editSessions,
    open_file: (a) => {
      if (a.location.fs === "local" || !a.editor) return null;
      const s = {
        id: `edit-${++editCounter}`,
        remote: a.location,
        localPath: `/tmp/dualpal-edit/${editCounter}/${fsOf(a.location.fs).base(a.location.path)}`,
        status: "watching",
        uploads: 0,
        lastUploadMs: null,
        error: null,
      };
      editSessions = [...editSessions, s];
      emit("edit-session-updated", s);
      return s;
    },
    stop_edit_session: (a) => {
      const s = updateSession(a.id, { status: "stopped" });
      editSessions = editSessions.filter((x) => x.id !== a.id);
      return !!s;
    },
    upload_edit_session: (a) => updateSession(a.id, { status: "uploaded", error: null }),

    // Syncs.
    sync_favorites: () => syncFavorites,
    save_sync_favorite: (a) => {
      const f = { ...a.favorite, id: a.favorite.id || `sync-${syncFavorites.length + 1}` };
      syncFavorites = [...syncFavorites.filter((x) => x.id !== f.id), f];
      return f;
    },
    remove_sync_favorite: (a) => {
      syncFavorites = syncFavorites.filter((x) => x.id !== a.id);
      return null;
    },

    rename_presets: () => presets,
    save_rename_preset: (a) => {
      presets = [...presets.filter((p) => p.name !== a.preset.name), a.preset];
      return presets;
    },
    remove_rename_preset: (a) => {
      presets = presets.filter((p) => p.name !== a.name);
      return presets;
    },
    rename_preview: (a) => previewNames(a.rules, a.inputs, a.others ?? []),
    diff_tools: () => [],
    sync_plan: (a) => ({ id: "plan-1", mode: a.mode, comparison: { left: a.left, right: a.right, options: a.options, items: [], summary: { same: 0, leftOnly: 0, rightOnly: 0, leftNewer: 0, rightNewer: 0, different: 0, kindMismatch: 0 } }, steps: [], hasHistory: false }),
    live_sync_status: () => null,
    live_sync_stop: () => null,
    live_sync_start: (a) => ({ from: a.from, to: a.to, direction: a.direction, state: "watching", fromWatched: true, toWatched: true, lastSyncMs: null, itemsSynced: 0, syncs: 0, error: null, conflicts: 0 }),
    enqueue_operation: (a) => {
      const id = `op-${++opCounter}`;
      const op = a.operation;
      requested.set(id, op);
      const running = opInfo(id, op, "running");
      operations = [...operations.filter((o) => o.id !== id), running];
      if (control.holdOps) return id;
      const error = control.nextOpError;
      control.nextOpError = null;
      setTimeout(() => {
        emit("operation-updated", running);
        finishOp(id, error);
      }, 10);
      return id;
    },
    cancel_operation: () => false,
    clear_activity: () => {
      activity = [];
      return null;
    },
    clear_finished_operations: () => {
      operations = operations.filter((o) => o.status === "running" || o.status === "queued");
      return null;
    },
    open_url: () => null,
    open_path: () => null,
    preview_path: (a) => a.location.path,
    cancel_compare: () => null,
    folder_size: () => ({ bytes: 0, files: 0, folders: 0, skipped: 0 }),
    cancel_folder_size: () => null,
    run_sync: () => "op-1",
    forget_sync_history: () => null,
    compare_files: () => null,
    "plugin:event|listen": (a) => {
      // `handler` is the callback id from transformCallback; the returned
      // event id is what unlisten passes back.
      const list = listeners.get(a.event) ?? [];
      list.push(a.handler);
      listeners.set(a.event, list);
      return a.handler;
    },
    "plugin:event|unlisten": (a) => {
      const list = listeners.get(a.event) ?? [];
      listeners.set(a.event, list.filter((i) => i !== a.eventId));
      return null;
    },
  };

  const connectNow = (a: any) => {
    const s = server(a.id);
    const host = s.connection.endpoint.host ?? s.connection.endpoint.url ?? s.connection.name;
    if (s.hostKeyChanged) {
      throw {
        kind: "hostKeyChanged",
        message: `the host key of ${host} has changed (now ${s.fingerprint}); this could be an attack`,
        host,
        fingerprint: s.fingerprint,
      };
    }
    if (!s.trusted) {
      if (a.trust && a.trust === s.fingerprint) s.trusted = true;
      else throw { kind: "unknownHost", message: `unknown host ${host}, key fingerprint ${s.fingerprint}`, host, fingerprint: s.fingerprint };
    }
    if (s.password && !s.remembered && a.secret !== s.password) {
      throw { kind: "authFailed", message: "sign-in failed: wrong user name or password" };
    }
    if (a.remember) s.remembered = true;
    s.connected = true;
    return { location: { fs: fsId(s.connection.id), path: s.start ?? s.connection.initialPath ?? "/" }, name: s.connection.name };
  };

  const invoke = (cmd: string, args: any) => {
    if (!cmd.startsWith("plugin:event|")) calls.push({ cmd, args: JSON.parse(JSON.stringify(args ?? {})) });
    const failure = scenario.failures?.[cmd];
    if (failure) return Promise.reject(failure);
    if (scenario.results && cmd in scenario.results) return Promise.resolve(scenario.results[cmd]);
    const handler = handlers[cmd];
    if (!handler) return Promise.reject({ kind: "unsupported", message: `unsupported command ${cmd}` });
    try {
      return Promise.resolve(handler(args ?? {}));
    } catch (e) {
      return Promise.reject(e);
    }
  };

  // Test hooks: recorded calls, event emission, steering and the file trees.
  (window as any).__mock = {
    calls,
    emit,
    control,
    releaseConnects,
    handlers,
    finishOp,
    servers,
    tree: local.tree,
    dirs: local.dirs,
    /** Remove a local item and everything below it. */
    remove: (p: string) => local.remove(p),
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
    // Previews load the file itself: serve its content as a data URL so
    // images (and SVG with script in it, SEC-006) really load.
    convertFileSrc: (p: string) => {
      const content = local.tree.get(p);
      if (content === undefined) return `asset://${p}`;
      const ext = p.slice(p.lastIndexOf(".") + 1).toLowerCase();
      const mime = ext === "svg" ? "image/svg+xml" : ext === "html" ? "text/html" : ext === "png" ? "image/png" : "application/octet-stream";
      return `data:${mime};base64,${btoa(unescape(encodeURIComponent(content)))}`;
    },
  };
}
